#!/bin/sh
# aishe installer: download the right prebuilt binary for this machine from the
# latest GitHub release, verify its checksum, and install it.
#
#   curl -fsSL https://raw.githubusercontent.com/billiondollarsolo/aishe/main/install.sh | sh
#
# Environment overrides:
#   AISHE_VERSION   release tag to install (default: latest), e.g. v0.1.1
#   AISHE_BIN_DIR   install directory (default: /usr/local/bin, or ~/.local/bin
#                   if that is not writable)
#
# On Linux this prefers the fully-static musl build, so there are no glibc
# version requirements. For apt/dnf-managed installs, grab the .deb/.rpm from the
# release page instead.
set -eu

REPO="billiondollarsolo/aishe"
VERSION="${AISHE_VERSION:-latest}"

err() { printf 'aishe-install: %s\n' "$1" >&2; exit 1; }

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
"$bindir/aishe" --version || true
