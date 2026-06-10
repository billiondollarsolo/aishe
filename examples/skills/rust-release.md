---
name: rust-release
description: How to cut a Rust release for this project (when to bump, tag, publish)
---
To cut a release:
1. Ensure `cargo test`, `cargo clippy -- -D warnings`, and `cargo fmt --check` pass.
2. Bump the version in Cargo.toml and update CHANGELOG.md's [Unreleased] section.
3. Commit, then tag `vX.Y.Z` and push the tag — CI builds and attaches binaries.
4. Do not publish to crates.io unless explicitly asked.
