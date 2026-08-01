#!/usr/bin/env python3
"""Deterministic interactive qualification for AIShe's native Bash hook.

The harness launches each requested Bash in a real pseudo-terminal with an
isolated HOME, rc file, config/data/runtime/history directories, and AIShe's
in-process fake provider. It never needs credentials or network access.

By default, all locally discoverable Bash 3.2 and 5.x binaries are exercised.
An unavailable version is reported as ``unavailable`` and is never counted as
a pass. CI can intentionally qualify only its current Bash with, for example::

    python3 tests/bash_hook.py target/release/aishe \
      --bash /bin/bash --require-family 5.x

Use ``--strict-matrix`` on a host that is expected to provide both Bash 3.2 and
5.x. The command then fails if either family is unavailable.
"""

from __future__ import annotations

import argparse
import dataclasses
import fcntl
import json
import os
import pathlib
import pty
import re
import resource
import select
import shlex
import shutil
import signal
import subprocess
import struct
import sys
import tempfile
import termios
import time
from typing import Iterable

from harness_identity import parse_binary_identity, require_current_binary


TIMEOUT = 20.0
PROMPT = "AISHE_BASH_TEST> "
REQUIRED_FAMILIES = ("3.2", "5.x")

# These are product differences, not skipped tests. They define the reduced
# Tier-B surface and are printed/stored beside every qualification result.
TIER_B_DIFFERENCES = (
    "A leading # remains Bash comment syntax; use ? on every declared tier or "
    "Ctrl-G on Bash 5.x to force AIShe.",
    "Suggest mode prints a command; Bash 5.x offers Ctrl-X Ctrl-R recall while "
    "Bash 3.2 Tier B- uses manual copy/edit.",
    "Bash routes through command_not_found_handle, so a line whose first token "
    "is a real command stays native unless a supported explicit override is used.",
    "The native Bash hook does not provide the zsh full-buffer route coloring "
    "or the AIShe-owned zsh PTY/status-line experience.",
    "A status-127 fallback may print Bash's native command-not-found diagnostic "
    "before AIShe handles the request.",
)

CASE_IDS = (
    "hook-loaded",
    "selection-handoff",
    "real-command-collision",
    "unknown-natural-language",
    "question-prefix",
    "force-agent-key",
    "slash-command-dispatch",
    "err-trap-chain",
    "suggestion-recall-key",
    "mode-cycle-key",
    "details-key",
    "auto-main-shell-state",
    "failure-hint",
    "failure-fix-key",
    "history-recall-and-persistence",
    "sigint-recovery",
    "sigtstp-job-control",
    "exit-cleanup-and-trap-chain",
)

EXPECTED_DIFFERENCE_CASES = {
    "3.2": {
        "force-agent-key": "use the passing ? prefix route instead of Ctrl-G",
        "suggestion-recall-key": "copy/edit the printed suggestion manually",
        "mode-cycle-key": "set AISHE_MODE explicitly instead of Shift-Tab",
        "details-key": "use /details instead of Ctrl-O",
        "failure-fix-key": "use the printed failure hint and CLI suggestion for manual review",
    }
}


@dataclasses.dataclass(frozen=True)
class BashIdentity:
    path: str
    version: str
    major: int
    minor: int
    patch: int
    platform: str

    @property
    def family(self) -> str:
        return classify_family(self.major, self.minor)


@dataclasses.dataclass
class CaseResult:
    id: str
    status: str
    detail: str = ""


@dataclasses.dataclass
class BashResult:
    identity: BashIdentity
    cases: list[CaseResult]
    transcript_tail: str = ""

    @property
    def passed(self) -> bool:
        return not case_matrix_problems(self.identity, self.cases)


def effective_tier(identity: BashIdentity) -> str:
    return "B-" if identity.family == "3.2" else "B"


def case_matrix_problems(
    identity: BashIdentity, cases: Iterable[CaseResult]
) -> list[str]:
    """Validate completeness and family-scoped expected differences."""

    cases = list(cases)
    problems: list[str] = []
    ids = [case.id for case in cases]
    missing = [case_id for case_id in CASE_IDS if case_id not in ids]
    unexpected = [case_id for case_id in ids if case_id not in CASE_IDS]
    duplicates = sorted({case_id for case_id in ids if ids.count(case_id) > 1})
    if missing:
        problems.append("missing cases: " + ", ".join(missing))
    if unexpected:
        problems.append("unexpected cases: " + ", ".join(unexpected))
    if duplicates:
        problems.append("duplicate cases: " + ", ".join(duplicates))
    declared = EXPECTED_DIFFERENCE_CASES.get(identity.family, {})
    for case in cases:
        if case.status == "pass":
            continue
        if case.status == "expected_difference" and case.id in declared:
            expected_alternative = f"alternative: {declared[case.id]}"
            if expected_alternative in case.detail:
                continue
            problems.append(
                f"{case.id}: expected difference did not record its tested alternative"
            )
            continue
        problems.append(
            f"{case.id}: status {case.status!r} is not allowed for Bash {identity.family}"
        )
    return problems


_BASH_VERSION = re.compile(
    r"GNU bash, version "
    r"(?P<major>\d+)\.(?P<minor>\d+)\.(?P<patch>\d+)"
    r"(?:\([^)]*\))?-release \((?P<platform>[^)]+)\)"
)


def parse_bash_identity(path: str, output: str) -> BashIdentity:
    """Parse the first line of ``bash --version``."""

    match = _BASH_VERSION.search(output)
    if not match:
        raise ValueError(f"unrecognized Bash version output from {path}: {output!r}")
    return BashIdentity(
        path=str(pathlib.Path(path).resolve()),
        version="{major}.{minor}.{patch}".format(**match.groupdict()),
        major=int(match.group("major")),
        minor=int(match.group("minor")),
        patch=int(match.group("patch")),
        platform=match.group("platform"),
    )


def classify_family(major: int, minor: int) -> str:
    if major == 3 and minor == 2:
        return "3.2"
    if major >= 5:
        return "5.x"
    return "other"


def inspect_bash(path: str) -> BashIdentity:
    resolved = shutil.which(path) if os.path.sep not in path else path
    if not resolved or not pathlib.Path(resolved).is_file():
        raise FileNotFoundError(path)
    result = subprocess.run(
        [resolved, "--version"],
        capture_output=True,
        text=True,
        timeout=10,
        check=True,
    )
    return parse_bash_identity(resolved, result.stdout)


def wait_for_appended_text(
    path: pathlib.Path, offset: int, needle: str, timeout: float = TIMEOUT
) -> None:
    """Wait until a subprocess appends an observable call marker."""

    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if path.exists():
            with path.open("r", encoding="utf-8") as handle:
                handle.seek(offset)
                if needle in handle.read():
                    return
        time.sleep(0.02)
    raise AssertionError(f"timed out waiting for call log marker {needle!r}")


def discover_bashes(explicit: Iterable[str] = ()) -> tuple[list[BashIdentity], list[str]]:
    """Return unique usable Bash binaries and unusable explicit candidates."""

    requested = list(explicit)
    if not requested:
        requested.extend(
            candidate
            for candidate in (
                os.environ.get("AISHE_BASH_32"),
                os.environ.get("AISHE_BASH_5"),
                shutil.which("bash"),
                "/bin/bash",
                "/usr/local/bin/bash",
                "/opt/homebrew/bin/bash",
            )
            if candidate
        )

    identities: list[BashIdentity] = []
    unavailable: list[str] = []
    seen: set[str] = set()
    for candidate in requested:
        try:
            identity = inspect_bash(candidate)
        except (FileNotFoundError, OSError, subprocess.SubprocessError, ValueError) as error:
            unavailable.append(f"{candidate}: {error}")
            continue
        real = os.path.realpath(identity.path)
        if real in seen:
            continue
        seen.add(real)
        identities.append(identity)
    identities.sort(key=lambda item: (item.major, item.minor, item.patch, item.path))
    return identities, unavailable


def family_coverage(results: Iterable[BashResult]) -> dict[str, str]:
    """Summarize required Bash families without converting absence into pass."""

    by_family: dict[str, list[BashResult]] = {family: [] for family in REQUIRED_FAMILIES}
    for result in results:
        if result.identity.family in by_family:
            by_family[result.identity.family].append(result)
    coverage: dict[str, str] = {}
    for family, members in by_family.items():
        if not members:
            coverage[family] = "unavailable"
        elif all(member.passed for member in members):
            coverage[family] = "pass"
        else:
            coverage[family] = "fail"
    return coverage


def qualification_exit_code(
    results: Iterable[BashResult], required_families: Iterable[str]
) -> int:
    results = list(results)
    if not results or any(not result.passed for result in results):
        return 1
    coverage = family_coverage(results)
    if any(coverage.get(family) != "pass" for family in required_families):
        return 1
    return 0


def resolve_required_families(
    identities: Iterable[BashIdentity],
    requested: Iterable[str],
    *,
    strict_matrix: bool,
    require_current: bool,
) -> tuple[list[str], list[str]]:
    """Resolve family requirements and flag requested versions without a tier."""

    required = list(REQUIRED_FAMILIES if strict_matrix else requested)
    problems: list[str] = []
    if require_current:
        for identity in identities:
            if identity.family not in REQUIRED_FAMILIES:
                problems.append(
                    f"{identity.path}: Bash {identity.version} has no declared tier"
                )
            if identity.family not in required:
                required.append(identity.family)
    return required, problems


class ForkedProcess:
    """Small ``Popen``-like handle for a child created by ``pty.fork``."""

    def __init__(self, pid: int, argv0: str):
        self.pid = pid
        self.argv0 = argv0
        self.returncode: int | None = None

    def poll(self) -> int | None:
        if self.returncode is not None:
            return self.returncode
        waited, status = os.waitpid(self.pid, os.WNOHANG)
        if waited:
            self.returncode = os.waitstatus_to_exitcode(status)
        return self.returncode

    def wait(self, timeout: float) -> int:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            result = self.poll()
            if result is not None:
                return result
            time.sleep(0.01)
        raise subprocess.TimeoutExpired([self.argv0], timeout)


class PtyShell:
    def __init__(
        self,
        argv: list[str],
        env: dict[str, str],
        cwd: pathlib.Path,
        *,
        atomic_terminal: bool,
    ):
        self.atomic_terminal = atomic_terminal
        if atomic_terminal:
            window_size = struct.pack("HHHH", 24, 100, 0, 0)
            pid, self.master = pty.fork()
            if pid == 0:
                try:
                    fcntl.ioctl(0, termios.TIOCSWINSZ, window_size)
                    maximum_fd = min(
                        1_048_576,
                        resource.getrlimit(resource.RLIMIT_NOFILE)[0],
                    )
                    os.closerange(3, maximum_fd)
                    os.chdir(cwd)
                    os.execvpe(argv[0], argv, env)
                except BaseException as error:
                    os.write(2, f"could not start Bash fixture: {error}\n".encode())
                    os._exit(127)
            fcntl.ioctl(self.master, termios.TIOCSWINSZ, window_size)
            self.proc = ForkedProcess(pid, argv[0])
        else:
            self.master, slave = pty.openpty()
            self.proc = subprocess.Popen(
                argv,
                stdin=slave,
                stdout=slave,
                stderr=slave,
                cwd=cwd,
                env=env,
                preexec_fn=os.setsid,
                close_fds=True,
            )
            os.close(slave)
        self.buffer = ""
        self.transcript = ""

    def _read_once(self, deadline: float) -> bool:
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            return False
        ready, _, _ = select.select([self.master], [], [], min(remaining, 0.2))
        if not ready:
            return True
        try:
            chunk = os.read(self.master, 8192)
        except OSError:
            return False
        if not chunk:
            return False
        text = chunk.decode("utf-8", "replace")
        self.buffer += text
        self.transcript += text
        return True

    def expect(self, needle: str, timeout: float = TIMEOUT) -> None:
        deadline = time.monotonic() + timeout
        while True:
            position = self.buffer.find(needle)
            if position >= 0:
                self.buffer = self.buffer[position + len(needle) :]
                return
            if not self._read_once(deadline):
                raise AssertionError(f"timed out waiting for {needle!r}")

    def expect_prompt(self, timeout: float = TIMEOUT) -> None:
        self.expect(PROMPT, timeout)

    def _pre_send(self) -> None:
        if self.atomic_terminal:
            # Match mature PTY drivers' bounded delay immediately before each
            # write, after fixture state and response files are ready.
            time.sleep(1.0)

    def sendline(self, line: str) -> None:
        self._pre_send()
        os.write(self.master, (line + "\n").encode("utf-8"))

    def send_bytes(self, value: bytes) -> None:
        self._pre_send()
        os.write(self.master, value)

    def settle(self, seconds: float = 0.25) -> None:
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            if not self._read_once(deadline):
                break

    def wait_for_no_children(self, timeout: float = TIMEOUT) -> None:
        """Wait for a synchronous Readline widget's child process to finish."""

        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            parent_option = "--ppid" if sys.platform.startswith("linux") else "-ppid"
            result = subprocess.run(
                ["ps", "-o", "pid=", parent_option, str(self.proc.pid)],
                capture_output=True,
                text=True,
                timeout=2,
            )
            if result.returncode not in (0, 1):
                raise AssertionError(
                    "could not inspect Bash widget children: " + result.stderr.strip()
                )
            if not result.stdout.strip():
                self.settle(0.1)
                return
            time.sleep(0.02)
        raise AssertionError("timed out waiting for the Bash widget child to exit")

    def wait(self, timeout: float = 10.0) -> int | None:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if self.proc.poll() is not None:
                self.settle(0.1)
                return self.proc.returncode
            self._read_once(min(deadline, time.monotonic() + 0.2))
        return None

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


def _write_fake(response_file: pathlib.Path, command: str) -> None:
    response_file.write_text(
        json.dumps(
            {
                "type": "command",
                "command": command,
                "explanation": "deterministic Bash hook qualification fixture",
            }
        ),
        encoding="utf-8",
    )


def _write_fixture(
    root: pathlib.Path, binary: str, bash: BashIdentity
) -> tuple[pathlib.Path, dict[str, str], dict[str, pathlib.Path]]:
    home = root / "home"
    config_root = root / "config"
    data_root = root / "data"
    runtime_root = root / "runtime"
    temp_root = root / "tmp"
    work = root / "work"
    bin_root = root / "bin"
    state_target = work / "state-target"
    for directory in (
        home,
        config_root / "aishe",
        data_root,
        runtime_root,
        temp_root,
        work,
        bin_root,
        state_target,
    ):
        directory.mkdir(parents=True, exist_ok=True)

    config = config_root / "aishe" / "config.toml"
    config.write_text(
        '[aishe]\nmode = "suggest"\nprovider = "anthropic"\n'
        'pty_prompt = false\n\n[backend]\nengine = "native"\n',
        encoding="utf-8",
    )
    response_file = root / "fake-response.json"
    _write_fake(response_file, "printf '%s\\n' BASH_INITIAL_FAKE")

    call_log = root / "aishe-calls.log"
    wrapper = bin_root / "aishe"
    wrapper.write_text(
        "#!/bin/sh\n"
        'printf \'%s\\n\' "$*" >> "$AISHE_TEST_CALL_LOG"\n'
        f"exec {shlex.quote(binary)} \"$@\"\n",
        encoding="utf-8",
    )
    wrapper.chmod(0o755)
    os.symlink(bash.path, bin_root / "bash")

    hook = subprocess.run(
        [binary, "init", "bash"],
        capture_output=True,
        text=True,
        timeout=20,
        check=True,
    ).stdout

    history_file = root / "bash-history"
    trap_marker = root / "prior-exit-trap-ran"
    err_trap_log = root / "prior-err-trap.log"
    selection_file = root / "selection"
    selection_file.write_text(
        "bash-test\n"
        "Bash Test\n"
        "anthropic\n"
        "api.example.test\n"
        "environment:TEST_KEY\n"
        "bash-test-model\n"
        "medium\n"
        "shell\n",
        encoding="utf-8",
    )
    cleanup_files = {
        "pending": temp_root / "pending",
        "force": temp_root / "force",
        "session": temp_root / "session",
        "acceptance": temp_root / "acceptance",
    }

    rcfile = root / "bashrc"
    rcfile.write_text(
        "PS1='" + PROMPT + "'\n"
        "PS2='AISHE_BASH_CONT> '\n"
        f"HISTFILE={shlex.quote(str(history_file))}\n"
        "HISTSIZE=500\nHISTFILESIZE=500\nHISTCONTROL=\n"
        "shopt -s histappend cmdhist\n"
        f"trap 'printf prior-trap-ran > {shlex.quote(str(trap_marker))}' EXIT\n"
        f"trap 'printf \"prior-err=%s\\n\" \"$?\" >> {shlex.quote(str(err_trap_log))}' ERR\n"
        + hook
        + "\nprintf 'AISHE_BASH_READY\\n'\n",
        encoding="utf-8",
    )

    env = {
        key: value
        for key, value in os.environ.items()
        if not key.startswith("AISHE_")
        and key
        not in {
            "BASH_ENV",
            "CDPATH",
            "ENV",
            "HISTFILE",
            "INPUTRC",
            "PROMPT_COMMAND",
            "XDG_CONFIG_HOME",
            "XDG_DATA_HOME",
        }
    }
    env.update(
        {
            "HOME": str(home),
            "PATH": str(bin_root) + os.pathsep + os.environ.get("PATH", ""),
            "TERM": "xterm-256color",
            "LC_ALL": "C",
            "XDG_CONFIG_HOME": str(config_root),
            "XDG_DATA_HOME": str(data_root),
            "AISHE_CONFIG_DIR": str(config_root),
            "AISHE_DATA_DIR": str(data_root),
            "AISHE_RUNTIME_DIR": str(runtime_root),
            "AISHE_FAKE_LLM_FILE": str(response_file),
            "AISHE_FAKE_LLM": "fake-provider-enabled",
            "AISHE_TEST_CALL_LOG": str(call_log),
            "AISHE_PENDING_FILE": str(cleanup_files["pending"]),
            "AISHE_FORCE_FILE": str(cleanup_files["force"]),
            "AISHE_SESSION_FILE": str(cleanup_files["session"]),
            "AISHE_ACCEPTANCE_FILE": str(cleanup_files["acceptance"]),
            "AISHE_SELECTION_FILE": str(selection_file),
            "TMPDIR": str(temp_root),
            "ANTHROPIC_API_KEY": "",
            "OPENAI_API_KEY": "",
            "HTTP_PROXY": "http://127.0.0.1:9",
            "HTTPS_PROXY": "http://127.0.0.1:9",
            "ALL_PROXY": "http://127.0.0.1:9",
            "NO_PROXY": "localhost,127.0.0.1",
        }
    )
    paths = {
        "rcfile": rcfile,
        "response": response_file,
        "call_log": call_log,
        "history": history_file,
        "trap_marker": trap_marker,
        "err_trap_log": err_trap_log,
        "state_target": state_target,
        "selection": selection_file,
        **cleanup_files,
    }
    return work, env, paths


def qualify_bash(binary: str, identity: BashIdentity) -> BashResult:
    cases: list[CaseResult] = []
    shell: PtyShell | None = None
    reviewed_enter = b"\r" if identity.family == "5.x" else b"\n"

    def passed(case_id: str, detail: str = "") -> None:
        cases.append(CaseResult(case_id, "pass", detail))

    def failed(case_id: str, detail: str) -> None:
        cases.append(CaseResult(case_id, "fail", detail))

    def limited(case_id: str, observed: str) -> None:
        alternative = EXPECTED_DIFFERENCE_CASES.get(identity.family, {}).get(case_id)
        if alternative is None:
            failed(case_id, observed)
        else:
            cases.append(
                CaseResult(
                    case_id,
                    "expected_difference",
                    f"{observed}; alternative: {alternative}",
                )
            )

    try:
        with tempfile.TemporaryDirectory(prefix="aishe-bash-hook-") as temporary:
            root = pathlib.Path(temporary)
            work, env, paths = _write_fixture(root, binary, identity)
            shell = PtyShell(
                [identity.path, "--noprofile", "--rcfile", str(paths["rcfile"]), "-i"],
                env,
                work,
                atomic_terminal=identity.family == "5.x",
            )
            shell.expect("AISHE_BASH_READY")
            shell.expect_prompt()

            shell.sendline(
                "type command_not_found_handle >/dev/null 2>&1 && "
                "printf '%s%s\\n' HOOK_LOADED_ OK"
            )
            shell.expect("HOOK_LOADED_OK")
            shell.expect_prompt()
            passed("hook-loaded")

            shell.sendline(
                "printf 'SELECTION=%s|%s|%s|%s\\n' "
                '"$AISHE_CONNECTION" "$AISHE_MODEL" "$AISHE_REASONING" '
                '"$AISHE_SELECTION_SCOPE"'
            )
            shell.expect("SELECTION=bash-test|bash-test-model|medium|shell")
            shell.expect_prompt()
            passed("selection-handoff")
            # The synthetic selection proves the main-shell handoff only. It is
            # deliberately not a real configured account, so remove the source
            # and inherited values before exercising provider-backed paths.
            paths["selection"].unlink()
            shell.sendline(
                "unset AISHE_SELECTION_FILE AISHE_CONNECTION AISHE_CONNECTION_LABEL AISHE_PROVIDER "
                "AISHE_ENDPOINT_HOST AISHE_AUTH_LABEL AISHE_MODEL "
                "AISHE_REASONING AISHE_SELECTION_SCOPE"
            )
            shell.expect_prompt()

            _write_fake(paths["response"], "printf '%s\\n' MUST_NOT_ROUTE_COLLISION")
            start = len(shell.transcript)
            shell.sendline("printf '%s%s\\n' BASH_NATIVE_COLLISION_ OK")
            shell.expect("BASH_NATIVE_COLLISION_OK")
            shell.expect_prompt()
            if "MUST_NOT_ROUTE_COLLISION" in shell.transcript[start:]:
                raise AssertionError("a real printf command was routed to AIShe")
            passed("real-command-collision")

            _write_fake(paths["response"], "printf '%s\\n' BASH_UNKNOWN_NL_OK")
            shell.sendline("definitely unknown natural language request")
            shell.expect("aishe suggests:")
            shell.expect("BASH_UNKNOWN_NL_OK")
            shell.expect_prompt()
            passed("unknown-natural-language")

            _write_fake(paths["response"], "printf '%s\\n' BASH_QUESTION_PREFIX_OK")
            shell.sendline("? printf should still be an agent request")
            shell.expect("BASH_QUESTION_PREFIX_OK")
            shell.expect_prompt()
            calls = paths["call_log"].read_text(encoding="utf-8")
            if "--suggest-line ? printf should still be an agent request" not in calls:
                raise AssertionError("the ? request did not reach --suggest-line")
            passed("question-prefix", "? is retained in the model request on Tier B")

            _write_fake(paths["response"], "printf '%s%s\\n' BASH_CTRL_G_ EXECUTED_OK")
            shell.buffer = ""
            force_start = len(shell.transcript)
            force_call_offset = paths["call_log"].stat().st_size
            shell.send_bytes(b"printf is forced through the agent\x07")
            if identity.family == "5.x":
                # The widget calls AIShe synchronously. Wait for its observable
                # wrapper call and child exit so Enter cannot be consumed by
                # the child on a cold or loaded qualification host.
                wait_for_appended_text(
                    paths["call_log"],
                    force_call_offset,
                    "--suggest-line printf is forced through the agent",
                )
                shell.wait_for_no_children()
                # bind-x redisplays the prompt plus staged command before the
                # user presses Enter. Consume that redisplay so the following
                # prompt assertion cannot accidentally match the stale prompt
                # (a race that only showed up on a slower Linux host).
                shell.expect("BASH_CTRL_G_ EXECUTED_OK")
                # Readline can expose the replacement buffer one scheduler
                # tick before the bind-x widget has fully returned. Model a
                # human review/Enter cadence instead of injecting Enter into
                # that implementation-specific handoff window.
                shell.settle()
            else:
                shell.settle(0.5)
            shell.buffer = ""
            shell.send_bytes(reviewed_enter)
            if identity.family == "5.x":
                shell.expect("BASH_CTRL_G_EXECUTED_OK")
                shell.expect_prompt()
                passed("force-agent-key", "Ctrl-G works independently of command-not-found")
            else:
                shell.expect_prompt()
                force_segment = shell.transcript[force_start:].replace("\r", "")
                if "BASH_CTRL_G_EXECUTED_OK\n" in force_segment:
                    passed(
                        "force-agent-key",
                        "Ctrl-G works independently of command-not-found",
                    )
                else:
                    limited(
                        "force-agent-key",
                        "Ctrl-G did not replace and submit the editable buffer through AIShe",
                    )

            calls_before_slash = paths["call_log"].read_text(encoding="utf-8")
            shell.sendline("/help")
            shell.expect_prompt()
            shell.sendline("/status")
            shell.expect_prompt()
            calls = paths["call_log"].read_text(encoding="utf-8")
            slash_calls = calls[len(calls_before_slash) :]
            if "commands" in slash_calls and "status" in slash_calls:
                passed("slash-command-dispatch", "/help and /status")
            else:
                failed(
                    "slash-command-dispatch",
                    "Bash treats slash-prefixed names as paths and bypasses "
                    "command_not_found_handle",
                )
                # Prove that the generated dispatch cases themselves are wired,
                # while retaining the interactive failure above.
                shell.sendline("command_not_found_handle /help")
                shell.expect_prompt()
                shell.sendline("command_not_found_handle /status")
                shell.expect_prompt()
                calls = paths["call_log"].read_text(encoding="utf-8")
                if "commands" not in calls or "status" not in calls:
                    raise AssertionError("generated slash dispatch cases also failed directly")

            shell.sendline("false")
            shell.expect_prompt()
            prior_err = paths["err_trap_log"].read_text(encoding="utf-8")
            if "prior-err=127" not in prior_err or "prior-err=1" not in prior_err:
                raise AssertionError(
                    "the pre-existing ERR trap did not observe routed 127 and ordinary 1"
                )
            passed("err-trap-chain", "prior ERR trap observed status 127 and status 1")

            _write_fake(
                paths["response"],
                "printf '%s%s\\n' BASH_RECALL_ EXECUTED_OK",
            )
            shell.sendline("another unknown request for recall")
            shell.expect("aishe suggests:")
            shell.expect_prompt()
            shell.buffer = ""
            recall_start = len(shell.transcript)
            shell.send_bytes(b"\x18\x12")
            if identity.family == "5.x":
                # Consume Readline's staged-command redisplay before Enter, so
                # neither a stale prompt nor a still-running bind-x callback can
                # satisfy the post-execution wait on a slower host.
                shell.expect("BASH_RECALL_ EXECUTED_OK")
                shell.settle()
            else:
                shell.settle()
            shell.buffer = ""
            shell.send_bytes(reviewed_enter)
            if identity.family == "5.x":
                shell.expect("BASH_RECALL_EXECUTED_OK")
                shell.expect_prompt()
                passed("suggestion-recall-key", "Ctrl-X Ctrl-R")
            else:
                shell.expect_prompt()
                recall_segment = shell.transcript[recall_start:].replace("\r", "")
                if "BASH_RECALL_EXECUTED_OK\n" in recall_segment:
                    passed("suggestion-recall-key", "Ctrl-X Ctrl-R")
                else:
                    limited(
                        "suggestion-recall-key",
                        "the recall macro did not execute the printed command cleanly",
                    )

            shell.sendline("export AISHE_MODE=suggest")
            shell.expect_prompt()
            shell.buffer = ""
            mode_start = len(shell.transcript)
            shell.send_bytes(b"\x1b[Z")
            shell.settle(0.5)
            shell.buffer = ""
            shell.sendline(
                "[ \"$AISHE_MODE\" = auto ] && "
                "printf '%s%s\\n' MODE_CYCLE_ OK || printf '%s%s\\n' MODE_CYCLE_ FAILED"
            )
            shell.expect_prompt()
            mode_segment = shell.transcript[mode_start:].replace("\r", "")
            if "MODE_CYCLE_OK\n" in mode_segment:
                passed("mode-cycle-key", "Shift-Tab suggest -> auto")
            else:
                limited(
                    "mode-cycle-key",
                    "Shift-Tab did not change AISHE_MODE from suggest to auto",
                )
                shell.sendline(
                    "export AISHE_MODE=auto; printf '%s%s\\n' MODE_ALTERNATIVE_ OK"
                )
                shell.expect("MODE_ALTERNATIVE_OK")
                shell.expect_prompt()

            shell.sendline("export AISHE_AGENT_OUTPUT=focus")
            shell.expect_prompt()
            shell.buffer = ""
            details_start = len(shell.transcript)
            shell.send_bytes(b"\x0f")
            shell.settle(0.5)
            shell.buffer = ""
            shell.sendline(
                "[ \"$AISHE_AGENT_OUTPUT\" = detailed ] && "
                "printf '%s%s\\n' DETAILS_KEY_ OK || printf '%s%s\\n' DETAILS_KEY_ FAILED"
            )
            shell.expect_prompt()
            details_segment = shell.transcript[details_start:].replace("\r", "")
            if "DETAILS_KEY_OK\n" in details_segment:
                passed("details-key", "Ctrl-O focus -> detailed")
            else:
                limited(
                    "details-key",
                    "Ctrl-O did not change AISHE_AGENT_OUTPUT from focus to detailed",
                )
                shell.sendline("/details")
                shell.expect("aishe agent details: detailed")
                shell.expect_prompt()

            _write_fake(paths["response"], "cd " + shlex.quote(str(paths["state_target"])))
            shell.sendline("state handoff unknown request")
            shell.expect_prompt()
            shell.sendline("printf 'STATE_PWD=%s\\n' \"$PWD\"")
            shell.expect("STATE_PWD=" + str(paths["state_target"]))
            shell.expect_prompt()
            passed("auto-main-shell-state", "safe cd persisted in the parent Bash")

            shell.sendline("export AISHE_MODE=suggest AISHE_FAILURE_HINTS=1")
            shell.expect_prompt()
            shell.sendline("false")
            shell.expect("aishe: exit 1")
            shell.expect_prompt()
            passed("failure-hint")

            _write_fake(
                paths["response"],
                "printf '%s%s\\n' BASH_FIX_EXECUTED _OK",
            )
            shell.buffer = ""
            fix_start = len(shell.transcript)
            fix_call_offset = paths["call_log"].stat().st_size
            shell.send_bytes(b"\x18\x06")
            if identity.family == "5.x":
                wait_for_appended_text(
                    paths["call_log"],
                    fix_call_offset,
                    "--suggest-line The previous shell command failed",
                )
                shell.wait_for_no_children()
                shell.expect("BASH_FIX_EXECUTED _OK")
                shell.settle()
            else:
                shell.settle(0.5)
            shell.buffer = ""
            shell.send_bytes(reviewed_enter)
            if identity.family == "5.x":
                shell.expect("BASH_FIX_EXECUTED_OK")
                shell.expect_prompt()
                passed("failure-fix-key", "Ctrl-X Ctrl-F prefills without auto-running")
            else:
                shell.expect_prompt()
                fix_segment = shell.transcript[fix_start:].replace("\r", "")
                if "BASH_FIX_EXECUTED_OK\n" in fix_segment:
                    passed(
                        "failure-fix-key",
                        "Ctrl-X Ctrl-F prefills without auto-running",
                    )
                else:
                    limited(
                        "failure-fix-key",
                        "Ctrl-X Ctrl-F did not prefill and execute the reviewed correction",
                    )
                    shell.sendline(
                        "command aishe --suggest-line "
                        "'Return a corrected command for manual review'"
                    )
                    shell.expect("printf '%s%s\\n' BASH_FIX_EXECUTED _OK")
                    shell.expect_prompt()

            history_command = "printf '%s%s\\n' BASH_HISTORY_ OK"
            shell.sendline(history_command)
            shell.expect("BASH_HISTORY_OK")
            shell.expect_prompt()
            shell.buffer = ""
            shell.send_bytes(b"\x1b[A")
            shell.expect(history_command)
            # Clear the recalled line through Readline itself. Bash 3.2 can be
            # left in a multi-key bind prefix after probing an unsupported
            # bind-x sequence, where Ctrl-C is not a reliable editor reset.
            shell.send_bytes(b"\x01\x0b")  # beginning-of-line, kill-line
            shell.sendline("printf '%s%s\\n' HISTORY_EDITOR_RESET_ OK")
            shell.expect("HISTORY_EDITOR_RESET_OK")
            shell.expect_prompt()
            shell.sendline("history -w")
            shell.expect_prompt()
            if history_command not in paths["history"].read_text(encoding="utf-8"):
                raise AssertionError("isolated HISTFILE did not persist the command")
            passed("history-recall-and-persistence", "Up arrow and isolated HISTFILE")

            shell.sendline("sleep 10")
            shell.settle(0.3)
            shell.send_bytes(b"\x03")
            shell.expect_prompt()
            shell.sendline("printf 'SIGNAL_EXIT=%s\\n' \"$AISHE_LAST_EXIT\"")
            shell.expect("SIGNAL_EXIT=130")
            shell.expect_prompt()
            passed("sigint-recovery", "foreground Ctrl-C returned a live prompt")

            shell.sendline("sleep 10")
            shell.settle(0.3)
            shell.send_bytes(b"\x1a")
            shell.expect("Stopped")
            shell.expect_prompt()
            shell.sendline(
                "kill -KILL %1; wait %1 2>/dev/null || :; "
                "printf '%s%s\\n' JOB_CONTROL_RECOVERED_ OK"
            )
            shell.expect("JOB_CONTROL_RECOVERED_OK")
            shell.expect_prompt()
            passed("sigtstp-job-control", "Ctrl-Z stopped and cleaned a foreground job")

            cleanup = " ".join(
                shlex.quote(str(paths[name]))
                for name in ("pending", "force", "session", "acceptance")
            )
            shell.sendline(f"touch {cleanup}; exit")
            if shell.wait() != 0:
                raise AssertionError("interactive Bash did not exit cleanly")
            for name in ("pending", "force", "session", "acceptance"):
                if paths[name].exists():
                    raise AssertionError(f"cleanup left {name} behind")
            if paths["trap_marker"].read_text(encoding="utf-8") != "prior-trap-ran":
                raise AssertionError("AIShe cleanup clobbered the pre-existing EXIT trap")
            passed("exit-cleanup-and-trap-chain")

    except Exception as error:  # one failure invalidates this version's matrix
        completed = {case.id for case in cases}
        failed_id = next((case_id for case_id in CASE_IDS if case_id not in completed), "harness")
        cases.append(CaseResult(failed_id, "fail", str(error)))
        for case_id in CASE_IDS:
            if case_id not in completed and case_id != failed_id:
                cases.append(CaseResult(case_id, "not_run", "earlier case failed"))
        tail = shell.transcript[-5000:] if shell is not None else ""
        return BashResult(identity, cases, tail)
    finally:
        if shell is not None:
            shell.close()

    tail = ""
    if shell is not None and any(case.status == "fail" for case in cases):
        tail = shell.transcript[-5000:]
    return BashResult(identity, cases, tail)


def report_payload(
    binary: str,
    results: list[BashResult],
    unavailable_candidates: list[str],
    required_families: list[str],
    binary_identity: dict[str, str] | None = None,
) -> dict[str, object]:
    return {
        "schema_version": 1,
        "binary": {"path": binary, "identity": binary_identity},
        "declared_tiers": {"3.2": "B-", "5.x": "B"},
        "required_families": required_families,
        "family_coverage": family_coverage(results),
        "unavailable_candidates": unavailable_candidates,
        "expected_tier_b_differences": list(TIER_B_DIFFERENCES),
        "results": [
            {
                "bash": dataclasses.asdict(result.identity),
                "effective_tier": effective_tier(result.identity),
                "passed": result.passed,
                "matrix_problems": case_matrix_problems(result.identity, result.cases),
                "cases": [dataclasses.asdict(case) for case in result.cases],
                "transcript_tail": result.transcript_tail,
            }
            for result in results
        ],
    }


def print_report(payload: dict[str, object]) -> None:
    print("AIShe native Bash hook qualification · Tier B / B-")
    binary = payload["binary"]
    identity = binary["identity"]
    if identity:
        print(
            f"AIShe {identity['version']} ({identity['commit']}, {identity['date']}) "
            f"· {binary['path']}"
        )
    for result in payload["results"]:  # type: ignore[index]
        bash = result["bash"]
        print(
            f"\nBash {bash['version']} ({bash['platform']}) · {bash['path']} · "
            f"Tier {result['effective_tier']} · "
            + ("PASS" if result["passed"] else "FAIL")
        )
        for case in result["cases"]:
            suffix = f" — {case['detail']}" if case["detail"] else ""
            print(f"  {case['status']:<7} {case['id']}{suffix}")
        if result["transcript_tail"]:
            print("  transcript tail:")
            for line in result["transcript_tail"].splitlines()[-30:]:
                print("    " + line)

    print("\nRequired family coverage:")
    for family, status in payload["family_coverage"].items():  # type: ignore[union-attr]
        tier = payload["declared_tiers"][family]  # type: ignore[index]
        print(f"  Bash {family:<3} Tier {tier:<2} {status}")
    if payload["unavailable_candidates"]:
        print("\nUnavailable candidates (not passes):")
        for candidate in payload["unavailable_candidates"]:
            print("  " + candidate)
    print("\nExpected Tier-B differences:")
    for difference in payload["expected_tier_b_differences"]:
        print("  - " + difference)


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", nargs="?", default="target/release/aishe")
    parser.add_argument(
        "--bash",
        action="append",
        default=[],
        dest="bashes",
        metavar="PATH",
        help="Bash binary to qualify (repeatable); default discovers local 3.2/5.x",
    )
    parser.add_argument(
        "--require-family",
        action="append",
        choices=REQUIRED_FAMILIES,
        default=[],
        help="fail unless this Bash family was tested and passed",
    )
    parser.add_argument(
        "--strict-matrix",
        action="store_true",
        help="require both Bash 3.2 and 5.x",
    )
    parser.add_argument(
        "--require-current-family",
        action="store_true",
        help="require every requested binary's recognized 3.2/5.x family",
    )
    parser.add_argument("--json", metavar="PATH", help="also write a JSON report")
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    binary = require_current_binary(args.binary)
    identities, unavailable = discover_bashes(args.bashes)
    results = [qualify_bash(binary, identity) for identity in identities]
    required, family_problems = resolve_required_families(
        identities,
        args.require_family,
        strict_matrix=args.strict_matrix,
        require_current=args.require_current_family,
    )
    unavailable.extend(family_problems)
    binary_version = subprocess.run(
        [binary, "--version"],
        capture_output=True,
        text=True,
        timeout=10,
        check=True,
    )
    payload = report_payload(
        binary,
        results,
        unavailable,
        required,
        parse_binary_identity(binary_version.stdout),
    )
    print_report(payload)
    if args.json:
        destination = pathlib.Path(args.json)
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
    return qualification_exit_code(results, required)


if __name__ == "__main__":
    raise SystemExit(main())
