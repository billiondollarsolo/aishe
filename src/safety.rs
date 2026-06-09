//! Destructive-command gate. Conservative by design: we flag commands that can
//! cause irreversible loss. v0.1 has no path-awareness, so `rm -rf` is always
//! Dangerous (see PRD §4.7).

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
        // rm with recursive+force in any flag combination.
        (
            format!(
                r"{CMD}rm\b.*(-[a-z]*r[a-z]*f|-[a-z]*f[a-z]*r|--recursive\s+--force|--no-preserve-root)"
            ),
            "recursive force delete",
        ),
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
            r">\s*/dev/(sd|nvme|hd|disk)".to_string(),
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
        let seg = segment.trim();
        if seg.is_empty() {
            continue;
        }
        for (re, reason) in PATTERNS.iter() {
            if re.is_match(seg) {
                return Risk::Dangerous(reason);
            }
        }
    }
    Risk::Safe
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
        dangerous("rm -rf node_modules");
        dangerous("rm -fr ~/projects");
        dangerous("rm -rf --no-preserve-root /");
        dangerous("sudo rm -rf /var");
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
        dangerous("ls && rm -rf build");
    }

    #[test]
    fn safe_cases() {
        safe("rm file.txt");
        safe("rm -i file.txt");
        safe("rm *.tmp");
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
