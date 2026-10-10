# Installation

AIShe ships as a native Rust binary. Ordinary shell commands work without an
account, provider connection, or OpenCode payload. API keys and local endpoints
use its included native agent. Subscription OAuth is an explicit legacy choice:
setup installs a private, compatibility-pinned OpenCode runtime only when you
choose that transport. Existing runtime versions are preserved during updates.

## Requirements

- Rust 1.88 or newer (only to build from source; the prebuilt binaries need no
  toolchain). Install from [rustup.rs](https://rustup.rs).
- **`zsh`** for the full native interactive experience. AIShe drives real zsh in a PTY. The
  installer detects it and changes system packages only with authorization;
  on a manual install add it with your package manager
  (`apt install zsh`, etc.). **`bash`** supports interactive Bash integration
  with its documented reduced tier, as well as non-interactive commands and
  shell programs. Fresh account-free launch selects Bash when zsh is absent.
- **`bubblewrap`** is the supported Linux OS-isolation boundary for
  workspace-scoped agent actions and command previews. The core shell and
  suggest/chat paths work without it. Setup detects both presence and actual
  namespace functionality, explains the exact package-manager command, and
  offers to install it only after explicit consent. `.deb` and `.rpm` packages
  declare it as a recommended/weak dependency rather than a hard dependency
  because some containers cannot use its namespaces.
- For AI requests, a network-reachable LLM endpoint and either an API key
  (`aishe auth set` or an environment override) or a supported OpenAI/xAI subscription OAuth login
  (`aishe auth login`). See [Providers](providers.md).
- Platforms: macOS (arm64 and x86_64) and Linux (x86_64 and arm64). WSL is
  currently an unqualified research target, not a supported platform tier;
  native Windows is unsupported. See the
  [WSL decision](design/WSL_COMPATIBILITY_DECISION.md).

## Quick install (Linux and macOS)

The installer detects your OS and CPU, downloads the latest **published** AIShe
binary, verifies its required SHA-256 checksum and version, then atomically
activates it. Linux uses the static musl build. Current source can be ahead of
the public release; inspect `aishe --version` when comparing features.

```sh
curl -fsSL https://raw.githubusercontent.com/billiondollarsolo/aishe/main/install.sh | sh -s -- --launch
```

`--launch` opens the resolved executable through your controlling terminal even
when its directory is outside `PATH`. The path and PATH guidance appear before
setup. If no terminal is attached, installation still succeeds and prints the
absolute launch command. Omit `--launch` for an unattended install or upgrade.
Use `--setup --launch` to run guided setup before opening the shell; setup also
offers **Connect later**.

The default native install downloads no optional runtime. To preinstall the
pinned legacy runtime, pass `--backend` or `AISHE_INSTALL_BACKEND=1`. That explicit
path verifies its archive, version and authenticated health before replacing a
working AIShe executable. An offline or failed runtime install leaves the old
binary intact. Runtime versions live side by side. `AISHE_SKIP_BACKEND=1` remains
a compatibility recovery override and wins over an opt-in.

The script offers **zsh** installation for the full native experience (best
effort, via your system package manager with authorization). Without zsh,
interactive Bash integration, `aishe -c …`, and piped shell programs remain
available. Bash is qualified as Tier B on Bash 5.x and reduced
Tier B- on Bash 3.2; see the [tested matrix](bash-compatibility.md). Opt out of
the zsh step with `AISHE_SKIP_ZSH=1`.
On Linux the non-interactive installer reports when **bubblewrap** is absent but
does not run a package manager without authorization. Guided `aishe setup`
offers a consent-gated install and functional self-test. For scripted
provisioning, `AISHE_INSTALL_SYSTEM_DEPS=1` explicitly authorizes supported
system dependency installation.

An update replaces the binary and, when required, adds a new verified runtime
version. It inventories the existing config and data locations before and after
activation. Configuration, credentials, history, durable sessions/tasks, tool
journals, audit logs, undo journals, and trust data are never used as installer
scratch and remain untouched.

An existing release can also update itself without piping a second installer:

```sh
aishe update check
aishe update apply       # shows version/path and asks before activation
aishe update rollback    # restores the one previous verified binary
```

The updater accepts only HTTPS release downloads (loopback HTTP exists for
tests), bounds both responses, verifies the release SHA-256, rejects archives
that contain anything except one `aishe` regular file, checks the platform
binary format, runs `aishe --version`, and activates with an atomic rename in
the installed binary's directory. Use `--yes` only after a noninteractive job
has independently reviewed the printed version and path. Profile portability is
separate: `aishe profile export PATH` excludes credentials, while
`aishe profile import PATH` preserves the private credential stores and backs
up the current config.

It installs to `/usr/local/bin` when writable, otherwise `~/.local/bin`. Override
with environment variables:

```sh
AISHE_VERSION=vX.Y.Z AISHE_BIN_DIR="$HOME/.local/bin" \
  sh -c "$(curl -fsSL https://raw.githubusercontent.com/billiondollarsolo/aishe/main/install.sh)"
```

Installer/runtime controls for mirrors, offline systems, and managed images:

```sh
AISHE_RUNTIME_BASE_URL=https://mirror.example/aishe/runtime ./install.sh --backend
AISHE_RUNTIME_FILE=/media/opencode-1.18.27.tar.gz ./install.sh --backend
AISHE_SKIP_BACKEND=1 ./install.sh       # compatible binary-only recovery
AISHE_SKIP_ZSH=1 ./install.sh
```

The embedded compatibility checksum is still enforced for an explicitly
selected runtime mirror or local archive. Native AI turns require no managed
runtime. The optional man page is written to `~/.local/share/man/man1`; use
`AISHE_MAN_DIR` to choose its destination or `AISHE_SKIP_MAN=1` to omit it.

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
curl -fsSL -O "https://github.com/billiondollarsolo/aishe/releases/latest/download/aishe-${version}-1.${arch}.rpm"
sudo dnf install "./aishe-${version}-1.${arch}.rpm"
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

The default crates.io `cargo binstall aishe` path is not currently published.
Use the installer or packages above, or build from this checkout. The package's
binstall metadata is prepared for a future registry publication.

For a manual tarball install, verify its checksum before extraction:

```sh
target=x86_64-unknown-linux-musl
asset="aishe-$target.tar.gz"
base=https://github.com/billiondollarsolo/aishe/releases/latest/download
curl -fsSL -O "$base/$asset"
curl -fsSL -O "$base/$asset.sha256"
shasum -a 256 -c "$asset.sha256"
tar -xzf "$asset"
sudo install -m 0755 aishe /usr/local/bin/aishe
```

The repository contains a [Homebrew formula template](../packaging/aishe.rb) for
maintainers. It is not a supported install path until published in a tap with
release checksums.

## Reversible terminal activation

Open AIShe once before enabling it in every terminal. These commands show the
proposed startup file and guarded native launch block before writing:

```sh
aishe activate zsh              # preview ~/.zshrc
# Or: aishe activate bash       # preview ~/.bashrc
aishe activate zsh --apply      # private backup, then apply the reviewed block
aishe activate zsh --remove     # remove only the marked block
```

`--rcfile PATH` selects a custom startup file. AIShe preserves symlinks: inspect
their destination and pass the real file with `--rcfile`. `--json` reports the
same plan for automation. The guarded launch prevents nested activation when
AIShe loads your personal startup file. Removing it preserves surrounding user
edits. Activation changes no `chsh` setting or `/etc/shells` entry. The activation
block replaces the startup shell, so exiting AIShe normally closes that terminal
session. Launching `aishe` manually returns to the shell that launched it.

## Shell completions

aishe can print a completion script for itself:

```sh
aishe completions zsh  > ~/.zfunc/_aishe          # zsh (ensure ~/.zfunc is in $fpath)
aishe completions bash > /etc/bash_completion.d/aishe
aishe completions fish > ~/.config/fish/completions/aishe.fish
```

The Fish file completes AIShe's CLI. It is not an interactive Fish hook and
does not make `aishe init fish` supported. See the
[Fish integration decision](design/FISH_INTEGRATION_DECISION.md).

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
and reinstall. Native use needs no runtime update:

```sh
git pull
cargo install --path . --force --locked
aishe doctor
```

For managed subscription OAuth, explicitly run `aishe backend install` and
`aishe backend verify --live` to install and verify the new compatibility pin.

Re-running the install script is also an in-place binary update. It does
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

`--sessions` preserves API credentials and managed OpenCode OAuth login state;
those belong exclusively to `--config`. See [Data retention and deletion](data-retention.md)
for the complete state inventory, rotation bounds, exports, and exact category
semantics.

State removal requires explicit targeted confirmation (`--yes` for
non-interactive automation) and is reported as permanently unrecoverable by
AIShe. Package-manager ownership still applies: if a `.deb`, `.rpm`, Homebrew,
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
- `backend/opencode/profiles/` contains private connection- and OAuth-profile-
  isolated HOME/XDG/plugin/server state; `backend/instances/` contains only
  private safe-hash-keyed supervisor state;
  `backend/sessions/` and `backend/journal/` contain session mappings and
  idempotency/usage records.
- `tasks/` contains private, redacted durable agentic-task checkpoints. A
  stateless reasoning checkpoint can also contain opaque encrypted provider
  continuation data; support bundles never include task contents.
- `capabilities/` caches endpoint/model feature checks.
- `setup-draft.json` exists only while a resumable setup is in progress.
- `background-tasks/`, `repo-index/`, `failures/`, and `updates/` contain the
  bounded daily-driver task, retrieval, recovery, and rollback stores described
  in [Daily-driver agent workflows](daily-driver.md).

Next: [Getting started](getting-started.md).
For runtime lifecycle and security details, see
[Managed agent backend](managed-agent-backend.md).
