//! Sandbox and confirmation-tier policy for the yolo agentic loop.
//!
//! Two related, deterministic, best-effort policies layered on top of the
//! destructive-command [safety gate](crate::safety):
//!
//! 1. **Confirmation tiers** ([`Tier`], [`confirm_tier`]): decide *when* the loop
//!    pauses to confirm a `run_command` call, ranging from "never" to "every
//!    command".
//! 2. **Sandbox** ([`sandbox_refusal`]): when on, classify a command and refuse
//!    (feed an error back to the model instead of running) when it reaches the
//!    network or writes outside the working tree.
//!
//! These are *policy-based, best-effort* string classifiers, not a kernel
//! sandbox. They inspect the command text the model proposed; they cannot stop a
//! determined escape (a wrapper script, an alias, the zsh-PTY / real-shell paths,
//! etc.). They exist to catch the common cases and keep an autonomous loop on a
//! short leash.

use crate::config::Config;
use crate::safety::{self, Risk};

/// How yolo's `run_command` is sandboxed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// No sandbox (`yolo_sandbox = false`).
    Off,
    /// Best-effort policy gate ([`sandbox_refusal`]) — the default.
    Policy,
    /// Real OS isolation via `bubblewrap`: the command runs with a read-only root
    /// and only the working tree (and `/tmp`) writable, so it *physically* cannot
    /// modify the system. Falls back to [`Backend::Policy`] when `bwrap` is absent.
    Bwrap,
}

/// Resolve the active sandbox backend from config: `yolo_sandbox` (on/off) plus
/// `sandbox_backend` ("policy" | "bwrap"). A `bwrap` choice degrades to `Policy`
/// when bubblewrap isn't installed (the caller warns once).
pub fn backend(config: &Config) -> Backend {
    if !config.aishe.yolo_sandbox {
        return Backend::Off;
    }
    match config.aishe.sandbox_backend.as_str() {
        "bwrap" if bwrap_available() => Backend::Bwrap,
        "bwrap" => Backend::Policy, // requested but unavailable → degrade
        _ => Backend::Policy,
    }
}

/// Whether `bwrap` is requested but unavailable (so the caller can warn once).
pub fn bwrap_requested_but_missing(config: &Config) -> bool {
    config.aishe.yolo_sandbox && config.aishe.sandbox_backend == "bwrap" && !bwrap_available()
}

/// Whether `bubblewrap` (`bwrap`) is on `$PATH`.
pub fn bwrap_available() -> bool {
    crate::executor::which("bwrap").is_some()
}

/// The `bwrap` wrapper argv (without the trailing shell): a read-only root with
/// the working tree and `/tmp` writable, a private `/dev` and `/proc`, started in
/// `cwd`, and dying with the parent. Ends with `--`, so the executor appends the
/// shell + `-c <command>`. Network is left intact (reads/lookups still work); the
/// guarantee is that writes can't escape the working tree.
pub fn bwrap_wrap_argv(cwd: &std::path::Path) -> Vec<String> {
    let cwd = cwd.display().to_string();
    [
        "bwrap",
        "--ro-bind",
        "/",
        "/", // everything read-only …
        "--bind",
        "/tmp",
        "/tmp", // … except a writable /tmp …
        "--bind",
        &cwd,
        &cwd, // … and the working tree.
        "--dev",
        "/dev",
        "--proc",
        "/proc",
        "--chdir",
        &cwd,
        "--die-with-parent",
        "--",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// When the yolo loop pauses to confirm a `run_command` call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// Never confirm: run every command.
    Never,
    /// Confirm only commands the safety gate flags as dangerous (the historical
    /// default behavior).
    Dangerous,
    /// Confirm dangerous commands and any command that modifies state (anything
    /// not recognized as read-only by [`is_write_command`]).
    Writes,
    /// Confirm every command.
    All,
}

/// Resolve the effective confirmation tier from config.
///
/// Precedence:
/// - `yolo_confirm` (a string: `"never"` / `"dangerous"` / `"writes"` / `"all"`)
///   is the primary control. The default value is `"dangerous"`.
/// - The older boolean `yolo_confirm_dangerous` stays for backward compatibility.
///   It only takes effect when `yolo_confirm` is left at its default: in that case
///   `yolo_confirm_dangerous = false` resolves to [`Tier::Never`] (its historical
///   meaning). Whenever `yolo_confirm` is set to anything other than its default,
///   `yolo_confirm` wins and the boolean is ignored.
/// - An unrecognized `yolo_confirm` string falls back to [`Tier::Dangerous`].
pub fn confirm_tier(config: &Config) -> Tier {
    let raw = config.aishe.yolo_confirm.trim();
    let is_default = raw.eq_ignore_ascii_case(DEFAULT_CONFIRM);
    if is_default && !config.aishe.yolo_confirm_dangerous {
        return Tier::Never;
    }
    match raw.to_ascii_lowercase().as_str() {
        "never" => Tier::Never,
        "writes" => Tier::Writes,
        "all" => Tier::All,
        _ => Tier::Dangerous,
    }
}

/// The default value of `yolo_confirm`.
pub const DEFAULT_CONFIRM: &str = "dangerous";

/// Whether a command at the given tier needs a confirmation prompt, given its
/// safety assessment. Returns `(needs_confirm, flagged)`: `flagged` is true when
/// the safety gate flagged the command — either [`Risk::Dangerous`] or
/// [`Risk::Unknown`], i.e. it could not resolve what the command runs — so the
/// caller routes it through `safety_gate`, which picks the matching panel.
/// `flagged` is false when the confirm is purely tier-driven.
pub fn needs_confirm(tier: Tier, command: &str) -> (bool, bool) {
    // Fail closed: an unresolvable head is *not* a safe command.
    let flagged = !matches!(safety::assess(command), Risk::Safe);
    let needs = match tier {
        Tier::Never => false,
        Tier::Dangerous => flagged,
        Tier::Writes => flagged || is_write_command(command),
        Tier::All => true,
    };
    (needs, flagged)
}

/// Read-only command heads: running these does not modify state. Anything not on
/// this list is treated as a write by [`is_write_command`].
const READ_ONLY: &[&str] = &[
    "ls",
    "cat",
    "grep",
    "egrep",
    "fgrep",
    "rg",
    "find",
    "echo",
    "pwd",
    "head",
    "tail",
    "wc",
    "stat",
    "file",
    "which",
    "type",
    "whoami",
    "id",
    "date",
    "env",
    "printenv",
    "uname",
    "hostname",
    "df",
    "du",
    "ps",
    "top",
    "tree",
    "less",
    "more",
    "diff",
    "cmp",
    "sort",
    "uniq",
    "cut",
    "awk",
    "sed",
    "tr",
    "basename",
    "dirname",
    "realpath",
    "readlink",
    "true",
    "false",
    "test",
    "sleep",
    "printf",
    "column",
    "nl",
    "tac",
    "xxd",
    "od",
    "md5sum",
    "sha256sum",
    "cksum",
    "jq",
    "yq",
    "man",
    "help",
    "history",
    "uptime",
    "free",
    "lsof",
    "ss",
    "netstat",
];

/// Read-only `git` subcommands.
const READ_ONLY_GIT: &[&str] = &[
    "status",
    "log",
    "diff",
    "show",
    "branch",
    "remote",
    "config",
    "blame",
    "describe",
    "rev-parse",
    "ls-files",
    "ls-tree",
    "shortlog",
    "reflog",
    "cat-file",
    "tag",
    "whatchanged",
    "grep",
];

/// True if `cmd` modifies state (not read-only). Heuristic and best-effort: a
/// read-only head (see [`READ_ONLY`]) with no shell redirection/append is a read;
/// `git <read-only-subcommand>` is a read; everything else is treated as a write.
/// Any segment of a compound command being a write makes the whole line a write.
pub fn is_write_command(cmd: &str) -> bool {
    for segment in split_segments(cmd) {
        if segment_is_write(&segment) {
            return true;
        }
    }
    false
}

/// Whether a single (already split) segment is a write.
fn segment_is_write(segment: &str) -> bool {
    let seg = segment.trim();
    if seg.is_empty() {
        return false;
    }
    // A write redirection (`>`, `>>`) anywhere makes the segment a write. (`<`
    // input redirection and the `2>` style are handled by the same `>` check.)
    if seg.contains('>') {
        return true;
    }
    let toks: Vec<&str> = seg.split_whitespace().collect();
    let head = match toks.first() {
        Some(h) => strip_path(h),
        None => return false,
    };
    if head == "git" {
        // `git <sub>`: read-only only for the known read subcommands.
        let sub = toks
            .iter()
            .skip(1)
            .find(|t| !t.starts_with('-'))
            .copied()
            .unwrap_or("");
        return !READ_ONLY_GIT.contains(&sub);
    }
    !READ_ONLY.contains(&head)
}

/// Network-reaching command heads (the simple ones: the whole command reaches
/// out regardless of subcommand).
const NETWORK_HEADS: &[&str] = &[
    "curl",
    "wget",
    "ssh",
    "scp",
    "sftp",
    "nc",
    "ncat",
    "netcat",
    "telnet",
    "ftp",
    "rsync",
    "host",
    "dig",
    "nslookup",
    "ping",
    "traceroute",
    "whois",
];

/// True if `cmd` reaches the network. Covers the common direct tools (curl, wget,
/// ssh, scp, nc, ...) and the network subcommands of package managers and `git`.
/// Best-effort: any segment reaching out makes the whole line a network command.
pub fn is_network_command(cmd: &str) -> bool {
    for segment in split_segments(cmd) {
        if segment_is_network(&segment) {
            return true;
        }
    }
    false
}

/// Whether a single (already split) segment reaches the network.
fn segment_is_network(segment: &str) -> bool {
    let toks: Vec<&str> = segment.split_whitespace().collect();
    let head = match toks.first() {
        Some(h) => strip_path(h),
        None => return false,
    };
    if NETWORK_HEADS.contains(&head) {
        return true;
    }
    // Subcommand args, skipping flags, for tools where only some subcommands
    // reach out.
    let args: Vec<&str> = toks
        .iter()
        .skip(1)
        .filter(|t| !t.starts_with('-'))
        .copied()
        .collect();
    let sub = args.first().copied().unwrap_or("");
    match head {
        "git" => matches!(
            sub,
            "clone" | "fetch" | "pull" | "push" | "remote" | "submodule"
        ),
        "npm" | "pnpm" | "yarn" => {
            matches!(
                sub,
                "install" | "i" | "add" | "ci" | "update" | "publish" | "audit"
            )
        }
        "pip" | "pip3" => matches!(sub, "install" | "download" | "wheel"),
        "cargo" => matches!(
            sub,
            "install" | "fetch" | "publish" | "update" | "add" | "search"
        ),
        "go" => matches!(sub, "get" | "install" | "download"),
        "gem" => matches!(sub, "install" | "fetch" | "update"),
        "apt" | "apt-get" => matches!(sub, "install" | "update" | "upgrade" | "download"),
        "brew" => matches!(sub, "install" | "update" | "upgrade" | "fetch"),
        "docker" | "podman" => matches!(sub, "pull" | "push" | "login"),
        _ => false,
    }
}

/// A write target outside the working tree: absolute (`/...`), home-relative
/// (`~...`), a variable that can expand out of tree (`$HOME/...`, `${TMPDIR}/...`),
/// or escaping the tree via a `..` path segment. Mirrors the notion in
/// [`crate::tools`] used by the file tools, and the safety gate's `$`-target check.
pub fn outside_tree(path: &str) -> bool {
    let p = unquote(path);
    p.starts_with('/')
        || p.starts_with('~')
        || p.starts_with('$')
        || p.split('/').any(|seg| seg == "..")
}

/// Best-effort detection of a command that writes outside the working tree.
/// Returns the offending path when found. Looks at write redirection targets
/// (`> /etc/x`, `>> ~/y`) and the destination arguments of obvious out-of-tree
/// write commands (`cp`/`mv`/`install`/`tee`/`dd of=`/`touch`/`mkdir`/`rm` to an
/// absolute, home, or `..`-escaping path). Documented as best-effort: it cannot
/// see paths hidden in variables or scripts.
pub fn out_of_tree_write(cmd: &str) -> Option<String> {
    for segment in split_segments(cmd) {
        if let Some(p) = segment_out_of_tree_write(&segment) {
            return Some(p);
        }
    }
    None
}

fn segment_out_of_tree_write(segment: &str) -> Option<String> {
    let toks: Vec<String> = tokenize_redirs(segment);
    // Redirection targets: a `>`/`>>` token followed by a path.
    let mut iter = toks.iter().peekable();
    while let Some(tok) = iter.next() {
        if tok == ">" || tok == ">>" {
            if let Some(target) = iter.peek() {
                if outside_tree(target) {
                    return Some(unquote(target));
                }
            }
        } else if let Some(rest) = tok.strip_prefix(">>") {
            if !rest.is_empty() && outside_tree(rest) {
                return Some(unquote(rest));
            }
        } else if let Some(rest) = tok.strip_prefix('>') {
            if !rest.is_empty() && outside_tree(rest) {
                return Some(unquote(rest));
            }
        }
    }

    // Out-of-tree write commands: inspect their path arguments.
    let words: Vec<&str> = segment.split_whitespace().collect();
    let head = strip_path(words.first()?);
    let is_write_head = matches!(
        head,
        "cp" | "mv"
            | "install"
            | "tee"
            | "touch"
            | "mkdir"
            | "rm"
            | "rmdir"
            | "ln"
            | "chmod"
            | "chown"
    );
    if head == "dd" {
        // `dd ... of=<path>`
        for w in &words[1..] {
            if let Some(target) = w.strip_prefix("of=") {
                if outside_tree(target) {
                    return Some(unquote(target));
                }
            }
        }
        return None;
    }
    if is_write_head {
        for w in &words[1..] {
            if w.starts_with('-') {
                continue;
            }
            if outside_tree(w) {
                return Some(unquote(w));
            }
        }
    }
    None
}

/// If the sandbox is on, return a refusal reason for `cmd` (network access or an
/// out-of-tree write), or `None` to allow it.
pub fn sandbox_refusal(cmd: &str) -> Option<String> {
    if is_network_command(cmd) {
        return Some("accesses the network".to_string());
    }
    if let Some(path) = out_of_tree_write(cmd) {
        return Some(format!("writes outside the working tree: {path}"));
    }
    None
}

/// Format the tool-result string fed back to the model when the sandbox refuses
/// a command, so the model can adapt instead of retrying blindly.
pub fn refusal_message(reason: &str) -> String {
    format!("Refused by sandbox: {reason}. Sandbox mode is on (yolo_sandbox).")
}

/// Split a command line on the shell operators `;`, `&&`, `||`, `|`. Naive but
/// good enough for these classifiers (mirrors the safety gate's splitter).
fn split_segments(command: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let bytes = command.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i] as char;
        let next = bytes.get(i + 1).map(|&b| b as char);
        match (c, next) {
            ('&', Some('&')) | ('|', Some('|')) => {
                segments.push(std::mem::take(&mut current));
                i += 2;
            }
            (';', _) | ('|', _) | ('&', _) => {
                segments.push(std::mem::take(&mut current));
                i += 1;
            }
            _ => {
                current.push(c);
                i += 1;
            }
        }
    }
    if !current.is_empty() {
        segments.push(current);
    }
    segments
}

/// Tokenize a segment while keeping redirection operators as their own tokens, so
/// `foo>bar` and `foo >bar` both surface `>` and the target. Quotes are kept
/// (unquoting happens at the path check).
fn tokenize_redirs(segment: &str) -> Vec<String> {
    let mut toks = Vec::new();
    let mut cur = String::new();
    let chars: Vec<char> = segment.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            if !cur.is_empty() {
                toks.push(std::mem::take(&mut cur));
            }
            i += 1;
            continue;
        }
        if c == '>' {
            if !cur.is_empty() {
                toks.push(std::mem::take(&mut cur));
            }
            if chars.get(i + 1) == Some(&'>') {
                toks.push(">>".to_string());
                i += 2;
            } else {
                toks.push(">".to_string());
                i += 1;
            }
            continue;
        }
        cur.push(c);
        i += 1;
    }
    if !cur.is_empty() {
        toks.push(cur);
    }
    toks
}

/// Drop a leading directory component from a command head (`/usr/bin/curl` ->
/// `curl`, `./tool` -> `tool`) so classification matches on the program name.
fn strip_path(head: &str) -> &str {
    head.rsplit('/').next().unwrap_or(head)
}

/// Remove quote characters from a token.
fn unquote(t: &str) -> String {
    t.replace(['"', '\''], "")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_selection() {
        let mut c = Config::default();
        c.aishe.yolo_sandbox = false;
        assert_eq!(backend(&c), Backend::Off);
        c.aishe.yolo_sandbox = true;
        c.aishe.sandbox_backend = "policy".to_string();
        assert_eq!(backend(&c), Backend::Policy);
        // "bwrap" resolves to Bwrap when installed, else degrades to Policy.
        c.aishe.sandbox_backend = "bwrap".to_string();
        let expected = if bwrap_available() {
            Backend::Bwrap
        } else {
            Backend::Policy
        };
        assert_eq!(backend(&c), expected);
        assert_eq!(bwrap_requested_but_missing(&c), !bwrap_available());
    }

    #[test]
    fn bwrap_argv_shape() {
        let argv = bwrap_wrap_argv(std::path::Path::new("/home/me/proj"));
        assert_eq!(argv[0], "bwrap");
        assert!(argv.contains(&"--ro-bind".to_string()));
        // the working tree is bound writable and is the chdir target
        assert!(argv
            .windows(3)
            .any(|w| w == ["--bind", "/home/me/proj", "/home/me/proj"]));
        assert!(argv.windows(2).any(|w| w == ["--chdir", "/home/me/proj"]));
        assert_eq!(argv.last().unwrap(), "--"); // shell is appended after
    }

    fn cfg_with(confirm: &str, dangerous: bool) -> Config {
        let mut c = Config::default();
        c.aishe.yolo_confirm = confirm.to_string();
        c.aishe.yolo_confirm_dangerous = dangerous;
        c
    }

    #[test]
    fn confirm_tier_precedence() {
        // Default string + boolean true => Dangerous (current behavior).
        assert_eq!(confirm_tier(&cfg_with("dangerous", true)), Tier::Dangerous);
        // Default string + boolean false => Never (legacy meaning honored).
        assert_eq!(confirm_tier(&cfg_with("dangerous", false)), Tier::Never);
        // Explicit non-default string wins over the boolean.
        assert_eq!(confirm_tier(&cfg_with("never", true)), Tier::Never);
        assert_eq!(confirm_tier(&cfg_with("writes", true)), Tier::Writes);
        assert_eq!(confirm_tier(&cfg_with("writes", false)), Tier::Writes);
        assert_eq!(confirm_tier(&cfg_with("all", false)), Tier::All);
        // Unrecognized string falls back to Dangerous.
        assert_eq!(confirm_tier(&cfg_with("bogus", true)), Tier::Dangerous);
        // Case-insensitive.
        assert_eq!(confirm_tier(&cfg_with("ALL", true)), Tier::All);
    }

    #[test]
    fn needs_confirm_by_tier() {
        assert_eq!(needs_confirm(Tier::Never, "rm -rf /"), (false, true));
        assert_eq!(needs_confirm(Tier::Dangerous, "rm -rf /"), (true, true));
        assert_eq!(needs_confirm(Tier::Dangerous, "ls"), (false, false));
        assert_eq!(needs_confirm(Tier::Writes, "ls"), (false, false));
        assert_eq!(needs_confirm(Tier::Writes, "touch f"), (true, false));
        assert_eq!(needs_confirm(Tier::All, "ls"), (true, false));
        // An unresolvable head is flagged like a dangerous one: confirm at the
        // default tier, and let `safety_gate` choose the (milder) panel.
        assert_eq!(
            needs_confirm(Tier::Dangerous, "$(which rm) -rf /"),
            (true, true)
        );
        assert_eq!(
            needs_confirm(Tier::Never, "$(which rm) -rf /"),
            (false, true)
        );
    }

    #[test]
    fn write_classifier_reads() {
        for cmd in [
            "ls -la",
            "cat README.md",
            "grep foo src",
            "find . -name '*.rs'",
            "echo hi",
            "pwd",
            "head -n 5 f",
            "tail f",
            "wc -l f",
            "stat f",
            "git status",
            "git log --oneline",
            "git diff HEAD",
            "git show abc",
            "/usr/bin/cat f",
        ] {
            assert!(!is_write_command(cmd), "expected READ: {cmd}");
        }
    }

    #[test]
    fn write_classifier_writes() {
        for cmd in [
            "touch f",
            "mkdir d",
            "rm f",
            "cp a b",
            "mv a b",
            "echo hi > f",
            "cat a >> b",
            "git commit -m x",
            "git push",
            "git add .",
            "tee out.txt",
            "sh build.sh",
            "ls && rm f",
        ] {
            assert!(is_write_command(cmd), "expected WRITE: {cmd}");
        }
    }

    #[test]
    fn network_classifier() {
        for cmd in [
            "curl https://x",
            "wget http://x",
            "ssh host",
            "scp a host:b",
            "nc -l 80",
            "telnet host 23",
            "rsync a host:b",
            "git clone https://x",
            "git fetch",
            "git pull",
            "git push origin main",
            "npm install",
            "pip install foo",
            "pip3 download bar",
            "cargo install ripgrep",
            "apt-get install foo",
            "/usr/bin/curl x",
            "ls | curl x",
        ] {
            assert!(is_network_command(cmd), "expected NETWORK: {cmd}");
        }
        for cmd in [
            "ls -la",
            "cat f",
            "git status",
            "git log",
            "cargo build",
            "npm run test",
            "echo curl",
        ] {
            assert!(!is_network_command(cmd), "expected LOCAL: {cmd}");
        }
    }

    #[test]
    fn out_of_tree_write_detection() {
        assert_eq!(
            out_of_tree_write("echo hi > /etc/passwd").as_deref(),
            Some("/etc/passwd")
        );
        assert_eq!(
            out_of_tree_write("cat x >> ~/.bashrc").as_deref(),
            Some("~/.bashrc")
        );
        assert_eq!(
            out_of_tree_write("cp a ../escape").as_deref(),
            Some("../escape")
        );
        assert_eq!(out_of_tree_write("mv f /tmp/g").as_deref(), Some("/tmp/g"));
        assert_eq!(
            out_of_tree_write("dd if=x of=/dev/sda").as_deref(),
            Some("/dev/sda")
        );
        assert_eq!(out_of_tree_write("touch ~/note").as_deref(), Some("~/note"));
        assert_eq!(
            out_of_tree_write("echo hi >/var/log/x").as_deref(),
            Some("/var/log/x")
        );
        // A variable-expanded target can escape the tree, so it counts as
        // out-of-tree (previously `$HOME/...` slipped through the sandbox).
        assert_eq!(
            out_of_tree_write("cp secret.txt $HOME/exfil").as_deref(),
            Some("$HOME/exfil")
        );
        assert_eq!(
            out_of_tree_write("echo hi > ${TMPDIR}/x").as_deref(),
            Some("${TMPDIR}/x")
        );
        // In-tree writes are allowed.
        assert_eq!(out_of_tree_write("echo hi > out.txt"), None);
        assert_eq!(out_of_tree_write("cp a b"), None);
        assert_eq!(out_of_tree_write("touch src/new.rs"), None);
        assert_eq!(out_of_tree_write("ls -la"), None);
    }

    #[test]
    fn sandbox_refusal_reasons() {
        assert!(sandbox_refusal("curl https://x")
            .unwrap()
            .contains("network"));
        assert!(sandbox_refusal("echo hi > /etc/x")
            .unwrap()
            .contains("outside"));
        assert!(sandbox_refusal("ls -la").is_none());
        assert!(sandbox_refusal("echo hi > out.txt").is_none());
    }
}
