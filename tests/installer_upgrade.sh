#!/bin/sh
# Hermetic installer contract: replacing the binary must not alter config,
# history, task records, or any other state file. Uses a local file:// release
# mirror so it is deterministic and makes no network request.
set -eu

binary="${1:-target/release/aishe}"
repo_root="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
case "$binary" in
  /*) source_binary="$binary" ;;
  *) source_binary="$repo_root/$binary" ;;
esac
[ -x "$source_binary" ] || {
  printf 'FAIL: binary not found: %s\n' "$source_binary" >&2
  exit 1
}

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT HUP INT TERM
test_home="$work/home"
test_bin="$work/bin"
config_root="$work/config-root"
data_root="$work/data-root"
mkdir -p "$test_home" "$test_bin" "$config_root/aishe" \
  "$data_root/aishe/tasks"

cat > "$test_bin/aishe" <<'EOF'
#!/bin/sh
if [ "${1:-}" = "--version" ]; then
  echo "aishe 0.0.0 (old-test-build)"
elif [ "${1:-}" = "man" ]; then
  echo ".TH aishe 1"
fi
EOF
chmod 0755 "$test_bin/aishe"

cat > "$config_root/aishe/config.toml" <<'EOF'
version = 1
[aishe]
provider = "openai"
[providers.openai]
model = "keep-this-model"
EOF
cat > "$config_root/aishe/credentials.toml" <<'EOF'
version = 1
[profiles.openai]
api_key = "synthetic-installer-preservation-key"
EOF
chmod 0600 "$config_root/aishe/credentials.toml"
cat > "$data_root/aishe/history.ext" <<'EOF'
: 1:0;echo keep-history
EOF
cat > "$data_root/aishe/tasks/keep.json" <<'EOF'
{"schema_version":1,"id":"keep","status":"interrupted"}
EOF
cat > "$data_root/aishe/other-state.bin" <<'EOF'
keep-other-state
EOF

hash_tree() {
  root="$1"
  find "$root" -type f -exec shasum -a 256 {} \; |
    sed "s|$root/||" |
    LC_ALL=C sort
}
config_before="$(hash_tree "$config_root")"
data_before="$(hash_tree "$data_root")"

case "$(uname -s)" in
  Linux) os_part="unknown-linux-musl" ;;
  Darwin) os_part="apple-darwin" ;;
  *) printf 'SKIP: unsupported installer-test OS\n'; exit 0 ;;
esac
case "$(uname -m)" in
  x86_64|amd64) arch_part="x86_64" ;;
  aarch64|arm64) arch_part="aarch64" ;;
  *) printf 'SKIP: unsupported installer-test architecture\n'; exit 0 ;;
esac
asset="aishe-${arch_part}-${os_part}.tar.gz"
mirror="$work/releases/latest/download"
stage="$work/stage"
mkdir -p "$mirror" "$stage"
cp "$source_binary" "$stage/aishe"
tar -czf "$mirror/$asset" -C "$stage" aishe

bad_output="$work/install-bad-checksum-output"
printf '%064d  %s\n' 0 "$asset" > "$mirror/$asset.sha256"
if HOME="$test_home" \
AISHE_BIN_DIR="$test_bin" \
AISHE_CONFIG_DIR="$config_root" \
AISHE_DATA_DIR="$data_root" \
AISHE_RELEASE_BASE_URL="file://$work/releases" \
AISHE_SKIP_ZSH=1 \
AISHE_SKIP_BACKEND=1 \
sh "$repo_root/install.sh" >"$bad_output" 2>&1; then
  printf 'FAIL: installer accepted a corrupt checksum\n' >&2
  exit 1
fi
grep -q 'checksum mismatch' "$bad_output"
grep -q 'old-test-build' "$test_bin/aishe"
[ "$config_before" = "$(hash_tree "$config_root")" ] || {
  printf 'FAIL: rejected installer changed config state\n' >&2
  exit 1
}
[ "$data_before" = "$(hash_tree "$data_root")" ] || {
  printf 'FAIL: rejected installer changed data state\n' >&2
  exit 1
}

shasum -a 256 "$mirror/$asset" > "$mirror/$asset.sha256"
output="$work/install-output"
HOME="$test_home" \
AISHE_BIN_DIR="$test_bin" \
AISHE_CONFIG_DIR="$config_root" \
AISHE_DATA_DIR="$data_root" \
AISHE_RELEASE_BASE_URL="file://$work/releases" \
AISHE_SKIP_ZSH=1 \
AISHE_SKIP_BACKEND=1 \
sh "$repo_root/install.sh" >"$output" 2>&1

cmp "$source_binary" "$test_bin/aishe"
[ "$config_before" = "$(hash_tree "$config_root")" ] || {
  printf 'FAIL: installer changed config state\n' >&2
  exit 1
}
[ "$data_before" = "$(hash_tree "$data_root")" ] || {
  printf 'FAIL: installer changed data/history/task state\n' >&2
  exit 1
}
grep -q 'upgrade:' "$output"
grep -q 'config after install (preserved)' "$output"
grep -q 'data after install (user state preserved' "$output"

printf 'PASS: installer rejected corruption and preserved config, credentials, history, tasks, and data\n'
