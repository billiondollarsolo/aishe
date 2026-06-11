//! Best-effort secret redaction.
//!
//! Used to scrub credentials out of anything that leaves the machine or lands on
//! disk: the environment context block sent to the model (recent commands can
//! contain `export TOKEN=...`, `mysql -p...`, URLs with credentials) and the
//! audit log. It is heuristic, not a guarantee: it catches common shapes and is
//! deliberately conservative to avoid mangling ordinary text.

use std::sync::OnceLock;

use regex::Regex;

/// Placeholder substituted for a detected secret.
const MASK: &str = "<redacted>";

/// Redact likely secrets from `input`, returning a scrubbed copy. Idempotent and
/// safe to call on already-clean text.
pub fn redact(input: &str) -> String {
    let mut s = input.to_string();
    for rule in rules() {
        s = rule.regex.replace_all(&s, rule.replacement).into_owned();
    }
    // Generic high-entropy token: a long run that mixes letters and digits. The
    // `regex` crate has no lookahead, so the letter+digit check is done here.
    s = generic_token()
        .replace_all(&s, |caps: &regex::Captures| {
            let m = &caps[0];
            let has_alpha = m.chars().any(|c| c.is_ascii_alphabetic());
            let has_digit = m.chars().any(|c| c.is_ascii_digit());
            if has_alpha && has_digit {
                MASK.to_string()
            } else {
                m.to_string()
            }
        })
        .into_owned();
    s
}

/// Long opaque token shape (length and content checked by the caller).
fn generic_token() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[A-Za-z0-9+/_=-]{40,}").expect("valid token regex"))
}

/// True if redaction would change `input` (i.e. a secret shape was found). Useful
/// for tests and conditional notices.
pub fn contains_secret(input: &str) -> bool {
    redact(input) != input
}

struct Rule {
    regex: Regex,
    replacement: &'static str,
}

fn rules() -> &'static [Rule] {
    static RULES: OnceLock<Vec<Rule>> = OnceLock::new();
    RULES.get_or_init(|| {
        // (pattern, replacement). `$N` backrefs keep the non-secret prefix.
        let specs: &[(&str, &str)] = &[
            // Credentials embedded in a URL: scheme://user:pass@host
            (r"([a-zA-Z][a-zA-Z0-9+.-]*://)[^/\s:@]+:[^/\s@]+@", "${1}<redacted>@"),
            // Authorization headers (curl -H "Authorization: Bearer ...").
            (r"(?i)(authorization:\s*)(?:bearer\s+|basic\s+)?[A-Za-z0-9._\-+/=]+", "${1}<redacted>"),
            // Assignment whose NAME *contains* a secret keyword behind a prefix:
            // FOO_TOKEN=..., DB_PASSWORD=..., AUTH_HEADER=... The leading char is
            // mandatory here, so `auth` only matches inside a longer name and
            // does not swallow ordinary words like `authors=`.
            (
                r"(?i)\b([A-Za-z_][A-Za-z0-9_]*(?:password|passwd|secret|token|api[_-]?key|access[_-]?key|private[_-]?key|credential|auth)[A-Za-z0-9_]*\s*=)\s*\S+",
                "${1}<redacted>",
            ),
            // Assignment whose NAME *is* an unambiguous secret keyword with no
            // prefix: PASSWORD=..., SECRET=..., TOKEN=..., API_KEY=... The rule
            // above misses these because it requires a character before the
            // keyword. `auth` is intentionally excluded here (it would match
            // `authors=`/`authority=`).
            (
                r"(?i)\b((?:password|passwd|secret|token|api[_-]?key|access[_-]?key|private[_-]?key|credential)[A-Za-z0-9_]*\s*=)\s*\S+",
                "${1}<redacted>",
            ),
            // Long-form credential flags: --password=..., --token x, --api-key=...
            (
                r"(?i)(--?(?:password|passwd|token|secret|api[_-]?key)(?:=|\s+))\S+",
                "${1}<redacted>",
            ),
            // Known provider key shapes.
            (r"sk-ant-[A-Za-z0-9_-]{16,}", MASK),
            (r"sk-[A-Za-z0-9_-]{20,}", MASK),
            (r"gh[pousr]_[A-Za-z0-9]{20,}", MASK),
            (r"gsk_[A-Za-z0-9]{20,}", MASK),
            (r"xox[baprs]-[A-Za-z0-9-]{10,}", MASK),
            (r"AKIA[0-9A-Z]{12,}", MASK),
            (r"AIza[0-9A-Za-z_-]{30,}", MASK),
        ];
        specs
            .iter()
            .map(|(p, r)| Rule {
                regex: Regex::new(p).expect("valid redaction regex"),
                replacement: r,
            })
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_secret_named_assignment() {
        assert_eq!(
            redact("export DB_PASSWORD=hunter2"),
            "export DB_PASSWORD=<redacted>"
        );
        assert_eq!(redact("API_TOKEN=abc123def"), "API_TOKEN=<redacted>");
        assert_eq!(redact("MY_SECRET = value"), "MY_SECRET =<redacted>");
    }

    #[test]
    fn redacts_bare_secret_named_assignment() {
        // A name that *is* the secret keyword (no prefix) must also be redacted:
        // these are exactly how top-level credential env vars are named.
        assert_eq!(
            redact("export PASSWORD=hunter2"),
            "export PASSWORD=<redacted>"
        );
        assert_eq!(redact("SECRET=abc"), "SECRET=<redacted>");
        assert_eq!(redact("TOKEN=xyz123"), "TOKEN=<redacted>");
        assert_eq!(redact("API_KEY=abcdef"), "API_KEY=<redacted>");
        assert_eq!(redact("APIKEY=abcdef"), "APIKEY=<redacted>");
        assert_eq!(redact("password=p@ss"), "password=<redacted>");
    }

    #[test]
    fn keeps_ordinary_assignments() {
        // A non-secret name with a short ordinary value is left alone.
        assert_eq!(redact("EDITOR=nvim"), "EDITOR=nvim");
        assert_eq!(redact("count=5"), "count=5");
        assert_eq!(redact("PATH=/usr/bin:/bin"), "PATH=/usr/bin:/bin");
        // `auth` is excluded from the no-prefix rule so common words survive.
        assert_eq!(redact("authors=jane"), "authors=jane");
        assert_eq!(redact("authority=local"), "authority=local");
    }

    #[test]
    fn redacts_url_credentials() {
        assert_eq!(
            redact("git clone https://alice:s3cr3tpw@github.com/x/y.git"),
            "git clone https://<redacted>@github.com/x/y.git"
        );
    }

    #[test]
    fn redacts_authorization_header() {
        let r = redact(r#"curl -H "Authorization: Bearer sk-abc.def-123" https://api"#);
        assert!(r.contains("Authorization: <redacted>"), "{r}");
        assert!(!r.contains("sk-abc.def-123"), "{r}");
    }

    #[test]
    fn redacts_password_flags() {
        assert!(redact("psql --password=hunter2 -h db").contains("--password=<redacted>"));
        assert!(redact("tool --token abcdef -x").contains("--token <redacted>"));
    }

    #[test]
    fn redacts_known_key_shapes() {
        assert_eq!(
            redact("key is gsk_ABCDEFGHIJKLMNOPQRSTUVWX"),
            "key is <redacted>"
        );
        assert_eq!(redact("ghp_0123456789abcdefghijABCDEF"), "<redacted>");
        assert!(redact("AKIAIOSFODNN7EXAMPLE done").contains("<redacted>"));
    }

    #[test]
    fn redacts_generic_high_entropy() {
        let s = "token aGVsbG8xMjM0NTY3ODkwYWJjZGVmZ2hpamtsbW5vcA12 end";
        assert!(contains_secret(s), "{}", redact(s));
    }

    #[test]
    fn leaves_plain_text_and_hashes_words() {
        // Ordinary prose and short tokens are untouched.
        assert_eq!(
            redact("list files in the current directory"),
            "list files in the current directory"
        );
        // A 40-char all-hex git sha has digits+letters and would match the
        // generic rule; that is acceptable (over-redaction is safe), but a plain
        // english sentence must never trip it.
        assert_eq!(
            redact("rebuild the project from scratch please"),
            "rebuild the project from scratch please"
        );
    }

    #[test]
    fn idempotent() {
        let once = redact("PASSWORD=abc https://u:p@h");
        assert_eq!(redact(&once), once);
    }
}
