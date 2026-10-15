//! File search (SPEC §8): name masks or regex, content search, depth limits, Spotlight.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

use regex::bytes::Regex as BytesRegex;
use regex::Regex;

use crate::pattern::Mask;

/// Content criteria ("Find text").
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ContentQuery {
    pub text: String,
    pub regex: bool,
    pub case_sensitive: bool,
    pub whole_words: bool,
}

/// A search request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchQuery {
    /// Folders to search (the current folder, or the selected folders).
    pub roots: Vec<PathBuf>,
    /// Name criteria: wildcard mask (substring when no wildcard) or a regex.
    pub name: String,
    pub name_regex: bool,
    pub case_sensitive: bool,
    /// `Some(0)` = only the roots' direct contents, `Some(n)` = n folder levels below,
    /// `None` = unlimited.
    pub depth: Option<usize>,
    pub content: Option<ContentQuery>,
    pub include_hidden: bool,
}

impl Default for SearchQuery {
    fn default() -> Self {
        SearchQuery {
            roots: Vec::new(),
            name: String::new(),
            name_regex: false,
            case_sensitive: false,
            depth: None,
            content: None,
            include_hidden: true,
        }
    }
}

/// Invalid input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SearchError {
    BadNameRegex(String),
    BadContentRegex(String),
}

impl std::fmt::Display for SearchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SearchError::BadNameRegex(e) => write!(f, "The regular expression for file names isn't valid: {e}"),
            SearchError::BadContentRegex(e) => write!(f, "The regular expression for file contents isn't valid: {e}"),
        }
    }
}

/// Case-insensitive masks use [`Mask`]; case-sensitive masks and regexes use [`Regex`].
enum NameMatcher {
    Mask(Mask),
    Regex(Regex),
}

impl NameMatcher {
    fn matches(&self, name: &str) -> bool {
        match self {
            NameMatcher::Mask(mask) => mask.matches(name),
            NameMatcher::Regex(re) => re.is_match(name),
        }
    }
}

/// A validated, ready-to-run search.
pub struct CompiledSearch {
    query: SearchQuery,
    name: NameMatcher,
    content: Option<BytesRegex>,
}

/// Validates `query` (regexes, defaults).
pub fn compile(query: SearchQuery) -> Result<CompiledSearch, SearchError> {
    let name = if query.name_regex {
        let pattern = if query.case_sensitive { query.name.clone() } else { format!("(?i){}", query.name) };
        NameMatcher::Regex(Regex::new(&pattern).map_err(|e| SearchError::BadNameRegex(e.to_string()))?)
    } else if query.case_sensitive {
        NameMatcher::Regex(mask_to_regex(&query.name))
    } else {
        NameMatcher::Mask(Mask::parse_search(&query.name))
    };
    let content = match &query.content {
        Some(c) if !c.text.is_empty() => {
            let mut body = if c.regex { c.text.clone() } else { regex::escape(&c.text) };
            if c.whole_words {
                body = format!(r"\b(?:{body})\b");
            }
            if !c.case_sensitive {
                body = format!("(?i){body}");
            }
            Some(BytesRegex::new(&body).map_err(|e| SearchError::BadContentRegex(e.to_string()))?)
        }
        _ => None,
    };
    Ok(CompiledSearch { query, name, content })
}

/// Translates a search mask into an anchored, case-sensitive regex.
fn mask_to_regex(text: &str) -> Regex {
    let text = if text.trim().is_empty() { "*" } else { text };
    let alternatives: Vec<String> = text
        .split(';')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|p| {
            let p = if p.contains(['*', '?', '[']) { p.to_string() } else { format!("*{p}*") };
            let mut out = String::from("^");
            let mut chars = p.chars().peekable();
            while let Some(c) = chars.next() {
                match c {
                    '*' => out.push_str(".*"),
                    '?' => out.push('.'),
                    '[' => {
                        let mut class = String::from("[");
                        if matches!(chars.peek(), Some('!') | Some('^')) {
                            chars.next();
                            class.push('^');
                        }
                        let mut closed = false;
                        for d in chars.by_ref() {
                            if d == ']' {
                                closed = true;
                                break;
                            }
                            if d == '\\' {
                                class.push_str("\\\\");
                            } else {
                                class.push(d);
                            }
                        }
                        if closed {
                            class.push(']');
                            out.push_str(&class);
                        } else {
                            out.push_str(&regex::escape(&class));
                        }
                    }
                    other => out.push_str(&regex::escape(&other.to_string())),
                }
            }
            out.push('$');
            out
        })
        .collect();
    Regex::new(&alternatives.join("|")).unwrap_or_else(|_| Regex::new("^$").expect("valid"))
}

/// Statistics of a finished run.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchStats {
    pub scanned: u64,
    pub hits: u64,
    pub cancelled: bool,
}

const CONTENT_LIMIT: u64 = 256 * 1024 * 1024;

impl CompiledSearch {
    pub fn query(&self) -> &SearchQuery {
        &self.query
    }

    /// Walks the roots and calls `on_hit` for every match. Symlinked folders are not
    /// followed, which prevents cycles.
    pub fn run(&self, cancel: &AtomicBool, mut on_hit: impl FnMut(PathBuf)) -> SearchStats {
        let mut stats = SearchStats::default();
        let mut stack: Vec<(PathBuf, usize)> = self.query.roots.iter().map(|r| (r.clone(), 0)).collect();
        while let Some((dir, level)) = stack.pop() {
            let Ok(rd) = fs::read_dir(&dir) else { continue };
            let mut children: Vec<_> = rd.flatten().collect();
            children.sort_by_key(|e| e.file_name());
            let mut subdirs = Vec::new();
            for item in children {
                if cancel.load(Ordering::Relaxed) {
                    stats.cancelled = true;
                    return stats;
                }
                let name = item.file_name().to_string_lossy().into_owned();
                if !self.query.include_hidden && name.starts_with('.') {
                    continue;
                }
                let path = item.path();
                let Ok(ft) = item.file_type() else { continue };
                stats.scanned += 1;
                if ft.is_dir() && self.query.depth.is_none_or(|d| level < d) {
                    subdirs.push((path.clone(), level + 1));
                }
                if !self.name.matches(&name) {
                    continue;
                }
                if let Some(re) = &self.content
                    && (!ft.is_file() || !file_contains(&path, re, cancel)) {
                        continue;
                    }
                stats.hits += 1;
                on_hit(path);
            }
            // Depth-first in name order.
            subdirs.reverse();
            stack.extend(subdirs);
        }
        stats
    }
}

fn file_contains(path: &Path, re: &BytesRegex, cancel: &AtomicBool) -> bool {
    let Ok(file) = fs::File::open(path) else { return false };
    let mut data = Vec::new();
    if file.take(CONTENT_LIMIT).read_to_end(&mut data).is_err() || cancel.load(Ordering::Relaxed) {
        return false;
    }
    re.is_match(&data)
}

/// Spotlight search via `mdfind`, limited to `roots`. With `in_content`, the query is
/// a free-text query (names and contents); otherwise it matches display names.
pub fn spotlight(query: &str, roots: &[PathBuf], in_content: bool) -> io::Result<Vec<PathBuf>> {
    let mut cmd = Command::new("/usr/bin/mdfind");
    for r in roots {
        cmd.arg("-onlyin").arg(r);
    }
    if in_content {
        cmd.arg(query);
    } else {
        cmd.arg("-name").arg(query);
    }
    let out = cmd.output()?;
    if !out.status.success() {
        return Err(io::Error::other(String::from_utf8_lossy(&out.stderr).into_owned()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).lines().filter(|l| !l.is_empty()).map(PathBuf::from).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> tempfile::TempDir {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        fs::create_dir_all(r.join("l1/l2/l3")).unwrap();
        fs::write(r.join("Report.md"), "Quarterly numbers\nrevenue up").unwrap();
        fs::write(r.join("notes.txt"), "the reporter wrote this").unwrap();
        fs::write(r.join("l1/report-old.txt"), "Revenue down").unwrap();
        fs::write(r.join("l1/l2/deep.txt"), "revenue").unwrap();
        fs::write(r.join("l1/l2/l3/deeper.txt"), "revenue").unwrap();
        fs::write(r.join(".hidden.txt"), "revenue").unwrap();
        t
    }

    fn run(q: SearchQuery) -> Vec<String> {
        let root = q.roots[0].clone();
        let c = compile(q).unwrap();
        let mut hits = Vec::new();
        c.run(&AtomicBool::new(false), |p| hits.push(p.strip_prefix(&root).unwrap().to_string_lossy().into_owned()));
        hits.sort();
        hits
    }

    fn q(root: &Path) -> SearchQuery {
        SearchQuery { roots: vec![root.to_path_buf()], ..Default::default() }
    }

    #[test]
    fn name_masks_and_case() {
        let t = fixture();
        let r = t.path();
        assert_eq!(run(SearchQuery { name: "report".into(), ..q(r) }), ["Report.md", "l1/report-old.txt"]);
        assert_eq!(run(SearchQuery { name: "report".into(), case_sensitive: true, ..q(r) }), ["l1/report-old.txt"]);
        assert_eq!(run(SearchQuery { name: "*.md".into(), ..q(r) }), ["Report.md"]);
        assert_eq!(
            run(SearchQuery { name: "*.txt".into(), include_hidden: false, ..q(r) }),
            ["l1/l2/deep.txt", "l1/l2/l3/deeper.txt", "l1/report-old.txt", "notes.txt"]
        );
    }

    #[test]
    fn regex_names_and_errors() {
        let t = fixture();
        let r = t.path();
        assert_eq!(run(SearchQuery { name: r"^de+p".into(), name_regex: true, ..q(r) }), ["l1/l2/deep.txt", "l1/l2/l3/deeper.txt"]);
        assert!(matches!(compile(SearchQuery { name: "(".into(), name_regex: true, ..q(r) }), Err(SearchError::BadNameRegex(_))));
        let bad = SearchQuery { content: Some(ContentQuery { text: "[".into(), regex: true, ..Default::default() }), ..q(r) };
        assert!(matches!(compile(bad), Err(SearchError::BadContentRegex(_))));
    }

    #[test]
    fn depth_limits() {
        let t = fixture();
        let r = t.path();
        let txt = |depth| run(SearchQuery { name: "*.txt".into(), depth, include_hidden: false, ..q(r) });
        assert_eq!(txt(Some(0)), ["notes.txt"]);
        assert_eq!(txt(Some(1)), ["l1/report-old.txt", "notes.txt"]);
        assert_eq!(txt(Some(2)).len(), 3);
        assert_eq!(txt(None).len(), 4);
    }

    #[test]
    fn content_search() {
        let t = fixture();
        let r = t.path();
        let content = |text: &str, case_sensitive, whole_words, regex| SearchQuery {
            name: "*".into(),
            include_hidden: false,
            content: Some(ContentQuery { text: text.into(), regex, case_sensitive, whole_words }),
            ..q(r)
        };
        assert_eq!(run(content("revenue", false, false, false)).len(), 4);
        assert_eq!(run(content("Revenue", true, false, false)), ["l1/report-old.txt"]);
        assert_eq!(run(content("report", false, true, false)), Vec::<String>::new());
        assert_eq!(run(content("report", false, false, false)), ["notes.txt"]);
        assert_eq!(run(content(r"rev\w+ (up|down)", false, false, true)), ["Report.md", "l1/report-old.txt"]);
    }

    #[test]
    fn cancellation() {
        let t = fixture();
        let c = compile(SearchQuery { name: "*".into(), ..q(t.path()) }).unwrap();
        let stats = c.run(&AtomicBool::new(true), |_| panic!("no hits after cancel"));
        assert!(stats.cancelled);
    }
}
