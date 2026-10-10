//! Extended (timestamped) history log, written alongside reedline's own history.
//!
//! Each persisted command is appended in zsh `EXTENDED_HISTORY` format
//! (`: <epoch>:<duration>;<command>`), which zsh itself can read. This gives the
//! `history` builtin numbered, timestamped entries, and - when the log is shared
//! across sessions - cross-session history visibility (zsh `SHARE_HISTORY`),
//! without a database dependency.

use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Byte size past which [`append`] trims the log (cheap `stat` gate so we don't
/// re-read on every append). The interactive PTY also caps the log on exit; this
/// keeps it bounded under sustained `-c`/hook usage between interactive sessions.
const MAX_BYTES: u64 = 4 * 1024 * 1024;
/// Entries kept when the log is trimmed (most recent).
const KEEP_LINES: usize = 10_000;

/// Seconds since the Unix epoch (0 if the clock is before 1970).
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Append one command to the log in `EXTENDED_HISTORY` format. Newlines in the
/// command are flattened so each entry stays on one line. Best-effort; the log is
/// trimmed to [`KEEP_LINES`] once it grows past [`MAX_BYTES`], so it can't grow
/// without bound.
pub fn append(path: &Path, command: &str) {
    let flat = command.replace('\n', " ");
    // Ensure the data dir exists; a fresh install may not have created it yet.
    if let Some(parent) = path.parent() {
        let metadata = std::fs::metadata(parent).ok();
        if !metadata.as_ref().is_some_and(|metadata| metadata.is_dir()) {
            let _ = std::fs::create_dir_all(parent);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // Warm commands already have a private history directory. Avoid
            // rewriting its inode permissions on every append, while still
            // repairing an existing directory with broader permissions.
            if metadata
                .as_ref()
                .is_none_or(|metadata| metadata.permissions().mode() & 0o7777 != 0o700)
            {
                let _ = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700));
            }
        }
    }
    let mut options = std::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut recorded_size = None;
    if let Ok(mut f) = options.open(path) {
        // Formatting directly into File issues a write for each formatting
        // fragment. Assemble the record so a normal append is one write,
        // rather than independently appended timestamp/command fragments.
        let entry = format!(": {}:0;{}\n", now(), flat);
        let _ = f.write_all(entry.as_bytes());
        let metadata = f.metadata().ok();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata
                .as_ref()
                .is_none_or(|metadata| metadata.permissions().mode() & 0o7777 != 0o600)
            {
                // Repair the file we actually appended to, even if another
                // shell replaces the pathname while we hold it open.
                let _ = f.set_permissions(std::fs::Permissions::from_mode(0o600));
            }
        }
        recorded_size = metadata.map(|metadata| metadata.len());
    }
    maybe_trim(path, recorded_size);
}

/// Trim the log to the most recent [`KEEP_LINES`] entries when it exceeds
/// [`MAX_BYTES`]. Gated on a cheap size check so the O(n) rewrite happens rarely
/// (only when over the cap). Best-effort and atomic; a lost concurrent append is
/// acceptable for a history log.
fn maybe_trim(path: &Path, recorded_size: Option<u64>) {
    let over = recorded_size
        .or_else(|| std::fs::metadata(path).ok().map(|metadata| metadata.len()))
        .map(|size| size > MAX_BYTES)
        .unwrap_or(false);
    if !over {
        return;
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= KEEP_LINES {
        return; // huge lines, not many — leave it
    }
    let start = lines.len() - KEEP_LINES;
    let mut buf = lines[start..].join("\n");
    buf.push('\n');
    let _ = crate::config::write_atomic(path, buf.as_bytes());
}

/// Parse the log into `(epoch, command)` entries, oldest first. Lines without the
/// `: <epoch>:<dur>;` prefix are kept verbatim with epoch 0 (so a plain history
/// file still lists).
pub fn read(path: &Path) -> Vec<(u64, String)> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut out = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix(": ") {
            if let Some((meta, cmd)) = rest.split_once(';') {
                let epoch = meta
                    .split(':')
                    .next()
                    .and_then(|s| s.trim().parse().ok())
                    .unwrap_or(0);
                out.push((epoch, cmd.to_string()));
                continue;
            }
        }
        if !line.trim().is_empty() {
            out.push((0, line.to_string()));
        }
    }
    out
}

/// Format a numbered listing of the last `count` entries (all when `None`), with
/// an optional UTC timestamp column, like zsh `history` / `history -E`.
pub fn format(entries: &[(u64, String)], count: Option<usize>, with_ts: bool) -> String {
    let start = match count {
        Some(n) if n < entries.len() => entries.len() - n,
        _ => 0,
    };
    let mut out = String::new();
    for (i, (ts, cmd)) in entries.iter().enumerate().skip(start) {
        let n = i + 1;
        if with_ts {
            out.push_str(&format!("{n:>5}  {}  {cmd}\n", fmt_time(*ts)));
        } else {
            out.push_str(&format!("{n:>5}  {cmd}\n"));
        }
    }
    out
}

/// Format a Unix timestamp as `YYYY-MM-DD HH:MM` (UTC). `0` (unknown) renders as
/// a blank, fixed-width placeholder so columns stay aligned.
fn fmt_time(epoch: u64) -> String {
    if epoch == 0 {
        return " ".repeat(16);
    }
    let days = (epoch / 86_400) as i64;
    let secs = epoch % 86_400;
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}",
        secs / 3600,
        (secs % 3600) / 60
    )
}

/// Days since 1970-01-01 to `(year, month, day)`, UTC (Howard Hinnant's
/// `civil_from_days`).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary_history(label: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let directory = std::env::temp_dir().join(format!(
            "aishe-histlog-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        directory.join("history.ext")
    }

    fn remove_history(path: &Path) {
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn append_trims_when_over_the_byte_cap() {
        let path = temporary_history("trim");
        // Seed a log already past MAX_BYTES with more than KEEP_LINES entries.
        let pad = "x".repeat(400);
        let mut seed = String::new();
        for i in 0..(KEEP_LINES + 1500) {
            seed.push_str(&format!(": 1700000000:0;cmd{i} {pad}\n"));
        }
        std::fs::write(&path, &seed).unwrap();
        assert!(std::fs::metadata(&path).unwrap().len() > MAX_BYTES);
        // One append triggers the trim down to KEEP_LINES (+ the appended line).
        append(&path, "final marker");
        let entries = read(&path);
        assert!(entries.len() <= KEEP_LINES + 1, "got {}", entries.len());
        assert_eq!(entries.last().unwrap().1, "final marker");
        remove_history(&path);
    }

    #[test]
    fn append_and_read_roundtrip() {
        let path = temporary_history("roundtrip");
        append(&path, "echo one");
        append(&path, "git status\nstray"); // newline flattened
        let entries = read(&path);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].1, "echo one");
        assert_eq!(entries[1].1, "git status stray");
        assert!(entries[0].0 > 0, "timestamp recorded");
        remove_history(&path);
    }

    #[cfg(unix)]
    #[test]
    fn append_repairs_private_permissions_and_preserves_entries() {
        use std::os::unix::fs::PermissionsExt;
        let path = temporary_history("permissions");
        let parent = path.parent().unwrap();
        std::fs::write(&path, ": 1700000000:0;previous entry\n").unwrap();
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        append(&path, "first private entry");
        append(&path, "second private entry");
        assert_eq!(
            std::fs::metadata(parent).unwrap().permissions().mode() & 0o7777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o7777,
            0o600
        );
        let entries = read(&path);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].1, "previous entry");
        assert_eq!(entries[2].1, "second private entry");
        remove_history(&path);
    }

    #[test]
    fn concurrent_appends_keep_complete_timestamped_entries() {
        use std::collections::HashSet;
        use std::sync::{Arc, Barrier};
        const WORKERS: usize = 8;
        const ENTRIES_PER_WORKER: usize = 100;
        let path = temporary_history("concurrent");
        append(&path, "warm-up");
        let ready = Arc::new(Barrier::new(WORKERS));
        let threads: Vec<_> = (0..WORKERS)
            .map(|worker| {
                let path = path.clone();
                let ready = Arc::clone(&ready);
                std::thread::spawn(move || {
                    ready.wait();
                    for entry in 0..ENTRIES_PER_WORKER {
                        append(&path, &format!("worker-{worker}-entry-{entry}"));
                    }
                })
            })
            .collect();
        for thread in threads {
            thread.join().unwrap();
        }
        let entries = read(&path);
        assert_eq!(entries.len(), 1 + WORKERS * ENTRIES_PER_WORKER);
        assert!(entries.iter().all(|(epoch, _)| *epoch > 0));
        let commands: HashSet<_> = entries.into_iter().map(|(_, command)| command).collect();
        assert!(commands.contains("warm-up"));
        for worker in 0..WORKERS {
            for entry in 0..ENTRIES_PER_WORKER {
                assert!(commands.contains(&format!("worker-{worker}-entry-{entry}")));
            }
        }
        remove_history(&path);
    }

    #[test]
    fn read_tolerates_plain_lines() {
        let entries_text = ": 1700000000:0;extended cmd\nplain cmd\n";
        let path = temporary_history("plain");
        std::fs::write(&path, entries_text).unwrap();
        let entries = read(&path);
        assert_eq!(entries[0], (1_700_000_000, "extended cmd".to_string()));
        assert_eq!(entries[1], (0, "plain cmd".to_string()));
        remove_history(&path);
    }

    #[test]
    fn format_numbers_and_limits() {
        let e = vec![
            (0, "a".to_string()),
            (0, "b".to_string()),
            (0, "c".to_string()),
        ];
        let all = format(&e, None, false);
        assert!(all.contains("    1  a"));
        assert!(all.contains("    3  c"));
        // Last 2 keeps the original numbering (2 and 3).
        let last2 = format(&e, Some(2), false);
        assert!(!last2.contains("    1  a"));
        assert!(last2.contains("    2  b"));
        assert!(last2.contains("    3  c"));
    }

    #[test]
    fn fmt_time_is_utc() {
        // 2023-11-14 22:13:20 UTC
        assert_eq!(fmt_time(1_700_000_000), "2023-11-14 22:13");
        assert_eq!(fmt_time(0).trim(), "");
    }
}
