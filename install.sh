#!/bin/sh
# aishe installer: download the right prebuilt binary for this machine from the
# latest GitHub release, verify its checksum, and install it.
#
#   curl -fsSL https://raw.githubusercontent.com/billiondollarsolo/aishe/main/install.sh | sh
#
# Environment overrides:
#   AISHE_VERSION   release tag to install (default: latest), e.g. v0.1.5
#   AISHE_BIN_DIR   install directory (default: /usr/local/bin, or ~/.local/bin
#                   if that is not writable)
#   AISHE_SKIP_ZSH  set to 1 to skip ensuring zsh is installed
#
# aishe's interactive shell drives your real zsh in a PTY (your plugins, history,
# job control, completions), so it needs zsh. This installer ensures zsh is
# installed (best effort; set AISHE_SKIP_ZSH=1 to opt out). Without zsh you can
# still use `aishe -c …`, piped input, and the bash hook (`aishe init bash`).
#
# On Linux this prefers the fully-static musl build, so there are no glibc
# version requirements. For apt/dnf-managed installs, grab the .deb/.rpm from the
# release page instead.
set -eu

REPO="billiondollarsolo/aishe"
VERSION="${AISHE_VERSION:-latest}"

err() { printf 'aishe-install: %s\n' "$1" >&2; exit 1; }
note() { printf 'aishe-install: %s\n' "$1" >&2; }

# Best-effort: make sure zsh is installed (aishe's interactive shell drives it).
# Never fatal -- the binary is already installed by the time this runs, and the
# non-interactive paths (`aishe -c …`, piped input) and the bash hook work
# without zsh.
ensure_zsh() {
  if command -v zsh >/dev/null 2>&1; then
    note "zsh present ($(command -v zsh)); aishe will use the zsh front-end"
    return 0
  fi
  if [ "${AISHE_SKIP_ZSH:-0}" = 1 ]; then
    note "zsh not found; skipping (AISHE_SKIP_ZSH=1). The interactive shell needs zsh; -c and the bash hook still work."
    return 0
  fi
  note "zsh not found; aishe is most robust with it. Attempting to install zsh..."

  sudo_cmd=""
  if [ "$(id -u)" != 0 ]; then
    if command -v sudo >/dev/null 2>&1; then
      sudo_cmd="sudo"
    else
      note "need root or sudo to install zsh; install it manually (e.g. 'apt install zsh'). The interactive shell needs zsh until then."
      return 0
    fi
  fi

  if command -v apt-get >/dev/null 2>&1; then
    $sudo_cmd apt-get update >/dev/null 2>&1 || true
    $sudo_cmd apt-get install -y zsh || true
  elif command -v dnf >/dev/null 2>&1; then
    $sudo_cmd dnf install -y zsh || true
  elif command -v yum >/dev/null 2>&1; then
    $sudo_cmd yum install -y zsh || true
  elif command -v zypper >/dev/null 2>&1; then
    $sudo_cmd zypper --non-interactive install zsh || true
  elif command -v pacman >/dev/null 2>&1; then
    $sudo_cmd pacman -Sy --noconfirm zsh || true
  elif command -v apk >/dev/null 2>&1; then
    $sudo_cmd apk add zsh || true
  elif command -v brew >/dev/null 2>&1; then
    brew install zsh || true
  else
    note "no known package manager; please install zsh manually for the best experience."
    return 0
  fi

  if command -v zsh >/dev/null 2>&1; then
    note "zsh installed ($(command -v zsh))"
  else
    note "could not install zsh automatically; install it manually for the interactive shell (e.g. 'apt install zsh')."
  fi
}

# --- detect platform --------------------------------------------------------
command -v uname >/dev/null 2>&1 || err "'uname' not found; cannot detect platform"
os="$(uname -s)"
arch="$(uname -m)"

case "$os" in
  Linux)  os_part="unknown-linux-musl"; want_fmt="ELF" ;;
  Darwin) os_part="apple-darwin";       want_fmt="Mach-O" ;;
  *) err "unsupported OS '$os' (aishe supports Linux and macOS)" ;;
esac

case "$arch" in
  x86_64 | amd64)        arch_part="x86_64" ;;
  aarch64 | arm64)       arch_part="aarch64" ;;
  *) err "unsupported architecture '$arch'" ;;
esac

target="${arch_part}-${os_part}"
tarball="aishe-${target}.tar.gz"
printf 'aishe-install: detected %s/%s -> target %s\n' "$os" "$arch" "$target" >&2

# --- resolve download URL ---------------------------------------------------
base="https://github.com/${REPO}/releases"
if [ "$VERSION" = "latest" ]; then
  url="${base}/latest/download/${tarball}"
else
  url="${base}/download/${VERSION}/${tarball}"
fi

# --- pick a downloader ------------------------------------------------------
if command -v curl >/dev/null 2>&1; then
  dl() { curl -fsSL "$1" -o "$2"; }
elif command -v wget >/dev/null 2>&1; then
  dl() { wget -qO "$2" "$1"; }
else
  err "need curl or wget to download"
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

printf 'aishe-install: downloading %s\n' "$target" >&2
dl "$url" "$tmp/$tarball" || err "download failed: $url"

# --- verify checksum (best effort: skip if no sha256 tool) ------------------
if dl "${url}.sha256" "$tmp/$tarball.sha256" 2>/dev/null; then
  expected="$(awk '{print $1}' "$tmp/$tarball.sha256")"
  if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "$tmp/$tarball" | awk '{print $1}')"
  elif command -v shasum >/dev/null 2>&1; then
    actual="$(shasum -a 256 "$tmp/$tarball" | awk '{print $1}')"
  else
    actual=""
  fi
  if [ -n "$actual" ] && [ "$actual" != "$expected" ]; then
    err "checksum mismatch (expected $expected, got $actual)"
  fi
fi

tar -xzf "$tmp/$tarball" -C "$tmp"
[ -f "$tmp/aishe" ] || err "archive did not contain an 'aishe' binary"
chmod +x "$tmp/aishe"

# Sanity-check that the binary matches this OS, so a wrong download can never be
# silently installed. ELF = Linux, Mach-O = macOS (Mach-O magic has no "ELF").
got_fmt="unknown"
if head -c 4 "$tmp/aishe" | grep -q "ELF"; then
  got_fmt="ELF"
elif command -v file >/dev/null 2>&1 && file "$tmp/aishe" | grep -q "Mach-O"; then
  got_fmt="Mach-O"
fi
if [ "$want_fmt" = "ELF" ] && [ "$got_fmt" != "ELF" ]; then
  err "downloaded a non-Linux binary ($got_fmt) for target $target; aborting. Please report this with the lines above."
fi
if [ "$want_fmt" = "Mach-O" ] && [ "$got_fmt" = "ELF" ]; then
  err "downloaded a Linux binary for a macOS target ($target); aborting. Please report this with the lines above."
fi

# --- choose an install dir --------------------------------------------------
if [ -n "${AISHE_BIN_DIR:-}" ]; then
  bindir="$AISHE_BIN_DIR"
elif [ -w /usr/local/bin ] 2>/dev/null; then
  bindir="/usr/local/bin"
elif [ "$(id -u)" = "0" ]; then
  bindir="/usr/local/bin"
else
  bindir="$HOME/.local/bin"
fi
mkdir -p "$bindir"

if [ -w "$bindir" ]; then
  install -m 0755 "$tmp/aishe" "$bindir/aishe"
elif command -v sudo >/dev/null 2>&1; then
  sudo install -m 0755 "$tmp/aishe" "$bindir/aishe"
else
  err "cannot write to $bindir and sudo is unavailable; set AISHE_BIN_DIR"
fi

printf 'aishe-install: installed %s to %s/aishe\n' "$target" "$bindir" >&2
case ":$PATH:" in
  *":$bindir:"*) : ;;
  *) printf 'aishe-install: note: %s is not on your PATH\n' "$bindir" >&2 ;;
esac

# Best-effort man page: `aishe man` emits a roff page; install it if a standard
# man dir is writable (system, then the per-user fallback). Never fatal.
for mandir in /usr/local/share/man/man1 /usr/share/man/man1 "$HOME/.local/share/man/man1"; do
  if mkdir -p "$mandir" 2>/dev/null && [ -w "$mandir" ]; then
    if "$bindir/aishe" man > "$mandir/aishe.1" 2>/dev/null; then
      printf 'aishe-install: installed man page to %s/aishe.1\n' "$mandir" >&2
    fi
    break
  fi
done

# Ensure zsh for the robust front-end (best effort; opt out with AISHE_SKIP_ZSH=1).
ensure_zsh

# Bubblewrap is Linux-only and optional: the core shell works without it, while
# `aishe dry-run` and the bwrap yolo sandbox need it for OS isolation. Keep the
# tarball installer non-invasive and explain the missing capability.
if [ "$os" = "Linux" ] && ! command -v bwrap >/dev/null 2>&1; then
  note "optional bubblewrap not found; core aishe works, but install bubblewrap to enable 'aishe dry-run' and the bwrap sandbox."
fi

"$bindir/aishe" --version || true
