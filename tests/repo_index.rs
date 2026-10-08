//! Repository retrieval must stay usable as working-tree files disappear and
//! when tracked binary assets precede the eligible text files.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "aishe-repo-index-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(root.join("config/aishe")).unwrap();
        std::fs::write(
            root.join("config/aishe/config.toml"),
            "[aishe]\nprovider = \"openai\"\n",
        )
        .unwrap();
        git(&root, &["init", "--quiet"]);
        Self(root)
    }

    fn index(&self) -> assert_cmd::Command {
        let mut command = assert_cmd::Command::cargo_bin("aishe").unwrap();
        command
            .current_dir(&self.0)
            .env("AISHE_CONFIG_DIR", self.0.join("config"))
            .env("AISHE_DATA_DIR", self.0.join("data"));
        command
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn git(root: &Path, args: &[&str]) {
    assert!(Command::new("git")
        .args(args)
        .current_dir(root)
        .status()
        .unwrap()
        .success());
}

#[test]
fn refresh_removes_deleted_tracked_files_and_preserves_search() {
    let fixture = Fixture::new();
    std::fs::write(fixture.0.join("keep.rs"), "retained_search_token").unwrap();
    std::fs::write(fixture.0.join("gone.rs"), "deleted_search_token").unwrap();
    git(&fixture.0, &["add", "keep.rs", "gone.rs"]);
    fixture.index().args(["index", "--json"]).assert().success();
    std::fs::remove_file(fixture.0.join("gone.rs")).unwrap();
    let output = fixture.index().args(["index", "--json"]).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["index"]["files"], 1);
    let output = fixture
        .index()
        .args(["index", "--query", "retained_search_token", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["matches"][0]["path"], "keep.rs");
}

#[test]
fn skipped_binary_assets_do_not_truncate_tracked_text_discovery() {
    let fixture = Fixture::new();
    std::fs::create_dir(fixture.0.join("assets")).unwrap();
    for i in 0..10_002 {
        std::fs::write(fixture.0.join(format!("assets/{i:05}.bin")), [0]).unwrap();
    }
    std::fs::write(fixture.0.join("z-last.rs"), "late_text_token").unwrap();
    git(&fixture.0, &["add", "assets", "z-last.rs"]);
    let output = fixture.index().args(["index", "--json"]).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["index"]["files"], 1);
}
