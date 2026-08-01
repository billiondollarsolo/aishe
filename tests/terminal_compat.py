#!/usr/bin/env python3
"""Deterministic terminal-transport compatibility qualification for AIShe.

The report deliberately distinguishes product failures from missing or
unconfigured environments. A capability is one of:

* pass: the complete automated contract ran and passed;
* fail: the contract ran and an AIShe behavior assertion failed;
* limitation: the transport exists but this machine cannot qualify it;
* unsupported: the transport executable is unavailable.

SSH is opt-in because a useful SSH check requires a host the operator is
authorized to access. Without ``--ssh-target`` it is reported as a limitation,
never silently counted as a pass.
"""

from __future__ import annotations

import argparse
import dataclasses
import fcntl
import json
import os
import pathlib
import platform
import pty
import re
import select
import shlex
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time
from typing import Callable

from harness_identity import require_current_binary


TIMEOUT = 12.0
CAPABILITIES = ("local-latency", "tmux", "screen", "ssh")


class ContractFailure(RuntimeError):
    """The transport started, but an asserted behavior failed."""


class CapabilityLimitation(RuntimeError):
    """The transport exists, but the current host cannot qualify it."""


@dataclasses.dataclass
class CapabilityResult:
    capability: str
    status: str
    detail: str
    checks: list[str] = dataclasses.field(default_factory=list)
    tool_version: str | None = None
    observed_term: str | None = None


class Fixture:
    """An isolated user/config/runtime environment for one transport."""

    def __init__(self, binary: str, prefix: str):
        self.binary = binary
        self.prefix = prefix
        self._temp = tempfile.TemporaryDirectory(prefix="aishe-terminal-compat-")
        self.root = pathlib.Path(self._temp.name)
        self.home = self.root / "home"
        config = self.root / "config" / "aishe"
        data = self.root / "data"
        runtime = self.root / "runtime"
        bindir = self.root / "bin"
        for directory in (self.home, config, data, runtime, bindir):
            directory.mkdir(parents=True, exist_ok=True)
        (config / "config.toml").write_text(
            "[aishe]\n"
            'mode = "suggest"\n'
            'provider = "anthropic"\n'
            'front_end = "zsh-pty"\n'
            "pty_prompt = false\n\n"
            "[backend]\n"
            'engine = "native"\n',
            encoding="utf-8",
        )
        (self.home / ".zshrc").write_text(
            "PROMPT='ZP> '\n"
            "PS2='C> '\n"
            f"HISTFILE={shlex.quote(str(self.root / 'history'))}\n"
            "HISTSIZE=100\nSAVEHIST=100\n"
            "setopt inc_append_history\n",
            encoding="utf-8",
        )
        os.symlink(binary, bindir / "aishe")
        fake_response = {
            "type": "command",
            "command": f"printf '%s%s\\n' {prefix}_AI_ OK",
            "explanation": "terminal compatibility fixture",
        }
        self.env = {
            key: value
            for key, value in os.environ.items()
            if not key.startswith("AISHE_")
            and key not in {"ANTHROPIC_API_KEY", "OPENAI_API_KEY", "ZDOTDIR"}
        }
        self.env.update(
            {
                "HOME": str(self.home),
                "XDG_CONFIG_HOME": str(self.root / "config"),
                "XDG_DATA_HOME": str(data),
                "XDG_RUNTIME_DIR": str(runtime),
                "AISHE_CONFIG_DIR": str(self.root / "config"),
                "AISHE_DATA_DIR": str(data),
                "AISHE_RUNTIME_DIR": str(runtime),
                "AISHE_FAKE_LLM": json.dumps(fake_response, separators=(",", ":")),
                "ZDOTDIR": str(self.home),
                "ZSH_DISABLE_COMPFIX": "true",
                "TERM": "xterm-256color",
                "PATH": f"{bindir}{os.pathsep}{os.environ.get('PATH', '')}",
                "LC_ALL": "C",
            }
        )

    def close(self) -> None:
        # Multiplexer servers can exit a fraction after their control command
        # returns and briefly recreate/remove socket files under the fixture.
        # Retry boundedly so that cleanup races never mask the actual contract
        # result.
        for attempt in range(5):
            try:
                self._temp.cleanup()
                return
            except OSError:
                if attempt == 4:
                    raise
                time.sleep(0.1 * (attempt + 1))


class PtyTransport:
    def __init__(
        self,
        argv: list[str],
        env: dict[str, str],
        rows: int = 24,
        cols: int = 80,
        *,
        controlling_terminal: bool = False,
    ):
        self.master, slave = pty.openpty()
        self._set_pty_size(rows, cols)

        def child_setup() -> None:
            os.setsid()
            if controlling_terminal:
                fcntl.ioctl(slave, termios.TIOCSCTTY, 0)

        try:
            self.proc = subprocess.Popen(
                argv,
                stdin=slave,
                stdout=slave,
                stderr=slave,
                env=env,
                preexec_fn=child_setup,
                close_fds=True,
            )
        except Exception:
            os.close(slave)
            os.close(self.master)
            raise
        os.close(slave)
        self.transcript = ""

    def _set_pty_size(self, rows: int, cols: int) -> None:
        fcntl.ioctl(
            self.master,
            termios.TIOCSWINSZ,
            struct.pack("HHHH", rows, cols, 0, 0),
        )

    def capture(self) -> str:
        while True:
            ready, _, _ = select.select([self.master], [], [], 0)
            if not ready:
                break
            try:
                chunk = os.read(self.master, 65536)
            except OSError:
                break
            if not chunk:
                break
            self.transcript += chunk.decode("utf-8", "replace")
        return self.transcript

    def sendline(self, line: str) -> None:
        self.raw(line.encode("utf-8") + b"\r")

    def raw(self, data: bytes) -> None:
        os.write(self.master, data)

    def resize(self, rows: int, cols: int) -> None:
        self._set_pty_size(rows, cols)

    def close(self) -> None:
        try:
            os.close(self.master)
        except OSError:
            pass
        if self.proc.poll() is None:
            try:
                os.killpg(os.getpgid(self.proc.pid), signal.SIGKILL)
            except (ProcessLookupError, PermissionError):
                pass
        try:
            self.proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            pass


class TmuxTransport:
    def __init__(self, binary: str, fixture: Fixture):
        self.fixture = fixture
        self.session = f"aishe_compat_{os.getpid()}_{time.time_ns()}"
        self.socket = f"aishe_compat_{os.getpid()}_{time.time_ns()}"
        self.base = [binary, "-L", self.socket, "-f", "/dev/null"]
        result = subprocess.run(
            self.base
            + [
                "new-session",
                "-d",
                "-s",
                self.session,
                "-x",
                "80",
                "-y",
                "24",
                fixture.binary,
                "zsh",
            ],
            env=fixture.env,
            capture_output=True,
            text=True,
            timeout=10,
        )
        if result.returncode != 0:
            raise CapabilityLimitation(
                f"tmux could not create an isolated session: {result.stderr.strip()}"
            )

    def _run(self, args: list[str], *, check: bool = True) -> subprocess.CompletedProcess[str]:
        result = subprocess.run(
            self.base + args,
            env=self.fixture.env,
            capture_output=True,
            text=True,
            timeout=10,
        )
        if check and result.returncode != 0:
            raise ContractFailure(
                f"tmux {' '.join(args)} failed: {result.stderr.strip()}"
            )
        return result

    def capture(self) -> str:
        return self._run(
            ["capture-pane", "-p", "-e", "-t", self.session, "-S", "-1000"]
        ).stdout

    def sendline(self, line: str) -> None:
        self._run(["send-keys", "-t", self.session, "-l", line])
        self._run(["send-keys", "-t", self.session, "Enter"])

    def raw(self, data: bytes) -> None:
        hex_bytes = [f"{byte:02x}" for byte in data]
        self._run(["send-keys", "-t", self.session, "-H", *hex_bytes])

    def resize(self, rows: int, cols: int) -> None:
        self._run(
            ["resize-window", "-t", self.session, "-x", str(cols), "-y", str(rows)]
        )

    def close(self) -> None:
        self._run(["kill-server"], check=False)


def attached_screen_argv(binary: str, session: str, candidate: str) -> list[str]:
    return [binary, "-c", "/dev/null", "-S", session, candidate, "zsh"]


class ScreenTransport:
    def __init__(self, binary: str, fixture: Fixture):
        self.binary = binary
        self.fixture = fixture
        self.session = f"aishe_compat_{os.getpid()}_{time.time_ns()}"
        self.screen_dir = fixture.root / "screen"
        self.screen_dir.mkdir(mode=0o700)
        self.env = dict(fixture.env)
        self.env["SCREENDIR"] = str(self.screen_dir)
        # A detached GNU screen has no display whose kernel PTY can be resized:
        # `screen -X width` changes screen's logical width but leaves `stty
        # size` at 80x24 and zsh reports COLUMNS=-1. Run an attached screen on
        # a real controlling PTY so an outer TIOCSWINSZ follows the same path as
        # an actual terminal emulator.
        self.pty = PtyTransport(
            attached_screen_argv(binary, self.session, fixture.binary),
            self.env,
            controlling_terminal=True,
        )

    def _run(self, args: list[str], *, check: bool = True) -> subprocess.CompletedProcess[str]:
        result = subprocess.run(
            [self.binary, "-S", self.session, "-p", "0", "-X", *args],
            env=self.env,
            capture_output=True,
            text=True,
            timeout=10,
        )
        if check and result.returncode != 0:
            raise ContractFailure(
                f"screen {' '.join(args)} failed: {result.stderr.strip()}"
            )
        return result

    def capture(self) -> str:
        return self.pty.capture()

    def sendline(self, line: str) -> None:
        self.raw(line.encode("utf-8") + b"\r")

    def raw(self, data: bytes) -> None:
        self.pty.raw(data)

    def resize(self, rows: int, cols: int) -> None:
        self.pty.resize(rows, cols)

    def close(self) -> None:
        self._run(["quit"], check=False)
        self.pty.close()


def wait_for(
    transport: PtyTransport | TmuxTransport | ScreenTransport,
    predicate: Callable[[str], bool],
    description: str,
    timeout: float = TIMEOUT,
) -> str:
    deadline = time.monotonic() + timeout
    last = ""
    while time.monotonic() < deadline:
        try:
            last = transport.capture()
        except ContractFailure:
            raise
        if predicate(last):
            return last
        time.sleep(0.15)
    tail = last[-3000:].replace("\x1b", "<ESC>")
    raise ContractFailure(f"timed out waiting for {description}; transcript tail:\n{tail}")


def exercise_contract(
    transport: PtyTransport | TmuxTransport | ScreenTransport,
    prefix: str,
) -> tuple[list[str], str]:
    checks: list[str] = []
    # Multiplexer pane capture trims trailing prompt spaces, so the stable
    # observable is the prompt token rather than its final padding byte.
    wait_for(transport, lambda text: "ZP>" in text, "the initial prompt", timeout=20)

    ready = f"{prefix}_READY_"
    transport.sendline(f"printf '%s%s\\n' {ready} OK")
    wait_for(transport, lambda text: f"{ready}OK" in text, "the readiness marker")
    checks.append("interactive zsh became ready")

    transport.sendline("? produce the deterministic terminal marker")
    wait_for(transport, lambda text: "terminal compatibility fixture" in text, "the fake-provider proposal")
    transport.raw(b"\r")
    wait_for(transport, lambda text: f"{prefix}_AI_OK" in text, "the accepted AI command")
    checks.append("agent proposal remained interactive and executed only after Enter")

    history = f"{prefix}_HISTORY_"
    transport.sendline(f"printf '%s%s\\n' {history} OK")
    transcript = wait_for(
        transport,
        lambda text: f"{history}OK" in text,
        "the direct-command history marker",
    )
    baseline = transcript.count(f"{history}OK")
    transport.raw(b"\x1b")
    time.sleep(0.300)
    transport.raw(b"[A")
    transport.raw(b"\r")
    wait_for(
        transport,
        lambda text: text.count(f"{history}OK") > baseline,
        "history recall after a 300 ms split escape sequence",
    )
    checks.append("300 ms split ESC+[A sequence recalled and executed history")

    transport.resize(40, 120)
    time.sleep(0.7)
    cols = f"{prefix}_COLS_"
    transport.sendline(f"printf '%s%s\\n' {cols} \"$COLUMNS\"")
    wait_for(transport, lambda text: f"{cols}120" in text, "resize propagation to COLUMNS=120")
    checks.append("80x24 to 120x40 resize propagated through the transport")

    term = f"{prefix}_TERM_"
    transport.sendline(f"printf '%s%s\\n' {term} \"$TERM\"")
    transcript = wait_for(
        transport,
        lambda text: re.search(rf"{re.escape(term)}[^\s]+", text) is not None,
        "the TERM observation",
    )
    matches = re.findall(rf"{re.escape(term)}([^\s]+)", transcript)
    observed_term = matches[-1].strip() if matches else "unknown"
    checks.append(f"reported TERM={observed_term}")
    transport.sendline("exit")
    return checks, observed_term


def tool_version(argv: list[str]) -> str:
    try:
        result = subprocess.run(
            argv, capture_output=True, text=True, timeout=10, check=False
        )
    except OSError as error:
        return str(error)
    output = (result.stdout or result.stderr).strip().splitlines()
    return output[0] if output else f"exit {result.returncode}"


def run_transport(
    capability: str,
    binary: str,
    executable: str | None,
    constructor: Callable[[Fixture], PtyTransport | TmuxTransport | ScreenTransport],
    version_argv: list[str] | None = None,
) -> CapabilityResult:
    if executable is None:
        return CapabilityResult(
            capability,
            "unsupported",
            f"{capability} executable is not installed on this host",
        )
    prefix = capability.upper().replace("-", "_")
    fixture = Fixture(binary, prefix)
    transport = None
    try:
        transport = constructor(fixture)
        checks, observed_term = exercise_contract(transport, prefix)
        return CapabilityResult(
            capability,
            "pass",
            "complete deterministic terminal contract passed",
            checks=checks,
            tool_version=tool_version(version_argv) if version_argv else None,
            observed_term=observed_term,
        )
    except CapabilityLimitation as error:
        return CapabilityResult(
            capability,
            "limitation",
            str(error),
            tool_version=tool_version(version_argv) if version_argv else None,
        )
    except (ContractFailure, OSError, subprocess.SubprocessError) as error:
        return CapabilityResult(
            capability,
            "fail",
            str(error),
            tool_version=tool_version(version_argv) if version_argv else None,
        )
    finally:
        if transport is not None:
            transport.close()
        fixture.close()


def remote_fixture_command(remote_binary: str, prefix: str) -> str:
    response = json.dumps(
        {
            "type": "command",
            "command": f"printf '%s%s\\n' {prefix}_AI_ OK",
            "explanation": "terminal compatibility fixture",
        },
        separators=(",", ":"),
    )
    config = (
        "[aishe]\n"
        'mode = "suggest"\n'
        'provider = "anthropic"\n'
        'front_end = "zsh-pty"\n'
        "pty_prompt = false\n\n"
        "[backend]\n"
        'engine = "native"\n'
    )
    zshrc = (
        "PROMPT='ZP> '\nPS2='C> '\n"
        "HISTFILE=$HOME/history\nHISTSIZE=100\nSAVEHIST=100\n"
        "setopt inc_append_history\n"
    )
    quoted_binary = shlex.quote(remote_binary)
    return (
        "root=$(mktemp -d \"${TMPDIR:-/tmp}/aishe-terminal-compat.XXXXXX\") || exit 70; "
        "cleanup() { rm -rf -- \"$root\"; }; trap cleanup EXIT HUP INT TERM; "
        "mkdir -p \"$root/home\" \"$root/config/aishe\" \"$root/data\" \"$root/runtime\" \"$root/bin\"; "
        f"ln -s {quoted_binary} \"$root/bin/aishe\"; "
        f"printf %s {shlex.quote(config)} >\"$root/config/aishe/config.toml\"; "
        f"printf %s {shlex.quote(zshrc)} >\"$root/home/.zshrc\"; "
        "env HOME=\"$root/home\" ZDOTDIR=\"$root/home\" "
        "XDG_CONFIG_HOME=\"$root/config\" XDG_DATA_HOME=\"$root/data\" "
        "XDG_RUNTIME_DIR=\"$root/runtime\" AISHE_CONFIG_DIR=\"$root/config\" "
        "AISHE_DATA_DIR=\"$root/data\" AISHE_RUNTIME_DIR=\"$root/runtime\" "
        "PATH=\"$root/bin:$PATH\" "
        f"AISHE_FAKE_LLM={shlex.quote(response)} ZSH_DISABLE_COMPFIX=true "
        f"{quoted_binary} zsh"
    )


def sanitize_ssh_detail(
    detail: str, target: str, identity_file: pathlib.Path | None
) -> str:
    """Remove connection identifiers from persisted/user-facing failure detail."""

    sanitized = detail
    values = [target]
    if "@" in target:
        values.append(target.rsplit("@", 1)[1])
    if identity_file is not None:
        values.append(str(identity_file.expanduser()))
    for value in sorted(set(values), key=len, reverse=True):
        if value:
            sanitized = sanitized.replace(value, "<ssh-target>")
    return sanitized


def run_ssh(
    binary: str,
    target: str | None,
    remote_binary: str,
    identity_file: pathlib.Path | None,
) -> CapabilityResult:
    ssh = shutil.which("ssh")
    version = tool_version([ssh, "-V"]) if ssh else None
    if ssh is None:
        return CapabilityResult("ssh", "unsupported", "ssh client is not installed", tool_version=version)
    if not target:
        return CapabilityResult(
            "ssh",
            "limitation",
            "ssh client exists, but no authorized target was supplied with --ssh-target",
            tool_version=version,
        )
    if identity_file is not None and not identity_file.expanduser().is_file():
        return CapabilityResult(
            "ssh",
            "limitation",
            "the configured SSH identity file is unavailable",
            tool_version=version,
        )
    prefix = "SSH"
    ssh_options = [ssh]
    if identity_file is not None:
        # Keep the identity as a discrete argv value. It is never interpolated
        # into the remote shell command or emitted into the JSON report.
        ssh_options.extend(["-i", str(identity_file.expanduser())])
    ssh_options.extend(
        [
        "-tt",
        "-o",
        "BatchMode=yes",
        "-o",
        "ConnectTimeout=10",
        "-o",
        "StrictHostKeyChecking=accept-new",
        "-o",
        "UserKnownHostsFile=/dev/null",
        target,
        ]
    )
    identity = subprocess.run(
        ssh_options[:-1]
        + [target, f"{shlex.quote(remote_binary)} --version"],
        capture_output=True,
        text=True,
        timeout=20,
    )
    if identity.returncode != 0:
        return CapabilityResult(
            "ssh",
            "limitation",
            sanitize_ssh_detail(
                f"authorized remote identity check failed: {identity.stderr.strip()}",
                target,
                identity_file,
            ),
            tool_version=version,
        )
    command = remote_fixture_command(remote_binary, prefix)
    env = dict(os.environ)
    env["TERM"] = "xterm-256color"
    transport = None
    try:
        transport = PtyTransport(
            ssh_options + [f"/bin/sh -c {shlex.quote(command)}"],
            env,
            controlling_terminal=True,
        )
        checks, observed_term = exercise_contract(transport, prefix)
        checks.insert(0, f"remote identity: {identity.stdout.strip()}")
        return CapabilityResult(
            "ssh",
            "pass",
            "complete opt-in SSH PTY contract passed",
            checks=checks,
            tool_version=version,
            observed_term=observed_term,
        )
    except (ContractFailure, OSError, subprocess.SubprocessError) as error:
        return CapabilityResult(
            "ssh",
            "fail",
            sanitize_ssh_detail(str(error), target, identity_file),
            checks=[f"remote identity: {identity.stdout.strip()}"],
            tool_version=version,
        )
    finally:
        if transport is not None:
            transport.close()


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    result.add_argument("binary", nargs="?", default="target/release/aishe")
    result.add_argument(
        "--capability",
        action="append",
        choices=CAPABILITIES,
        dest="capabilities",
        help="run only this capability (repeatable); default: all",
    )
    result.add_argument(
        "--require-capability",
        action="append",
        choices=CAPABILITIES,
        default=[],
        help="exit non-zero unless this capability passes (repeatable)",
    )
    result.add_argument(
        "--ssh-target",
        default=os.environ.get("AISHE_COMPAT_SSH_TARGET"),
        help="authorized SSH destination; omitted SSH is an explicit limitation",
    )
    result.add_argument(
        "--ssh-binary",
        default="aishe",
        help="AIShe executable on the SSH target (default: aishe)",
    )
    result.add_argument(
        "--ssh-identity",
        type=pathlib.Path,
        default=(
            pathlib.Path(os.environ["AISHE_COMPAT_SSH_IDENTITY"])
            if os.environ.get("AISHE_COMPAT_SSH_IDENTITY")
            else None
        ),
        help="private-key path passed as one ssh -i argv value; never reported",
    )
    result.add_argument("--json", type=pathlib.Path, help="write machine-readable evidence")
    return result


def main(argv: list[str] | None = None) -> int:
    args = parser().parse_args(argv)
    requested = list(dict.fromkeys(args.capabilities or CAPABILITIES))
    for required in args.require_capability:
        if required not in requested:
            raise SystemExit(
                f"--require-capability {required} also requires --capability {required}"
            )
    binary = require_current_binary(args.binary)
    binary_identity = subprocess.run(
        [binary, "--version"],
        capture_output=True,
        text=True,
        timeout=10,
        check=True,
    ).stdout.strip()
    results: list[CapabilityResult] = []
    if "local-latency" in requested:
        results.append(
            run_transport(
                "local-latency",
                binary,
                shutil.which("zsh"),
                lambda fixture: PtyTransport([binary, "zsh"], fixture.env),
                [shutil.which("zsh") or "zsh", "--version"],
            )
        )
    if "tmux" in requested:
        tmux = shutil.which("tmux")
        results.append(
            run_transport(
                "tmux",
                binary,
                tmux,
                lambda fixture: TmuxTransport(tmux or "tmux", fixture),
                [tmux or "tmux", "-V"],
            )
        )
    if "screen" in requested:
        screen = shutil.which("screen")
        results.append(
            run_transport(
                "screen",
                binary,
                screen,
                lambda fixture: ScreenTransport(screen or "screen", fixture),
                [screen or "screen", "--version"],
            )
        )
    if "ssh" in requested:
        results.append(
            run_ssh(binary, args.ssh_target, args.ssh_binary, args.ssh_identity)
        )

    required = set(args.require_capability)
    failures = [result.capability for result in results if result.status == "fail"]
    unmet = [
        result.capability
        for result in results
        if result.capability in required and result.status != "pass"
    ]
    limitations = [
        result.capability
        for result in results
        if result.status in {"limitation", "unsupported"}
    ]
    payload = {
        "schema_version": 1,
        "binary": str(pathlib.Path(binary).resolve()),
        "binary_identity": binary_identity,
        "host": {
            "platform": platform.platform(),
            "system": platform.system(),
            "machine": platform.machine(),
        },
        "generated_at_unix": int(time.time()),
        "escape_sequence_delay_ms": 300,
        "results": [dataclasses.asdict(result) for result in results],
        "manual_terminal_checks": [
            {"terminal": name, "status": "not_run", "evidence": None}
            for name in (
                "Apple Terminal",
                "iTerm2",
                "WezTerm",
                "kitty",
                "GNOME Terminal",
                "VS Code integrated terminal",
                "Cursor integrated terminal",
            )
        ],
        "required_capabilities": sorted(required),
        "outcome": (
            "fail"
            if failures or unmet
            else "pass_with_explicit_limitations"
            if limitations
            else "pass"
        ),
        "failures": failures,
        "limitations": limitations,
        "unmet_required_capabilities": unmet,
    }
    if args.json:
        args.json.parent.mkdir(parents=True, exist_ok=True)
        args.json.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
    for result in results:
        print(f"{result.status:11} {result.capability}: {result.detail}")
        if result.tool_version:
            print(f"            tool: {result.tool_version}")
        for check in result.checks:
            print(f"            - {check}")
    if args.json:
        print(f"report: {args.json}")
    if failures:
        print(f"failed capabilities: {', '.join(failures)}", file=sys.stderr)
    if unmet:
        print(f"required capabilities did not pass: {', '.join(unmet)}", file=sys.stderr)
    return 1 if failures or unmet else 0


if __name__ == "__main__":
    raise SystemExit(main())
