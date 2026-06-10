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

/// Case-insensitive Damerau-Levenshtein (optimal string alignment) edit distance
/// between `a` and `b`: single-character insertions, deletions, substitutions,
/// and adjacent transpositions each cost 1. Transpositions cost 1 (not 2) because
/// swapped letters (`gti` for `git`) are the most common typo.
pub fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().flat_map(|c| c.to_lowercase()).collect();
    let b: Vec<char> = b.chars().flat_map(|c| c.to_lowercase()).collect();
    let (n, m) = (a.len(), b.len());
    if n == 0 {
        return m;
    }
    if m == 0 {
        return n;
    }
    let mut d = vec![vec![0usize; m + 1]; n + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, v) in d[0].iter_mut().enumerate() {
        *v = j;
    }
    for i in 1..=n {
        for j in 1..=m {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut v = (d[i - 1][j] + 1)
                .min(d[i][j - 1] + 1)
                .min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                v = v.min(d[i - 2][j - 2] + 1);
            }
            d[i][j] = v;
        }
    }
    d[n][m]
}

/// The best correction for `query` among `candidates`: the candidate with the
/// smallest edit distance, when that distance is in `1..=max_dist` (so an exact
/// match is never "corrected"). Ties break toward the shorter, then
/// alphabetically-first candidate.
pub fn correction<'a, I>(query: &str, candidates: I, max_dist: usize) -> Option<String>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut best: Option<(usize, &str)> = None;
    for cand in candidates {
        let d = edit_distance(query, cand);
        if d == 0 || d > max_dist {
            continue;
        }
        let better = match best {
            None => true,
            Some((bd, bc)) => {
                d < bd
                    || (d == bd && (cand.len() < bc.len() || (cand.len() == bc.len() && cand < bc)))
            }
        };
        if better {
            best = Some((d, cand));
        }
    }
    best.map(|(_, c)| c.to_string())
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

    #[test]
    fn edit_distance_basics() {
        assert_eq!(edit_distance("git", "git"), 0);
        assert_eq!(edit_distance("gti", "git"), 1); // adjacent transposition = 1
        assert_eq!(edit_distance("gut", "git"), 1); // one substitution
        assert_eq!(edit_distance("dcoker", "docker"), 1); // transposition
        assert_eq!(edit_distance("gitt", "git"), 1); // one insertion
        assert_eq!(edit_distance("", "abc"), 3);
    }

    #[test]
    fn correction_picks_closest() {
        let cmds = ["git", "grep", "cd", "ls", "docker"];
        // typo within max_dist → closest known command.
        assert_eq!(correction("gitt", cmds, 2).as_deref(), Some("git"));
        assert_eq!(correction("dcoker", cmds, 2).as_deref(), Some("docker"));
        // too far → no correction.
        assert_eq!(correction("zzzzz", cmds, 2), None);
        // a distance-0 (exact) candidate is never returned; only OTHER close ones
        // are considered (the caller skips correction for real commands).
        assert_eq!(correction("git", ["git", "cd"], 2), None);
    }
}
