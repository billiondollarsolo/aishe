//! Capture the short git SHA and build date so `aishe --version` can report the
//! exact build. Both fall back to "unknown" when unavailable (e.g. a source
//! tarball without a git checkout), so the build never fails on their account.

use std::process::Command;

fn main() {
    let sha = Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=AISHE_GIT_SHA={sha}");

    let date = Command::new("date")
        .args(["-u", "+%Y-%m-%d"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=AISHE_BUILD_DATE={date}");

    // Rebuild the version string when the checked-out commit changes.
    println!("cargo:rerun-if-changed=.git/HEAD");
}
