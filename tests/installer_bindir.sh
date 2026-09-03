#!/bin/sh
# choose_bindir picks a PATH-visible writable directory, in a defined order.
set -eu
cd "$(dirname "$0")/.."
# shellcheck source=/dev/null
AISHE_INSTALL_LIB_ONLY=1 . ./install.sh

got="$(AISHE_BIN_DIR=/tmp/explicit choose_bindir)"
[ "$got" = /tmp/explicit ] || { echo "AISHE_BIN_DIR ignored: $got"; exit 1; }

if [ ! -w /usr/local/bin ] && [ "$(id -u)" != 0 ]; then
  got="$(PATH=/usr/bin:/bin HOME=/tmp/aishe-home choose_bindir)"
  [ "$got" = /tmp/aishe-home/.local/bin ] || { echo "fallback was $got"; exit 1; }
fi

brew="$(mktemp -d)"
got="$(PATH="$brew:/usr/bin" AISHE_HOMEBREW_BIN="$brew" choose_bindir)"
[ "$got" = "$brew" ] || { echo "writable Homebrew bin on PATH not preferred: $got"; exit 1; }

# Present but not on PATH: not a candidate.
got="$(PATH=/usr/bin:/bin HOME=/tmp/aishe-home AISHE_HOMEBREW_BIN="$brew" choose_bindir)"
[ "$got" != "$brew" ] || { echo "off-PATH Homebrew bin was chosen"; exit 1; }
rmdir "$brew"

echo "installer bindir: ok"
