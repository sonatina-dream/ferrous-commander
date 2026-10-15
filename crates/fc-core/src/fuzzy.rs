//! Fuzzy matching for the command palette.

/// Scores `candidate` against `query` as a case-insensitive subsequence match.
/// Returns `None` when some query character is missing. Higher is better:
/// matches at word starts and runs of consecutive characters score more, and gaps
/// cost a little.
pub fn score(query: &str, candidate: &str) -> Option<i32> {
    let query: Vec<char> = query.trim().to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
    if query.is_empty() {
        return Some(0);
    }
    let original: Vec<char> = candidate.chars().collect();
    let lower: Vec<char> = candidate.to_lowercase().chars().collect();
    if lower.len() != original.len() {
        // Rare case-mapping length changes: fall back to plain comparison.
        return plain(&query, &lower);
    }
    let mut total = 0i32;
    let mut qi = 0;
    let mut last_match: Option<usize> = None;
    for (i, &c) in lower.iter().enumerate() {
        if qi == query.len() {
            break;
        }
        if c != query[qi] {
            continue;
        }
        let word_start = i == 0
            || !original[i - 1].is_alphanumeric()
            || (original[i].is_uppercase() && original[i - 1].is_lowercase());
        let mut s = 1;
        if word_start {
            s += 8;
        }
        match last_match {
            Some(prev) if prev + 1 == i => s += 6,
            Some(prev) => s -= ((i - prev - 1) as i32).min(4),
            None => s -= (i as i32).min(6),
        }
        total += s;
        last_match = Some(i);
        qi += 1;
    }
    if qi < query.len() {
        return None;
    }
    let prefix: String = query.iter().collect();
    if candidate.to_lowercase().starts_with(&prefix) {
        total += 12;
    }
    // Prefer shorter candidates among equals.
    total -= (lower.len() as i32 / 12).min(4);
    Some(total)
}

fn plain(query: &[char], lower: &[char]) -> Option<i32> {
    let mut qi = 0;
    for &c in lower {
        if qi < query.len() && c == query[qi] {
            qi += 1;
        }
    }
    (qi == query.len()).then_some(qi as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subsequence_required() {
        assert!(score("cpy", "Copy").is_some());
        assert!(score("xyz", "Copy").is_none());
        assert_eq!(score("", "anything"), Some(0));
    }

    #[test]
    fn word_starts_and_prefixes_rank_higher() {
        let nf = score("nf", "New folder").unwrap();
        let inf = score("nf", "Show info").unwrap();
        assert!(nf > inf, "word starts beat inner letters ({nf} vs {inf})");
        let copy = score("copy", "Copy").unwrap();
        let path = score("copy", "Copy path to clipboard").unwrap();
        assert!(copy > path, "shorter exact prefix wins");
        assert!(score("dl", "Downloads").unwrap() < score("dow", "Downloads").unwrap());
        assert!(score("gh", "Go home").unwrap() > score("gh", "Toggle hidden").unwrap_or(i32::MIN));
    }

    #[test]
    fn camel_case_boundaries() {
        assert!(score("sp", "SearchPanel").unwrap() > score("sp", "Display").unwrap());
    }
}
