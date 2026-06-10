//! Fuzzy (subsequence) matching used as a completion fallback.
//!
//! When a case-insensitive *prefix* match yields nothing, completion falls back
//! to this: a candidate matches if the query is a case-insensitive *subsequence*
//! (its characters appear in order), so `gco` can still find `git-checkout` and
//! `dwn` can find `Downloads`. Results are ranked so the best matches come first.

/// Score `candidate` against `query` as a case-insensitive subsequence. Returns
/// `None` if it doesn't match. Higher is better. An empty query matches anything
/// with a neutral score.
pub fn subsequence_score(candidate: &str, query: &str) -> Option<i32> {
    if query.is_empty() {
        return Some(0);
    }
    let cand: Vec<char> = candidate.chars().flat_map(|c| c.to_lowercase()).collect();
    let q: Vec<char> = query.chars().flat_map(|c| c.to_lowercase()).collect();

    let mut qi = 0;
    let mut first: Option<usize> = None;
    let mut contiguous = 0i32;
    let mut prev_match: Option<usize> = None;
    for (i, &ch) in cand.iter().enumerate() {
        if qi < q.len() && ch == q[qi] {
            if first.is_none() {
                first = Some(i);
            }
            if let Some(p) = prev_match {
                if p + 1 == i {
                    contiguous += 1;
                }
            }
            prev_match = Some(i);
            qi += 1;
        }
    }
    if qi != q.len() {
        return None;
    }

    let cand_lc: String = cand.iter().collect();
    let q_lc: String = q.iter().collect();
    let mut score = 0i32;
    if cand_lc.starts_with(&q_lc) {
        score += 1000;
    } else if cand_lc.contains(&q_lc) {
        score += 500;
    }
    score += contiguous * 10;
    score -= first.unwrap_or(0) as i32; // earlier first match is better
    score -= (cand.len() as i32) / 4; // mildly prefer shorter candidates
    Some(score)
}

/// Filter and rank `items` by fuzzy score against `query` (best first; ties
/// broken alphabetically).
pub fn rank(items: Vec<String>, query: &str) -> Vec<String> {
    let mut scored: Vec<(i32, String)> = items
        .into_iter()
        .filter_map(|c| subsequence_score(&c, query).map(|s| (s, c)))
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, c)| c).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subsequence_matches_in_order() {
        assert!(subsequence_score("git-checkout", "gco").is_some());
        assert!(subsequence_score("Downloads", "dwn").is_some());
        assert!(subsequence_score("Downloads", "dl").is_some());
        // out-of-order or missing chars don't match
        assert!(subsequence_score("git", "tg").is_none());
        assert!(subsequence_score("git", "gx").is_none());
    }

    #[test]
    fn case_insensitive() {
        assert!(subsequence_score("Cargo.toml", "cargo").is_some());
        assert!(subsequence_score("README", "readme").is_some());
    }

    #[test]
    fn prefix_outranks_scattered() {
        // "git" (prefix) should outrank "legit" (scattered) for query "git".
        let ranked = rank(vec!["legit".into(), "digit".into(), "git".into()], "git");
        assert_eq!(ranked.first().unwrap(), "git");
    }

    #[test]
    fn empty_query_keeps_all() {
        let ranked = rank(vec!["b".into(), "a".into()], "");
        assert_eq!(ranked, vec!["a", "b"]); // tie → alphabetical
    }
}
