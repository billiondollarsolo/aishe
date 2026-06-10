# Installation

aishe is distributed as source today. Prebuilt packages for common systems
(Homebrew, a Linux tarball or package, cargo-binstall, and similar) are planned
but not available yet. Until then, you build from source with Cargo. It is a
single static-ish binary with no runtime services, so building is quick and the
result is easy to move around.

## Requirements

- Rust 1.80 or newer (install from [rustup.rs](https://rustup.rs)).
- `zsh` or `bash` on your `PATH`. aishe delegates command execution to one of
  these, preferring zsh.
- A network-reachable LLM endpoint and an API key, set in an environment
  variable. See [Providers](providers.md).
- Platforms: macOS (arm64 and x86_64) and Linux (x86_64 and arm64). Windows is
  not supported.

## Build and install with Cargo

From a checkout of the repository:

```sh
git clone https://github.com/mjtechguy/aishe
cd aishe
cargo install --path .
```

`cargo install` places the `aishe` binary in `~/.cargo/bin`, which is usually
already on your `PATH`. Confirm it works:

```sh
aishe --version
aishe doctor
```

## Build without installing

If you would rather not install into `~/.cargo/bin`, just build the release
binary and run or copy it yourself:

```sh
cargo build --release
./target/release/aishe --version
```

You can copy `target/release/aishe` anywhere on your `PATH`, for example:

```sh
sudo install -m 0755 target/release/aishe /usr/local/bin/aishe
```

## Keeping it up to date

When installed with `cargo install --path .`, pull the latest source and reinstall:

```sh
git pull
cargo install --path . --force
```

## Uninstall

```sh
cargo uninstall aishe        # if installed with cargo install
# or remove the binary you copied:
sudo rm /usr/local/bin/aishe
```

To remove configuration and data as well:

```sh
rm -rf ~/.config/aishe        # config, custom commands, skills
rm -rf ~/.local/share/aishe   # history
```

## What gets created on first run

- `~/.config/aishe/config.toml` is written by the first-run wizard.
- `~/.local/share/aishe/history` stores reedline command history.
- Nothing else is created until you add custom commands or skills.

## Prebuilt packages (planned)

We intend to publish prebuilt binaries and packages so you do not need a Rust
toolchain. This is not ready yet. When it lands, this page will list the exact
commands (for example a Homebrew formula, a `cargo binstall aishe` line, and a
download for tagged releases). For now, build from source as above.

Next: [Getting started](getting-started.md).
