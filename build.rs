//! Capture the short git SHA and build date so `aishe --version` can report the
//! exact build. Both fall back to "unknown" when unavailable (e.g. a source
//! tarball without a git checkout), so the build never fails on their account.

use sha2::{Digest, Sha256};
use std::process::Command;

fn git_output(args: &[&str]) -> Option<String> {
    Command::new("git")
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

fn watch_git_path(path: &str) {
    if let Some(resolved) = git_output(&["rev-parse", "--git-path", path]) {
        println!("cargo:rerun-if-changed={resolved}");
    }
}

fn main() {
    let sha =
        git_output(&["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "unknown".to_string());
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

    let plugin_path = "assets/backend/opencode/aishe-plugin.mjs";
    let plugin = std::fs::read(plugin_path).expect("reading trusted OpenCode plugin");
    let plugin_sha = format!("{:x}", Sha256::digest(&plugin));
    println!("cargo:rustc-env=AISHE_OPENCODE_PLUGIN_SHA256={plugin_sha}");
    println!("cargo:rerun-if-changed={plugin_path}");
    println!("cargo:rerun-if-changed=assets/backend/opencode/runtime-manifest.json");

    // `.git/HEAD` usually contains only `ref: refs/heads/<branch>`, so committing
    // advances the referenced file without changing HEAD itself. Watch both the
    // resolved Git paths and packed-refs; `git rev-parse --git-path` also handles
    // linked worktrees where `.git` is a file rather than a directory.
    watch_git_path("HEAD");
    if let Some(head_ref) = git_output(&["symbolic-ref", "-q", "HEAD"]) {
        watch_git_path(&head_ref);
    }
    watch_git_path("packed-refs");
}
