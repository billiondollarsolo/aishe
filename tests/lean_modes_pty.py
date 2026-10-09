#!/usr/bin/env python3
"""Real lean mode/grant handoffs, cancellation, policy, and scope regressions."""
from pathlib import Path
import shlex
import shutil
import sys

from pty_helper import Pty, environment


def fixture(label, mode="ask", scope="workspace", host_allowed=True):
    home, env = environment(
        "lean-mode-" + label,
        mode=mode,
        extra={
            "AISHE_LEAN": "1",
            "AISHE_LEGACY_OPENCODE": "0",
            "AISHE_FAKE_LLM": "mode regression answer",
        },
    )
    config = Path(home) / ".config" / "aishe" / "config.toml"
    config.write_text(config.read_text() + f'\ndefault_scope = "{scope}"\n'
                      + f'\n[sandbox]\nallow_host_yolo = {str(host_allowed).lower()}\n')
    # Fail the functional isolation probe deterministically, even on CI hosts
    # that support namespaces. No agent action may fall through to the host.
    bwrap = Path(home) / "bin" / "bwrap"
    bwrap.write_text("#!/bin/sh\nexit 1\n")
    bwrap.chmod(0o755)
    return str(Path(home).resolve()), env


def send(shell, keys, seconds=0.25):
    start = len(shell.plain())
    shell.send(keys)
    shell.drain(seconds)
    return shell.plain()[start:]


def wait_new(shell, text, start, timeout=5):
    import time
    end = time.monotonic() + timeout
    while text not in shell.plain()[start:] and time.monotonic() < end:
        shell.drain(0.1)
    assert text in shell.plain()[start:], shell.plain()[-2500:]


def mode(shell, expected):
    start = len(shell.plain())
    shell.send("print -r -- EFFECTIVE_MODE=$AISHE_MODE\r")
    wait_new(shell, "EFFECTIVE_MODE=" + expected, start)


def main():
    home, env = fixture("handoff")
    # A visible consent cue must mean terminal input is ready. Make the
    # stty transition slow enough to expose a cue published before it finishes.
    real_stty = shutil.which("stty")
    assert real_stty, "stty is required for mode grant qualification"
    grant_ready = Path(home) / "grant-terminal-ready"
    stty = Path(home) / "bin" / "stty"
    stty.write_text(
        '#!/bin/sh\nif [ "$1" = "-icanon" ]; then\n'
        '  sleep 0.25\n'
        f'  {shlex.quote(real_stty)} "$@"\n'
        '  result=$?\n'
        f'  if [ "$result" -eq 0 ]; then : > {shlex.quote(str(grant_ready))}; fi\n'
        '  exit "$result"\nfi\n'
        f'exec {shlex.quote(real_stty)} "$@"\n'
    )
    stty.chmod(0o755)
    shell = Pty(env)
    try:
        assert shell.ready()
        start = len(shell.plain())
        shell.send("/mode auto\r")
        wait_new(shell, "Type allow to continue", start)
        assert grant_ready.exists(), "consent cue appeared before terminal input was ready"
        send(shell, "allow\r")
        mode(shell, "allow")
        send(shell, "/mode suggest\r")
        mode(shell, "ask")
        start = len(shell.plain())
        send(shell, "/mode allow\r")
        mode(shell, "allow")
        assert "Type allow to continue" not in shell.plain()[start:]
        send(shell, "aishe mode ask\r")
        mode(shell, "ask")
        start = len(shell.plain())
        send(shell, "aishe mode auto\r")
        mode(shell, "allow")
        assert "Type allow to continue" not in shell.plain()[start:]
        start = len(shell.plain())
        send(shell, "/mode agent\r")
        if sys.platform == "darwin":
            # macOS workspace mode deliberately uses an explicit policy-only
            # grant; Linux refuses the fixture's unusable bubblewrap boundary.
            wait_new(shell, "Type agent to continue", start)
            assert "macOS workspace mode is policy-only" in shell.plain()[start:]
            send(shell, "\x1b")
            wait_new(shell, "grant declined", start)
        mode(shell, "allow")
        if sys.platform != "darwin":
            assert "requires functional bubblewrap" in shell.plain()[start:]
            assert "Type agent to continue" not in shell.plain()[start:]
    finally:
        shell.close()
        shutil.rmtree(home, ignore_errors=True)

    home, env = fixture("cancel")
    shell = Pty(env)
    try:
        assert shell.ready()
        start = len(shell.plain())
        shell.send("\x1b[Z")
        wait_new(shell, "Type allow to continue", start)
        send(shell, "\x1b")
        wait_new(shell, "grant declined", start)
        mode(shell, "ask")
        assert "grant declined" in shell.plain()[start:]
        assert "exit 1" not in shell.plain()[start:], shell.plain()[-2500:]
        start = len(shell.plain())
        shell.send("\x1b[Z")
        wait_new(shell, "Type allow to continue", start)
        send(shell, "\x03")
        mode(shell, "ask")
        assert "exit 1" not in shell.plain()[start:], shell.plain()[-2500:]
        send(shell, "echo partial")
        start = len(shell.plain())
        send(shell, "\x1b[Z")
        assert "Type allow to continue" not in shell.plain()[start:]
        send(shell, "\x15")
        mode(shell, "ask")
        send(shell, "false\r", 0.6)
        capsules = list((Path(home) / ".local/share/aishe/failures").glob("*.json"))
        assert len(capsules) == 1
        previous_capsule = capsules[0].read_bytes()
        for cancel in ("\x1b", "\x03"):
            start = len(shell.plain())
            shell.send("\x1b[Z")
            wait_new(shell, "Type allow to continue", start)
            send(shell, cancel)
            if cancel == "\x1b":
                wait_new(shell, "grant declined", start)
            assert "exit 1" not in shell.plain()[start:], shell.plain()[-2500:]
            assert capsules[0].read_bytes() == previous_capsule
        mode(shell, "ask")
        start = len(shell.plain())
        send(shell, "false\r", 0.6)
        assert "exit 1" in shell.plain()[start:]
    finally:
        shell.close()
        shutil.rmtree(home, ignore_errors=True)

    home, env = fixture("host-disabled", host_allowed=False)
    shell = Pty(env)
    try:
        assert shell.ready()
        start = len(shell.plain())
        send(shell, "/mode agent-host\r")
        mode(shell, "ask")
        assert "host scope is disabled by policy" in shell.plain()[start:]
        assert "Type agent-host" not in shell.plain()[start:]
    finally:
        shell.close()
        shutil.rmtree(home, ignore_errors=True)

    home, env = fixture("explicit-host", mode="allow", scope="host")
    config = Path(home) / ".config/aishe/config.toml"
    config.write_text(config.read_text().replace('[aishe]\n', '[aishe]\nyolo_confirm = "all"\n'))
    marker = Path(home) / "host-scope-marker"
    env["AISHE_FAKE_TOOL"] = "touch " + str(marker)
    shell = Pty(env)
    try:
        assert shell.ready()
        start = len(shell.plain())
        shell.send("\x1b[Z")
        wait_new(shell, "Type agent-host to continue", start)
        assert "Enter agent · workspace" not in shell.plain()[start:]
        send(shell, "\x1b")
        mode(shell, "allow")
        assert not marker.exists()
        start = len(shell.plain())
        shell.send("/mode agent-host\r")
        wait_new(shell, "Type agent-host to continue", start)
        send(shell, "agent-host\r")
        mode(shell, "agent")
        start = len(shell.plain())
        shell.send("print -r -- EFFECTIVE_SCOPE=$AISHE_SCOPE\r")
        wait_new(shell, "EFFECTIVE_SCOPE=host", start)
        start = len(shell.plain())
        send(shell, "?create the harmless test marker\r", 1)
        assert marker.exists(), shell.plain()[-3000:]
        assert "[Y/n]" not in shell.plain()[start:]
        assert "Type yes to run" not in shell.plain()[start:]
        start = len(shell.plain())
        send(shell, "/mode ask\r")
        send(shell, "/mode agent-host\r")
        mode(shell, "agent")
        assert "Type agent-host to continue" not in shell.plain()[start:]
    finally:
        shell.close()
        shutil.rmtree(home, ignore_errors=True)
    home, env = fixture("workspace-root")
    # This stand-in succeeds only the functional readiness probe. It never
    # runs an agent command: this fixture exercises grant and prompt metadata.
    bwrap = Path(home) / "bin" / "bwrap"
    bwrap.write_text(f"#!{sys.executable}\n"
                    "import pathlib, sys\n"
                    "args = sys.argv[1:]\n"
                    "if 'aishe-bwrap-probe' not in args: sys.exit(77)\n"
                    "root = args[args.index('--bind') + 1]\n"
                    "pathlib.Path(root, '.aishe-probe-writable').touch()\n")
    workspace = Path(home) / "workspace"
    (workspace / "child").mkdir(parents=True)
    shell = Pty(env)
    try:
        assert shell.ready()
        send(shell, "cd " + str(workspace) + "\r")
        start = len(shell.plain())
        shell.send("/mode agent\r")
        wait_new(shell, "Type agent to continue", start)
        send(shell, "agent\r")
        mode(shell, "agent")
        send(shell, "cd child\r")
        start = len(shell.plain())
        send(shell, "/mode agent\r")
        assert "Type agent to continue" not in shell.plain()[start:]
        start = len(shell.plain())
        shell.send("print -r -- ACCEPTED_ROOT=$_AISHE_AGENT_WORKSPACE_ROOT\r")
        wait_new(shell, "ACCEPTED_ROOT=" + str(workspace), start)
        start = len(shell.plain())
        send(shell, "cd ..\r")
        mode(shell, "agent")
        assert "[grant needed]" not in shell.plain()[start:]
    finally:
        shell.close()
        shutil.rmtree(home, ignore_errors=True)

    home, env = fixture("host-cli")
    marker = Path(home) / "standalone-agent-marker"
    env["AISHE_FAKE_TOOL"] = "touch " + str(marker)
    shell = Pty(env)
    try:
        assert shell.ready()
        start = len(shell.plain())
        shell.send("/mode agent-host\r")
        wait_new(shell, "Type agent-host to continue", start)
        send(shell, "agent-host\r")
        mode(shell, "agent")
        start = len(shell.plain())
        send(shell, 'aishe --mode agent -c "?create the harmless standalone marker"\r', 1)
        assert marker.exists(), shell.plain()[-3000:]
        assert "Type agent-host to continue" not in shell.plain()[start:]
    finally:
        shell.close()
        shutil.rmtree(home, ignore_errors=True)
    print("lean modes: ok")


if __name__ == "__main__":
    main()
