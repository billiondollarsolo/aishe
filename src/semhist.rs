//! Semantic history search: a tiny, dependency-free on-disk vector store over
//! past shell commands, queried by meaning ("that docker run with the volume
//! mount").
//!
//! Each indexed command is one JSONL line in `history.vec`
//! (`{"cmd": "...", "vec": [f32, ...]}`), appended incrementally as you index.
//! Search embeds the query and returns the top-k closest commands by cosine
//! similarity. The store is capped and fully rebuildable from the history log,
//! and the whole feature is opt-in (`semantic_history`) and silent when off — no
//! command is embedded unless you run `aishe history index`.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Cap on how many command vectors the store keeps. When indexing would exceed
/// this, the oldest entries are dropped (history search cares about recency and
/// the file stays bounded). ~5k entries of a small embedding is a few MB.
pub const STORE_CAP: usize = 5000;

/// One indexed command and its embedding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub cmd: String,
    pub vec: Vec<f32>,
}

/// Load the store, tolerating partial/corrupt lines (a torn append never breaks
/// search — the bad line is skipped). Missing file → empty.
pub fn load(path: &Path) -> Vec<Entry> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(e) = serde_json::from_str::<Entry>(line) {
            out.push(e);
        }
    }
    out
}

/// Rewrite the store with exactly `entries` (newest last), keeping at most
/// [`STORE_CAP`] of the most recent. Written atomically via the config helper so
/// a crash can't leave a half-written index.
pub fn save(path: &Path, entries: &[Entry]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let start = entries.len().saturating_sub(STORE_CAP);
    let mut buf = String::new();
    for e in &entries[start..] {
        // serde_json on a flat struct of String + Vec<f32> cannot fail here, but
        // skip any entry that somehow does rather than abort the whole save.
        if let Ok(line) = serde_json::to_string(e) {
            buf.push_str(&line);
            buf.push('\n');
        }
    }
    crate::config::write_atomic(path, buf.as_bytes())
}

/// The set of commands already in the store, so indexing can skip them and only
/// embed what's new (incremental indexing on the cheap).
pub fn indexed_commands(path: &Path) -> std::collections::HashSet<String> {
    load(path).into_iter().map(|e| e.cmd).collect()
}

/// Cosine similarity of two equal-length vectors, in `[-1, 1]`. Mismatched
/// lengths or a zero-magnitude vector yield 0 (no signal), never NaN.
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let mut dot = 0f32;
    let mut na = 0f32;
    let mut nb = 0f32;
    for i in 0..a.len() {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

/// The `k` entries most similar to `query`, highest score first. Duplicate
/// commands are collapsed to their single best score so a repeated command can't
/// crowd out the rest of the results.
pub fn top_k(entries: &[Entry], query: &[f32], k: usize) -> Vec<(f32, String)> {
    use std::collections::HashMap;
    let mut best: HashMap<&str, f32> = HashMap::new();
    for e in entries {
        let score = cosine(query, &e.vec);
        let slot = best.entry(e.cmd.as_str()).or_insert(f32::MIN);
        if score > *slot {
            *slot = score;
        }
    }
    let mut scored: Vec<(f32, String)> =
        best.into_iter().map(|(c, s)| (s, c.to_string())).collect();
    // Sort by score desc; ties broken by command text for determinism.
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.1.cmp(&b.1))
    });
    scored.truncate(k);
    scored
}

/// Unique commands from a history listing, most-recent-last order preserved,
/// dropping blanks and aishe's own `history` management commands (so the index
/// doesn't fill up with `aishe history search ...`). Caps to the last
/// [`STORE_CAP`] so a huge history doesn't embed unboundedly.
pub fn candidates(history: &[(u64, String)]) -> Vec<String> {
    use std::collections::HashSet;
    let mut seen: HashSet<&str> = HashSet::new();
    let mut out: Vec<String> = Vec::new();
    for (_, cmd) in history {
        let trimmed = cmd.trim();
        if trimmed.is_empty() || is_history_mgmt(trimmed) || is_trivial(trimmed) {
            continue;
        }
        // Keep the most recent occurrence: record in order, dedup later from the
        // back. Simpler: track seen and skip; but we want the *latest* position to
        // win for recency. Re-insert by removing an earlier copy.
        if seen.contains(trimmed) {
            if let Some(pos) = out.iter().position(|c| c == trimmed) {
                out.remove(pos);
            }
        } else {
            seen.insert(trimmed);
        }
        out.push(trimmed.to_string());
    }
    let start = out.len().saturating_sub(STORE_CAP);
    out.split_off(start)
}

/// True for aishe's own history-index/search invocations, which we never index.
fn is_history_mgmt(cmd: &str) -> bool {
    let c = cmd.trim_start();
    c.starts_with("aishe history") || c == "aishe history"
}

/// Trivial no-recall-value commands worth excluding from the semantic index so
/// they don't dilute results or spend embedding tokens: bare navigation/noise
/// commands (`exit`, `clear`, `pwd`, a bare `cd`/`ls`, …). Commands with arguments
/// (`cd ~/projects/x`, `ls -la /var/log`) are kept — those carry recall value.
fn is_trivial(cmd: &str) -> bool {
    let c = cmd.trim();
    // Always-noise commands regardless of arguments.
    let head = c.split_whitespace().next().unwrap_or("");
    if matches!(head, "exit" | "logout" | "clear" | "reset" | "cls") {
        return true;
    }
    // Bare navigation/inspection commands (no meaningful argument).
    matches!(
        c,
        "cd" | "ls" | "ll" | "la" | "pwd" | "cd -" | "cd ~" | "cd .."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(cmd: &str, vec: Vec<f32>) -> Entry {
        Entry {
            cmd: cmd.to_string(),
            vec,
        }
    }

    #[test]
    fn cosine_basics() {
        assert!((cosine(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-6);
        assert!(cosine(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
        // length mismatch / empty / zero-vector → 0, never NaN
        assert_eq!(cosine(&[1.0], &[1.0, 2.0]), 0.0);
        assert_eq!(cosine(&[], &[]), 0.0);
        assert_eq!(cosine(&[0.0, 0.0], &[1.0, 1.0]), 0.0);
    }

    #[test]
    fn top_k_ranks_by_similarity_and_dedups() {
        let entries = vec![
            e("git status", vec![0.0, 1.0, 0.0]),
            e("docker run -v /data prometheus", vec![1.0, 0.0, 0.0]),
            e("docker run -v /data prometheus", vec![0.9, 0.1, 0.0]), // dup, weaker
            e("ls -la", vec![0.0, 0.0, 1.0]),
        ];
        let query = vec![1.0, 0.0, 0.0];
        let top = top_k(&entries, &query, 2);
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].1, "docker run -v /data prometheus");
        // The duplicate command appears only once.
        assert!(top.iter().filter(|(_, c)| c.contains("docker")).count() == 1);
    }

    #[test]
    fn save_caps_and_load_roundtrips() {
        let path = std::env::temp_dir().join(format!("aishe-semhist-{}.vec", std::process::id()));
        std::fs::remove_file(&path).ok();
        // More than the cap: only the most recent STORE_CAP survive.
        let mut entries = Vec::new();
        for i in 0..(STORE_CAP + 5) {
            entries.push(e(&format!("cmd {i}"), vec![i as f32, 0.0]));
        }
        save(&path, &entries).unwrap();
        let loaded = load(&path);
        assert_eq!(loaded.len(), STORE_CAP);
        // The oldest 5 were dropped; the newest survived.
        assert_eq!(loaded.first().unwrap().cmd, "cmd 5");
        assert_eq!(loaded.last().unwrap().cmd, format!("cmd {}", STORE_CAP + 4));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn load_skips_corrupt_lines() {
        let path =
            std::env::temp_dir().join(format!("aishe-semhist-bad-{}.vec", std::process::id()));
        std::fs::write(
            &path,
            "{\"cmd\":\"ok\",\"vec\":[1.0]}\nnot json\n\n{\"cmd\":\"two\",\"vec\":[2.0]}\n",
        )
        .unwrap();
        let loaded = load(&path);
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].cmd, "ok");
        assert_eq!(loaded[1].cmd, "two");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn candidates_dedup_keeps_latest_and_drops_mgmt() {
        let hist = vec![
            (1, "git status".to_string()),
            (2, "docker ps".to_string()),
            (3, "git status".to_string()), // later duplicate wins position
            (4, "aishe history search foo".to_string()), // dropped
            (5, "".to_string()),           // dropped
        ];
        let c = candidates(&hist);
        assert_eq!(c, vec!["docker ps".to_string(), "git status".to_string()]);
    }

    #[test]
    fn candidates_drop_trivial_commands() {
        let hist = vec![
            (1, "exit".to_string()),              // dropped (noise)
            (2, "clear".to_string()),             // dropped
            (3, "cd".to_string()),                // dropped (bare)
            (4, "ls".to_string()),                // dropped (bare)
            (5, "cd ~/projects/app".to_string()), // kept (has a real target)
            (6, "ls -la /var/log".to_string()),   // kept (has args)
            (7, "docker compose up".to_string()), // kept
        ];
        let c = candidates(&hist);
        assert_eq!(
            c,
            vec![
                "cd ~/projects/app".to_string(),
                "ls -la /var/log".to_string(),
                "docker compose up".to_string(),
            ]
        );
    }
}
