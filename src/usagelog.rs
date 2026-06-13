//! Per-session usage tally for the interactive zsh front-end.
//!
//! The PTY front-end runs each natural-language line as a separate `aishe`
//! child process, so no single in-process [`crate::usage::UsageMeter`] spans the
//! whole session. To still show a one-line "what did this session cost" summary
//! on exit, the PTY points the children at a shared tally file via
//! `AISHE_USAGE_FILE`; each child appends its own metered usage, and the parent
//! aggregates and prints it when zsh exits.
//!
//! The format is one tab-separated line per child: `<input>\t<output>\t<model>`.
//! Appends are best-effort and tolerant of torn/garbled lines — a missing or
//! unreadable tally just means no summary, never an error.

use std::io::Write;
use std::path::Path;

use crate::usage::{self, Price, Usage};

/// Append one process's metered usage to the session tally. Best-effort: any IO
/// error (no file, permissions) is silently ignored — usage accounting must never
/// disrupt the shell. `O_APPEND` keeps concurrent child appends from interleaving
/// within a single short line.
pub fn append(path: &Path, usage: Usage, model: &str) {
    // Defend the line format against a model name with embedded tabs/newlines.
    let model = model.replace(['\t', '\n', '\r'], " ");
    let line = format!("{}\t{}\t{}\n", usage.input, usage.output, model);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = f.write_all(line.as_bytes());
    }
}

/// Parse the tally into `(usage, model)` entries. Each line contributes one
/// request. Malformed lines are skipped. Missing file → empty.
pub fn read(path: &Path) -> Vec<(Usage, String)> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut out = Vec::new();
    for line in text.lines() {
        let mut parts = line.splitn(3, '\t');
        let (Some(i), Some(o), Some(model)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let (Ok(input), Ok(output)) = (i.trim().parse::<u64>(), o.trim().parse::<u64>()) else {
            continue;
        };
        out.push((
            Usage {
                input,
                output,
                requests: 1,
            },
            model.to_string(),
        ));
    }
    out
}

/// A one-line session summary (`aishe session: X in · Y out · N reqs · ~$Z`),
/// or `None` when the tally is empty. Cost is summed per entry using each
/// command's own model price, so a mixed-model session is still accurate;
/// unpriced models are counted and disclosed.
pub fn summarize(
    path: &Path,
    pricing: &std::collections::BTreeMap<String, Price>,
) -> Option<String> {
    let entries = read(path);
    if entries.is_empty() {
        return None;
    }
    let (mut tin, mut tout, mut reqs, mut unpriced) = (0u64, 0u64, 0u64, 0u64);
    let mut total_cost = 0f64;
    for (u, model) in &entries {
        tin += u.input;
        tout += u.output;
        reqs += u.requests;
        match usage::price_for(model, pricing) {
            Some(p) => total_cost += usage::cost(*u, p),
            None => unpriced += 1,
        }
    }
    if reqs == 0 {
        return None;
    }
    let cost_str = if unpriced == 0 {
        format!("~${total_cost:.4}")
    } else if total_cost > 0.0 {
        format!("~${total_cost:.4} (+{unpriced} unpriced)")
    } else {
        "cost n/a".to_string()
    };
    Some(format!(
        "aishe session: {} in · {} out · {reqs} req{} · {cost_str}",
        usage::group(tin),
        usage::group(tout),
        if reqs == 1 { "" } else { "s" },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("aishe-usagelog-{label}-{}", std::process::id()))
    }

    #[test]
    fn append_then_read_roundtrip() {
        let p = tmp("rt");
        std::fs::remove_file(&p).ok();
        append(
            &p,
            Usage {
                input: 100,
                output: 50,
                requests: 1,
            },
            "claude-sonnet-x",
        );
        append(
            &p,
            Usage {
                input: 7,
                output: 3,
                requests: 1,
            },
            "gpt-x",
        );
        let entries = read(&p);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].0.input, 100);
        assert_eq!(entries[0].1, "claude-sonnet-x");
        assert_eq!(entries[1].0.output, 3);
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn read_skips_malformed_lines() {
        let p = tmp("bad");
        std::fs::write(&p, "100\t50\tmodel-a\ngarbage line\n\n5\tx\tmodel-b\n").unwrap();
        let entries = read(&p);
        // Only the first line is well-formed (second has too few fields after the
        // split test, third is blank, fourth has a non-numeric token count).
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].1, "model-a");
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn summarize_aggregates_and_counts_requests() {
        let p = tmp("sum");
        std::fs::remove_file(&p).ok();
        // One priced model (matches the builtin `claude-sonnet` price) and one
        // unknown model that must be disclosed as unpriced.
        append(
            &p,
            Usage {
                input: 1000,
                output: 200,
                requests: 1,
            },
            "claude-sonnet-x",
        );
        append(
            &p,
            Usage {
                input: 2000,
                output: 300,
                requests: 1,
            },
            "totally-unknown-model",
        );
        let line = summarize(&p, &std::collections::BTreeMap::new()).unwrap();
        assert!(line.contains("3,000 in"), "got: {line}");
        assert!(line.contains("500 out"), "got: {line}");
        assert!(line.contains("2 reqs"), "got: {line}");
        assert!(line.contains("(+1 unpriced)"), "got: {line}");
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn summarize_all_unpriced_says_cost_na() {
        let p = tmp("na");
        std::fs::remove_file(&p).ok();
        append(
            &p,
            Usage {
                input: 5,
                output: 5,
                requests: 1,
            },
            "mystery-model",
        );
        let line = summarize(&p, &std::collections::BTreeMap::new()).unwrap();
        assert!(line.contains("cost n/a"), "got: {line}");
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn summarize_is_none_when_empty() {
        let p = tmp("empty");
        std::fs::remove_file(&p).ok();
        assert!(summarize(&p, &std::collections::BTreeMap::new()).is_none());
    }
}
