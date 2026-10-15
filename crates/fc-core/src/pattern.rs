//! Wildcard masks used by select/unselect-by-pattern and name search (SPEC §4, §8).

/// A `;`-separated list of shell wildcards (`*`, `?`, `[a-z]`, `[!x]`), matched
/// case-insensitively against a whole name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mask {
    patterns: Vec<Vec<char>>,
}

impl Mask {
    /// Parses a mask. Blank components are ignored; a blank mask matches nothing.
    pub fn parse(text: &str) -> Mask {
        let patterns = text
            .split(';')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(|p| p.to_lowercase().chars().collect())
            .collect();
        Mask { patterns }
    }

    /// Like [`Mask::parse`], but a component without wildcard characters matches as a
    /// substring (`report` behaves like `*report*`). Used by name search.
    pub fn parse_search(text: &str) -> Mask {
        let text = if text.trim().is_empty() { "*" } else { text };
        let wrapped: Vec<String> = text
            .split(';')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(|p| {
                if p.contains(['*', '?', '[']) { p.to_string() } else { format!("*{p}*") }
            })
            .collect();
        Mask::parse(&wrapped.join(";"))
    }

    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    pub fn matches(&self, name: &str) -> bool {
        let lowered: Vec<char> = name.to_lowercase().chars().collect();
        self.patterns.iter().any(|p| wildcard_match(p, &lowered))
    }
}

/// Matches `text` against `pattern` with `*`, `?` and bracket classes. Iterative with
/// single-star backtracking, so it runs in O(len(p) × len(t)) worst case.
fn wildcard_match(pattern: &[char], text: &[char]) -> bool {
    let (mut p, mut t) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        if p < pattern.len() {
            match pattern[p] {
                '*' => {
                    star = Some((p, t));
                    p += 1;
                    continue;
                }
                '?' => {
                    p += 1;
                    t += 1;
                    continue;
                }
                '[' => {
                    if let Some((matched, next_p)) = match_class(pattern, p, text[t]) {
                        if matched {
                            p = next_p;
                            t += 1;
                            continue;
                        }
                    } else if text[t] == '[' {
                        // Unterminated bracket: treat literally.
                        p += 1;
                        t += 1;
                        continue;
                    }
                }
                c if c == text[t] => {
                    p += 1;
                    t += 1;
                    continue;
                }
                _ => {}
            }
        }
        match star {
            Some((sp, st)) => {
                p = sp + 1;
                t = st + 1;
                star = Some((sp, st + 1));
            }
            None => return false,
        }
    }
    pattern[p..].iter().all(|&c| c == '*')
}

/// Evaluates a bracket class starting at `pattern[start] == '['`.
/// Returns (matched, index after `]`), or `None` when the class is unterminated.
fn match_class(pattern: &[char], start: usize, ch: char) -> Option<(bool, usize)> {
    let mut i = start + 1;
    let negate = matches!(pattern.get(i), Some('!') | Some('^'));
    if negate {
        i += 1;
    }
    let mut matched = false;
    let mut first = true;
    while i < pattern.len() {
        let c = pattern[i];
        if c == ']' && !first {
            return Some((matched != negate, i + 1));
        }
        first = false;
        if i + 2 < pattern.len() && pattern[i + 1] == '-' && pattern[i + 2] != ']' {
            if pattern[i] <= ch && ch <= pattern[i + 2] {
                matched = true;
            }
            i += 3;
        } else {
            if c == ch {
                matched = true;
            }
            i += 1;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basics() {
        let m = Mask::parse("*.txt");
        assert!(m.matches("notes.txt"));
        assert!(m.matches("NOTES.TXT"));
        assert!(!m.matches("notes.txt.bak"));
        assert!(Mask::parse("*").matches(".hidden"));
        assert!(Mask::parse("a?c").matches("abc"));
        assert!(!Mask::parse("a?c").matches("ac"));
        assert!(Mask::parse("*.*").matches("a.b"));
        assert!(!Mask::parse("*.*").matches("README"));
    }

    #[test]
    fn lists_and_classes() {
        let m = Mask::parse("*.jpg; *.png ;");
        assert!(m.matches("a.png") && m.matches("b.JPG") && !m.matches("c.gif"));
        assert!(Mask::parse("file[0-9].log").matches("file7.log"));
        assert!(!Mask::parse("file[!0-9].log").matches("file7.log"));
        assert!(Mask::parse("file[!0-9].log").matches("fileX.log"));
        assert!(Mask::parse("x[").matches("x["));
        assert!(Mask::parse("").is_empty());
        assert!(!Mask::parse(" ; ").matches("anything"));
    }

    #[test]
    fn backtracking() {
        assert!(Mask::parse("*a*b*c").matches("xxaxxbxxbxxc"));
        assert!(!Mask::parse("*a*b*c").matches("xxaxxbxxbxx"));
        assert!(Mask::parse("**").matches(""));
    }

    #[test]
    fn search_masks_are_substring_by_default() {
        assert!(Mask::parse_search("port").matches("Report.pdf"));
        assert!(Mask::parse_search("").matches("anything"));
        assert!(!Mask::parse_search("*.md").matches("Report.pdf"));
    }
}
