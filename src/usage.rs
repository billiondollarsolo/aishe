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
    unreported_requests: AtomicU64,
    reported_input: AtomicU64,
    reported_output: AtomicU64,
    attributed_requests: AtomicU64,
    attributed_input: AtomicU64,
    attributed_output: AtomicU64,
}

impl UsageMeter {
    /// Record one API response's token usage.
    pub fn record(&self, input: u64, output: u64) {
        self.record_reported(input, output, true);
    }

    /// A response without complete usage is still a request, but zero-filled
    /// missing fields cannot establish its cost.
    pub fn record_reported(&self, input: u64, output: u64, reported: bool) {
        self.record_delta(if reported {
            Usage::reported(input, output, 1)
        } else {
            Usage::unknown(input, output, 1)
        });
    }

    /// Fold a provider's delta without collapsing its request counts or coverage.
    pub fn record_delta(&self, usage: Usage) {
        self.input.fetch_add(usage.input, Ordering::Relaxed);
        self.output.fetch_add(usage.output, Ordering::Relaxed);
        self.requests.fetch_add(usage.requests, Ordering::Relaxed);
        self.unreported_requests
            .fetch_add(usage.unreported_requests, Ordering::Relaxed);
        self.reported_input
            .fetch_add(usage.reported_input, Ordering::Relaxed);
        self.reported_output
            .fetch_add(usage.reported_output, Ordering::Relaxed);
        self.attributed_requests
            .fetch_add(usage.attributed_requests, Ordering::Relaxed);
        self.attributed_input
            .fetch_add(usage.attributed_input, Ordering::Relaxed);
        self.attributed_output
            .fetch_add(usage.attributed_output, Ordering::Relaxed);
    }

    pub fn unreported_requests(&self) -> u64 {
        self.unreported_requests.load(Ordering::Relaxed)
    }

    /// A point-in-time read of the counters.
    pub fn snapshot(&self) -> Usage {
        Usage {
            input: self.input.load(Ordering::Relaxed),
            output: self.output.load(Ordering::Relaxed),
            requests: self.requests.load(Ordering::Relaxed),
            unreported_requests: self.unreported_requests.load(Ordering::Relaxed),
            reported_input: self.reported_input.load(Ordering::Relaxed),
            reported_output: self.reported_output.load(Ordering::Relaxed),
            attributed_requests: self.attributed_requests.load(Ordering::Relaxed),
            attributed_input: self.attributed_input.load(Ordering::Relaxed),
            attributed_output: self.attributed_output.load(Ordering::Relaxed),
        }
    }
}

/// A point-in-time copy of the counters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub requests: u64,
    /// Requests without complete provider usage, including legacy tallies.
    pub unreported_requests: u64,
    /// Token subtotals from requests with both provider token counts present.
    pub reported_input: u64,
    pub reported_output: u64,
    /// Complete-usage subset attributed to the recorded billing model.
    pub attributed_requests: u64,
    pub attributed_input: u64,
    pub attributed_output: u64,
}

impl Usage {
    pub fn reported(input: u64, output: u64, requests: u64) -> Self {
        Self {
            input,
            output,
            requests,
            unreported_requests: 0,
            reported_input: input,
            reported_output: output,
            attributed_requests: requests,
            attributed_input: input,
            attributed_output: output,
        }
    }

    pub fn unknown(input: u64, output: u64, requests: u64) -> Self {
        Self {
            input,
            output,
            requests,
            unreported_requests: requests,
            reported_input: 0,
            reported_output: 0,
            attributed_requests: 0,
            attributed_input: 0,
            attributed_output: 0,
        }
    }

    pub fn without_attribution(mut self) -> Self {
        self.attributed_requests = 0;
        self.attributed_input = 0;
        self.attributed_output = 0;
        self
    }

    pub fn reported_requests(&self) -> u64 {
        self.requests.saturating_sub(self.unreported_requests)
    }

    pub fn delta_since(self, before: Self) -> Self {
        Self {
            input: self.input.saturating_sub(before.input),
            output: self.output.saturating_sub(before.output),
            requests: self.requests.saturating_sub(before.requests),
            unreported_requests: self
                .unreported_requests
                .saturating_sub(before.unreported_requests),
            reported_input: self.reported_input.saturating_sub(before.reported_input),
            reported_output: self.reported_output.saturating_sub(before.reported_output),
            attributed_requests: self
                .attributed_requests
                .saturating_sub(before.attributed_requests),
            attributed_input: self
                .attributed_input
                .saturating_sub(before.attributed_input),
            attributed_output: self
                .attributed_output
                .saturating_sub(before.attributed_output),
        }
    }

    pub fn add(&mut self, usage: Self) {
        self.input = self.input.saturating_add(usage.input);
        self.output = self.output.saturating_add(usage.output);
        self.requests = self.requests.saturating_add(usage.requests);
        self.unreported_requests = self
            .unreported_requests
            .saturating_add(usage.unreported_requests);
        self.reported_input = self.reported_input.saturating_add(usage.reported_input);
        self.reported_output = self.reported_output.saturating_add(usage.reported_output);
        self.attributed_requests = self
            .attributed_requests
            .saturating_add(usage.attributed_requests);
        self.attributed_input = self.attributed_input.saturating_add(usage.attributed_input);
        self.attributed_output = self
            .attributed_output
            .saturating_add(usage.attributed_output);
    }

    pub fn tokens_label(&self) -> String {
        if self.reported_requests() == 0 && !self.is_empty() {
            return "tokens n/a".into();
        }
        format!(
            "{} in · {} out{}",
            group(self.reported_input),
            group(self.reported_output),
            if self.unreported_requests > 0 {
                " (partial)"
            } else {
                ""
            }
        )
    }

    pub fn compact_tokens_label(&self) -> String {
        if self.reported_requests() == 0 && !self.is_empty() {
            return "tokens n/a".into();
        }
        format!(
            "{}/{} tok{}",
            group(self.reported_input),
            group(self.reported_output),
            if self.unreported_requests > 0 {
                " (partial)"
            } else {
                ""
            }
        )
    }

    pub fn is_empty(&self) -> bool {
        self.requests == 0
    }
}

/// Honest display accounting across differently priced models. Enforcement
/// continues to use `cost` and its existing admission rules.
#[derive(Debug, Default)]
pub struct CostTally {
    pub subtotal: f64,
    pub known_requests: u64,
    pub unknown_requests: u64,
}

impl CostTally {
    pub fn add(&mut self, usage: Usage, price: Option<Price>) {
        match price.filter(|p| {
            p.input.is_finite() && p.output.is_finite() && p.input >= 0.0 && p.output >= 0.0
        }) {
            Some(price) => {
                self.subtotal += cost(
                    Usage::reported(
                        usage.attributed_input,
                        usage.attributed_output,
                        usage.attributed_requests,
                    ),
                    price,
                );
                self.known_requests = self
                    .known_requests
                    .saturating_add(usage.attributed_requests);
                self.unknown_requests = self
                    .unknown_requests
                    .saturating_add(usage.requests.saturating_sub(usage.attributed_requests));
            }
            None => self.unknown_requests = self.unknown_requests.saturating_add(usage.requests),
        }
    }

    pub fn label(&self) -> String {
        if self.known_requests == 0 && self.unknown_requests > 0 {
            "cost n/a".into()
        } else if self.unknown_requests > 0 {
            format!(
                "~${:.4} (partial; {} unknown)",
                self.subtotal, self.unknown_requests
            )
        } else {
            format!("~${:.4}", self.subtotal)
        }
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
        "grok-4.5",
        Price {
            input: 2.0,
            output: 6.0,
        },
    ),
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

/// Conservative price resolver for hard budget enforcement. Unlike display
/// estimates, it never substring-matches a user override or catalog pattern.
pub fn budget_price_for(model: &str, overrides: &BTreeMap<String, Price>) -> Option<Price> {
    if let Some(price) = overrides.get(model) {
        return Some(*price);
    }
    BUILTIN_PRICES
        .iter()
        .find(|(known, _)| known.eq_ignore_ascii_case(model))
        .map(|(_, price)| *price)
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
        "{} · {} req{}",
        usage.tokens_label(),
        usage.requests,
        if usage.requests == 1 { "" } else { "s" },
    );
    match price_for(model, overrides) {
        Some(p) => {
            let mut tally = CostTally::default();
            tally.add(usage, Some(p));
            format!("{base} · {}", tally.label())
        }
        None => format!(
            "{base} · cost n/a (no price for '{}')",
            crate::commands::display_safe(model)
        ),
    }
}

/// Group a number with thousands separators: `1234567` → `1,234,567`.
/// Group an integer with thousands separators (e.g. `12345` → `12,345`).
pub fn group(n: u64) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i).is_multiple_of(3) {
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
    fn hard_budget_prices_never_use_substring_matches() {
        let mut prices = BTreeMap::new();
        prices.insert(
            "custom".into(),
            Price {
                input: 1.0,
                output: 2.0,
            },
        );
        assert!(budget_price_for("my-custom-model", &prices).is_none());
        assert_eq!(
            budget_price_for("custom", &prices),
            prices.get("custom").copied()
        );
        assert!(budget_price_for("gpt-4o-2024-08-06", &prices).is_none());
        assert!(budget_price_for("gpt-4o", &prices).is_some());
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
        assert_eq!(s.reported_input, 13);
        assert_eq!(s.reported_output, 12);
        assert_eq!(s.unreported_requests, 0);
    }

    #[test]
    fn coverage_survives_snapshot_delta_and_mixed_partial_token_reports() {
        let meter = UsageMeter::default();
        meter.record(0, 0);
        let before = meter.snapshot();
        meter.record_reported(900, 0, false);
        meter.record(10, 5);
        let delta = meter.snapshot().delta_since(before);
        assert_eq!(delta.input, 910);
        assert_eq!(delta.output, 5);
        assert_eq!(delta.requests, 2);
        assert_eq!(delta.unreported_requests, 1);
        assert_eq!(delta.reported_input, 10);
        assert_eq!(delta.reported_output, 5);
        let text = summary(delta, "gpt-4o", &no_overrides());
        assert!(text.contains("10 in · 5 out (partial)"), "{text}");
        assert!(text.contains("partial; 1 unknown"), "{text}");
        assert!(
            !text.contains("910 in"),
            "missing output count was priced as complete: {text}"
        );
    }

    #[test]
    fn priced_missing_usage_is_unknown_but_explicit_zero_is_known() {
        let missing = summary(Usage::unknown(0, 0, 1), "gpt-4o", &no_overrides());
        assert!(
            missing.contains("tokens n/a") && missing.contains("cost n/a"),
            "{missing}"
        );
        assert!(!missing.contains("$0.0000"), "{missing}");
        let zero = summary(Usage::reported(0, 0, 1), "gpt-4o", &no_overrides());
        assert!(
            zero.contains("0 in · 0 out") && zero.contains("~$0.0000"),
            "{zero}"
        );
        let mut mixed = Usage::reported(0, 0, 1);
        mixed.add(Usage::unknown(0, 0, 1));
        let text = summary(mixed, "gpt-4o", &no_overrides());
        assert!(text.contains("~$0.0000 (partial; 1 unknown)"), "{text}");
    }

    #[test]
    fn invalid_display_prices_are_unknown() {
        let mut costs = CostTally::default();
        costs.add(
            Usage::reported(0, 0, 1),
            Some(Price {
                input: f64::NAN,
                output: 1.0,
            }),
        );
        assert_eq!(costs.label(), "cost n/a");
    }

    #[test]
    fn unknown_model_attribution_preserves_reported_tokens_without_pricing_them() {
        let mut usage = Usage::reported(1000, 500, 1).without_attribution();
        let text = summary(usage, "gpt-4o", &no_overrides());
        assert!(
            text.contains("1,000 in · 500 out · 1 req · cost n/a"),
            "{text}"
        );
        usage.add(Usage::reported(20, 30, 1));
        let text = summary(usage, "gpt-4o", &no_overrides());
        assert!(text.contains("1,020 in · 530 out · 2 reqs"), "{text}");
        assert!(text.contains("~$0.0004 (partial; 1 unknown)"), "{text}");
        assert!(
            !text.contains("tokens n/a") && !text.contains("out (partial)"),
            "{text}"
        );
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
        let u = Usage::reported(1_000_000, 1_000_000, 1);
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
        let u = Usage::reported(2_000_000, 0, 1);
        // price 3/Mtok → $6 cost ≥ $5 budget → blocked.
        assert!(over_budget(u, "claude-sonnet-4-6", &no_overrides(), 5.0));
        // budget 0 = unlimited.
        assert!(!over_budget(u, "claude-sonnet-4-6", &no_overrides(), 0.0));
        // unknown price → cannot enforce.
        assert!(!over_budget(u, "mystery-model", &no_overrides(), 0.01));
    }

    #[test]
    fn summary_groups_and_prices() {
        let u = Usage::reported(1234, 567, 1);
        let s = summary(u, "gpt-oss", &no_overrides());
        assert!(s.contains("1,234 in"));
        assert!(s.contains("567 out"));
        assert!(s.contains("~$"));
        let s2 = summary(u, "unknown", &no_overrides());
        assert!(s2.contains("cost n/a"));
    }
}
