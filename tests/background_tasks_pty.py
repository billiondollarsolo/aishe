#!/usr/bin/env python3
"""Background task visibility and controls through a real controlling terminal.

Fixtures are ordinary private persisted records, with genuine sleeping process
identities. Viewing never needs a provider, MCP server, or managed runtime.
Checks observe files/processes and ZLE buffer probes rather than echoed input.
Run: python3 tests/background_tasks_pty.py target/debug/aishe
"""

from __future__ import annotations

import json
import fcntl
import os
from pathlib import Path
import shutil
import signal
import struct
import subprocess
import termios
import time
import traceback

from pty_helper import Pty, binary, environment


PROBE_RC = r'''
_background_fixture_probe() {
  builtin printf '%s\0' "$BUFFER" "$CURSOR" "${(%)RPROMPT}" "$AISHE_BACKGROUND_INDICATOR" \
    > "$HOME/background-probe"
}
zle -N _background_fixture_probe
bindkey -M emacs '^X^P' _background_fixture_probe
bindkey -M viins '^X^P' _background_fixture_probe
'''

CAPTURES = []


def capture(shell, label):
    if not os.environ.get("AISHE_TEST_CAPTURE_PATH"):
        return
    rows, cols, _, _ = struct.unpack("HHHH", fcntl.ioctl(shell.master, termios.TIOCGWINSZ, b"\0" * 8))
    CAPTURES.append({"label": label, "cols": cols, "rows": rows, "transcript": shell.transcript})


def wait_until(shell, predicate, description, timeout=8):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return
        shell.drain(.08)
    raise AssertionError(f"timed out waiting for {description}\n{shell.plain()[-6000:]}")


def shown(shell, text, start=0, timeout=5):
    wait_until(shell, lambda: text in shell.plain()[start:], text, timeout)


def probe(shell, root):
    path = root / "background-probe"
    path.unlink(missing_ok=True)
    shell.send("\x18\x10")
    wait_until(shell, path.exists, "ZLE buffer probe", 3)
    parts = path.read_bytes().split(b"\0")
    assert len(parts) == 5 and parts[-1] == b"", parts
    return tuple(part.decode() for part in parts[:-1])


def process_identity(pid):
    result = subprocess.run(["ps", "-o", "lstart=", "-p", str(pid)],
                            capture_output=True, text=True, timeout=3, check=True)
    assert result.stdout.strip(), "fixture process has no start identity"
    return result.stdout.strip()


def atomic_json(path, value):
    temporary = path.with_suffix(".tmp")
    temporary.write_text(json.dumps(value), encoding="utf-8")
    temporary.chmod(0o600)
    temporary.replace(path)


class Fixture:
    def __init__(self, label, personal=False, indicator=False):
        rc = PROBE_RC + ("\nPROMPT='PERSONAL> '\nRPROMPT='personal-right'\n" if personal else "")
        home, self.env = environment("background-" + label, mode="ask", zshrc=rc,
                                    config_extra='yolo_confirm = "never"\nyolo_plan = false\n'
                                                 'yolo_sandbox = false\nmax_yolo_iterations = 20\n'
                                                 'budget_usd = 0.0', extra={
            "AISHE_LEAN": "1", "AISHE_LEGACY_OPENCODE": "0",
            "AISHE_ZSH_PROFILE": "personal" if personal else "clean",
            "AISHE_PERSONAL_INDICATOR": "1" if indicator else "0",
            "AISHE_UNICODE": "ascii", "AISHE_MOTION": "live", "NO_COLOR": "1",
            "AISHE_COMMAND_HINT_SHOWN": "1", "AISHE_FAKE_LLM": "fixture response",
        })
        self.root = Path(home)
        self.work = self.root / "work"
        self.work.mkdir()
        self.other_work = self.root / "other-work"
        self.other_work.mkdir()
        self.data = Path(self.env["AISHE_DATA_DIR"]) / "aishe"
        self.records = {}
        self.processes = []
        self.shells = []
        self.created = int(time.time() * 1000)
        self.runtime_spy = self.root / "managed-runtime-started"
        self.provider_spy = self.root / "provider-initialized"
        self.env["AISHE_SPY_OPENCODE"] = str(self.runtime_spy)
        self.env["AISHE_SPY_PROVIDER_MAKE"] = str(self.provider_spy)
        self.env["AISHE_POLICY_FILE"] = str(self.root / "no-policy.toml")
        self.env["AISHE_RUNTIME_DIR"] = str(self.root / "runtime")
        config = self.root / ".config/aishe/config.toml"
        config.write_text(config.read_text() + '\n[sandbox]\nallow_host_yolo = true\n')
        if not personal:
            leanrc = self.root / "leanrc"
            leanrc.write_text(PROBE_RC)
            self.env["AISHE_LEANRC"] = str(leanrc)

    def cli(self, *args, success=True):
        result = subprocess.run([binary(), *args], env=self.env, cwd=self.work,
                                capture_output=True, text=True, timeout=10)
        if success:
            assert result.returncode == 0, result.stdout + result.stderr
        return result

    def shell(self, cols=100, browser=False):
        args = [binary(), "task", "browse"] if browser else [binary(), "zsh"]
        if isinstance(browser, str):
            args.append(browser)
        # The common PTY transport has no cwd parameter. Exec a short launcher
        # so the tested shell/browser has the fixture's real project directory.
        argv = ["/bin/sh", "-c", 'cd "$1" && shift && exec "$@"', "fixture", str(self.work), *args]
        shell = Pty(self.env, cols=cols, rows=26, argv=argv)
        self.shells.append(shell)
        return shell

    def record(self, task_id, state, objective, *, elsewhere=False, isolated=False):
        now = self.created + len(self.records) * 1000
        source = self.other_work if elsewhere else self.work
        row = {
            "schema_version": 1, "id": task_id, "objective": objective,
            "source_cwd": str(source), "run_cwd": str(source),
            "source_repo": None, "worktree": None, "base_head": None, "source_branch": None,
            "created_at_ms": now, "updated_at_ms": now, "state": state,
            "engine": "native", "connection_id": "anthropic", "provider": "anthropic",
            "model": "menu-model", "role": "build", "scope": "host", "network": "allow",
            "elapsed_ms": 4500, "pid": None, "process_start": None,
            "exit_code": 0 if state == "completed" else None,
            "budget": {"max_minutes": 5, "max_provider_turns": 20, "max_cost_usd": 0,
                       "max_tool_calls": 20, "max_changed_files": 10,
                       "max_changed_bytes": 100000, "max_network_calls": 20},
            "plan": [{"id": 1, "text": "Inspect fixture", "state": "completed",
                      "evidence": "inspection evidence"}],
            "plan_revision": 1, "budget_exceeded": False,
        }
        if state == "running":
            process = subprocess.Popen(["python3", "-c", "import time; time.sleep(120)"],
                                       stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                       stderr=subprocess.DEVNULL, start_new_session=True)
            self.processes.append(process)
            row.update(pid=process.pid, process_start=process_identity(process.pid))
        directory = self.data / "background-tasks" / task_id
        directory.mkdir(parents=True, mode=0o700)
        (directory / "request.txt").write_text(objective)
        (directory / "activity.log").write_text(
            "".join(f"ACTIVITY_PROOF line {i:03}\n" for i in range(180)))
        if isolated:
            subprocess.run(["git", "init", "-q", str(source)], check=True)
            (source / "review.txt").write_text("before fixture change\n")
            subprocess.run(["git", "-C", str(source), "add", "review.txt"], check=True)
            subprocess.run(["git", "-C", str(source), "-c", "user.name=Fixture",
                            "-c", "user.email=fixture@localhost", "commit", "-qm", "fixture base"], check=True)
            head = subprocess.run(["git", "-C", str(source), "rev-parse", "HEAD"],
                                  capture_output=True, text=True, check=True).stdout.strip()
            worktree = directory / "worktree"
            subprocess.run(["git", "-C", str(source), "worktree", "add", "--detach", "-q",
                            str(worktree), head], check=True)
            (worktree / "review.txt").write_text("REVIEW_PROOF after fixture change\n")
            row.update(source_repo=str(source), worktree=str(worktree), run_cwd=str(worktree), base_head=head)
        atomic_json(directory / "record.json", row)
        self.records[task_id] = row
        # Ordinary workers update the shared index on every save. An imported
        # fixture uses the public observer to publish its persisted record.
        self.cli("task", "browse")
        return row

    def update(self, task_id, publish=True, **changes):
        row = self.records[task_id]
        row.update(changes)
        row["updated_at_ms"] = max(int(time.time() * 1000), row["updated_at_ms"] + 1)
        atomic_json(self.data / "background-tasks" / task_id / "record.json", row)
        if publish:
            self.cli("task", "browse")

    def persisted(self, task_id):
        return json.loads((self.data / "background-tasks" / task_id / "record.json").read_text())

    def checkpoint(self, task_id, result=None):
        row = self.records[task_id]
        native_id = "native-" + task_id
        completed = row["state"] == "completed"
        checkpoint = {
            "schema_version": 1, "id": native_id, "name": None,
            "created_at_ms": row["created_at_ms"], "updated_at_ms": row["updated_at_ms"],
            "status": "completed" if completed else "interrupted", "mode": "yolo", "provider": "anthropic",
            "model": "menu-model", "connection_id": "anthropic",
            "execution_scope": "host", "network_policy": "allow",
            "workspace_root": row["run_cwd"], "cwd": row["run_cwd"],
            "objective": row["objective"],
            "messages": [{"role": "user", "data": row["objective"]}],
            "completed_tools": [], "pending_tool": None,
            "usage": {"input": 12, "output": 34, "requests": 1},
            "execution": {"provider_turns": 1, "tool_calls": 0, "network_calls": 0,
                          "elapsed_ms": 4500, "cost_usd": 0},
            "native_state": "completed" if completed else "cancelled",
            "last_error": None if completed else "fixture interruption",
        }
        if result is not None:
            checkpoint["messages"].append({"role": "assistant", "data": {"text": result, "tool_calls": []}})
        path = self.data / "tasks" / (native_id + ".json")
        path.parent.mkdir(parents=True, exist_ok=True)
        atomic_json(path, checkpoint)
        self.update(task_id, native_task_id=native_id)
        return path

    def assert_quiet(self):
        assert not self.runtime_spy.exists(), "task viewing started the managed runtime"
        assert not self.provider_spy.exists(), "task viewing initialized a provider"

    def close(self):
        for shell in self.shells:
            shell.close()
        # A failing resume assertion must not leave a detached worker behind.
        for task_id in self.records:
            try:
                row = self.persisted(task_id)
                if row["state"] in {"starting", "running"}:
                    self.cli("task", "cancel", task_id, success=False)
            except (OSError, ValueError, subprocess.SubprocessError, AssertionError):
                pass
        for process in self.processes:
            if process.poll() is None:
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
            process.wait(timeout=5)
        shutil.rmtree(self.root, ignore_errors=True)


def quiet_idle_and_live_badge():
    fixture = Fixture("badge")
    try:
        shell = fixture.shell()
        assert shell.ready(), shell.plain()[-4000:]
        assert probe(shell, fixture.root)[3] == "", "idle shell has a task badge"
        capture(shell, "Idle shell")
        shell.send("print -r -- DRAFT_''SURVIVED > \"$HOME/draft-effect\"")
        shell.send("\x01\x06\x06\x06")
        before = probe(shell, fixture.root)
        fixture.record("badge-running-001", "running", "Background fixture is working")
        shown(shell, "1 running", timeout=10)
        assert probe(shell, fixture.root)[:2] == before[:2], "idle refresh changed draft or cursor"
        capture(shell, "Running task while editing")
        fixture.update("badge-running-001", state="completed", pid=None, process_start=None, exit_code=0)
        start = len(shell.plain())
        shown(shell, "1 ready", start, timeout=10)
        after = probe(shell, fixture.root)
        assert after[:2] == before[:2], "completion refresh changed draft or cursor"
        assert "running" not in after[3] and "1 ready" in after[3], after
        capture(shell, "Ready task while editing")
        shell.send("\x05\r")
        wait_until(shell, lambda: (fixture.root / "draft-effect").exists(), "preserved draft effect")
        assert (fixture.root / "draft-effect").read_text().strip() == "DRAFT_SURVIVED"
        fixture.assert_quiet()
        print("  ok   idle badge hidden; running/completion update during editing preserves buffer and cursor")
    finally:
        fixture.close()


def browser_views_and_acknowledgement():
    fixture = Fixture("views")
    try:
        task_id = "review-complete-001"
        fixture.record(task_id, "completed", "Review the completed fixture", isolated=True)
        fixture.checkpoint(task_id, result="NATIVE_RESULT_PROOF final answer")
        fixture.record("elsewhere-complete-001", "completed", "ELSEWHERE_PROOF task", elsewhere=True)
        shell = fixture.shell()
        assert shell.ready()
        shown(shell, "2 ready", timeout=10)
        draft = "print -r -- BROWSER_''DRAFT > \"$HOME/browser-effect\""
        shell.send(draft + "\x01\x06\x06")
        before = probe(shell, fixture.root)
        flags = termios.tcgetattr(shell.master)
        shell.send("\x18b")
        shown(shell, "Background tasks")
        shown(shell, "Review the completed fixture")
        capture(shell, "Tasks in this project")
        assert "ELSEWHERE_PROOF" not in shell.plain(), "project filter included another project"
        shell.send("\t")
        shown(shell, "ELSEWHERE_PROOF")
        shell.send("\t")
        fixture.update(task_id, publish=False, objective="Review the completed fixture UPDATED_VISIBLE")
        shell.send("\x12")
        shown(shell, "UPDATED_VISIBLE")
        shell.send("Review")
        shell.send("\r")
        shown(shell, "Task details")
        shown(shell, "Used: 1/20 turns")
        shown(shell, "Usage: 12 input")
        shown(shell, "NATIVE_RESULT_PROOF")
        capture(shell, "Task details and result")
        shell.send("l")
        shown(shell, "Task activity")
        shown(shell, "ACTIVITY_PROOF")
        capture(shell, "Task activity")
        shell.send("\x1b[6~\x1b[5~\x1b[B\x1b[A")
        shell.send("\x1b")
        shell.drain(.15)
        shell.send("p")
        shown(shell, "Task changes")
        shown(shell, "REVIEW_PROOF")
        capture(shell, "Task changes preview")
        assert (fixture.work / "review.txt").read_text() == "before fixture change\n", "review applied changes"
        shell.send("\x1b")
        shell.drain(.15)
        shell.send("\x1b")
        shell.drain(.15)
        shell.send("\x1b")
        shell.drain(.15)
        assert probe(shell, fixture.root)[:2] == before[:2], "browser changed editing buffer/cursor"
        assert termios.tcgetattr(shell.master) == flags, "browser did not restore terminal modes"
        badge = probe(shell, fixture.root)[3]
        assert "1 ready" in badge and "2 ready" not in badge, "viewed completion was not acknowledged per shell"
        assert fixture.persisted(task_id)["state"] == "completed", "viewing changed durable task lifecycle"
        shell.send("\x05\r")
        wait_until(shell, lambda: (fixture.root / "browser-effect").exists(), "browser-restored draft effect")
        assert (fixture.root / "browser-effect").read_text().strip() == "BROWSER_DRAFT"
        fixture.assert_quiet()
        print("  ok   browser project/all, refresh, details, scrollable activity, read-only changes, acknowledgement, draft/TTY restoration")
    finally:
        fixture.close()


def confirmed_controls():
    fixture = Fixture("controls")
    try:
        task_id = "control-running-001"
        fixture.record(task_id, "running", "CANCEL_PROOF sleeping task")
        shell = fixture.shell()
        assert shell.ready()
        shell.send("/tasks\r")
        shown(shell, "Background tasks")
        shell.send("\r")
        shown(shell, "Task details")
        shell.send("c")
        shell.drain(.2)
        assert fixture.persisted(task_id)["state"] == "running", "cancel ran before confirmation"
        shell.send("\r")
        shell.drain(.3)
        assert fixture.persisted(task_id)["state"] == "running", "default confirmation cancelled task"
        assert fixture.processes[0].poll() is None, "declined cancel signalled process"
        shell.send("c")
        shell.drain(.2)
        shell.send("y\r")
        wait_until(shell, lambda: fixture.persisted(task_id)["state"] == "cancelled", "confirmed cancellation")
        wait_until(shell, lambda: fixture.processes[0].poll() is not None, "fixture process cancellation")
        assert fixture.persisted(task_id)["exit_code"] == 130
        shell.send("\x1b")
        shell.drain(.15)
        shell.send("\x1b")
        shell.drain(.15)
        fixture.assert_quiet()

        resume_id = "control-resume-001"
        fixture.record(resume_id, "cancelled", "RESUME_PROOF existing checkpoint")
        checkpoint = fixture.checkpoint(resume_id)
        before = checkpoint.read_bytes()
        shell.send("aishe task browse " + resume_id + "\r")
        shell.drain(.15)
        shell.send("r")
        shell.drain(.2)
        assert fixture.persisted(resume_id)["state"] == "cancelled", "resume ran before confirmation"
        shell.send("\r")
        shell.drain(.3)
        assert checkpoint.read_bytes() == before, "declined resume changed checkpoint"
        fixture.assert_quiet()
        shell.send("r")
        shell.drain(.2)
        shell.send("y\r")
        wait_until(shell, lambda: fixture.persisted(resume_id)["state"] == "completed", "confirmed native resume", 12)
        after = json.loads(checkpoint.read_text())
        assert after["execution"]["provider_turns"] > 1, "resume did not continue saved cumulative counters"
        assert after["messages"][0] == {"role": "user", "data": "RESUME_PROOF existing checkpoint"}
        assert fixture.persisted(resume_id)["native_task_id"] == after["id"], "resume replaced native checkpoint"
        print("  ok   cancel and resume are false-default, explicit controls; confirmed effects keep durable identity")
    finally:
        fixture.close()


def narrow_ctrl_c_and_cli_fallback():
    fixture = Fixture("narrow")
    try:
        fixture.record("narrow-complete-001", "completed", "NARROW_PROOF task")
        cli = fixture.cli("task", "browse")
        assert "NARROW_PROOF" in cli.stdout and "\x1b[" not in cli.stdout, cli.stdout
        shell = fixture.shell(cols=32)
        assert shell.ready()
        shell.send("print -r -- CTRL_''C_RESTORED > \"$HOME/ctrl-c-effect\"")
        before = probe(shell, fixture.root)
        flags = termios.tcgetattr(shell.master)
        shell.send("\x18b")
        shown(shell, "Background tasks")
        shell.send("\r")
        shown(shell, "Task details")
        shell.send("l")
        shown(shell, "Task activity")
        capture(shell, "Task activity at 32 columns")
        shell.resize(24, 12)
        shell.drain(.2)
        shell.send("\x1b[6~\x1b[5~")
        shell.send("\x03")
        shell.drain(.15)
        assert probe(shell, fixture.root)[:2] == before[:2], "Ctrl-C lost existing shell draft"
        assert termios.tcgetattr(shell.master) == flags, "Ctrl-C left terminal modes changed"
        shell.send("\r")
        wait_until(shell, lambda: (fixture.root / "ctrl-c-effect").exists(), "draft after browser Ctrl-C")
        assert (fixture.root / "ctrl-c-effect").read_text().strip() == "CTRL_C_RESTORED"
        assert shell.transcript.isascii(), "ASCII task UI emitted fixed Unicode"
        fixture.assert_quiet()
        print("  ok   plain CLI fallback, narrow resize/scroll, Ctrl-C and terminal/draft restoration")
    finally:
        fixture.close()


def personal_theme_and_ack_isolation():
    for enabled in (False, True):
        fixture = Fixture("personal-" + str(enabled), personal=True, indicator=enabled)
        try:
            fixture.record("personal-complete-001", "completed", "PERSONAL_PROOF completed task")
            first = fixture.shell()
            second = fixture.shell()
            assert first.ready() and second.ready()
            wait_until(first, lambda: "1 ready" in probe(first, fixture.root)[3], "personal task state", 10)
            right = probe(first, fixture.root)[2]
            assert "personal-right" in right, "personal theme RPROMPT was replaced"
            if not enabled:
                assert right == "personal-right", "background integration altered non-opted-in RPROMPT"
            else:
                assert "AISHE" in right or "ready" in right or "ask" in right, "opt-in indicator not integrated"
            first.send("\x18b")
            shown(first, "Background tasks")
            first.send("\r")
            shown(first, "Task details")
            first.send("\x1b")
            first.drain(.15)
            first.send("\x1b")
            first.drain(.15)
            wait_until(first, lambda: "ready" not in probe(first, fixture.root)[3], "read completion acknowledgement")
            assert "1 ready" in probe(second, fixture.root)[3], "viewing acknowledged another shell's completion"
            fixture.update("personal-complete-001", error="new revision detail")
            wait_until(first, lambda: "1 ready" in probe(first, fixture.root)[3], "new terminal revision becomes unread", 10)
            fixture.record("personal-failed-001", "failed", "ATTENTION_PROOF failed task")
            wait_until(first, lambda: "1 attention" in probe(first, fixture.root)[3], "failed task attention", 10)
            first.send("aishe task browse personal-failed-001\r")
            start = len(first.plain())
            shown(first, "Task details", start)
            first.send("\x1b")
            first.drain(.15)
            first.send("\x1b")
            first.drain(.15)
            wait_until(first, lambda: "attention" not in probe(first, fixture.root)[3], "read failure acknowledgement")
            assert fixture.persisted("personal-failed-001")["state"] == "failed", "viewing failure changed lifecycle"
            fixture.assert_quiet()
        finally:
            fixture.close()
    print("  ok   personal RPROMPT preserved/opt-in supported; acknowledgements isolated and revision-aware")


def static_plain_browser():
    fixture = Fixture("static")
    try:
        fixture.env["AISHE_MOTION"] = "static"
        fixture.record("static-complete-001", "completed", "STATIC_PROOF completed task")
        shell = fixture.shell(cols=60, browser=True)
        shown(shell, "Background tasks")
        shell.send("\r")
        shown(shell, "Task details")
        shown(shell, "Task actions")
        shell.send("1\r")
        shown(shell, "Task activity")
        shown(shell, "ACTIVITY_PROOF")
        shell.send(":cancel\r")
        shell.drain(.2)
        shell.send(":cancel\r")
        wait_until(shell, lambda: shell.proc.poll() is not None, "static browser exit")
        assert shell.proc.returncode == 0, shell.plain()[-4000:]
        assert "\x1b[" not in shell.transcript, "static task browser redraws or colors"
        assert shell.transcript.isascii(), "static ASCII task browser emitted fixed Unicode"
        fixture.assert_quiet()
        print("  ok   static/plain numbered browser is usable and viewing remains local")
    finally:
        fixture.close()


def standalone_reconcile_and_wrapped_end():
    fixture = Fixture("standalone")
    try:
        running = "standalone-running-001"
        fixture.record(running, "running", "RUNNING_PROCESS_PROOF fixture")
        shell = fixture.shell(browser=True)
        shown(shell, "Background tasks")
        shown(shell, "running")
        shell.send("\x03")
        wait_until(shell, lambda: shell.proc.poll() is not None, "closing a running task's browser")
        assert fixture.persisted(running)["state"] == "running", "closing browser stopped its task"
        assert fixture.processes[0].poll() is None, "browser Ctrl-C signalled a background worker"
        shell = fixture.shell(browser=True)
        shown(shell, "Background tasks")
        fixture.processes[0].terminate()
        fixture.processes[0].wait(timeout=5)
        wait_until(shell, lambda: fixture.persisted(running)["state"] == "interrupted",
                   "standalone browser reconciles an ended worker", 10)
        shown(shell, "interrupted", timeout=5)
        shell.send("\x03")
        wait_until(shell, lambda: shell.proc.poll() is not None, "standalone interrupted browser exit")

        complete = "wrapped-complete-001"
        fixture.record(complete, "completed", "WRAPPED_ACTIVITY_PROOF task")
        log = fixture.data / "background-tasks" / complete / "activity.log"
        # One persisted line wraps to dozens of terminal rows. End must reach
        # its actual visual end instead of clamping to the source line count.
        log.write_text("X" * 1000 + " WRAPPED_END_PROOF\n")
        shell = fixture.shell(cols=32, browser=complete)
        shown(shell, "Task details")
        shell.send("l")
        shown(shell, "Task activity")
        assert "WRAPPED_END_PROOF" not in shell.plain(), "wrapped fixture does not exceed viewport"
        shell.send("\x1b[F")
        shown(shell, "WRAPPED_END_PROOF")
        shell.send("\x03")
        wait_until(shell, lambda: shell.proc.poll() is not None, "Ctrl-C closes activity directly")
        assert shell.proc.returncode == 0, shell.plain()[-3000:]
        fixture.assert_quiet()
        print("  ok   standalone auto-reconciles ended workers; End reaches wrapped activity; direct activity Ctrl-C exits")
    finally:
        fixture.close()


def run():
    scenarios = [quiet_idle_and_live_badge, browser_views_and_acknowledgement,
                 confirmed_controls, narrow_ctrl_c_and_cli_fallback,
                 personal_theme_and_ack_isolation, static_plain_browser,
                 standalone_reconcile_and_wrapped_end]
    failed = []
    for scenario in scenarios:
        try:
            scenario()
        except Exception:
            failed.append(scenario.__name__)
            traceback.print_exc()
            print("  FAIL " + scenario.__name__, flush=True)
    if output := os.environ.get("AISHE_TEST_CAPTURE_PATH"):
        Path(output).write_text(json.dumps({"schema_version": 1, "captures": CAPTURES}, indent=2))
    if failed:
        raise SystemExit(f"FAIL: {len(failed)}/{len(scenarios)} background scenarios: {', '.join(failed)}")
    print(f"PASS: {len(scenarios)}/{len(scenarios)} background task visibility and terminal scenarios")


if __name__ == "__main__":
    run()
