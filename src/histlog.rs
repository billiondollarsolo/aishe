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

/// Seconds since the Unix epoch (0 if the clock is before 1970).
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Append one command to the log in `EXTENDED_HISTORY` format. Newlines in the
/// command are flattened so each entry stays on one line. Best-effort.
pub fn append(path: &Path, command: &str) {
    let flat = command.replace('\n', " ");
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(f, ": {}:0;{}", now(), flat);
    }
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

    #[test]
    fn append_and_read_roundtrip() {
        let path = std::env::temp_dir().join(format!("aishe-histlog-{}.ext", std::process::id()));
        std::fs::remove_file(&path).ok();
        append(&path, "echo one");
        append(&path, "git status\nstray"); // newline flattened
        let entries = read(&path);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].1, "echo one");
        assert_eq!(entries[1].1, "git status stray");
        assert!(entries[0].0 > 0, "timestamp recorded");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn read_tolerates_plain_lines() {
        let entries_text = ": 1700000000:0;extended cmd\nplain cmd\n";
        let path =
            std::env::temp_dir().join(format!("aishe-histlog-plain-{}.ext", std::process::id()));
        std::fs::write(&path, entries_text).unwrap();
        let entries = read(&path);
        assert_eq!(entries[0], (1_700_000_000, "extended cmd".to_string()));
        assert_eq!(entries[1], (0, "plain cmd".to_string()));
        std::fs::remove_file(&path).ok();
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
