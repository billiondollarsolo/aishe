#!/bin/sh
# Hermetic transaction/fault contract for install.sh. A tiny native fake Aishe
# binary records backend argv and injects failures while satisfying the
# installer's platform-format and self-test checks.
set -eu
LC_ALL=C
LANG=C
export LC_ALL LANG

repo_root="$(CDPATH='' cd -- "$(dirname "$0")/.." && pwd)"
compiler=""
for candidate in cc clang gcc; do
  if command -v "$candidate" >/dev/null 2>&1; then
    compiler="$candidate"
    break
  fi
done
if [ -z "$compiler" ]; then
  printf 'SKIP: no C compiler for native installer transaction fixture\n'
  exit 0
fi

work="$(mktemp -d)"
cleanup() {
  find "$work" -depth -delete 2>/dev/null || true
}
trap cleanup EXIT HUP INT TERM
test_home="$work/home"
test_bin="$work/bin"
config_root="$work/config-root"
data_root="$work/data-root"
mirror="$work/releases/latest/download"
stage="$work/stage"
mkdir -p "$test_home" "$test_bin" "$config_root/aishe" \
  "$data_root/aishe/tasks" "$mirror" "$stage"

cat > "$work/fake-aishe.c" <<'EOF'
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static void record(int argc, char **argv) {
    const char *path = getenv("AISHE_TEST_BACKEND_LOG");
    if (!path) return;
    FILE *file = fopen(path, "a");
    if (!file) exit(90);
    for (int index = 1; index < argc; index++) {
        if (index > 1) fputc(' ', file);
        fputs(argv[index], file);
    }
    fputc('\n', file);
    fclose(file);
}

int main(int argc, char **argv) {
    record(argc, argv);
    if (argc == 2 && strcmp(argv[1], "--version") == 0) {
        puts("aishe 0.5.0 (installer-fixture)");
        return 0;
    }
    if (argc >= 3 && strcmp(argv[1], "backend") == 0) {
        const char *failure = getenv("AISHE_TEST_BACKEND_FAIL");
        if (failure && strcmp(argv[2], failure) == 0) return 71;
        return 0;
    }
    if (argc == 2 && strcmp(argv[1], "man") == 0) {
        puts(".TH aishe 1");
        return 0;
    }
    return 0;
}
EOF
"$compiler" -O2 -o "$stage/aishe" "$work/fake-aishe.c"

cat > "$test_bin/aishe" <<'EOF'
#!/bin/sh
if [ "${1:-}" = "--version" ]; then
  echo "aishe 0.4.1 (old-transaction-fixture)"
fi
EOF
chmod 0755 "$test_bin/aishe"

cat > "$config_root/aishe/config.toml" <<'EOF'
version = 3
[aishe]
provider = "openai"
EOF
cat > "$config_root/aishe/credentials.toml" <<'EOF'
version = 1
[profiles.openai]
api_key = "synthetic-transaction-secret"
EOF
chmod 0600 "$config_root/aishe/credentials.toml"
cat > "$data_root/aishe/history.ext" <<'EOF'
: 1:0;echo preserved
EOF
cat > "$data_root/aishe/tasks/resumable.json" <<'EOF'
{"schema_version":1,"id":"resumable","status":"interrupted"}
EOF
cat > "$data_root/aishe/session-map.json" <<'EOF'
{"schema_version":1,"sessions":{"shell":"managed"}}
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
tar -czf "$mirror/$asset" -C "$stage" aishe
shasum -a 256 "$mirror/$asset" > "$mirror/$asset.sha256"
runtime_file="$work/pinned-runtime.tar.gz"
printf 'approved-runtime-fixture\n' > "$runtime_file"
backend_log="$work/backend.log"

install_attempt() {
  output="$1"
  shift
  env \
    HOME="$test_home" \
    AISHE_BIN_DIR="$test_bin" \
    AISHE_CONFIG_DIR="$config_root" \
    AISHE_DATA_DIR="$data_root" \
    AISHE_RELEASE_BASE_URL="file://$work/releases" \
    AISHE_RUNTIME_FILE="$runtime_file" \
    AISHE_SKIP_ZSH=1 \
    AISHE_TEST_BACKEND_LOG="$backend_log" \
    "$@" \
    sh "$repo_root/install.sh" >"$output" 2>&1
}

assert_old_binary_and_state() {
  grep -q 'old-transaction-fixture' "$test_bin/aishe"
  [ "$config_before" = "$(hash_tree "$config_root")" ] || {
    printf 'FAIL: failed installer changed config/credential state\n' >&2
    exit 1
  }
  [ "$data_before" = "$(hash_tree "$data_root")" ] || {
    printf 'FAIL: failed installer changed history/task/session state\n' >&2
    exit 1
  }
}

# The staged runtime install must happen before verification, and either failure
# must leave the active binary and every user-state byte untouched.
: > "$backend_log"
if install_attempt "$work/install-failure.out" AISHE_TEST_BACKEND_FAIL=install; then
  printf 'FAIL: installer ignored managed runtime install failure\n' >&2
  exit 1
fi
grep -Fx "backend install --from $runtime_file" "$backend_log"
if grep -q '^backend verify ' "$backend_log"; then
  printf 'FAIL: live verification ran after failed runtime installation\n' >&2
  exit 1
fi
assert_old_binary_and_state

: > "$backend_log"
if install_attempt "$work/verify-failure.out" AISHE_TEST_BACKEND_FAIL=verify; then
  printf 'FAIL: installer ignored managed runtime live-verification failure\n' >&2
  exit 1
fi
grep -Fx "backend install --from $runtime_file" "$backend_log"
grep -Fx 'backend verify --live' "$backend_log"
assert_old_binary_and_state

# A matching checksum does not make a truncated archive acceptable.
cp "$mirror/$asset" "$work/valid-$asset"
printf 'truncated-gzip\n' > "$mirror/$asset"
shasum -a 256 "$mirror/$asset" > "$mirror/$asset.sha256"
if install_attempt "$work/extraction-failure.out"; then
  printf 'FAIL: installer accepted a truncated release archive\n' >&2
  exit 1
fi
assert_old_binary_and_state
cp "$work/valid-$asset" "$mirror/$asset"
shasum -a 256 "$mirror/$asset" > "$mirror/$asset.sha256"

# Failure while activating the staged Aishe executable also preserves the old
# binary. The managed runtime has already passed both checks at this point.
fake_tools="$work/fake-tools"
mkdir -p "$fake_tools"
cat > "$fake_tools/install" <<'EOF'
#!/bin/sh
exit 79
EOF
chmod 0755 "$fake_tools/install"
: > "$backend_log"
if (
  PATH="$fake_tools:$PATH"
  export PATH
  install_attempt "$work/activation-failure.out"
); then
  printf 'FAIL: installer ignored binary activation failure\n' >&2
  exit 1
fi
grep -Fx 'backend verify --live' "$backend_log"
assert_old_binary_and_state

# The success path preserves state, executes exact backend argv, and atomically
# replaces the old executable only after both managed-runtime checks.
: > "$backend_log"
install_attempt "$work/success.out"
grep -Fx "backend install --from $runtime_file" "$backend_log"
grep -Fx 'backend verify --live' "$backend_log"
grep -q 'installer-fixture' "$test_bin/aishe"
[ "$config_before" = "$(hash_tree "$config_root")" ] || {
  printf 'FAIL: successful installer changed config/credential state\n' >&2
  exit 1
}
[ "$data_before" = "$(hash_tree "$data_root")" ] || {
  printf 'FAIL: successful installer changed history/task/session state\n' >&2
  exit 1
}

# The documented curl-pipe form still has a controlling terminal even though
# the installer's stdin is the script pipe. Prove --setup reaches the binary.
if command -v python3 >/dev/null 2>&1; then
  : > "$backend_log"
  HOME="$test_home" AISHE_BIN_DIR="$test_bin" \
  AISHE_CONFIG_DIR="$config_root" AISHE_DATA_DIR="$data_root" \
  AISHE_RELEASE_BASE_URL="file://$work/releases" \
  AISHE_RUNTIME_FILE="$runtime_file" AISHE_SKIP_ZSH=1 \
  AISHE_TEST_BACKEND_LOG="$backend_log" \
  AISHE_TEST_INSTALLER="$repo_root/install.sh" AISHE_TEST_OUTPUT="$work/setup-pipe.out" \
  python3 - <<'PY'
import os
import pty

pid, fd = pty.fork()
if pid == 0:
    os.execl("/bin/sh", "sh", "-c", 'cat "$AISHE_TEST_INSTALLER" | sh -s -- --setup')
with open(os.environ["AISHE_TEST_OUTPUT"], "wb") as output:
    while True:
        try:
            chunk = os.read(fd, 8192)
        except OSError:
            break
        if not chunk:
            break
        output.write(chunk)
_, status = os.waitpid(pid, 0)
raise SystemExit(os.waitstatus_to_exitcode(status))
PY
  grep -Fx 'setup' "$backend_log"
fi

printf 'PASS: runtime staging, live verification, extraction, activation, exact argv, and state preservation\n'
