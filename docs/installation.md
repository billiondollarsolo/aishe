# Installation

aishe is a single static-ish binary with no runtime services. You can install a
prebuilt binary from a tagged release (fastest, no Rust toolchain) or build from
source with Cargo.

## Requirements

- Rust 1.80 or newer (install from [rustup.rs](https://rustup.rs)).
- `zsh` or `bash` on your `PATH`. aishe delegates command execution to one of
  these, preferring zsh.
- A network-reachable LLM endpoint and an API key, set in an environment
  variable. See [Providers](providers.md).
- Platforms: macOS (arm64 and x86_64) and Linux (x86_64 and arm64). Windows is
  not supported.

## Quick install (Linux and macOS)

The fastest path is the install script. It detects your OS and CPU, downloads the
right prebuilt binary from the latest release (the fully-static musl build on
Linux, so there are no glibc requirements), verifies its checksum, and installs
it:

```sh
curl -fsSL https://raw.githubusercontent.com/billiondollarsolo/aishe/main/install.sh | sh
```

The script also ensures **zsh** is installed (best effort, via your system
package manager), because aishe is most robust when it can drive your real zsh in
a PTY; without zsh it falls back to a built-in editor that is more sensitive to
terminal quirks. Opt out of the zsh step with `AISHE_SKIP_ZSH=1`.

It installs to `/usr/local/bin` when writable, otherwise `~/.local/bin`. Override
with environment variables:

```sh
AISHE_VERSION=v0.1.4 AISHE_BIN_DIR="$HOME/.local/bin" \
  sh -c "$(curl -fsSL https://raw.githubusercontent.com/billiondollarsolo/aishe/main/install.sh)"
```

## Linux packages (.deb / .rpm)

Each tagged release attaches Debian and RPM packages for `amd64` and `arm64`.
They install the binary to `/usr/bin/aishe` plus shell completions (bash, zsh,
fish) and the `aishe(1)` man page into the standard system locations.

Debian / Ubuntu:

```sh
arch=amd64   # or arm64
curl -fsSL -O "https://github.com/billiondollarsolo/aishe/releases/latest/download/aishe_0.1.4_${arch}.deb"
sudo apt install "./aishe_0.1.4_${arch}.deb"
```

Fedora / RHEL / openSUSE:

```sh
arch=x86_64  # or aarch64
curl -fsSL -O "https://github.com/billiondollarsolo/aishe/releases/latest/download/aishe-0.1.4.${arch}.rpm"
sudo dnf install "./aishe-0.1.4.${arch}.rpm"
```

(Substitute the release version for `0.1.4`.)

## Prebuilt binary (tarball)

Each tagged release attaches per-platform tarballs (and `.sha256` checksums):
`aishe-<target>.tar.gz` for these targets:

| Platform        | Target                          |
| --------------- | ------------------------------- |
| Linux x86_64    | `x86_64-unknown-linux-gnu`      |
| Linux x86_64    | `x86_64-unknown-linux-musl` (static) |
| Linux arm64     | `aarch64-unknown-linux-gnu`     |
| Linux arm64     | `aarch64-unknown-linux-musl` (static) |
| macOS arm64     | `aarch64-apple-darwin`          |
| macOS x86_64    | `x86_64-apple-darwin`           |

The `-musl` builds are fully static and have no glibc version requirement, which
makes them the most portable choice on Linux (and what the install script uses).

With [cargo-binstall](https://github.com/cargo-bins/cargo-binstall) (no Rust build
needed, just the installer):

```sh
cargo binstall aishe
```

Or download and install the tarball for your platform by hand:

```sh
target=x86_64-unknown-linux-musl   # see the table above
curl -fsSL -O "https://github.com/billiondollarsolo/aishe/releases/latest/download/aishe-$target.tar.gz"
tar -xzf "aishe-$target.tar.gz"
sudo install -m 0755 aishe /usr/local/bin/aishe
```

A [Homebrew formula](../packaging/aishe.rb) is provided (point a tap at it, or
`brew install --formula ./packaging/aishe.rb` once the release shas are filled
in). It also installs shell completions.

## Shell completions

aishe can print a completion script for itself:

```sh
aishe completions zsh  > ~/.zfunc/_aishe          # zsh (ensure ~/.zfunc is in $fpath)
aishe completions bash > /etc/bash_completion.d/aishe
aishe completions fish > ~/.config/fish/completions/aishe.fish
```

## Build and install with Cargo

From a checkout of the repository:

```sh
git clone https://github.com/billiondollarsolo/aishe
cd aishe
cargo install --path .
```

`cargo install` places the `aishe` binary in `~/.cargo/bin`, which is usually
already on your `PATH`. Confirm it works:

```sh
aishe --version
aishe doctor
```

## Build options

Syntax highlighting for code blocks in model answers is on by default (it bundles
a set of syntaxes and themes, which adds a few MB to the binary). For a smaller
binary without it, build with default features off:

```sh
cargo build --release --no-default-features
```

Code blocks then render as plain styled blocks instead of being color-tokenized.

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

Next: [Getting started](getting-started.md).
