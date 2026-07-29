use std::io::BufRead;

use anyhow::{Context, Result};

const MAX_EVENT_BYTES: usize = 1024 * 1024;

/// Bounded parser for one Server-Sent Event. Unknown fields and comments are
/// ignored. EOF flushes a final unterminated frame.
pub fn next_json(reader: &mut dyn BufRead) -> Result<Option<serde_json::Value>> {
    let mut data = String::new();
    let mut line = String::new();
    loop {
        line.clear();
        let read = reader.read_line(&mut line)?;
        if read == 0 {
            if data.is_empty() {
                return Ok(None);
            }
            break;
        }
        if line.len() > MAX_EVENT_BYTES || data.len().saturating_add(line.len()) > MAX_EVENT_BYTES {
            anyhow::bail!("OpenCode SSE event exceeds the 1 MiB limit");
        }
        let trimmed = line.trim_end_matches(['\r', '\n']);
        if trimmed.is_empty() {
            if data.is_empty() {
                continue;
            }
            break;
        }
        if let Some(value) = trimmed.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(value.strip_prefix(' ').unwrap_or(value));
        }
    }
    serde_json::from_str(&data)
        .context("decoding bounded OpenCode SSE event")
        .map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fragmented_multiline_and_final_frame() {
        let input = b": keepalive\r\ndata: {\"a\":\r\ndata: 1}\r\n\r\ndata: {\"b\":2}";
        let mut reader = std::io::BufReader::new(input.as_slice());
        assert_eq!(
            next_json(&mut reader).unwrap(),
            Some(serde_json::json!({"a": 1}))
        );
        assert_eq!(
            next_json(&mut reader).unwrap(),
            Some(serde_json::json!({"b": 2}))
        );
        assert_eq!(next_json(&mut reader).unwrap(), None);
    }

    #[test]
    fn rejects_oversized_frames() {
        let input = format!("data: \"{}\"\n\n", "x".repeat(MAX_EVENT_BYTES));
        let mut reader = std::io::BufReader::new(input.as_bytes());
        assert!(next_json(&mut reader).is_err());
    }
}
