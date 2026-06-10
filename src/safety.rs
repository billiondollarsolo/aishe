//! Destructive-command gate. Conservative by design: we flag commands that can
//! cause irreversible loss, while staying path-aware for `rm -rf` (a relative
//! in-tree target is the user's own files and is allowed).
//!
//! Each command line is normalized (lowercased, whitespace-collapsed), split on
//! shell operators, and each segment has its leading prefixes stripped (env
//! assignments and privilege/wrapper words like `sudo`, `doas`, `env`, `time`,
//! `nohup`, `nice`, `timeout`) so wrappers cannot smuggle a dangerous command
//! past the anchored patterns. `rm` targets are unquoted before the path check so
//! `rm -rf "$HOME"` and `rm -rf '/'` are still caught.

use std::sync::LazyLock;

use regex::Regex;

/// Risk classification for a command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Risk {
    Safe,
    /// Dangerous, with a short human-readable reason.
    Dangerous(&'static str),
}

/// Patterns tested against each whitespace-normalized command segment.
static PATTERNS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    // Command-name patterns are anchored to the start of a segment (after an
    // optional `sudo`) so a tool name appearing inside a quoted argument — e.g.
    // `echo 'rm -rf ...'` — is not flagged. `CMD` is replaced with that anchor.
    const CMD: &str = r"^(sudo\s+)?";
    let raw: &[(String, &str)] = &[
        // Recursive-force `rm` is handled separately by `rm_recursive_force_risk`,
        // which is path-aware (a relative target inside the tree is allowed).
        // rm/mv targeting bare /, ~, or $HOME.
        (
            format!(r"{CMD}(rm|mv)\b[^|]*\s(/|~|\$home)(\s|$)"),
            "destroys home or root",
        ),
        // dd writing to a device.
        (format!(r"{CMD}dd\b.*of=/dev/"), "raw device write"),
        // filesystem creation / disk tools.
        (format!(r"{CMD}mkfs"), "disk formatting"),
        (format!(r"{CMD}fdisk\b"), "disk partitioning"),
        (format!(r"{CMD}parted\b"), "disk partitioning"),
        (format!(r"{CMD}diskutil\s+erase"), "disk formatting"),
        (format!(r"{CMD}wipefs\b"), "wipes filesystem signatures"),
        (format!(r"{CMD}shred\b.*\s/dev/"), "shred a device"),
        // recursive perms/ownership on root.
        (
            format!(r"{CMD}chmod\b.*-r.*\s/(\s|$)"),
            "recursive root perms",
        ),
        (
            format!(r"{CMD}chown\b.*-r.*\s/(\s|$)"),
            "recursive root ownership",
        ),
        // fork bomb.
        (r":\(\)\s*\{.*:\|:.*&.*\}".to_string(), "fork bomb"),
        (r":\(\)\{:\|:&\};:".to_string(), "fork bomb"),
        // redirect to a block device.
        (
            r">\s*/dev/(sd|nvme|hd|disk|vd|xvd|mmcblk|loop)".to_string(),
            "device overwrite",
        ),
        // piping a remote script straight into a shell.
        (
            r"\b(curl|wget)\b[^|]*\|\s*(sudo\s+)?(ba)?sh".to_string(),
            "remote script piped to shell",
        ),
        // git history loss.
        (
            format!(r"{CMD}git\s+push\b.*(--force|-f)\b.*\b(main|master)\b"),
            "force push to main/master",
        ),
        (
            format!(r"{CMD}git\s+push\b.*\b(origin\s+)?(main|master)\b.*(--force|-f)\b"),
            "force push to main/master",
        ),
        (
            format!(r"{CMD}git\s+reset\s+--hard\b"),
            "discards local changes",
        ),
        (
            format!(r"{CMD}git\s+clean\b[^|]*\s-\S*f"),
            "deletes untracked files",
        ),
        // taking the system down.
        (format!(r"{CMD}kill\s+-9\s+1(\s|$)"), "killing init (pid 1)"),
        (format!(r"{CMD}shutdown\b"), "system shutdown"),
        (format!(r"{CMD}reboot\b"), "system reboot"),
        (format!(r"{CMD}halt\b"), "system halt"),
        // mass delete via find rooted at / or ~.
        (format!(r"{CMD}find\s+(/|~)\S*.*-delete"), "mass delete"),
        (format!(r"{CMD}find\s+(/|~)\S*.*-exec\s+rm"), "mass delete"),
        // mass truncate via glob.
        (format!(r"{CMD}truncate\s+-s\s*0\b.*\*"), "mass truncate"),
    ];
    raw.iter()
        .map(|(p, reason)| (Regex::new(p).expect("safety regex must compile"), *reason))
        .collect::<Vec<_>>()
});

/// Assess a command line. Splits on shell operators and tests each segment.
pub fn assess(command: &str) -> Risk {
    let normalized = normalize(command);
    for segment in split_segments(&normalized) {
        // Drop leading env assignments and privilege/wrapper prefixes so a
        // wrapped command (`sudo -i rm -rf /`, `FOO=bar rm -rf /`, `env rm …`,
        // `time rm …`) is judged on its real head.
        let stripped = strip_prefixes(segment.trim());
        let seg = stripped.trim();
        if seg.is_empty() {
            continue;
        }
        // Path-aware recursive-`rm` check first (it may *clear* an otherwise
        // scary-looking `rm -rf` when every target is a relative in-tree path).
        if let Some(reason) = rm_recursive_force_risk(seg) {
            return Risk::Dangerous(reason);
        }
        for (re, reason) in PATTERNS.iter() {
            if re.is_match(seg) {
                return Risk::Dangerous(reason);
            }
        }
    }
    Risk::Safe
}

/// Strip leading environment assignments and privilege/wrapper words from a
/// segment, returning the remainder (the real command and its arguments). This
/// prevents wrappers from hiding a dangerous command from the anchored patterns.
fn strip_prefixes(seg: &str) -> String {
    let toks: Vec<&str> = seg.split_whitespace().collect();
    let mut i = 0;
    while i < toks.len() {
        let t = toks[i];
        if is_leading_assignment(t) {
            i += 1;
            continue;
        }
        match t {
            "sudo" | "doas" => {
                i += 1;
                i = skip_opts(
                    &toks,
                    i,
                    &["-u", "-g", "--user", "--group", "-p", "--prompt"],
                );
            }
            // env: following `K=V` are handled by the assignment branch above.
            "env" => {
                i += 1;
                i = skip_opts(&toks, i, &[]);
            }
            "time" | "nohup" | "command" | "builtin" | "exec" | "setsid" => {
                i += 1;
            }
            "nice" | "ionice" => {
                i += 1;
                i = skip_opts(&toks, i, &["-n", "-c", "-p", "--adjustment"]);
            }
            "timeout" => {
                i += 1;
                i = skip_opts(&toks, i, &["-s", "--signal", "-k", "--kill-after"]);
                // Consume the DURATION argument.
                if i < toks.len() && !toks[i].starts_with('-') {
                    i += 1;
                }
            }
            _ => break,
        }
    }
    toks[i..].join(" ")
}

/// Skip a run of leading `-flag` tokens starting at `i`; when a flag is in
/// `with_arg`, also consume the following non-flag argument. Returns the new
/// index.
fn skip_opts(toks: &[&str], mut i: usize, with_arg: &[&str]) -> usize {
    while i < toks.len() && toks[i].starts_with('-') {
        let opt = toks[i];
        i += 1;
        if with_arg.contains(&opt) && i < toks.len() && !toks[i].starts_with('-') {
            i += 1;
        }
    }
    i
}

/// A leading `NAME=VALUE` environment assignment (valid shell identifier name).
fn is_leading_assignment(tok: &str) -> bool {
    match tok.find('=') {
        Some(eq) if eq > 0 => {
            let name = &tok[..eq];
            name.bytes()
                .next()
                .map(|b| b == b'_' || b.is_ascii_alphabetic())
                .unwrap_or(false)
                && name.bytes().all(|b| b == b'_' || b.is_ascii_alphanumeric())
        }
        _ => false,
    }
}

/// Path-aware risk for a recursive, forced `rm` (`rm -rf`, `rm -fr`,
/// `rm --recursive --force`, …). Returns `Some(reason)` when the deletion could
/// be catastrophic — a system/absolute path, the home dir, a variable, a bare
/// glob, or a target that escapes the current tree (`..`) — and `None` when
/// every target is an ordinary relative path inside the working tree (e.g.
/// `node_modules`, `build dist`, `./target`), which is treated as the user's own
/// project files and left to run.
///
/// Operates on the normalized (lowercased, space-collapsed) segment; the danger
/// signals (`/`, `~`, `$`, `..`, `*`) are case-insensitive, so lowercasing the
/// path is harmless here.
fn rm_recursive_force_risk(seg: &str) -> Option<&'static str> {
    let mut tokens = seg.split_whitespace();
    let mut head = tokens.next()?;
    if head == "sudo" {
        head = tokens.next()?;
    }
    if head != "rm" {
        return None;
    }

    let mut recursive = false;
    let mut force = false;
    let mut targets: Vec<&str> = Vec::new();
    for tok in tokens {
        match tok {
            "--recursive" => recursive = true,
            "--force" => force = true,
            "--no-preserve-root" => return Some("recursive delete of root"),
            _ if tok.starts_with("--") => {} // other long options: ignore
            _ if tok.starts_with('-') => {
                let flags = &tok[1..];
                if flags.contains('r') || flags.contains('R') {
                    recursive = true;
                }
                if flags.contains('f') {
                    force = true;
                }
            }
            _ => targets.push(tok),
        }
    }

    if !(recursive && force) {
        return None;
    }
    // Strip quotes so `rm -rf "$HOME"`, `'/'`, `"/etc"` are still seen as their
    // dangerous targets.
    let cleaned: Vec<String> = targets.iter().map(|t| unquote(t)).collect();
    if cleaned.iter().all(|t| t.is_empty()) {
        return Some("recursive force delete with no target");
    }
    if cleaned.iter().any(|t| is_dangerous_path(t)) {
        return Some("recursive force delete of a system or out-of-tree path");
    }
    None
}

/// Remove all quote characters from a token (so partial quoting like
/// `"$HOME"/.config` is also neutralized for the path check).
fn unquote(t: &str) -> String {
    t.replace(['"', '\''], "")
}

/// Lexical test for a deletion target that is *not* a safe relative in-tree
/// path: absolute (`/…`), home (`~…`), a variable (`$…`), a bare or cwd-wiping
/// target (`/`/`~`/`.`/`./`/`..`/`*`/`./*`), or anything containing a `..`
/// segment that could escape the tree.
fn is_dangerous_path(p: &str) -> bool {
    matches!(
        p,
        "/" | "~" | "." | "./" | ".." | "../" | "*" | "./*" | "/*" | "~/*"
    ) || p.starts_with('/')
        || p.starts_with('~')
        || p.starts_with('$')
        || p.starts_with("../")
        || p.contains("/../")
        || p.ends_with("/..")
}

/// Lowercase and collapse runs of whitespace into single spaces.
fn normalize(command: &str) -> String {
    let lower = command.to_lowercase();
    let mut out = String::with_capacity(lower.len());
    let mut prev_space = false;
    for c in lower.chars() {
        if c.is_whitespace() {
            if !prev_space {
                out.push(' ');
            }
            prev_space = true;
        } else {
            out.push(c);
            prev_space = false;
        }
    }
    out.trim().to_string()
}

/// Split on `;`, `&&`, `||`, `|` (naive; good enough for the safety gate).
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
            (';', _) | ('|', _) => {
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
    // For pipe-into-shell detection we also keep the whole line as a segment,
    // since the pattern spans the `|`.
    segments.push(command.to_string());
    segments
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dangerous(cmd: &str) {
        assert!(
            matches!(assess(cmd), Risk::Dangerous(_)),
            "expected DANGEROUS: {cmd}"
        );
    }
    fn safe(cmd: &str) {
        assert_eq!(assess(cmd), Risk::Safe, "expected SAFE: {cmd}");
    }

    #[test]
    fn dangerous_cases() {
        dangerous("rm -rf /");
        dangerous("rm -fr ~/projects");
        dangerous("rm -rf --no-preserve-root /");
        dangerous("sudo rm -rf /var");
        // Path-aware recursive rm: catastrophic / out-of-tree targets.
        dangerous("rm -rf /tmp/cache");
        dangerous("rm -rf ../sibling");
        dangerous("rm -rf $HOME");
        dangerous("rm -rf *");
        dangerous("rm -rf foo/../../etc");
        dangerous("dd if=/dev/zero of=/dev/sda");
        dangerous("mkfs.ext4 /dev/sdb1");
        dangerous("fdisk /dev/sda");
        dangerous("parted /dev/sda");
        dangerous("diskutil eraseDisk JHFS+ Disk /dev/disk2");
        dangerous("chmod -R 777 /");
        dangerous("chown -R root /");
        dangerous(":(){ :|:& };:");
        dangerous("echo hi > /dev/sda");
        dangerous("rm /");
        dangerous("mv ~ /tmp/x");
        dangerous("curl http://x.sh | sh");
        dangerous("wget -qO- http://x.sh | bash");
        dangerous("curl x.sh | sudo bash");
        dangerous("git push --force origin main");
        dangerous("git push -f origin master");
        dangerous("git reset --hard HEAD~3");
        dangerous("kill -9 1");
        dangerous("shutdown -h now");
        dangerous("reboot");
        dangerous("find / -name '*.log' -delete");
        dangerous("find ~ -type f -exec rm {} +");
    }

    #[test]
    fn safe_cases() {
        safe("rm file.txt");
        safe("rm -i file.txt");
        safe("rm *.tmp");
        // Path-aware recursive rm: relative, in-tree targets are the user's own
        // project files — not flagged (the whole point of path-awareness).
        safe("rm -rf node_modules");
        safe("rm -rf build dist");
        safe("rm -rf ./target");
        safe("ls && rm -rf build");
        safe("ls -la");
        safe("git status");
        safe("git push origin feature-branch");
        safe("git reset --soft HEAD~1");
        safe("grep rf file");
        safe("grep -rf patterns.txt .");
        safe("echo 'rm -rf is dangerous'");
        safe("cat README.md");
        safe("dd if=input.img of=output.img");
        safe("find . -name '*.rs'");
        safe("chmod 644 file.txt");
        safe("chmod +x script.sh");
        safe("chown user:user file.txt");
        safe("docker ps");
        safe("cargo build");
        safe("npm install");
        safe("curl https://api.example.com/data");
        safe("kill -9 12345");
        safe("truncate -s 0 logfile.txt");
        safe("mv old.txt new.txt");
    }
}
