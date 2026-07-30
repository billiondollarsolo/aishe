# Installation

aishe ships as a native Rust binary plus a private, compatibility-pinned
OpenCode agent runtime. The runtime is lazy: ordinary zsh commands never start
it, and its per-user supervisor exits when idle. The install script or first
setup downloads the exact version supported by the Aishe build, verifies its
size, SHA-256, executable version, license, and trusted integration, and keeps it
inside Aishe's data directory. You never install or configure OpenCode
separately.

## Requirements

- Rust 1.88 or newer (only to build from source; the prebuilt binaries need no
  toolchain). Install from [rustup.rs](https://rustup.rs).
- **`zsh`** for the interactive shell: aishe drives your real zsh in a PTY. The
  installer ensures it; on a manual install add it with your package manager
  (`apt install zsh`, etc.). `bash` is enough for the non-interactive paths
  (`aishe -c …` and piped input).
- **`bubblewrap`** is the supported Linux OS-isolation boundary for
  workspace-scoped agent actions and command previews. The core shell and
  suggest/chat paths work without it. Setup detects both presence and actual
  namespace functionality, explains the exact package-manager command, and
  offers to install it only after explicit consent. `.deb` and `.rpm` packages
  declare it as a recommended/weak dependency rather than a hard dependency
  because some containers cannot use its namespaces.
- A network-reachable LLM endpoint and either an API key (`aishe auth set` or an
  environment override) or a supported OpenAI/xAI subscription OAuth login
  (`aishe auth login`). See [Providers](providers.md).
- Platforms: macOS (arm64 and x86_64) and Linux (x86_64 and arm64). Windows is
  not supported.

## Quick install (Linux and macOS)

The fastest path is the install script. It detects your OS and CPU, downloads
the right Aishe binary and exact managed runtime, verifies both, performs a live
authenticated backend health check, and only then atomically activates the
binary. Linux uses the fully-static musl build, so there are no glibc
requirements:

```sh
curl -fsSL https://raw.githubusercontent.com/billiondollarsolo/aishe/main/install.sh | sh
```

Checksum verification is mandatory for both artifacts. If the runtime cannot be
downloaded, extracted, version-checked, or live-verified, an existing Aishe
binary is not replaced. Runtime versions live side by side, so a failed new
binary activation does not invalidate the runtime expected by the old binary.

The script also ensures **zsh** is installed (best effort, via your system
package manager), because aishe's interactive shell drives your real zsh in a
PTY. Without zsh you can still use `aishe -c …`, piped input, and the bash hook
(`aishe init bash`). Opt out of the zsh step with `AISHE_SKIP_ZSH=1`.
On Linux the non-interactive installer reports when **bubblewrap** is absent but
does not run a package manager without authorization. Guided `aishe setup`
offers a consent-gated install and functional self-test. For scripted
provisioning, `AISHE_INSTALL_SYSTEM_DEPS=1` explicitly authorizes supported
system dependency installation.

Pass `--setup` to start guided setup after installation:

```sh
curl -fsSL https://raw.githubusercontent.com/billiondollarsolo/aishe/main/install.sh | sh -s -- --setup
```

An update replaces the binary and, when required, adds a new verified runtime
version. It inventories the existing config and data locations before and after
activation. Configuration, credentials, history, durable sessions/tasks, tool
journals, audit logs, undo journals, and trust data are never used as installer
scratch and remain untouched.

It installs to `/usr/local/bin` when writable, otherwise `~/.local/bin`. Override
with environment variables:

```sh
AISHE_VERSION=vX.Y.Z AISHE_BIN_DIR="$HOME/.local/bin" \
  sh -c "$(curl -fsSL https://raw.githubusercontent.com/billiondollarsolo/aishe/main/install.sh)"
```

Installer/runtime controls for mirrors, offline systems, and managed images:

```sh
AISHE_RUNTIME_BASE_URL=https://mirror.example/aishe/runtime ./install.sh
AISHE_RUNTIME_FILE=/media/opencode-1.18.9.tar.gz ./install.sh
AISHE_SKIP_BACKEND=1 ./install.sh       # binary-only recovery/development
AISHE_SKIP_ZSH=1 ./install.sh
```

The embedded compatibility checksum is still enforced for a mirror or local
archive. `AISHE_SKIP_BACKEND=1` is a recovery/development override; normal AI
turns require the managed runtime or an allowed pre-admission native fallback.

## Linux packages (.deb / .rpm)

Each tagged release attaches Debian and RPM packages for `amd64` and `arm64`.
They install the binary to `/usr/bin/aishe` plus shell completions (bash, zsh,
fish) and the `aishe(1)` man page into the standard system locations. They
declare zsh and bubblewrap as recommended dependencies. Package scripts do not
download user-specific runtime content; the invoking user's first setup installs
the pinned runtime in that user's data directory.

Debian / Ubuntu:

```sh
version=X.Y.Z
arch=amd64   # or arm64
curl -fsSL -O "https://github.com/billiondollarsolo/aishe/releases/latest/download/aishe_${version}_${arch}.deb"
sudo apt install "./aishe_${version}_${arch}.deb"
```

Fedora / RHEL / openSUSE:

```sh
version=X.Y.Z
arch=x86_64  # or aarch64
curl -fsSL -O "https://github.com/billiondollarsolo/aishe/releases/latest/download/aishe-${version}.${arch}.rpm"
sudo dnf install "./aishe-${version}.${arch}.rpm"
```

(Substitute the release version for `X.Y.Z`.)

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

When installed with `cargo install --path .`, pull the latest source, reinstall,
then let the new binary verify/install its compatible runtime:

```sh
git pull
cargo install --path . --force
aishe backend install
aishe backend verify --live
```

Re-running the install script is also an in-place binary/runtime update. It does
not rerun setup unless you pass `--setup`, and never removes user state.

## Uninstall

Use the built-in category-based workflow:

```sh
aishe uninstall --dry-run       # exact paths; changes nothing
aishe uninstall                 # binary/completions/man + managed runtime only
```

The default preserves config, credentials, shell history, AI sessions/tool
journals, audit, and undo data. User-state categories are separate and never
implied:

```sh
aishe uninstall --sessions --dry-run
aishe uninstall --config --history --audit-undo
aishe uninstall --all --dry-run
```

State removal requires explicit targeted confirmation (`--yes` for
non-interactive automation) and is reported as permanently unrecoverable by
Aishe. Package-manager ownership still applies: if a `.deb`, `.rpm`, Homebrew,
or Cargo installed the binary, remove that package through the same manager
after using `aishe uninstall --runtime --yes` as appropriate.

## What setup and use create

aishe uses each platform's own directories — `~/.config/aishe` and
`~/.local/share/aishe` on Linux, `~/Library/Application Support/aishe` for both
on macOS. See [File locations](configuration.md#file-locations) for the full
table and the `AISHE_CONFIG_DIR` / `AISHE_DATA_DIR` overrides.

- `config.toml` in the config directory is written only after you apply
  `aishe setup` or save a setting.
- `credentials.toml` in the config directory is a separate mode-`0600` shared
  credential store written only by setup Apply or `aishe auth`.
- `history.ext` in the data directory is the timestamped shared shell history.
- `runtime/opencode/<version>/` contains the exact verified OpenCode executable,
  install metadata, license, and third-party notices.
- `backend/opencode/` contains the private isolated HOME/XDG/plugin/server state;
  `backend/sessions/` and `backend/journal/` contain session mappings and
  idempotency/usage records.
- `tasks/` contains private, redacted durable agentic-task checkpoints. A
  stateless reasoning checkpoint can also contain opaque encrypted provider
  continuation data; support bundles never include task contents.
- `capabilities/` caches endpoint/model feature checks.
- `setup-draft.json` exists only while a resumable setup is in progress.

Next: [Getting started](getting-started.md).
For runtime lifecycle and security details, see
[Managed agent backend](managed-agent-backend.md).
