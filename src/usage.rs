//! Token & cost accounting.
//!
//! Each provider owns a shared [`UsageMeter`] (atomic counters) and records the
//! `usage` reported by every API response. The modes read the meter to display a
//! per-session `tokens · ~$cost` line and to enforce an optional `budget_usd`
//! guard. Cost is derived from a small built-in price table (USD per million
//! tokens), overridable per model in `[pricing]`.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

/// Shared, thread-safe token counters for one process/session.
#[derive(Debug, Default)]
pub struct UsageMeter {
    input: AtomicU64,
    output: AtomicU64,
    requests: AtomicU64,
}

impl UsageMeter {
    /// Record one API response's token usage.
    pub fn record(&self, input: u64, output: u64) {
        self.input.fetch_add(input, Ordering::Relaxed);
        self.output.fetch_add(output, Ordering::Relaxed);
        self.requests.fetch_add(1, Ordering::Relaxed);
    }

    /// A point-in-time read of the counters.
    pub fn snapshot(&self) -> Usage {
        Usage {
            input: self.input.load(Ordering::Relaxed),
            output: self.output.load(Ordering::Relaxed),
            requests: self.requests.load(Ordering::Relaxed),
        }
    }
}

/// A point-in-time copy of the counters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub requests: u64,
}

impl Usage {
    pub fn is_empty(&self) -> bool {
        self.requests == 0
    }
}

/// Price in USD per **million** tokens.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Price {
    pub input: f64,
    pub output: f64,
}

/// Built-in price table (USD / 1M tokens), most-specific patterns first. These
/// are best-effort estimates; override exact figures in `[pricing]`.
const BUILTIN_PRICES: &[(&str, Price)] = &[
    (
        "claude-opus",
        Price {
            input: 15.0,
            output: 75.0,
        },
    ),
    (
        "claude-sonnet",
        Price {
            input: 3.0,
            output: 15.0,
        },
    ),
    (
        "claude-haiku",
        Price {
            input: 0.80,
            output: 4.0,
        },
    ),
    (
        "claude-3-5-haiku",
        Price {
            input: 0.80,
            output: 4.0,
        },
    ),
    (
        "claude-3-haiku",
        Price {
            input: 0.25,
            output: 1.25,
        },
    ),
    (
        "gpt-4o-mini",
        Price {
            input: 0.15,
            output: 0.60,
        },
    ),
    (
        "gpt-4o",
        Price {
            input: 2.50,
            output: 10.0,
        },
    ),
    (
        "gpt-4.1-mini",
        Price {
            input: 0.40,
            output: 1.60,
        },
    ),
    (
        "gpt-4.1",
        Price {
            input: 2.0,
            output: 8.0,
        },
    ),
    (
        "gpt-oss",
        Price {
            input: 0.15,
            output: 0.60,
        },
    ),
    (
        "o3-mini",
        Price {
            input: 1.10,
            output: 4.40,
        },
    ),
    (
        "o1-mini",
        Price {
            input: 1.10,
            output: 4.40,
        },
    ),
];

/// Resolve a price for `model`: exact `[pricing]` override, then substring
/// override, then the built-in table. `None` when nothing matches.
pub fn price_for(model: &str, overrides: &BTreeMap<String, Price>) -> Option<Price> {
    if let Some(p) = overrides.get(model) {
        return Some(*p);
    }
    let m = model.to_lowercase();
    for (k, v) in overrides {
        if m.contains(&k.to_lowercase()) {
            return Some(*v);
        }
    }
    for (pat, p) in BUILTIN_PRICES {
        if m.contains(pat) {
            return Some(*p);
        }
    }
    None
}

/// Estimated USD cost for `usage` at `price`.
pub fn cost(usage: Usage, price: Price) -> f64 {
    (usage.input as f64 / 1_000_000.0) * price.input
        + (usage.output as f64 / 1_000_000.0) * price.output
}

/// `true` when a positive budget is set, a price is known, and the accrued cost
/// has reached it. Unknown price ⇒ cannot enforce ⇒ never blocks.
pub fn over_budget(
    usage: Usage,
    model: &str,
    overrides: &BTreeMap<String, Price>,
    budget_usd: f64,
) -> bool {
    if budget_usd <= 0.0 {
        return false;
    }
    match price_for(model, overrides) {
        Some(p) => cost(usage, p) >= budget_usd,
        None => false,
    }
}

/// A one-line, human-readable usage summary, e.g.
/// `1,234 in · 567 out · 3 reqs · ~$0.0021`. Cost is omitted (with a hint) when
/// the model's price is unknown.
pub fn summary(usage: Usage, model: &str, overrides: &BTreeMap<String, Price>) -> String {
    let base = format!(
        "{} in · {} out · {} req{}",
        group(usage.input),
        group(usage.output),
        usage.requests,
        if usage.requests == 1 { "" } else { "s" },
    );
    match price_for(model, overrides) {
        Some(p) => format!("{base} · ~${:.4}", cost(usage, p)),
        None => format!("{base} · cost n/a (no price for '{model}')"),
    }
}

/// Group a number with thousands separators: `1234567` → `1,234,567`.
/// Group an integer with thousands separators (e.g. `12345` → `12,345`).
pub fn group(n: u64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_overrides() -> BTreeMap<String, Price> {
        BTreeMap::new()
    }

    #[test]
    fn meter_records_and_snapshots() {
        let m = UsageMeter::default();
        m.record(10, 5);
        m.record(3, 7);
        let s = m.snapshot();
        assert_eq!(s.input, 13);
        assert_eq!(s.output, 12);
        assert_eq!(s.requests, 2);
    }

    #[test]
    fn builtin_price_matches_substring() {
        let p = price_for("openai/gpt-oss-120b", &no_overrides()).unwrap();
        assert_eq!(p.input, 0.15);
        // claude-opus is more specific than a bare "claude".
        let p = price_for("claude-opus-4-8", &no_overrides()).unwrap();
        assert_eq!(p.output, 75.0);
    }

    #[test]
    fn override_wins_over_builtin() {
        let mut ov = BTreeMap::new();
        ov.insert(
            "gpt-oss".to_string(),
            Price {
                input: 1.0,
                output: 2.0,
            },
        );
        let p = price_for("openai/gpt-oss-120b", &ov).unwrap();
        assert_eq!(p.input, 1.0);
    }

    #[test]
    fn unknown_model_has_no_price() {
        assert!(price_for("some-local-llama", &no_overrides()).is_none());
    }

    #[test]
    fn cost_math() {
        let u = Usage {
            input: 1_000_000,
            output: 1_000_000,
            requests: 1,
        };
        let c = cost(
            u,
            Price {
                input: 3.0,
                output: 15.0,
            },
        );
        assert!((c - 18.0).abs() < 1e-9);
    }

    #[test]
    fn budget_enforced_only_with_known_price() {
        let u = Usage {
            input: 2_000_000,
            output: 0,
            requests: 1,
        };
        // price 3/Mtok → $6 cost ≥ $5 budget → blocked.
        assert!(over_budget(u, "claude-sonnet-4-6", &no_overrides(), 5.0));
        // budget 0 = unlimited.
        assert!(!over_budget(u, "claude-sonnet-4-6", &no_overrides(), 0.0));
        // unknown price → cannot enforce.
        assert!(!over_budget(u, "mystery-model", &no_overrides(), 0.01));
    }

    #[test]
    fn summary_groups_and_prices() {
        let u = Usage {
            input: 1234,
            output: 567,
            requests: 1,
        };
        let s = summary(u, "gpt-oss", &no_overrides());
        assert!(s.contains("1,234 in"));
        assert!(s.contains("567 out"));
        assert!(s.contains("~$"));
        let s2 = summary(u, "unknown", &no_overrides());
        assert!(s2.contains("cost n/a"));
    }
}
