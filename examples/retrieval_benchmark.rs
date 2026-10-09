//! Compare bounded retrieval against the pre-refactor algorithms locally.
//! Run: cargo run --release --locked --example retrieval_benchmark

use std::collections::HashSet;
use std::hint::black_box;
use std::time::Instant;

use aishe::repo_index::{FileEntry, Index, Match};

fn previous_search(index: &Index, query: &str, limit: usize) -> Vec<Match> {
    let terms: Vec<_> = query
        .split(|ch: char| !ch.is_alphanumeric() && ch != '_')
        .filter(|term| term.len() > 1)
        .map(str::to_ascii_lowercase)
        .collect();
    let mut hits = Vec::new();
    for file in &index.files {
        for (chunk, text) in file.chunks.iter().enumerate() {
            let haystack = format!("{}\n{text}", file.path).to_ascii_lowercase();
            let score = terms
                .iter()
                .map(|term| haystack.match_indices(term).count())
                .sum();
            if score > 0 {
                hits.push(Match {
                    path: file.path.clone(),
                    chunk,
                    score,
                    hash: file.hash.clone(),
                    text: text.clone(),
                });
            }
        }
    }
    hits.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.path.cmp(&b.path)));
    hits.truncate(limit);
    hits
}

fn previous_candidates(history: &[(u64, String)]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    // Fixture commands are all eligible; preserve the former duplicate handling.
    for (_, command) in history {
        if seen.contains(command) {
            if let Some(position) = out.iter().position(|c| c == command) {
                out.remove(position);
            }
        } else {
            seen.insert(command);
        }
        out.push(command.clone());
    }
    out.split_off(out.len().saturating_sub(aishe::semhist::STORE_CAP))
}

fn median_ms<T>(mut f: impl FnMut() -> T) -> f64 {
    black_box(f());
    let mut timings = Vec::new();
    for _ in 0..7 {
        let start = Instant::now();
        black_box(f());
        timings.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    timings.sort_by(f64::total_cmp);
    timings[timings.len() / 2]
}

fn main() {
    let index = Index {
        schema_version: 1,
        repository: "/benchmark".into(),
        head: "fixture".into(),
        updated_at_ms: 0,
        files: (0..2_000)
            .map(|i| {
                let chunks: Vec<_> = (0..4)
                    .map(|j| format!("token {i} {j}\n{}", "reference data\n".repeat(540)))
                    .collect();
                FileEntry {
                    path: format!("src/{i:05}.rs"),
                    hash: "fixture".into(),
                    language: "rs".into(),
                    bytes: chunks.iter().map(String::len).sum(),
                    chunks,
                }
            })
            .collect(),
    };
    let history: Vec<_> = (0..50_000)
        .map(|i| (i, format!("echo command-{}", i % 5_000)))
        .collect();
    let previous = previous_search(&index, "token", 5);
    let current = aishe::repo_index::search(&index, "token", 5);
    assert_eq!(
        serde_json::to_value(previous).unwrap(),
        serde_json::to_value(current).unwrap()
    );
    assert_eq!(
        previous_candidates(&history),
        aishe::semhist::candidates(&history)
    );
    let report = serde_json::json!({
        "schema_version": 1,
        "method": "median of 7 same-process release samples after warmup; synthetic fixtures",
        "repo_search": {
            "chunks": 8_000,
            "text_bytes": index.files.iter().map(|file| file.bytes).sum::<usize>(),
            "limit": 5,
            "previous_ms": median_ms(|| previous_search(black_box(&index), "token", 5)),
            "current_ms": median_ms(|| aishe::repo_index::search(black_box(&index), "token", 5)),
        },
        "history_candidates": {
            "commands": history.len(),
            "unique_commands": 5_000,
            "previous_ms": median_ms(|| previous_candidates(black_box(&history))),
            "current_ms": median_ms(|| aishe::semhist::candidates(black_box(&history))),
        },
    });
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
}
