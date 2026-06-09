//! Dispatcher integration tests against a manually-seeded cache.

use llmsh::dispatcher::{dispatch, CommandCache, Dispatch};

/// Build a cache seeded by a real PATH scan plus a few known names.
fn seeded_cache() -> CommandCache {
    let cache = CommandCache::new();
    // Rehash synchronously so PATH + builtins are present.
    cache.rehash(std::path::Path::new("/bin/sh"));
    cache
}

#[test]
fn real_commands_route_to_shell() {
    let cache = seeded_cache();
    // `ls` exists on every supported platform.
    assert!(matches!(dispatch("ls -la", &cache), Dispatch::Shell(_)));
}

#[test]
fn natural_language_routes_to_nl() {
    let cache = seeded_cache();
    assert!(matches!(
        dispatch("what is eating my disk space", &cache),
        Dispatch::NaturalLanguage(_)
    ));
}

#[test]
fn forced_prefixes() {
    let cache = CommandCache::new();
    assert_eq!(
        dispatch("?how do I list files", &cache),
        Dispatch::NaturalLanguage("how do I list files".to_string())
    );
    assert_eq!(
        dispatch("!some-binary --flag", &cache),
        Dispatch::Shell("some-binary --flag".to_string())
    );
}

#[test]
fn builtins_are_intercepted() {
    let cache = CommandCache::new();
    assert!(matches!(dispatch("cd ..", &cache), Dispatch::Builtin(_)));
    assert!(matches!(dispatch("exit", &cache), Dispatch::Builtin(_)));
}
