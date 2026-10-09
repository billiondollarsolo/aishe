#!/usr/bin/env python3
"""Agent commands use the live shell exports, PATH and venv; secrets stay local."""

from pathlib import Path
import shlex
import shutil
import time

from pty_helper import Pty, environment


def wait_effect(shell, path, timeout=8):
    end = time.monotonic() + timeout
    while not path.exists() and time.monotonic() < end:
        shell.drain(.1)
    assert path.exists(), shell.plain()[-7000:]
    shell.drain(.2)


def run():
    home, env = environment("live-execution-state", mode="ask", extra={
        "AISHE_LEAN": "1", "AISHE_LEGACY_OPENCODE": "0",
        "AISHE_FAKE_LLM": "Live execution complete.",
        "AISHE_FAKE_TOOL": "aishe-live-state-probe",
        "INITIAL_REMOVED": "startup-value",
        "UNUSED_FAKE_KEY": "configured-credential-must-not-cross",
        "NO_COLOR": "1",
    })
    config = Path(home) / ".config/aishe/config.toml"
    config.write_text(config.read_text() + '\ndefault_scope = "host"\n\n[sandbox]\nallow_host_yolo = true\n')
    probe = Path(home) / "execution-result"
    secret_source = Path(home) / "private-execution-value"
    secret_source.write_text("runtime-secret-must-not-cross")
    secret_source.chmod(0o600)
    environment_dirs = []
    for number in (1, 2):
        directory = Path(home) / ("virtual environment %d" % number) / "bin"
        directory.mkdir(parents=True)
        executable = directory / "aishe-live-state-probe"
        executable.write_text(
            "#!/bin/sh\n"
            "printf '%s\\n' "
            + shlex.quote("program=%d" % number)
            + ' "venv=$VIRTUAL_ENV" "removed=${INITIAL_REMOVED-unset}"'
              ' "custom=$LIVE_CUSTOM" "name=$name" "value=$value" "credential=${UNUSED_FAKE_KEY-unset}"'
              ' "rpc=$line:$payload:$reply:$arg:$body:$submitted"'
              ' "secret=${NEW_SECRET-unset}" "control=${AISHE_SCOPE-unset}"'
              ' "grant=${AISHE_GRANT-unset}" > '
            + shlex.quote(str(probe)) + "\n"
        )
        executable.chmod(0o700)
        environment_dirs.append(directory)
    shell = Pty(env)
    try:
        assert shell.ready()
        # Ordinary known commands stay in zsh and must not capture any state.
        state_path = Path(home) / "state-path"
        shell.send("print -r -- \"$AISHE_EXECUTION_STATE_FILE\" > %s\r" % shlex.quote(str(state_path)))
        wait_effect(shell, state_path)
        state = Path(state_path.read_text().strip())
        assert state.is_absolute() and state.name == "state", state
        assert not state.exists(), "ordinary command wrote an execution snapshot"
        shell.send("/mode agent-host\r")
        assert shell.expect("Type agent-host to continue", 5), shell.plain()[-4000:]
        shell.send("agent-host\r")
        shell.drain(.3)
        for number, directory in enumerate(environment_dirs, 1):
            if probe.exists():
                probe.unlink()
            values = (
                "export PATH=%s:$PATH VIRTUAL_ENV=%s LIVE_CUSTOM=%s name=live-name value=live-value line=live-line payload=live-payload reply=live-reply arg=live-arg body=live-body submitted=live-submitted NEW_SECRET=\"$(<%s)\"; unset INITIAL_REMOVED\r"
                % (shlex.quote(str(directory)), shlex.quote(str(directory.parent)),
                   shlex.quote("exact literal $() value %d" % number), shlex.quote(str(secret_source)))
            )
            shell.send(values)
            shell.drain(.3)
            assert not state.exists(), "export/unset wrote a snapshot on the shell hot path"
            shell.send("? inspect the live environment %d\r" % number)
            wait_effect(shell, probe)
            actual = probe.read_text()
            expected = (
                "program=%d\nvenv=%s\nremoved=unset\ncustom=exact literal $() value %d\nname=live-name\nvalue=live-value\n"
                "credential=unset\nrpc=live-line:live-payload:live-reply:live-arg:live-body:live-submitted\nsecret=unset\ncontrol=unset\ngrant=unset\n"
                % (number, directory.parent, number)
            )
            assert actual == expected, (actual, expected, shell.plain()[-6000:])
            assert not state.exists(), "consumed environment remained on disk"
        # A malformed/oversized shell-only value must reject the next request
        # rather than execute with a stale environment from the previous turn.
        oversize = Path(home) / "oversized-execution-value"
        oversize.write_text("x" * (65536 + 1))
        shell.send('export LIVE_OVERSIZED="$(<%s)"\r' % shlex.quote(str(oversize)))
        shell.drain(.2)
        probe.unlink()
        start = len(shell.plain())
        shell.send("? reject oversized live state\r")
        shell.drain(.6)
        assert "live shell state unavailable" in shell.plain()[start:], shell.plain()[-4000:]
        assert not probe.exists(), "invalid snapshot reused stale execution state"
        assert not state.exists(), "producer published a partial oversized state"
        shell.send("unset LIVE_OVERSIZED\r")
        shell.drain(.2)
        # The parent input pump owns stdin. A protected host transition must
        # refuse promptly rather than deadlock on a second terminal reader.
        shell.send("export AWS_PROFILE=production-east\r")
        shell.drain(.2)
        start = len(shell.plain())
        shell.send("? reject protected live host target\r")
        shell.drain(.6)
        assert "separate terminal confirmation" in shell.plain()[start:], shell.plain()[-4000:]
        assert not probe.exists(), "protected live host target ran agent work"
        shell.send("unset AWS_PROFILE\r")
        shell.drain(.2)
        # Values that were never output by the executed program must not enter
        # task/session persistence or terminal output through context assembly.
        assert "configured-credential-must-not-cross" not in shell.plain()
        for path in (Path(home) / ".local/share/aishe").rglob("*.json"):
            text = path.read_text(errors="replace")
            assert "configured-credential-must-not-cross" not in text, path
            assert "runtime-secret-must-not-cross" not in text, path
        shell.send("print -r -- LIVE_''STATE_RESPONSIVE\r")
        assert shell.expect("LIVE_STATE_RESPONSIVE", 3)
        print("PASS: live PATH/venv, exact exports, unsets, credential/control exclusion, bounded state, protected host refusal, responsive zsh")
    finally:
        shell.close()
        shutil.rmtree(home, ignore_errors=True)


if __name__ == "__main__":
    run()
