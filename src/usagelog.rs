//! Per-session usage tally for the interactive zsh front-end.
//!
//! The PTY front-end runs each natural-language line as a separate `aishe`
//! child process, so no single in-process [`crate::usage::UsageMeter`] spans the
//! whole session. To still show a one-line "what did this session cost" summary
//! on exit, the PTY points the children at a shared tally file via
//! `AISHE_USAGE_FILE`; each child appends its own metered usage, and the parent
//! aggregates and prints it when zsh exits.
//!
//! The current format is one versioned tab-separated line per child:
//! `v2\t<input>\t<output>\t<requests>\t<model>\t<connection>`. The reader also
//! accepts the older model-only three- and four-column formats.
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
    append_attributed(path, usage, model, None);
}

/// Append usage with a safe connection ID so live status can filter spend after
/// a shell switches between multiple credentials for the same model.
pub fn append_attributed(path: &Path, usage: Usage, model: &str, connection_id: Option<&str>) {
    // Defend the line format against a model name with embedded tabs/newlines.
    let model = model.replace(['\t', '\n', '\r'], " ");
    let connection_id = connection_id
        .unwrap_or("legacy/unknown")
        .replace(['\t', '\n', '\r'], " ");
    let line = format!(
        "v2\t{}\t{}\t{}\t{}\t{}\n",
        usage.input, usage.output, usage.requests, model, connection_id
    );
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = f.write_all(line.as_bytes());
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Entry {
    pub usage: Usage,
    pub model: String,
    pub connection_id: Option<String>,
}

pub fn read_entries(path: &Path) -> Vec<Entry> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut out = Vec::new();
    for line in text.lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        let (i, o, requests, model, connection_id) = if parts.first() == Some(&"v2") {
            if parts.len() != 6 {
                continue;
            }
            (
                parts[1],
                parts[2],
                parts[3],
                parts[4].to_string(),
                Some(parts[5].to_string()),
            )
        } else if parts.len() >= 4 {
            (parts[0], parts[1], parts[2], parts[3..].join(" "), None)
        } else if parts.len() == 3 {
            (parts[0], parts[1], "1", parts[2].to_string(), None)
        } else {
            continue;
        };
        let (Ok(input), Ok(output)) = (i.trim().parse::<u64>(), o.trim().parse::<u64>()) else {
            continue;
        };
        let Ok(requests) = requests.trim().parse::<u64>() else {
            continue;
        };
        out.push(Entry {
            usage: Usage {
                input,
                output,
                requests,
            },
            model,
            connection_id,
        });
    }
    out
}

/// Parse the tally into `(usage, model)` entries. Each line contributes one
/// request. Malformed lines are skipped. Missing file → empty.
pub fn read(path: &Path) -> Vec<(Usage, String)> {
    read_entries(path)
        .into_iter()
        .map(|entry| (entry.usage, entry.model))
        .collect()
}

/// A one-line session summary (`aishe session: X in · Y out · N reqs · ~$Z`),
/// or `None` when the tally is empty. Cost is summed per entry using each
/// command's own model price, so a mixed-model session is still accurate;
/// unpriced models are counted and disclosed.
pub fn summarize(
    path: &Path,
    pricing: &std::collections::BTreeMap<String, Price>,
) -> Option<String> {
    summarize_for_connection(path, pricing, None)
}

pub fn summarize_for_connection(
    path: &Path,
    pricing: &std::collections::BTreeMap<String, Price>,
    connection_id: Option<&str>,
) -> Option<String> {
    let entries: Vec<Entry> = read_entries(path)
        .into_iter()
        .filter(|entry| {
            connection_id.is_none()
                || entry.connection_id.as_deref() == connection_id
                || entry.connection_id.is_none()
        })
        .collect();
    if entries.is_empty() {
        return None;
    }
    let (mut tin, mut tout, mut reqs, mut unpriced) = (0u64, 0u64, 0u64, 0u64);
    let mut total_cost = 0f64;
    for entry in &entries {
        tin += entry.usage.input;
        tout += entry.usage.output;
        reqs += entry.usage.requests;
        match usage::price_for(&entry.model, pricing) {
            Some(p) => total_cost += usage::cost(entry.usage, p),
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

/// Render the dynamic metrics portion of the live prompt status. Model and mode
/// are rendered by zsh from their session variables so a Shift-Tab mode change
/// is visible immediately without spawning a helper process.
pub fn status_metrics(
    path: &Path,
    pricing: &std::collections::BTreeMap<String, Price>,
    last: Option<(Usage, &str)>,
    items: &[String],
) -> String {
    status_values(path, pricing, last, items)
        .into_iter()
        .map(|(_, value)| value)
        .collect::<Vec<_>>()
        .join(" · ")
}

fn status_values(
    path: &Path,
    pricing: &std::collections::BTreeMap<String, Price>,
    last: Option<(Usage, &str)>,
    items: &[String],
) -> Vec<(String, String)> {
    status_values_for_connection(path, pricing, last, items, None)
}

fn status_values_for_connection(
    path: &Path,
    pricing: &std::collections::BTreeMap<String, Price>,
    last: Option<(Usage, &str)>,
    items: &[String],
    connection_id: Option<&str>,
) -> Vec<(String, String)> {
    let entries: Vec<Entry> = read_entries(path)
        .into_iter()
        .filter(|entry| {
            connection_id.is_none()
                || entry.connection_id.as_deref() == connection_id
                || entry.connection_id.is_none()
        })
        .collect();
    let mut total = Usage::default();
    let mut total_cost = 0.0;
    let mut unpriced = 0u64;
    for entry in &entries {
        total.input += entry.usage.input;
        total.output += entry.usage.output;
        total.requests += entry.usage.requests;
        match usage::price_for(&entry.model, pricing) {
            Some(price) => total_cost += usage::cost(entry.usage, price),
            None => unpriced += entry.usage.requests,
        }
    }
    let mut fields = Vec::new();
    for item in items {
        let value = match item.as_str() {
            "last_tokens" => last.map(|(usage, _)| {
                format!(
                    "last {}/{} tok",
                    usage::group(usage.input),
                    usage::group(usage.output)
                )
            }),
            "last_cost" => last.map(|(usage, model)| match usage::price_for(model, pricing) {
                Some(price) => format!("last ~${:.4}", usage::cost(usage, price)),
                None => "last cost n/a".to_string(),
            }),
            "session_tokens" if !total.is_empty() => Some(format!(
                "session {}/{} tok",
                usage::group(total.input),
                usage::group(total.output)
            )),
            "session_cost" if !total.is_empty() => Some(if unpriced == 0 {
                format!("session ~${total_cost:.4}")
            } else if total_cost > 0.0 {
                format!("session ~${total_cost:.4} +{unpriced} unpriced")
            } else {
                "session cost n/a".to_string()
            }),
            "requests" if !total.is_empty() => Some(format!(
                "{} req{}",
                total.requests,
                if total.requests == 1 { "" } else { "s" }
            )),
            // Subscription quota is merged only from an authoritative provider.
            "plan" => None,
            // model/mode/auth/connection are handled in the parent shell;
            // unknown fields are ignored for forward/backward compatibility.
            _ => None,
        };
        if let Some(value) = value {
            fields.push((item.clone(), value));
        }
    }
    fields
}

/// Atomically refresh the file consumed by the parent zsh prompt.
pub fn write_status(
    status_path: &Path,
    usage_path: &Path,
    pricing: &std::collections::BTreeMap<String, Price>,
    last: Option<(Usage, &str)>,
    items: &[String],
) {
    let rendered = status_values(usage_path, pricing, last, items)
        .into_iter()
        .map(|(key, value)| {
            format!(
                "{}\t{}\n",
                key.replace(['\t', '\n', '\r'], " "),
                value.replace(['\t', '\n', '\r'], " ")
            )
        })
        .collect::<String>();
    let _ = crate::config::write_atomic(status_path, rendered.as_bytes());
}

pub fn write_status_for_connection(
    status_path: &Path,
    usage_path: &Path,
    pricing: &std::collections::BTreeMap<String, Price>,
    last: Option<(Usage, &str)>,
    items: &[String],
    connection_id: &str,
) {
    let rendered =
        status_values_for_connection(usage_path, pricing, last, items, Some(connection_id))
            .into_iter()
            .map(|(key, value)| {
                format!(
                    "{}\t{}\n",
                    key.replace(['\t', '\n', '\r'], " "),
                    value.replace(['\t', '\n', '\r'], " ")
                )
            })
            .collect::<String>();
    let _ = crate::config::write_atomic(status_path, rendered.as_bytes());
}

/// Merge bounded non-usage agent metadata into the status file. Values are
/// terminal-sanitized by the zsh prompt renderer and line-format sanitized
/// here. This deliberately carries no prompt or reasoning text.
pub fn merge_status(status_path: &Path, fields: &[(&str, String)]) {
    let existing = std::fs::read(status_path)
        .ok()
        .filter(|bytes| bytes.len() <= 64 * 1024)
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .unwrap_or_default();
    let mut values = std::collections::BTreeMap::new();
    for line in existing.lines() {
        if let Some((key, value)) = line.split_once('\t') {
            values.insert(key.to_string(), value.to_string());
        }
    }
    for (key, value) in fields {
        values.insert(
            key.replace(['\t', '\n', '\r'], " "),
            value.replace(['\t', '\n', '\r'], " "),
        );
    }
    let rendered = values
        .into_iter()
        .map(|(key, value)| format!("{key}\t{value}\n"))
        .collect::<String>();
    let _ = crate::config::write_atomic(status_path, rendered.as_bytes());
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
    fn attributed_usage_filters_same_model_connections() {
        let usage_path = tmp("connections");
        let status_path = tmp("connections-status");
        std::fs::remove_file(&usage_path).ok();
        append_attributed(
            &usage_path,
            Usage {
                input: 10,
                output: 1,
                requests: 1,
            },
            "same-model",
            Some("openai-work"),
        );
        append_attributed(
            &usage_path,
            Usage {
                input: 20,
                output: 2,
                requests: 1,
            },
            "same-model",
            Some("openai-personal"),
        );
        let entries = read_entries(&usage_path);
        assert_eq!(entries[0].connection_id.as_deref(), Some("openai-work"));
        assert_eq!(entries[1].connection_id.as_deref(), Some("openai-personal"));
        let pricing = std::collections::BTreeMap::new();
        let summary = summarize_for_connection(&usage_path, &pricing, Some("openai-work")).unwrap();
        assert!(summary.contains("session: 10 in"));
        assert!(!summary.contains("30 in"));

        write_status_for_connection(
            &status_path,
            &usage_path,
            &pricing,
            None,
            &["session_tokens".into(), "requests".into()],
            "openai-personal",
        );
        let status = std::fs::read_to_string(&status_path).unwrap();
        assert!(status.contains("session 20/2 tok"));
        assert!(status.contains("1 req"));
        assert!(!status.contains("30/3"));
        std::fs::remove_file(usage_path).ok();
        std::fs::remove_file(status_path).ok();
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

    #[test]
    fn status_metrics_are_selectable_and_preserve_unknown_cost() {
        let p = tmp("status");
        std::fs::remove_file(&p).ok();
        let last = Usage {
            input: 1697,
            output: 374,
            requests: 2,
        };
        append(&p, last, "gpt-5.6-luna");
        let rendered = status_metrics(
            &p,
            &std::collections::BTreeMap::new(),
            Some((last, "gpt-5.6-luna")),
            &[
                "last_tokens".into(),
                "last_cost".into(),
                "session_cost".into(),
                "requests".into(),
            ],
        );
        assert!(rendered.contains("last 1,697/374 tok"), "{rendered}");
        assert!(rendered.contains("last cost n/a"), "{rendered}");
        assert!(rendered.contains("session cost n/a"), "{rendered}");
        assert!(rendered.contains("2 reqs"), "{rendered}");
        std::fs::remove_file(&p).ok();
    }

    #[test]
    fn status_file_preserves_configured_order_and_mixed_model_accounting() {
        let usage_path = tmp("status-order-usage");
        let status_path = tmp("status-order-rendered");
        std::fs::remove_file(&usage_path).ok();
        std::fs::remove_file(&status_path).ok();
        let priced = Usage {
            input: 1_000,
            output: 500,
            requests: 2,
        };
        let unpriced = Usage {
            input: 20,
            output: 10,
            requests: 3,
        };
        append(&usage_path, priced, "priced-exact");
        append(&usage_path, unpriced, "unknown-exact");
        let pricing = [(
            "priced-exact".to_string(),
            Price {
                input: 1.0,
                output: 2.0,
            },
        )]
        .into_iter()
        .collect();
        let items = vec![
            "requests".into(),
            "session_cost".into(),
            "session_tokens".into(),
        ];
        write_status(
            &status_path,
            &usage_path,
            &pricing,
            Some((unpriced, "unknown-exact")),
            &items,
        );
        let rendered = std::fs::read_to_string(&status_path).unwrap();
        let lines: Vec<&str> = rendered.lines().collect();
        assert!(lines[0].starts_with("requests\t5 reqs"), "{rendered}");
        assert!(
            lines[1].starts_with("session_cost\tsession ~$0.0020 +3 unpriced"),
            "{rendered}"
        );
        assert!(
            lines[2].starts_with("session_tokens\tsession 1,020/510 tok"),
            "{rendered}"
        );
        std::fs::remove_file(usage_path).ok();
        std::fs::remove_file(status_path).ok();
    }
}
