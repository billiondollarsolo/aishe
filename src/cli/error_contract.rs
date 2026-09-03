//! Common CLI fatal-output contract and milestone migration inventory.

use std::error::Error;

use crate::user_error::{ErrorNamespace, UserError};

#[derive(Clone, Copy, Debug)]
pub struct FatalPath {
    pub id: &'static str,
    pub owner: &'static str,
    pub source: &'static str,
    pub evidence: &'static str,
    pub structured: bool,
}

/// The common-path denominator for ERR-002. These are user-reachable fatal
/// paths, not every diagnostic, warning, answer, audit notice, or invariant.
pub const COMMON_FATAL_PATHS: &[FatalPath] = &[
    path(
        "outer_untyped_error",
        "cli-architecture",
        "src/main.rs",
        "UserError::from_error",
    ),
    path(
        "config_load",
        "config",
        "src/main.rs",
        "Config::load_or_init()?",
    ),
    path(
        "policy_constraint",
        "policy",
        "src/main.rs",
        "policy::constrain(&mut config)?",
    ),
    path("auth_command", "auth", "src/main.rs", "auth::run(cmd)"),
    path(
        "backend_command",
        "backend",
        "src/main.rs",
        "cli::backend::command",
    ),
    path("setup_failure", "setup", "src/main.rs", "setup_failed"),
    path(
        "yolo_acceptance",
        "runtime",
        "src/main.rs",
        "emit_from(error.as_ref())",
    ),
    path(
        "interactive_zsh_missing",
        "pty",
        "src/main.rs",
        "interactive_shell_missing",
    ),
    path(
        "suggest_missing_request",
        "runtime",
        "src/cli/runtime.rs",
        "missing_request",
    ),
    path(
        "suggest_provider_failure",
        "providers",
        "src/cli/runtime.rs",
        "print_suggest_error",
    ),
    path(
        "suggest_backend_failure",
        "backend",
        "src/cli/runtime.rs",
        "suggest_failed",
    ),
    path(
        "hook_provider_unavailable",
        "runtime",
        "src/cli/runtime.rs",
        "print_llm_unavailable",
    ),
    path(
        "suggest_hook_managed_failure",
        "runtime",
        "src/cli/runtime.rs",
        "suggest_line_managed",
    ),
    path(
        "fix_hook_managed_failure",
        "runtime",
        "src/cli/runtime.rs",
        "fix_line_managed",
    ),
    path(
        "yolo_hook_managed_failure",
        "runtime",
        "src/cli/runtime.rs",
        "yolo_line_managed",
    ),
    path(
        "auto_hook_managed_failure",
        "runtime",
        "src/cli/runtime.rs",
        "auto_line_managed",
    ),
    path(
        "one_shot_managed_failure",
        "runtime",
        "src/cli/runtime.rs",
        "one_shot_managed",
    ),
    path(
        "session_resume_failure",
        "session",
        "src/main.rs",
        "cli::session::resume",
    ),
    path(
        "history_command_failure",
        "history",
        "src/main.rs",
        "cli::history::command",
    ),
];

/// Internal-only emissions excluded from the user-reachable common-path
/// denominator. Every entry has a named owner and exact source evidence.
pub const INTERNAL_ONLY_ALLOWLIST: &[FatalPath] = &[
    FatalPath {
        id: "fatal_json_serialization_fallback",
        owner: "cli-architecture",
        source: "src/main.rs",
        evidence: "internal.serialization_failed",
        structured: false,
    },
    FatalPath {
        id: "one_shot_registry_invariant",
        owner: "command-registry",
        source: "src/cli/runtime.rs",
        evidence: "command registry has no one-shot handler",
        structured: false,
    },
];

const fn path(
    id: &'static str,
    owner: &'static str,
    source: &'static str,
    evidence: &'static str,
) -> FatalPath {
    FatalPath {
        id,
        owner,
        source,
        evidence,
        structured: true,
    }
}

pub fn coverage() -> (usize, usize) {
    (
        COMMON_FATAL_PATHS
            .iter()
            .filter(|path| path.structured)
            .count(),
        COMMON_FATAL_PATHS.len(),
    )
}

/// Print the classified error and return its contract exit code, so callers
/// that hand back a `u8` cannot drift from the documented namespace codes.
pub fn emit_from(source: &(dyn Error + 'static)) -> u8 {
    let error = UserError::from_error(source);
    eprintln!("{}", error.render_text());
    error.exit_code()
}

pub fn emit_classified(
    namespace: ErrorNamespace,
    name: &'static str,
    message: impl AsRef<str>,
    next_action: impl AsRef<str>,
    detail: Option<&str>,
) {
    let mut error = UserError::classified(namespace, name, message, next_action)
        .expect("static CLI user-error code is valid");
    if let Some(detail) = detail {
        error = error.with_detail(detail);
    }
    eprintln!("{}", error.render_text());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(relative: &str) -> String {
        std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(relative))
            .unwrap()
    }

    #[test]
    fn common_fatal_coverage_is_static_owned_and_at_least_ninety_five_percent() {
        let (migrated, total) = coverage();
        assert_eq!((migrated, total), (19, 19));
        assert!(migrated * 100 >= total * 95);
        for item in COMMON_FATAL_PATHS.iter().chain(INTERNAL_ONLY_ALLOWLIST) {
            assert!(!item.id.is_empty());
            assert!(!item.owner.is_empty(), "{} has no owner", item.id);
            assert!(
                source(item.source).contains(item.evidence),
                "{} lost implementation evidence {:?} in {}",
                item.id,
                item.evidence,
                item.source
            );
        }
    }

    #[test]
    fn internal_allowlist_is_explicit_and_never_counts_as_migrated() {
        assert_eq!(INTERNAL_ONLY_ALLOWLIST.len(), 2);
        assert!(INTERNAL_ONLY_ALLOWLIST.iter().all(|item| !item.structured));
    }

    #[test]
    fn every_common_public_error_code_has_one_troubleshooting_entry() {
        let document = source("docs/troubleshooting.md");
        let codes = [
            "cli.missing_request",
            "cli.interactive_shell_missing",
            "config.setup_failed",
            "config.provider_unavailable",
            "provider.connection_unavailable",
            "backend.suggest_failed",
            "backend.suggest_line_managed",
            "backend.fix_line_managed",
            "backend.yolo_line_managed",
            "backend.auto_line_managed",
            "backend.one_shot_managed",
            "network.operation_failed",
            "auth.unavailable",
            "policy.denied",
            "sandbox.unavailable",
            "backend.operation_failed",
            "config.invalid",
            "io.operation_failed",
            "internal.unexpected",
            "cli.unknown_connection",
            "cli.unknown_task",
            "cli.shell_required",
            "cli.unsupported_shell",
            "cli.unknown_profile",
            "provider.model_list_failed",
            "config.setup_incomplete",
        ];
        for code in codes {
            let row = format!("| `{code}` |");
            assert_eq!(
                document.matches(&row).count(),
                1,
                "{code} needs exactly one maintained troubleshooting row"
            );
        }
    }
}
