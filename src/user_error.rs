//! Stable, presentation-safe errors for CLI and automation surfaces.
//!
//! Backend and provider errors are often verbose, inconsistent, or contain
//! untrusted text. `UserError` is the boundary between those errors and public
//! output: it assigns a stable namespaced code and exit status, redacts secrets,
//! removes terminal control sequences, bounds every string, and offers exactly
//! one primary recovery action.

use std::error::Error;
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize};

/// Version of the machine-readable [`UserError`] document.
pub const USER_ERROR_SCHEMA_VERSION: u32 = 1;

/// Maximum UTF-8 byte length of the primary message after sanitization.
pub const MAX_MESSAGE_BYTES: usize = 320;
/// Maximum UTF-8 byte length of the primary recovery action after sanitization.
pub const MAX_NEXT_ACTION_BYTES: usize = 320;
/// Maximum UTF-8 byte length of optional diagnostic detail after sanitization.
pub const MAX_DETAIL_BYTES: usize = 2_048;
/// Maximum number of errors retained from a source chain.
pub const MAX_SOURCE_CHAIN_ENTRIES: usize = 8;
/// Maximum ASCII byte length of the portion after a code namespace.
pub const MAX_CODE_NAME_BYTES: usize = 64;

const DEFAULT_MESSAGE: &str = "AIShe could not complete the request.";
const DEFAULT_NEXT_ACTION: &str = "Run `aishe doctor` and retry.";

/// An error caused by user input or an unsupported invocation. [`UserError::from_error`]
/// keeps its namespace and code instead of classifying it as `internal.unexpected`
/// with a support-bundle suggestion.
#[derive(Debug)]
pub struct UserFacing {
    pub namespace: ErrorNamespace,
    pub name: &'static str,
    pub message: String,
    pub next_action: &'static str,
}

impl fmt::Display for UserFacing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for UserFacing {}

impl UserFacing {
    pub fn new(
        namespace: ErrorNamespace,
        name: &'static str,
        message: impl Into<String>,
        next_action: &'static str,
    ) -> anyhow::Error {
        anyhow::Error::new(Self {
            namespace,
            name,
            message: message.into(),
            next_action,
        })
    }

    pub fn cli(
        name: &'static str,
        message: impl Into<String>,
        next_action: &'static str,
    ) -> anyhow::Error {
        Self::new(ErrorNamespace::Cli, name, message, next_action)
    }
}

/// Stable public error domains. The namespace determines the process exit code;
/// adding a new error within a namespace does not change script behavior.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorNamespace {
    /// Invalid command-line input or unsupported invocation.
    Cli,
    /// Invalid, missing, or incompatible configuration.
    Config,
    /// Missing, invalid, or insufficient credentials.
    Auth,
    /// A model provider rejected or could not complete a request.
    Provider,
    /// Connectivity, DNS, or timeout failure.
    Network,
    /// Organization or user policy denied an operation.
    Policy,
    /// A sandbox could not start or denied an operation.
    Sandbox,
    /// The managed agent backend failed.
    Backend,
    /// A local filesystem or operating-system operation failed.
    Io,
    /// An unexpected invariant or implementation failure.
    Internal,
}

impl ErrorNamespace {
    /// Stable process status for all codes in this namespace.
    pub const fn exit_code(self) -> u8 {
        match self {
            Self::Internal => 1,
            Self::Cli => 2,
            Self::Config => 3,
            Self::Auth => 4,
            Self::Provider => 5,
            Self::Network => 6,
            Self::Policy => 7,
            Self::Sandbox => 8,
            Self::Backend => 9,
            Self::Io => 10,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Cli => "cli",
            Self::Config => "config",
            Self::Auth => "auth",
            Self::Provider => "provider",
            Self::Network => "network",
            Self::Policy => "policy",
            Self::Sandbox => "sandbox",
            Self::Backend => "backend",
            Self::Io => "io",
            Self::Internal => "internal",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "cli" => Some(Self::Cli),
            "config" => Some(Self::Config),
            "auth" => Some(Self::Auth),
            "provider" => Some(Self::Provider),
            "network" => Some(Self::Network),
            "policy" => Some(Self::Policy),
            "sandbox" => Some(Self::Sandbox),
            "backend" => Some(Self::Backend),
            "io" => Some(Self::Io),
            "internal" => Some(Self::Internal),
            _ => None,
        }
    }
}

/// A validated public code in `namespace.snake_case_name` form.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize)]
#[serde(transparent)]
pub struct UserErrorCode(String);

impl UserErrorCode {
    /// Build a code from a stable namespace and lowercase snake-case name.
    pub fn new(
        namespace: ErrorNamespace,
        name: impl AsRef<str>,
    ) -> Result<Self, ErrorContractError> {
        let name = name.as_ref();
        if !valid_code_name(name) {
            return Err(ErrorContractError::InvalidCodeName(name.to_owned()));
        }
        Ok(Self(format!("{}.{}", namespace.as_str(), name)))
    }

    /// Parse and validate a complete public code.
    pub fn parse(value: impl AsRef<str>) -> Result<Self, ErrorContractError> {
        let value = value.as_ref();
        let Some((namespace, name)) = value.split_once('.') else {
            return Err(ErrorContractError::InvalidCode(value.to_owned()));
        };
        if name.contains('.')
            || ErrorNamespace::parse(namespace).is_none()
            || !valid_code_name(name)
        {
            return Err(ErrorContractError::InvalidCode(value.to_owned()));
        }
        Ok(Self(value.to_owned()))
    }

    pub fn namespace(&self) -> ErrorNamespace {
        let namespace = self
            .0
            .split_once('.')
            .map(|(namespace, _)| namespace)
            .unwrap_or("internal");
        // Construction and deserialization validate the namespace.
        ErrorNamespace::parse(namespace).unwrap_or(ErrorNamespace::Internal)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for UserErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for UserErrorCode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::parse(value).map_err(serde::de::Error::custom)
    }
}

/// Failure to construct or decode a valid public error contract.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ErrorContractError {
    #[error("invalid user-error code name `{0}`")]
    InvalidCodeName(String),
    #[error("invalid user-error code `{0}`")]
    InvalidCode(String),
    #[error("unsupported user-error schema version {0}")]
    UnsupportedSchema(u32),
    #[error("exit code {actual} does not match {code} (expected {expected})")]
    ExitCodeMismatch {
        code: String,
        expected: u8,
        actual: u8,
    },
}

/// Schema-versioned error envelope for both human and JSON output.
///
/// Fields are private so public output can only be created through the safety
/// boundary. Deserialization also re-applies that boundary and rejects forged
/// schema or exit-code values.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct UserError {
    schema_version: u32,
    code: UserErrorCode,
    message: String,
    retryable: bool,
    exit_code: u8,
    next_action: String,
    detail: Option<String>,
}

impl UserError {
    /// Construct a safe error with one required primary recovery action.
    pub fn new(
        code: UserErrorCode,
        message: impl AsRef<str>,
        next_action: impl AsRef<str>,
    ) -> Self {
        let namespace = code.namespace();
        Self {
            schema_version: USER_ERROR_SCHEMA_VERSION,
            code,
            message: safe_single_line(message.as_ref(), MAX_MESSAGE_BYTES, DEFAULT_MESSAGE),
            retryable: false,
            exit_code: namespace.exit_code(),
            next_action: safe_single_line(
                next_action.as_ref(),
                MAX_NEXT_ACTION_BYTES,
                DEFAULT_NEXT_ACTION,
            ),
            detail: None,
        }
    }

    /// Convenience constructor that validates `namespace.name`.
    pub fn classified(
        namespace: ErrorNamespace,
        name: impl AsRef<str>,
        message: impl AsRef<str>,
        next_action: impl AsRef<str>,
    ) -> Result<Self, ErrorContractError> {
        Ok(Self::new(
            UserErrorCode::new(namespace, name)?,
            message,
            next_action,
        ))
    }

    pub fn with_retryable(mut self, retryable: bool) -> Self {
        self.retryable = retryable;
        self
    }

    /// Attach bounded, redacted, terminal-safe diagnostic detail.
    pub fn with_detail(mut self, detail: impl AsRef<str>) -> Self {
        let detail = safe_multiline(detail.as_ref(), MAX_DETAIL_BYTES);
        self.detail = (!detail.is_empty()).then_some(detail);
        self
    }

    /// Attach a bounded snapshot of an error's source chain.
    pub fn with_source_chain(mut self, source: &(dyn Error + 'static)) -> Self {
        let detail = source_chain(source);
        if !detail.is_empty() {
            self = self.with_detail(detail);
        }
        self
    }

    /// Classify an otherwise-untyped application error at the outer CLI
    /// boundary. Domain modules should prefer constructing an exact code, but
    /// this guarantees that a remaining `anyhow` path still has a bounded,
    /// redacted contract instead of falling back to ad hoc stderr.
    pub fn from_error(source: &(dyn Error + 'static)) -> Self {
        let chain = source_chain(source);
        let mut current: Option<&(dyn Error + 'static)> = Some(source);
        while let Some(candidate) = current {
            if let Some(facing) = candidate.downcast_ref::<UserFacing>() {
                return Self::classified(
                    facing.namespace,
                    facing.name,
                    &facing.message,
                    facing.next_action,
                )
                .expect("user-facing error code is valid")
                .with_detail(chain);
            }
            current = candidate.source();
        }
        let lower = chain.to_ascii_lowercase();
        let (namespace, name, message, next_action, retryable) = if contains_any(
            &lower,
            &["timed out", "timeout", "dns", "tls", "network error"],
        ) {
            (
                ErrorNamespace::Network,
                "operation_failed",
                "A network operation failed.",
                "Run `aishe doctor --probe`, verify the endpoint, and retry.",
                true,
            )
        } else if contains_any(
            &lower,
            &[
                "credential",
                "api key",
                "api_key",
                "oauth",
                "authentication",
            ],
        ) {
            (
                ErrorNamespace::Auth,
                "unavailable",
                "The required authentication is unavailable.",
                "Run `aishe auth status`, repair the named credential, then retry.",
                false,
            )
        } else if contains_any(
            &lower,
            &["organization policy", "policy denied", "denied by policy"],
        ) {
            (
                ErrorNamespace::Policy,
                "denied",
                "Policy denied the requested operation.",
                "Run `aishe doctor --json` and review the effective organization policy.",
                false,
            )
        } else if contains_any(&lower, &["sandbox", "bubblewrap", "bwrap"]) {
            (
                ErrorNamespace::Sandbox,
                "unavailable",
                "The requested sandbox boundary is unavailable.",
                "Run `aishe doctor --json` and repair the reported sandbox requirement.",
                false,
            )
        } else if contains_any(
            &lower,
            &["opencode", "managed runtime", "supervisor", "backend"],
        ) {
            (
                ErrorNamespace::Backend,
                "operation_failed",
                "The managed agent backend could not complete the operation.",
                "Run `aishe backend status`, then `aishe backend verify --live`.",
                true,
            )
        } else if contains_any(
            &lower,
            &["config.toml", "configuration", "toml", "config schema"],
        ) {
            (
                ErrorNamespace::Config,
                "invalid",
                "AIShe could not load the effective configuration.",
                "Run `aishe doctor`; repair the reported config file, then retry.",
                false,
            )
        } else if contains_any(
            &lower,
            &[
                "permission denied",
                "no such file",
                "filesystem",
                "reading ",
                "writing ",
                "opening ",
            ],
        ) {
            (
                ErrorNamespace::Io,
                "operation_failed",
                "A local file or operating-system operation failed.",
                "Check the path and permissions reported by `aishe doctor`, then retry.",
                false,
            )
        } else {
            (
                ErrorNamespace::Internal,
                "unexpected",
                "AIShe could not complete the request.",
                "Run `aishe doctor`; if it persists, create a redacted support bundle.",
                false,
            )
        };
        Self::classified(namespace, name, message, next_action)
            .expect("static fallback user-error code is valid")
            .with_retryable(retryable)
            .with_detail(chain)
    }

    pub const fn schema_version(&self) -> u32 {
        self.schema_version
    }

    pub fn code(&self) -> &UserErrorCode {
        &self.code
    }

    pub fn namespace(&self) -> ErrorNamespace {
        self.code.namespace()
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub const fn retryable(&self) -> bool {
        self.retryable
    }

    pub const fn exit_code(&self) -> u8 {
        self.exit_code
    }

    pub fn next_action(&self) -> &str {
        &self.next_action
    }

    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }

    /// Render a complete, readable error for stderr. This output never contains
    /// terminal escape sequences, even when the original error did.
    pub fn render_text(&self) -> String {
        format!("aishe: {}", self.render_body())
    }

    /// Render without the product prefix for callers that already own it.
    pub fn render_body(&self) -> String {
        let mut output = format!(
            "{} [{}]\nNext: {}",
            self.message, self.code, self.next_action
        );
        if let Some(detail) = &self.detail {
            output.push_str("\nDetails: ");
            output.push_str(detail);
        }
        output
    }

    /// Render the versioned, single-document JSON form.
    pub fn render_json(&self) -> serde_json::Result<String> {
        serde_json::to_string(self)
    }
}

fn contains_any(value: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| value.contains(needle))
}

impl fmt::Display for UserError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.render_text())
    }
}

#[derive(Deserialize)]
struct RawUserError {
    schema_version: u32,
    code: UserErrorCode,
    message: String,
    retryable: bool,
    exit_code: u8,
    next_action: String,
    detail: Option<String>,
}

impl<'de> Deserialize<'de> for UserError {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = RawUserError::deserialize(deserializer)?;
        if raw.schema_version != USER_ERROR_SCHEMA_VERSION {
            return Err(serde::de::Error::custom(
                ErrorContractError::UnsupportedSchema(raw.schema_version),
            ));
        }
        let expected = raw.code.namespace().exit_code();
        if raw.exit_code != expected {
            return Err(serde::de::Error::custom(
                ErrorContractError::ExitCodeMismatch {
                    code: raw.code.to_string(),
                    expected,
                    actual: raw.exit_code,
                },
            ));
        }
        let mut error =
            Self::new(raw.code, raw.message, raw.next_action).with_retryable(raw.retryable);
        if let Some(detail) = raw.detail {
            error = error.with_detail(detail);
        }
        Ok(error)
    }
}

/// Preserve the existing backend-neutral event shape while making conversion
/// to the richer CLI contract explicit.
impl From<&UserError> for crate::agent::UserFacingError {
    fn from(error: &UserError) -> Self {
        Self {
            code: error.code.to_string(),
            message: error.message.clone(),
            retryable: error.retryable,
        }
    }
}

impl From<UserError> for crate::agent::UserFacingError {
    fn from(error: UserError) -> Self {
        Self::from(&error)
    }
}

impl From<crate::agent::UserFacingError> for UserError {
    fn from(error: crate::agent::UserFacingError) -> Self {
        let code = match UserErrorCode::parse(&error.code) {
            Ok(code) => code,
            Err(_) => {
                UserErrorCode::new(ErrorNamespace::Backend, normalized_code_name(&error.code))
                    .unwrap_or_else(|_| {
                        UserErrorCode::new(ErrorNamespace::Backend, "agent_error")
                            .expect("static user-error code is valid")
                    })
            }
        };
        let next_action = if error.retryable {
            "Retry the request; if it fails again, run `aishe backend status`."
        } else {
            "Run `aishe backend status`, then run `aishe doctor`."
        };
        Self::new(code, error.message, next_action).with_retryable(error.retryable)
    }
}

fn valid_code_name(value: &str) -> bool {
    let mut bytes = value.bytes();
    !value.is_empty()
        && value.len() <= MAX_CODE_NAME_BYTES
        && matches!(bytes.next(), Some(b'a'..=b'z'))
        && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

fn normalized_code_name(value: &str) -> String {
    let mut output = String::with_capacity(value.len().min(64));
    for byte in value.bytes().take(64) {
        let normalized = if byte.is_ascii_lowercase() || byte.is_ascii_digit() {
            byte as char
        } else if byte.is_ascii_uppercase() {
            byte.to_ascii_lowercase() as char
        } else {
            '_'
        };
        if normalized == '_' && output.ends_with('_') {
            continue;
        }
        output.push(normalized);
    }
    let output = output.trim_matches('_');
    if valid_code_name(output) {
        output.to_owned()
    } else {
        "agent_error".to_owned()
    }
}

fn source_chain(source: &(dyn Error + 'static)) -> String {
    let mut output = String::new();
    let mut current = Some(source);
    for index in 0..MAX_SOURCE_CHAIN_ENTRIES {
        let Some(error) = current else {
            break;
        };
        if index > 0 {
            output.push_str("\nCaused by: ");
        }
        output.push_str(&safe_single_line(
            &error.to_string(),
            MAX_DETAIL_BYTES,
            "error detail unavailable",
        ));
        output = truncate_utf8(&output, MAX_DETAIL_BYTES);
        if output.len() == MAX_DETAIL_BYTES || output.ends_with('…') {
            break;
        }
        if index + 1 == MAX_SOURCE_CHAIN_ENTRIES {
            if error.source().is_some() {
                output.push_str("\nCaused by: additional causes omitted");
                output = truncate_utf8(&output, MAX_DETAIL_BYTES);
            }
            break;
        }
        current = error.source();
    }
    output
}

fn safe_single_line(input: &str, max_bytes: usize, fallback: &str) -> String {
    let sanitized = sanitize(input, false);
    let bounded = truncate_utf8(sanitized.trim(), max_bytes);
    if bounded.is_empty() {
        fallback.to_owned()
    } else {
        bounded
    }
}

fn safe_multiline(input: &str, max_bytes: usize) -> String {
    let sanitized = sanitize(input, true);
    let lines = sanitized
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    truncate_utf8(&lines, max_bytes)
}

fn truncate_utf8(input: &str, max_bytes: usize) -> String {
    if input.len() <= max_bytes {
        return input.to_owned();
    }
    const ELLIPSIS: &str = "…";
    if max_bytes < ELLIPSIS.len() {
        return String::new();
    }
    let mut end = max_bytes - ELLIPSIS.len();
    while !input.is_char_boundary(end) {
        end -= 1;
    }
    let mut output = input[..end].trim_end().to_owned();
    output.push_str(ELLIPSIS);
    output
}

#[derive(Clone, Copy)]
enum EscapeState {
    Text,
    Escape,
    ControlSequence,
    OperatingSystemCommand,
    OperatingSystemCommandEscape,
}

fn sanitize(input: &str, multiline: bool) -> String {
    let redacted = crate::redact::redact(input);
    let mut output = String::with_capacity(redacted.len());
    let mut state = EscapeState::Text;
    let mut pending_space = false;

    for character in redacted.chars() {
        match state {
            EscapeState::Escape => {
                state = match character {
                    '[' => EscapeState::ControlSequence,
                    ']' => EscapeState::OperatingSystemCommand,
                    _ => EscapeState::Text,
                };
            }
            EscapeState::ControlSequence => {
                if ('@'..='~').contains(&character) {
                    state = EscapeState::Text;
                }
            }
            EscapeState::OperatingSystemCommand => match character {
                '\u{7}' => state = EscapeState::Text,
                '\u{1b}' => state = EscapeState::OperatingSystemCommandEscape,
                _ => {}
            },
            EscapeState::OperatingSystemCommandEscape => {
                state = if character == '\\' {
                    EscapeState::Text
                } else {
                    EscapeState::OperatingSystemCommand
                };
            }
            EscapeState::Text => match character {
                '\u{1b}' => state = EscapeState::Escape,
                '\n' if multiline => {
                    while output.ends_with(' ') {
                        output.pop();
                    }
                    if !output.ends_with('\n') {
                        output.push('\n');
                    }
                    pending_space = false;
                }
                character if unsafe_format_character(character) || character.is_control() => {
                    pending_space = !output.is_empty() && !output.ends_with('\n');
                }
                character if character.is_whitespace() => {
                    pending_space = !output.is_empty() && !output.ends_with('\n');
                }
                character => {
                    if pending_space && !output.ends_with(' ') {
                        output.push(' ');
                    }
                    output.push(character);
                    pending_space = false;
                }
            },
        }
    }
    output.trim().to_owned()
}

fn unsafe_format_character(character: char) -> bool {
    matches!(
        character,
        '\u{061c}'
            | '\u{200e}'
            | '\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(namespace: ErrorNamespace, name: &str) -> UserErrorCode {
        UserErrorCode::new(namespace, name).unwrap()
    }


    #[test]
    fn user_facing_errors_keep_their_namespace_through_from_error() {
        let error = UserFacing::cli(
            "unknown_connection",
            "Unknown connection or provider 'nope'.",
            "Run `aishe connection list` to see the available ids.",
        );
        let public = UserError::from_error(error.as_ref());
        assert_eq!(public.code().to_string(), "cli.unknown_connection");
        assert_eq!(public.exit_code(), 2);
        assert!(!public.render_text().contains("support bundle"));
        let wrapped = error.context("selecting a connection");
        assert_eq!(
            UserError::from_error(wrapped.as_ref()).code().to_string(),
            "cli.unknown_connection"
        );
    }

    #[test]
    fn namespace_exit_code_table_is_stable() {
        let cases = [
            (ErrorNamespace::Cli, "cli", 2),
            (ErrorNamespace::Config, "config", 3),
            (ErrorNamespace::Auth, "auth", 4),
            (ErrorNamespace::Provider, "provider", 5),
            (ErrorNamespace::Network, "network", 6),
            (ErrorNamespace::Policy, "policy", 7),
            (ErrorNamespace::Sandbox, "sandbox", 8),
            (ErrorNamespace::Backend, "backend", 9),
            (ErrorNamespace::Io, "io", 10),
            (ErrorNamespace::Internal, "internal", 1),
        ];
        for (namespace, name, exit_code) in cases {
            assert_eq!(namespace.as_str(), name);
            assert_eq!(namespace.exit_code(), exit_code);
            let error = UserError::new(code(namespace, "example"), "message", "action");
            assert_eq!(error.exit_code(), exit_code);
        }
    }

    #[test]
    fn codes_are_strict_and_namespaced() {
        assert_eq!(
            code(ErrorNamespace::Provider, "invalid_credential").as_str(),
            "provider.invalid_credential"
        );
        for invalid in [
            "provider",
            "Provider.bad",
            "provider.Bad",
            "provider.bad-code",
            "provider.bad.code",
            "unknown.bad",
            "provider._bad",
            "provider.bad!",
            &format!("provider.{}", "a".repeat(MAX_CODE_NAME_BYTES + 1)),
        ] {
            assert!(UserErrorCode::parse(invalid).is_err(), "accepted {invalid}");
        }
    }

    #[test]
    fn text_and_json_snapshot_are_stable_and_ansi_free() {
        let error = UserError::new(
            code(ErrorNamespace::Auth, "invalid_credential"),
            "Provider authentication failed.",
            "Check the API-key environment variable, then run `aishe doctor --live`.",
        )
        .with_detail("HTTP 401");
        assert_eq!(
            error.render_text(),
            "aishe: Provider authentication failed. [auth.invalid_credential]\n\
             Next: Check the API-key environment variable, then run `aishe doctor --live`.\n\
             Details: HTTP 401"
        );
        assert_eq!(
            error.render_json().unwrap(),
            r#"{"schema_version":1,"code":"auth.invalid_credential","message":"Provider authentication failed.","retryable":false,"exit_code":4,"next_action":"Check the API-key environment variable, then run `aishe doctor --live`.","detail":"HTTP 401"}"#
        );
    }

    #[test]
    fn redacts_secrets_and_removes_terminal_and_bidi_controls() {
        let error = UserError::new(
            code(ErrorNamespace::Provider, "rejected"),
            "\u{1b}[31mRejected\u{1b}[0m TOKEN=secret-value\u{202e}",
            "\u{1b}]0;owned\u{7}Run doctor\rnow",
        )
        .with_detail("Authorization: Bearer sk-abcdefghijklmnopqrstuvwxyz123456\n\u{1b}[2Jsafe");
        let text = error.render_text();
        let json = error.render_json().unwrap();
        for output in [&text, &json] {
            assert!(!output.contains('\u{1b}'));
            assert!(!output.contains('\u{202e}'));
            assert!(!output.contains("abcdefghijklmnopqrstuvwxyz"));
            assert!(!output.contains("owned"));
        }
        assert!(text.contains("TOKEN=<redacted>"));
        assert!(text.contains("Authorization: <redacted>"));
    }

    #[test]
    fn every_field_is_bounded_at_utf8_boundaries() {
        let repeated = "é".repeat(4_000);
        let error = UserError::new(
            code(ErrorNamespace::Internal, "oversized"),
            &repeated,
            &repeated,
        )
        .with_detail(&repeated);
        assert!(error.message().len() <= MAX_MESSAGE_BYTES);
        assert!(error.next_action().len() <= MAX_NEXT_ACTION_BYTES);
        assert!(error.detail().unwrap().len() <= MAX_DETAIL_BYTES);
        assert!(error.message().ends_with('…'));
        assert!(error.next_action().ends_with('…'));
        assert!(error.detail().unwrap().ends_with('…'));
    }

    #[derive(Debug, thiserror::Error)]
    #[error("outer TOKEN=visible-secret")]
    struct OuterError {
        #[source]
        source: MiddleError,
    }

    #[derive(Debug, thiserror::Error)]
    #[error("middle")]
    struct MiddleError {
        #[source]
        source: LeafError,
    }

    #[derive(Debug, thiserror::Error)]
    #[error("leaf \u{1b}[31mfailure\u{1b}[0m")]
    struct LeafError;

    #[test]
    fn source_chain_conversion_is_redacted_bounded_and_ordered() {
        let source = OuterError {
            source: MiddleError { source: LeafError },
        };
        let error = UserError::new(
            code(ErrorNamespace::Internal, "source_chain"),
            "Operation failed.",
            "Run `aishe doctor`.",
        )
        .with_source_chain(&source);
        assert_eq!(
            error.detail(),
            Some("outer TOKEN=<redacted>\nCaused by: middle\nCaused by: leaf failure")
        );
    }

    #[test]
    fn outer_error_classifier_covers_common_domains_without_leaking_details() {
        let cases = [
            (
                "TLS connection timed out TOKEN=secret",
                "network.operation_failed",
                6,
            ),
            ("credential profile is unavailable", "auth.unavailable", 4),
            ("organization policy denied host scope", "policy.denied", 7),
            ("bubblewrap sandbox is unusable", "sandbox.unavailable", 8),
            ("OpenCode supervisor stopped", "backend.operation_failed", 9),
            ("config.toml has invalid TOML", "config.invalid", 3),
            ("permission denied opening state", "io.operation_failed", 10),
            ("unclassified invariant", "internal.unexpected", 1),
        ];
        for (message, expected_code, expected_exit) in cases {
            let source = std::io::Error::other(message);
            let error = UserError::from_error(&source);
            assert_eq!(error.code().as_str(), expected_code, "{message}");
            assert_eq!(error.exit_code(), expected_exit, "{message}");
            assert!(!error.render_json().unwrap().contains("TOKEN=secret"));
        }
    }

    #[test]
    fn deserialization_revalidates_safety_schema_and_exit_code() {
        let unsafe_json = r#"{
            "schema_version": 1,
            "code": "network.timeout",
            "message": "\u001b[31mTimeout TOKEN=secret\u001b[0m",
            "retryable": true,
            "exit_code": 6,
            "next_action": "Retry",
            "detail": null
        }"#;
        let error: UserError = serde_json::from_str(unsafe_json).unwrap();
        assert_eq!(error.message(), "Timeout TOKEN=<redacted>");
        assert!(error.retryable());
        assert!(serde_json::from_str::<UserError>(
            &unsafe_json.replace("\"schema_version\": 1", "\"schema_version\": 2")
        )
        .is_err());
        assert!(serde_json::from_str::<UserError>(
            &unsafe_json.replace("\"exit_code\": 6", "\"exit_code\": 5")
        )
        .is_err());
    }

    #[test]
    fn agent_event_bridge_preserves_event_shape_and_normalizes_legacy_codes() {
        let legacy = crate::agent::UserFacingError {
            code: "OpenCode Stream Lost!".into(),
            message: "Disconnected".into(),
            retryable: true,
        };
        let error = UserError::from(legacy);
        assert_eq!(error.code().as_str(), "backend.opencode_stream_lost");
        assert_eq!(error.exit_code(), ErrorNamespace::Backend.exit_code());
        let event_error = crate::agent::UserFacingError::from(&error);
        assert_eq!(event_error.code, "backend.opencode_stream_lost");
        assert_eq!(
            serde_json::to_value(event_error).unwrap(),
            serde_json::json!({
                "code": "backend.opencode_stream_lost",
                "message": "Disconnected",
                "retryable": true
            })
        );
    }
}
