#!/bin/sh
# Configure only this disposable GitHub runner for unprivileged namespaces.
# Ubuntu24.04's AppArmor restriction permits an unconfined clone while denying
# UID mapping/network capabilities. Product user/network isolation stays strict.
set -eu

policy=/proc/sys/kernel/apparmor_restrict_unprivileged_userns
if [ -r "$policy" ]; then
    current=$(cat "$policy")
    case "$current" in
        1) sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0 ;;
        0) printf '%s\n' 'Runner already supports unprivileged user namespaces.' ;;
        *) printf 'Unexpected runner AppArmor namespace policy: %s\n' "$current" >&2; exit 1 ;;
    esac
    test "$(cat "$policy")" = 0
fi

# This must succeed with both namespaces; never fall back to host execution.
bwrap --unshare-user --unshare-net --ro-bind / / --die-with-parent -- /bin/true
