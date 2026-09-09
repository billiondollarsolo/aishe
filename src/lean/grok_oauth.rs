//! Reuse Grok Build CLI subscription OAuth (`~/.grok/auth.json`) for lean live LLM.
//!
//! The CLI stores an OIDC access token in the `key` field. That bearer works against
//! `https://api.x.ai` (models + responses). Lean injects it into the process env as
//! [`SUBSCRIPTION_TOKEN_ENV`] — never into config files or git.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

/// Process-local env var lean uses so the existing ApiKey provider path can send Bearer.
pub const SUBSCRIPTION_TOKEN_ENV: &str = "AISHE_GROK_SUBSCRIPTION_TOKEN";

const REFRESH_SKEW_SECS: u64 = 120;
const TOKEN_URL: &str = "https://auth.x.ai/oauth2/token";

/// Resolve the Grok CLI auth.json path.
pub fn auth_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("AISHE_GROK_AUTH") {
        let p = p.trim();
        if !p.is_empty() {
            return Some(PathBuf::from(p));
        }
    }
    if let Ok(home) = std::env::var("GROK_HOME") {
        let home = home.trim();
        if !home.is_empty() {
            return Some(PathBuf::from(home).join("auth.json"));
        }
    }
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".grok").join("auth.json"))
}

pub fn available() -> bool {
    access_token().is_some()
}

/// Load (and refresh if needed) the OIDC access token from the Grok CLI session.
pub fn access_token() -> Option<String> {
    let path = auth_path()?;
    if !path.is_file() {
        return None;
    }
    if !owner_only_regular_file(&path) {
        return None;
    }
    let raw = fs::read_to_string(&path).ok()?;
    let mut store: Value = serde_json::from_str(&raw).ok()?;
    let map = store.as_object_mut()?;
    let key_name = map
        .keys()
        .find(|k| is_xai_entry(k, map.get(*k).unwrap()))?
        .clone();
    let refreshed = {
        let entry = map.get_mut(&key_name)?.as_object_mut()?;
        if needs_refresh(entry) {
            let _ = try_refresh(entry);
            true
        } else {
            false
        }
    };
    if refreshed {
        let _ = write_store(&path, &store);
    }

    let token = store
        .as_object()?
        .get(&key_name)?
        .get("key")
        .and_then(Value::as_str)?
        .trim();
    if token.is_empty() {
        return None;
    }
    Some(token.to_string())
}

fn is_xai_entry(store_key: &str, entry: &Value) -> bool {
    if store_key.starts_with("https://auth.x.ai") {
        return true;
    }
    let Some(obj) = entry.as_object() else {
        return false;
    };
    let issuer = obj
        .get("oidc_issuer")
        .and_then(Value::as_str)
        .unwrap_or("");
    let mode = obj.get("auth_mode").and_then(Value::as_str).unwrap_or("");
    mode.eq_ignore_ascii_case("oidc") && issuer.contains("auth.x.ai")
}

fn needs_refresh(entry: &serde_json::Map<String, Value>) -> bool {
    let Some(exp) = entry.get("expires_at").and_then(Value::as_str) else {
        return false;
    };
    let Some(when) = parse_expires_at(exp) else {
        return false;
    };
    let skew = Duration::from_secs(REFRESH_SKEW_SECS);
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|now| when <= now + skew)
        .unwrap_or(false)
}

fn parse_expires_at(s: &str) -> Option<Duration> {
    chrono_lite_parse(s.trim()).ok()
}

/// Minimal RFC3339 (UTC Z / +00:00) → Duration since epoch.
fn chrono_lite_parse(s: &str) -> Result<Duration, ()> {
    let core = s
        .strip_suffix('Z')
        .or_else(|| s.strip_suffix("+00:00"))
        .ok_or(())?;
    let (date, time) = core.split_once('T').ok_or(())?;
    let mut d = date.split('-');
    let y: i64 = d.next().ok_or(())?.parse().map_err(|_| ())?;
    let mo: i64 = d.next().ok_or(())?.parse().map_err(|_| ())?;
    let da: i64 = d.next().ok_or(())?.parse().map_err(|_| ())?;
    let time = time.split('.').next().unwrap_or(time);
    let mut t = time.split(':');
    let h: u32 = t.next().ok_or(())?.parse().map_err(|_| ())?;
    let mi: u32 = t.next().ok_or(())?.parse().map_err(|_| ())?;
    let se: u32 = t.next().ok_or(())?.parse().map_err(|_| ())?;
    let days = days_from_civil(y, mo, da)?;
    let secs = days * 86400 + i64::from(h) * 3600 + i64::from(mi) * 60 + i64::from(se);
    if secs < 0 {
        return Err(());
    }
    Ok(Duration::from_secs(secs as u64))
}

fn days_from_civil(y: i64, m: i64, d: i64) -> Result<i64, ()> {
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return Err(());
    }
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m_adj = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * m_adj + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    Ok(era * 146097 + doe - 719468)
}

fn try_refresh(entry: &mut serde_json::Map<String, Value>) -> Result<(), String> {
    let refresh = entry
        .get("refresh_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "missing refresh_token".to_string())?
        .to_string();
    let client_id = entry
        .get("oidc_client_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "missing oidc_client_id".to_string())?
        .to_string();

    let body = form_encode(&[
        ("grant_type", "refresh_token"),
        ("refresh_token", &refresh),
        ("client_id", &client_id),
    ]);

    let agent = crate::providers::external_http_agent(
        Duration::from_secs(5),
        Some(Duration::from_secs(20)),
        None,
        None,
    );
    let response = agent
        .post(TOKEN_URL)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("Accept", "application/json")
        .header("User-Agent", "aishe-lean-grok-oauth")
        .send(&body)
        .map_err(|e| e.to_string())?;
    let status = response.status();
    if !status.is_success() {
        return Err(format!("refresh HTTP {}", status));
    }
    let v: Value = response
        .into_body()
        .read_json()
        .map_err(|e| e.to_string())?;
    let access = v
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or_else(|| "no access_token".to_string())?;
    entry.insert("key".into(), Value::String(access.to_string()));
    if let Some(rt) = v.get("refresh_token").and_then(Value::as_str) {
        entry.insert("refresh_token".into(), Value::String(rt.to_string()));
    }
    if let Some(secs) = v.get("expires_in").and_then(Value::as_u64) {
        let exp = SystemTime::now() + Duration::from_secs(secs);
        if let Ok(d) = exp.duration_since(UNIX_EPOCH) {
            entry.insert(
                "expires_at".into(),
                Value::String(format_rfc3339(d.as_secs())),
            );
        }
    }
    Ok(())
}

fn format_rfc3339(secs: u64) -> String {
    let days = (secs / 86400) as i64;
    let rem = secs % 86400;
    let (y, m, d) = civil_from_days(days + 719468);
    let h = rem / 3600;
    let mi = (rem % 3600) / 60;
    let se = rem % 60;
    format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{se:02}Z")
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m as u32, d as u32)
}

fn form_encode(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", url_encode(k), url_encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}


/// Refuse symlinks and group/other-readable auth stores (same spirit as `oauth.rs`).
fn owner_only_regular_file(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = match fs::symlink_metadata(path) {
            Ok(m) => m,
            Err(_) => return false,
        };
        if !meta.file_type().is_file() {
            return false;
        }
        if meta.mode() & 0o077 != 0 {
            return false;
        }
        if let Some(home) = std::env::var_os("HOME") {
            if let Ok(hm) = fs::metadata(home) {
                if meta.uid() != hm.uid() {
                    return false;
                }
            }
        }
        true
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        true
    }
}

fn write_store(path: &Path, store: &Value) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let tmp = parent.join(format!(".auth.json.{}.tmp", std::process::id()));
    {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        opts.mode(0o600);
        let mut f = opts.open(&tmp)?;
        let body = serde_json::to_vec_pretty(store)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        f.write_all(&body)?;
        f.write_all(b"\n")?;
        f.sync_all()?;
    }
    #[cfg(unix)]
    {
        let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o600));
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_expires_with_nanos() {
        let d = parse_expires_at("2026-09-09T18:11:57.602925442Z").expect("parse");
        assert!(d.as_secs() > 1_700_000_000);
    }

    #[test]
    fn loads_token_from_fixture() {
        let dir = std::env::temp_dir().join(format!("aishe-grok-oauth-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("auth.json");
        fs::write(
            &path,
            r#"{"https://auth.x.ai::abc":{"auth_mode":"oidc","oidc_issuer":"https://auth.x.ai","oidc_client_id":"abc","key":"tok-live","refresh_token":"rt","expires_at":"2099-01-01T00:00:00Z"}}"#,
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        }

        std::env::set_var("AISHE_GROK_AUTH", &path);
        let tok = access_token().expect("token");
        assert_eq!(tok, "tok-live");
        std::env::remove_var("AISHE_GROK_AUTH");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn available_false_when_missing() {
        std::env::set_var(
            "AISHE_GROK_AUTH",
            "/tmp/aishe-definitely-missing-auth.json",
        );
        assert!(!available());
        std::env::remove_var("AISHE_GROK_AUTH");
    }
}
