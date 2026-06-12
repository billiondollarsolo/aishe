//! Provider fallback chain.
//!
//! Tries the primary provider, and on a *terminal* error (the underlying provider
//! already retries transient 429/5xx internally, so any error reaching us is
//! terminal) advances to the next configured provider — so a dead endpoint, a
//! hard auth failure, or a blown budget degrades to a secondary (or a local
//! model) instead of failing the call outright. Usage from whichever provider
//! answered is folded into one shared meter, so the per-session cost line and the
//! budget guard stay correct across the chain.
//!
//! Note: a `FallbackProvider` serves streaming via the trait defaults, which call
//! the (fallback-enabled) non-streaming methods. So configuring a fallback chain
//! trades live token streaming for resilience; single-provider setups are
//! unwrapped by `make` and stream as before.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use crossterm::style::Stylize;

use super::{Completion, Msg, Provider, ProviderError, ResponseFormat, ToolDef};
use crate::usage::{Usage, UsageMeter};

/// A provider that tries each member of a chain in order.
pub struct FallbackProvider {
    /// `(display name, provider)`, primary first.
    chain: Vec<(String, Arc<dyn Provider>)>,
    /// Unified meter returned to callers (cost display + budget).
    meter: Arc<UsageMeter>,
    /// Last-seen usage snapshot per chain member, so new usage is folded once.
    seen: Mutex<Vec<Usage>>,
    /// Fallback indices already announced, so the notice prints at most once each.
    announced: Mutex<HashSet<usize>>,
}

impl FallbackProvider {
    /// Build a fallback provider over `chain` (primary first). Caller ensures it
    /// has at least two members (a single member needs no wrapper).
    pub fn new(chain: Vec<(String, Arc<dyn Provider>)>) -> Self {
        let n = chain.len();
        Self {
            chain,
            meter: Arc::new(UsageMeter::default()),
            seen: Mutex::new(vec![Usage::default(); n]),
            announced: Mutex::new(HashSet::new()),
        }
    }

    /// Fold any new usage recorded by chain member `i` into the unified meter.
    fn sync_usage(&self, i: usize) {
        let snap = self.chain[i].1.meter().snapshot();
        let mut seen = self.seen.lock().unwrap();
        let prev = seen[i];
        let din = snap.input.saturating_sub(prev.input);
        let dout = snap.output.saturating_sub(prev.output);
        let dreq = snap.requests.saturating_sub(prev.requests);
        // One provider call records one request; fold the token delta onto it.
        if dreq > 0 || din > 0 || dout > 0 {
            self.meter.record(din, dout);
        }
        seen[i] = snap;
    }

    /// Print a one-time notice that we are moving from chain member `from` to the
    /// next one.
    fn announce(&self, from: usize, reason: &str) {
        let next = from + 1;
        if self.announced.lock().unwrap().insert(next) {
            eprintln!(
                "{}",
                format!(
                    "aishe: provider '{}' failed ({reason}); falling back to '{}'",
                    self.chain[from].0, self.chain[next].0
                )
                .dim()
            );
        }
    }
}

impl Provider for FallbackProvider {
    fn complete(
        &self,
        system: &str,
        messages: &[Msg],
        format: &ResponseFormat,
    ) -> Result<String, ProviderError> {
        let last = self.chain.len() - 1;
        for i in 0..self.chain.len() {
            let result = self.chain[i].1.complete(system, messages, format);
            self.sync_usage(i);
            match result {
                Ok(out) => return Ok(out),
                Err(e) if i == last => return Err(e),
                Err(e) => self.announce(i, &e.to_string()),
            }
        }
        unreachable!("chain is non-empty")
    }

    fn complete_with_tools(
        &self,
        system: &str,
        messages: &[Msg],
        tools: &[ToolDef],
    ) -> Result<Completion, ProviderError> {
        let last = self.chain.len() - 1;
        for i in 0..self.chain.len() {
            let result = self.chain[i].1.complete_with_tools(system, messages, tools);
            self.sync_usage(i);
            match result {
                Ok(out) => return Ok(out),
                Err(e) if i == last => return Err(e),
                Err(e) => self.announce(i, &e.to_string()),
            }
        }
        unreachable!("chain is non-empty")
    }

    fn meter(&self) -> Arc<UsageMeter> {
        self.meter.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A test double that always succeeds, recording fixed token usage.
    struct Ok2 {
        out: String,
        tin: u64,
        tout: u64,
        meter: Arc<UsageMeter>,
    }
    impl Ok2 {
        fn mk(out: &str, tin: u64, tout: u64) -> Arc<dyn Provider> {
            Arc::new(Self {
                out: out.to_string(),
                tin,
                tout,
                meter: Arc::new(UsageMeter::default()),
            })
        }
    }
    impl Provider for Ok2 {
        fn complete(
            &self,
            _: &str,
            _: &[Msg],
            _: &ResponseFormat,
        ) -> Result<String, ProviderError> {
            self.meter.record(self.tin, self.tout);
            Ok(self.out.clone())
        }
        fn complete_with_tools(
            &self,
            _: &str,
            _: &[Msg],
            _: &[ToolDef],
        ) -> Result<Completion, ProviderError> {
            self.meter.record(self.tin, self.tout);
            Ok(Completion {
                text: Some(self.out.clone()),
                tool_calls: Vec::new(),
            })
        }
        fn meter(&self) -> Arc<UsageMeter> {
            self.meter.clone()
        }
    }

    /// A test double that always fails.
    struct Boom;
    impl Boom {
        fn mk() -> Arc<dyn Provider> {
            Arc::new(Self)
        }
    }
    impl Provider for Boom {
        fn complete(
            &self,
            _: &str,
            _: &[Msg],
            _: &ResponseFormat,
        ) -> Result<String, ProviderError> {
            Err(ProviderError::Http("connection refused".into()))
        }
        fn complete_with_tools(
            &self,
            _: &str,
            _: &[Msg],
            _: &[ToolDef],
        ) -> Result<Completion, ProviderError> {
            Err(ProviderError::Http("connection refused".into()))
        }
        fn meter(&self) -> Arc<UsageMeter> {
            Arc::new(UsageMeter::default())
        }
    }

    fn fmt() -> ResponseFormat {
        ResponseFormat::Text
    }

    #[test]
    fn falls_back_past_a_failing_primary() {
        let fb = FallbackProvider::new(vec![
            ("primary".into(), Boom::mk()),
            ("secondary".into(), Ok2::mk("hello", 10, 5)),
        ]);
        let out = fb.complete("s", &[], &fmt()).unwrap();
        assert_eq!(out, "hello");
        // Usage from the secondary is folded into the unified meter.
        let snap = fb.meter().snapshot();
        assert_eq!((snap.input, snap.output, snap.requests), (10, 5, 1));
    }

    #[test]
    fn uses_primary_when_it_succeeds() {
        let fb = FallbackProvider::new(vec![
            ("primary".into(), Ok2::mk("from-primary", 7, 3)),
            ("secondary".into(), Ok2::mk("from-secondary", 99, 99)),
        ]);
        assert_eq!(fb.complete("s", &[], &fmt()).unwrap(), "from-primary");
        let snap = fb.meter().snapshot();
        assert_eq!((snap.input, snap.output, snap.requests), (7, 3, 1));
    }

    #[test]
    fn last_error_propagates_when_all_fail() {
        let fb = FallbackProvider::new(vec![
            ("primary".into(), Boom::mk()),
            ("secondary".into(), Boom::mk()),
        ]);
        assert!(fb.complete("s", &[], &fmt()).is_err());
    }

    #[test]
    fn tools_path_falls_back_too() {
        let fb = FallbackProvider::new(vec![
            ("primary".into(), Boom::mk()),
            ("secondary".into(), Ok2::mk("answer", 4, 2)),
        ]);
        let c = fb.complete_with_tools("s", &[], &[]).unwrap();
        assert_eq!(c.text.as_deref(), Some("answer"));
        assert_eq!(fb.meter().snapshot().input, 4);
    }

    #[test]
    fn usage_accumulates_across_calls() {
        let fb = FallbackProvider::new(vec![
            ("primary".into(), Boom::mk()),
            ("secondary".into(), Ok2::mk("x", 10, 5)),
        ]);
        fb.complete("s", &[], &fmt()).unwrap();
        fb.complete("s", &[], &fmt()).unwrap();
        let snap = fb.meter().snapshot();
        assert_eq!((snap.input, snap.output, snap.requests), (20, 10, 2));
    }
}
