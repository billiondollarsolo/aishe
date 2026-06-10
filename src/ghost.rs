//! Inline AI ghost-text autosuggestion for the reedline front-end.
//!
//! As you type, a background worker asks the model for the most likely full
//! command line and caches it. The reedline [`Hinter`] shows the remainder as dim
//! ghost text (accept with the Right arrow, like history hints), falling back to
//! ordinary history hints when there is no prediction.
//!
//! Design notes:
//! - The model call happens on a background thread (debounced + cached) so typing
//!   never blocks on the network.
//! - The worker shares the main provider (via `Arc`), so ghost tokens count in
//!   the same usage meter and respect the same `budget_usd`.
//! - reedline only repaints on input events, so a prediction that finishes during
//!   a pause appears on the next keystroke (the ghost tracks your prefix as you
//!   type).
//! - Off by default (it spends tokens as you type); toggle with `aishe ghost on`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nu_ansi_term::{Color, Style};
use reedline::{DefaultHinter, Hinter, History};

use crate::config::Config;
use crate::providers::{Msg, Provider, ResponseFormat};

/// Don't predict for very short inputs (noise and cost).
const MIN_PREFIX: usize = 3;
/// Wait until typing has been idle this long before firing a prediction.
const DEBOUNCE: Duration = Duration::from_millis(300);
/// Worker poll interval.
const POLL: Duration = Duration::from_millis(60);

const GHOST_SYSTEM: &str = "You are a fast command-line autocomplete for a Unix \
shell. Given the user's partial command line, reply with ONLY the single most \
likely complete command line, beginning with exactly the characters the user has \
typed. No explanation, no markdown, no code fences, no surrounding quotes. If you \
are unsure, repeat the user's text unchanged.";

/// State shared between the reedline hinter (main thread) and the worker.
#[derive(Default)]
struct Shared {
    enabled: bool,
    /// The buffer the user is currently editing and when it last changed.
    latest: String,
    latest_at: Option<Instant>,
    cwd: String,
    /// The cached prediction (a full command line) and the prefix it was made
    /// for. `predicted_for == Some(latest)` means "already attempted".
    prediction: String,
    predicted_for: Option<String>,
}

/// Handle to the ghost-text subsystem: shared state plus the worker thread.
pub struct Ghost {
    shared: Arc<Mutex<Shared>>,
    stop: Arc<AtomicBool>,
}

impl Ghost {
    /// Create the ghost handle and, when a provider is available, spawn the
    /// background prediction worker.
    pub fn new(enabled: bool, provider: Option<Arc<dyn Provider>>, config: Config) -> Self {
        let shared = Arc::new(Mutex::new(Shared {
            enabled,
            ..Default::default()
        }));
        let stop = Arc::new(AtomicBool::new(false));
        if let Some(provider) = provider {
            let shared_w = Arc::clone(&shared);
            let stop_w = Arc::clone(&stop);
            std::thread::Builder::new()
                .name("aishe-ghost".into())
                .spawn(move || worker(shared_w, stop_w, provider, config))
                .ok();
        }
        Ghost { shared, stop }
    }

    pub fn set_enabled(&self, on: bool) {
        if let Ok(mut s) = self.shared.lock() {
            s.enabled = on;
            if !on {
                s.prediction.clear();
                s.predicted_for = None;
            }
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.shared.lock().map(|s| s.enabled).unwrap_or(false)
    }

    /// Clear the current buffer/prediction. Call after a line is submitted so the
    /// worker does not predict for an already-run command.
    pub fn reset(&self) {
        if let Ok(mut s) = self.shared.lock() {
            s.latest.clear();
            s.latest_at = None;
            s.prediction.clear();
            s.predicted_for = None;
        }
    }

    /// Build the reedline hinter: ghost text when a prediction is available,
    /// otherwise ordinary history hints.
    pub fn hinter(&self, ghost_style: Style) -> Box<dyn Hinter> {
        Box::new(AiHinter {
            shared: Arc::clone(&self.shared),
            history: DefaultHinter::default(),
            style: ghost_style,
            current: String::new(),
        })
    }
}

impl Drop for Ghost {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// Background loop: when typing is idle and a fresh prediction is needed, ask the
/// model and cache the result.
fn worker(
    shared: Arc<Mutex<Shared>>,
    stop: Arc<AtomicBool>,
    provider: Arc<dyn Provider>,
    config: Config,
) {
    while !stop.load(Ordering::SeqCst) {
        std::thread::sleep(POLL);
        let (prefix, cwd) = {
            let s = match shared.lock() {
                Ok(s) => s,
                Err(_) => return,
            };
            let idle = s
                .latest_at
                .map(|t| t.elapsed() >= DEBOUNCE)
                .unwrap_or(false);
            let need = s.predicted_for.as_deref() != Some(s.latest.as_str());
            if !s.enabled || !idle || !need || s.latest.chars().count() < MIN_PREFIX {
                continue;
            }
            (s.latest.clone(), s.cwd.clone())
        };

        // Respect the shared budget; mark attempted so we don't spin on it.
        let snap = provider.meter().snapshot();
        let pred = if crate::usage::over_budget(
            snap,
            config.active_model(),
            &config.pricing,
            config.aishe.budget_usd,
        ) {
            None
        } else {
            predict(&*provider, &config, &prefix, &cwd)
        };
        record(&shared, &prefix, pred);
    }
}

/// Store a prediction (or a "no prediction" marker) for `prefix`.
fn record(shared: &Arc<Mutex<Shared>>, prefix: &str, pred: Option<String>) {
    if let Ok(mut s) = shared.lock() {
        s.predicted_for = Some(prefix.to_string());
        s.prediction = pred.unwrap_or_default();
    }
}

/// Ask the model for the most likely full command line for `prefix`. Returns the
/// prediction only when it genuinely extends the prefix.
fn predict(provider: &dyn Provider, config: &Config, prefix: &str, cwd: &str) -> Option<String> {
    let user = format!(
        "OS: {}\nCWD: {}\nPartial command: {prefix}\nComplete it:",
        std::env::consts::OS,
        cwd
    );
    let model = config.active_model();
    crate::audit::ai_request("ghost", model, prefix);
    let before = provider.meter().snapshot();
    let text = match provider.complete(GHOST_SYSTEM, &[Msg::User(user)], &ResponseFormat::Text) {
        Ok(t) => t,
        Err(e) => {
            crate::audit::ai_error("ghost", model, &e.to_string());
            return None;
        }
    };
    let after = provider.meter().snapshot();
    let cleaned = clean(&text);
    crate::audit::ai_response(
        "ghost",
        model,
        &cleaned,
        after.input.saturating_sub(before.input),
        after.output.saturating_sub(before.output),
    );
    if cleaned.starts_with(prefix) && cleaned.len() > prefix.len() {
        Some(cleaned)
    } else {
        None
    }
}

/// Reduce a model reply to a single command line: first non-empty line, with code
/// fences and surrounding backticks stripped.
fn clean(text: &str) -> String {
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("```") {
            continue;
        }
        return line.trim_matches('`').trim().to_string();
    }
    String::new()
}

/// reedline hinter that prefers a cached AI prediction, else history hints.
struct AiHinter {
    shared: Arc<Mutex<Shared>>,
    history: DefaultHinter,
    style: Style,
    current: String,
}

impl Hinter for AiHinter {
    fn handle(
        &mut self,
        line: &str,
        pos: usize,
        history: &dyn History,
        use_ansi_coloring: bool,
        cwd: &str,
    ) -> String {
        // Ghost text is a continuation, so only at the end of the line.
        let at_end = pos >= line.len();
        let ghost: Option<String> = match self.shared.lock() {
            Ok(mut s) => {
                if s.latest != line {
                    s.latest = line.to_string();
                    s.latest_at = Some(Instant::now());
                }
                if s.cwd != cwd {
                    s.cwd = cwd.to_string();
                }
                if s.enabled
                    && at_end
                    && !line.is_empty()
                    && s.prediction.starts_with(line)
                    && s.prediction.len() > line.len()
                {
                    Some(s.prediction[line.len()..].to_string())
                } else {
                    None
                }
            }
            Err(_) => None,
        };

        if let Some(remainder) = ghost {
            self.current = remainder.clone();
            return if use_ansi_coloring {
                self.style.paint(remainder).to_string()
            } else {
                remainder
            };
        }

        // Fall back to history-based hints.
        let styled = self
            .history
            .handle(line, pos, history, use_ansi_coloring, cwd);
        self.current = self.history.complete_hint();
        styled
    }

    fn complete_hint(&self) -> String {
        self.current.clone()
    }

    fn next_hint_token(&self) -> String {
        // First whitespace-delimited token (with any leading spaces), matching
        // reedline's word-accept convention.
        let mut out = String::new();
        let mut seen_word = false;
        for c in self.current.chars() {
            if c.is_whitespace() {
                if seen_word {
                    break;
                }
                out.push(c);
            } else {
                seen_word = true;
                out.push(c);
            }
        }
        out
    }
}

/// A reasonable default style for ghost text (dim italic), distinct from the
/// plain history hint.
pub fn default_style() -> Style {
    Style::new().fg(Color::DarkGray).italic()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_strips_fences_and_backticks() {
        assert_eq!(
            clean("```sh\ngit push origin main\n```"),
            "git push origin main"
        );
        assert_eq!(clean("`ls -la`"), "ls -la");
        assert_eq!(clean("  git status  "), "git status");
        assert_eq!(clean(""), "");
    }

    #[test]
    fn next_hint_token_takes_first_word() {
        let h = AiHinter {
            shared: Arc::new(Mutex::new(Shared::default())),
            history: DefaultHinter::default(),
            style: default_style(),
            current: "sh origin main".to_string(),
        };
        assert_eq!(h.next_hint_token(), "sh");
    }

    #[test]
    fn enable_disable_roundtrip() {
        let g = Ghost::new(false, None, Config::default());
        assert!(!g.is_enabled());
        g.set_enabled(true);
        assert!(g.is_enabled());
        g.reset();
        assert!(g.is_enabled()); // reset clears buffer, not the enabled flag
    }
}
