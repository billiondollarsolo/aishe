//! A small in-memory, TTL'd cache for model responses, and a [`Provider`]
//! decorator that serves repeat completions from it. Only the non-streaming
//! `complete` path (suggest mode) is cached; streaming and the agentic tool loop
//! always reach the real provider. A cache hit records no token usage, so an
//! identical repeat costs nothing and returns instantly.
//!
//! The cache key is the `(model, system prompt, current request, response
//! format)` tuple. The "current request" is the last message — for suggest mode
//! that is the user's input plus the freshly-built environment context (cwd,
//! recent commands, git state). Earlier conversation turns are deliberately
//! excluded so that accumulated session memory does not defeat the cache; and
//! because the context is part of the key, running anything between two otherwise
//! identical requests changes it and misses the cache, keeping stale suggestions
//! from being served after the situation has moved on.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::providers::{Completion, Msg, Provider, ProviderError, ResponseFormat, ToolDef};
use crate::usage::UsageMeter;

/// One cached response with the instant it was stored (for TTL checks).
struct Entry {
    stored: Instant,
    value: String,
}

/// An in-memory cache of completion responses keyed by a request hash, with a
/// fixed time-to-live.
pub struct ResponseCache {
    ttl: Duration,
    map: Mutex<HashMap<u64, Entry>>,
}

impl ResponseCache {
    /// A cache whose entries live for `ttl_secs` seconds.
    pub fn new(ttl_secs: u64) -> Self {
        Self {
            ttl: Duration::from_secs(ttl_secs),
            map: Mutex::new(HashMap::new()),
        }
    }

    /// The cached value for `key` if present and not expired (expired entries are
    /// dropped on access).
    pub fn get(&self, key: u64) -> Option<String> {
        let mut map = self.map.lock().ok()?;
        if let Some(entry) = map.get(&key) {
            if entry.stored.elapsed() < self.ttl {
                return Some(entry.value.clone());
            }
            map.remove(&key);
        }
        None
    }

    /// Store `value` under `key`, stamped now.
    pub fn put(&self, key: u64, value: String) {
        if let Ok(mut map) = self.map.lock() {
            map.insert(
                key,
                Entry {
                    stored: Instant::now(),
                    value,
                },
            );
        }
    }
}

/// Hash a completion request into a stable cache key from the model, system
/// prompt, response format, and the *current* turn (the last message), excluding
/// earlier conversation history. Tool calls on assistant turns are ignored
/// (suggest-mode messages never carry them).
fn request_key(model: &str, system: &str, messages: &[Msg], format: &ResponseFormat) -> u64 {
    let mut h = DefaultHasher::new();
    model.hash(&mut h);
    system.hash(&mut h);
    match messages.last() {
        Some(Msg::User(t)) => {
            0u8.hash(&mut h);
            t.hash(&mut h);
        }
        Some(Msg::Assistant(a)) => {
            1u8.hash(&mut h);
            a.text.hash(&mut h);
        }
        Some(Msg::ToolResult { call_id, content }) => {
            2u8.hash(&mut h);
            call_id.hash(&mut h);
            content.hash(&mut h);
        }
        None => 3u8.hash(&mut h),
    }
    match format {
        ResponseFormat::Text => 0u8.hash(&mut h),
        ResponseFormat::Json => 1u8.hash(&mut h),
        ResponseFormat::JsonSchema { name, schema } => {
            2u8.hash(&mut h);
            name.hash(&mut h);
            schema.to_string().hash(&mut h);
        }
    }
    h.finish()
}

/// A [`Provider`] that wraps another and serves repeat `complete` calls from an
/// in-memory cache. Streaming and tool-use calls pass straight through. The
/// shared meter is the inner provider's, so usage and the budget stay unified and
/// cache hits (which never call the inner provider) add no tokens.
pub struct CachingProvider {
    inner: Arc<dyn Provider>,
    cache: ResponseCache,
    model: String,
}

impl CachingProvider {
    pub fn new(inner: Arc<dyn Provider>, ttl_secs: u64, model: String) -> Self {
        Self {
            inner,
            cache: ResponseCache::new(ttl_secs),
            model,
        }
    }
}

impl Provider for CachingProvider {
    fn complete(
        &self,
        system: &str,
        messages: &[Msg],
        format: &ResponseFormat,
    ) -> Result<String, ProviderError> {
        let key = request_key(&self.model, system, messages, format);
        if let Some(hit) = self.cache.get(key) {
            return Ok(hit);
        }
        let value = self.inner.complete(system, messages, format)?;
        self.cache.put(key, value.clone());
        Ok(value)
    }

    fn complete_stream(
        &self,
        system: &str,
        messages: &[Msg],
        format: &ResponseFormat,
        sink: &mut dyn FnMut(&str),
    ) -> Result<String, ProviderError> {
        // Streaming is interactive prose; pass through so live tokens are real.
        self.inner.complete_stream(system, messages, format, sink)
    }

    fn complete_with_tools(
        &self,
        system: &str,
        messages: &[Msg],
        tools: &[ToolDef],
    ) -> Result<Completion, ProviderError> {
        self.inner.complete_with_tools(system, messages, tools)
    }

    fn complete_with_tools_stream(
        &self,
        system: &str,
        messages: &[Msg],
        tools: &[ToolDef],
        sink: &mut dyn FnMut(&str),
    ) -> Result<Completion, ProviderError> {
        self.inner
            .complete_with_tools_stream(system, messages, tools, sink)
    }

    fn meter(&self) -> Arc<UsageMeter> {
        self.inner.meter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_get_and_expiry() {
        let c = ResponseCache::new(60);
        c.put(7, "v".into());
        assert_eq!(c.get(7), Some("v".into()));
        assert_eq!(c.get(8), None);
        // A zero-TTL cache never returns a hit (already expired).
        let z = ResponseCache::new(0);
        z.put(1, "x".into());
        assert_eq!(z.get(1), None);
    }

    #[test]
    fn key_is_stable_and_distinguishing() {
        let m1 = vec![Msg::User("hello".into())];
        let m2 = vec![Msg::User("world".into())];
        let k = |msgs: &[Msg], model: &str, sys: &str, f: &ResponseFormat| {
            request_key(model, sys, msgs, f)
        };
        let f = ResponseFormat::Text;
        assert_eq!(k(&m1, "m", "s", &f), k(&m1, "m", "s", &f));
        assert_ne!(k(&m1, "m", "s", &f), k(&m2, "m", "s", &f));
        assert_ne!(k(&m1, "m", "s", &f), k(&m1, "m2", "s", &f));
        assert_ne!(k(&m1, "m", "s", &f), k(&m1, "m", "s2", &f));
        assert_ne!(
            k(&m1, "m", "s", &ResponseFormat::Text),
            k(&m1, "m", "s", &ResponseFormat::Json)
        );
    }

    #[test]
    fn earlier_history_does_not_change_the_key() {
        // Only the current turn (last message) keys the cache, so accumulated
        // session memory does not defeat it.
        let f = ResponseFormat::Text;
        let bare = vec![Msg::User("list files".into())];
        let with_history = vec![
            Msg::User("earlier question".into()),
            Msg::Assistant(crate::providers::AssistantMsg {
                text: Some("earlier answer".into()),
                tool_calls: vec![],
            }),
            Msg::User("list files".into()),
        ];
        assert_eq!(
            request_key("m", "s", &bare, &f),
            request_key("m", "s", &with_history, &f)
        );
    }

    /// A provider that counts how many times `complete` actually runs.
    struct Counting {
        calls: Mutex<u32>,
        meter: Arc<UsageMeter>,
    }
    impl Provider for Counting {
        fn complete(
            &self,
            _s: &str,
            _m: &[Msg],
            _f: &ResponseFormat,
        ) -> Result<String, ProviderError> {
            *self.calls.lock().unwrap() += 1;
            self.meter.record(10, 5);
            Ok("answer".into())
        }
        fn complete_with_tools(
            &self,
            _s: &str,
            _m: &[Msg],
            _t: &[ToolDef],
        ) -> Result<Completion, ProviderError> {
            Ok(Completion::default())
        }
        fn meter(&self) -> Arc<UsageMeter> {
            Arc::clone(&self.meter)
        }
    }

    #[test]
    fn repeat_complete_is_served_from_cache() {
        let inner = Arc::new(Counting {
            calls: Mutex::new(0),
            meter: Arc::new(UsageMeter::default()),
        });
        let cached = CachingProvider::new(inner.clone(), 60, "m".into());
        let msgs = vec![Msg::User("same".into())];
        let f = ResponseFormat::Text;

        let a = cached.complete("sys", &msgs, &f).unwrap();
        let b = cached.complete("sys", &msgs, &f).unwrap();
        assert_eq!(a, "answer");
        assert_eq!(b, "answer");
        // The inner provider ran exactly once; the second call hit the cache.
        assert_eq!(*inner.calls.lock().unwrap(), 1);
        // And usage was metered only for the one real call.
        assert_eq!(cached.meter().snapshot().requests, 1);

        // A different request misses and calls through.
        let other = vec![Msg::User("different".into())];
        cached.complete("sys", &other, &f).unwrap();
        assert_eq!(*inner.calls.lock().unwrap(), 2);
    }
}
