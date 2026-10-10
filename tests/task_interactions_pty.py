#!/usr/bin/env python3
"""Qualify the task inbox and controls with real workers and a real ZLE draft.

Loopback provider requests and on-disk effects are the proof of each action;
terminal text only proves the associated UI is visible and understandable.
"""

from __future__ import annotations

import json
import hashlib
import os
from pathlib import Path
import shlex
import subprocess
import termios
import traceback

from background_tasks_pty import PROBE_RC, capture, shown, probe, wait_until
from native_task_interactions import BINARY, Fixture, response, tool, input_text
from pty_helper import Pty, binary


class UiFixture(Fixture):
    def __init__(self, label, choose):
        super().__init__("ui-" + label, choose)
        self.config.write_text(self.config.read_text().replace('mode = "yolo"', 'mode = "ask"'))
        self.env.update({"AISHE_UNICODE": "ascii", "AISHE_MOTION": "live", "NO_COLOR": "1",
                         "TERM": "xterm-256color", "AISHE_COMMAND_HINT_SHOWN": "1"})
        leanrc = self.root / "leanrc"
        leanrc.write_text(PROBE_RC + r'''
_interaction_probe_with_terminal() {
  _background_fixture_probe
  builtin print -r -- "$TTY" "$$" > "$HOME/interaction-terminal-device"
}
zle -N _background_fixture_probe _interaction_probe_with_terminal
''')
        self.env["AISHE_LEANRC"] = str(leanrc)
        bin_dir = self.home / "bin"
        bin_dir.mkdir()
        (bin_dir / "aishe").symlink_to(binary())
        self.env["PATH"] = str(bin_dir) + ":" + self.env.get("PATH", "")
        self.shells = []

    def shell(self, cols=100, browser=None, rows=28):
        args = [binary(), "zsh"] if browser is None else [binary(), "task", "browse", browser]
        launcher = ["/bin/sh", "-c", 'cd "$1" && shift && exec "$@"', "fixture", str(self.work), *args]
        shell = Pty(self.env, cols=cols, rows=rows, argv=launcher)
        self.shells.append(shell)
        return shell

    def close(self):
        for shell in self.shells:
            shell.close()
        super().close()

    def terminal_group(self):
        _, pid = (self.home / "interaction-terminal-device").read_text().strip().rsplit(" ", 1)
        path = Path(f"/proc/{pid}/stat")
        if path.exists():
            return int(path.read_text().rsplit(") ", 1)[1].split()[5])
        result = subprocess.run(["ps", "-o", "tpgid=", "-p", pid], capture_output=True,
                                text=True, check=True, timeout=3)
        return int(result.stdout.strip())


def close_drawer(shell, fixture):
    shell.send("\x1b")
    shell.drain(.2)
    shell.send("\x1b")
    shell.drain(.2)
    return probe(shell, fixture.home)


def question_inbox_answer_preserves_the_editing_draft():
    holder = {}

    def choose(index, payload):
        if index == 0:
            return response(tool("ask_user", {"question": "QUESTION_UI_PROOF choose the report color",
                                               "choices": ["blue", "green"]}, "ui-question"))
        if index == 1:
            assert "blue" in input_text(payload), input_text(payload)
            return response(tool("run_check", {"command": holder["command"]}, "ui-answer-proof"))
        assert index == 2, index
        return response(text="ANSWER_UI_RESULT_PROOF blue report is ready")

    fixture = UiFixture("question", choose)
    try:
        answer_marker = fixture.work / "answered-effect"
        holder["command"] = "printf 'blue\\n' > " + shlex.quote(str(answer_marker))
        task_id = fixture.start("QUESTION_UI_TASK_PROOF report task")
        waiting = fixture.waiting(task_id)
        assert len(fixture.loopback.calls) == 1 and not answer_marker.exists()
        shell = fixture.shell()
        assert shell.ready(), shell.plain()[-4000:]
        wait_until(shell, lambda: "needs you" in probe(shell, fixture.home)[3].lower(), "Needs you prompt badge", 10)
        shell.send("print -r -- INBOX_''DRAFT > \"$HOME/inbox-draft-effect\"\x01\x06\x06\x06")
        before = probe(shell, fixture.home)
        tty = termios.tcgetattr(shell.master)
        shell_foreground = fixture.terminal_group()
        shell.send("\x18b")
        shown(shell, "Background tasks")
        shown(shell, "QUESTION_UI_TASK_PROOF")
        shell.send("\x16")
        shown(shell, "Task views")
        shell.send("\x1b[H\x1b[B\r")
        shown(shell, "Needs you")
        shell.send("\r")
        shown(shell, "Task details")
        shown(shell, "QUESTION_UI_PROOF")
        drawer_foreground = fixture.terminal_group()
        capture(shell, "Needs you question details")
        shell.send("u")
        shown(shell, "Answer question")
        shell.send("\x1b[H\r")
        wait_until(shell, answer_marker.exists, "answer resumes the actual task")
        assert answer_marker.read_text() == "blue\n"
        finished = fixture.finish(task_id)
        assert finished["native_task_id"] == waiting["native_task_id"] and finished["state"] == "completed", finished
        assert finished["mailbox"]["requests"][0]["status"] == "consumed", finished
        shown(shell, "ANSWER_UI_RESULT_PROOF", timeout=8)
        current_foreground = fixture.terminal_group()
        assert current_foreground == drawer_foreground, ("background action took foreground terminal ownership from the drawer",
                                                         drawer_foreground, current_foreground)
        capture(shell, "Answered task with recorded result")
        after = close_drawer(shell, fixture)
        assert after[:2] == before[:2], ("answer UI changed the editing draft or cursor", before, after,
                                       "draft effect", (fixture.home / "inbox-draft-effect").exists(),
                                       shell.plain()[-2500:])
        assert termios.tcgetattr(shell.master) == tty, "answer UI did not restore terminal settings"
        assert fixture.terminal_group() == shell_foreground, "answer UI did not restore shell terminal ownership"
        shell.send("\x05\r")
        wait_until(shell, lambda: (fixture.home / "inbox-draft-effect").exists(), "restored draft executes")
        assert (fixture.home / "inbox-draft-effect").read_text().strip() == "INBOX_DRAFT"
        fixture.loopback.assert_ok()
        print("  ok   Needs you badge/question/answer resume real task; draft/cursor/TTY survive the drawer", flush=True)
    finally:
        fixture.close()


def action_approval_is_specific_and_defaults_to_no():
    holder = {}

    def choose(index, payload):
        if index == 0:
            return response(tool("request_approval", {"tool": "run_command", "arguments": {"command": holder["command"]},
                                                       "reason": "APPROVAL_UI_PROOF create this private marker"}, "ui-approval"))
        if index == 1:
            assert "approved" in input_text(payload).lower(), input_text(payload)
            return response(tool("run_command", {"command": holder["command"]}, "ui-approved-action"))
        assert index == 2, index
        return response(text="APPROVAL_UI_RESULT_PROOF the approved action ran once")

    fixture = UiFixture("approval", choose)
    try:
        marker = fixture.work / "specific-approved-effect"
        holder["command"] = "printf 'once\\n' >> specific-approved-effect"
        task_id = fixture.start("APPROVAL_UI_TASK_PROOF exact action")
        fixture.waiting(task_id)
        shell = fixture.shell(browser=task_id)
        shown(shell, "Task details")
        shown(shell, "APPROVAL_UI_PROOF")
        shown(shell, "specific-approved-effect")
        assert not marker.exists(), "viewing an approval executed it"
        capture(shell, "Exact action awaiting approval")
        shell.send("u")
        shown(shell, "Review exact action")
        capture(shell, "Complete exact action before the approval decision")
        shell.send("\r")
        shown(shell, "Specific action approval")
        capture(shell, "Specific action approval defaults to Leave for later")
        # An empty/default choice cannot grant permission.
        shell.send("\r")
        shell.drain(.3)
        assert not marker.exists() and len(fixture.loopback.calls) == 1, "default UI choice approved an action"
        assert fixture.show(task_id)["state"] == "waiting"
        shell.send("u")
        shown(shell, "Review exact action")
        shell.send("\r")
        shown(shell, "Specific action approval")
        shell.send("\x1b[H\r")
        wait_until(shell, marker.exists, "specific approved action")
        finished = fixture.finish(task_id)
        assert finished["state"] == "completed" and marker.read_text() == "once\n", finished
        shown(shell, "finished |", timeout=8)
        capture(shell, "Approved task authority and recorded usage before its result")
        shell.send("\x1b[F")
        shown(shell, "APPROVAL_UI_RESULT_PROOF", timeout=8)
        capture(shell, "Approved task result reached with End")
        shell.send("\x03")
        wait_until(shell, lambda: shell.proc.poll() is not None, "approval browser exits")
        fixture.loopback.assert_ok()
        print("  ok   approval shows the exact action; default choice keeps it pending; explicit approval runs once", flush=True)
    finally:
        fixture.close()


def narrow_exact_action_review_is_complete_and_defaults_to_leave():
    for cols, static in ((58, False), (32, False), (32, True)):
        command = "printf '%s\\n' 'NARROW   EXACT  SPACES' > exact-approved-effect; "
        command += "; ".join(f"printf '%s' 'inspect-{index:02}' > /dev/null" for index in range(12))
        command += "; printf '%s\\n' 'ACTION_TAIL_PROOF' >> exact-approved-effect"

        def choose(index, payload):
            if index == 0:
                return response(tool("request_approval", {
                    "tool": "run_command", "arguments": {"command": command},
                    "reason": "NARROW_APPROVAL_PROOF inspect the whole action"}, "narrow-approval"))
            if index == 1:
                assert "approved" in input_text(payload).lower(), input_text(payload)
                return response(tool("run_command", {"command": command}, "narrow-approved-action"))
            assert index == 2, index
            return response(text="NARROW_APPROVED_RESULT_PROOF exact action ran once")

        fixture = UiFixture(f"narrow-approval-{cols}-{static}", choose)
        try:
            if static:
                fixture.env["AISHE_MOTION"] = "static"
            marker = fixture.work / "exact-approved-effect"
            task_id = fixture.start("NARROW_APPROVAL_TASK inspect exact action")
            waiting = fixture.waiting(task_id)
            request = waiting["mailbox"]["requests"][0]
            binding = request["binding"]
            shell = fixture.shell(cols=cols, rows=18, browser=task_id)
            view_cols = cols

            def fresh(text, keys):
                start = len(shell.plain())
                shell.send(keys)
                shown(shell, text, start, 8)
                return start

            def open_review():
                if static:
                    fresh("Review exact action", "respond\r\r")
                else:
                    fresh("Review exact action", "u")

            def tail_review():
                # Return to the start so End must paint a new tail frame even
                # after a resize or an earlier tail inspection.
                shell.send("home\r" if static else "\x1b[H")
                shell.drain(.2)
                start = len(shell.plain())
                shell.send("end\r" if static else "\x1b[F")
                # A cell-wrapped marker can cross rows; inspect the new frame
                # bytes, without borrowing an earlier details preview.
                wait_until(shell, lambda: "ACTION_TAIL_PROOF" in "".join(
                    line.removeprefix("  ") for line in shell.plain()[start:].splitlines()),
                    "complete exact action tail in the review viewport", 8)
                capture(shell, f"Exact approval tail at {view_cols}x18; " + ("static" if static else "live"))

            shown(shell, "Task actions" if static else "Task details")
            open_review()
            shown(shell, "d choices | b back" if static else "Enter choices | Esc back")
            capture(shell, f"Bounded exact approval identity at {cols}x18; " + ("static" if static else "live"))
            if cols == 58 and not static:
                for view_cols in (32, 58):
                    start = len(shell.plain())
                    shell.resize(cols=view_cols, rows=18)
                    shown(shell, "Review exact action", start, 8)
                    tail_review()
                    assert not marker.exists() and len(fixture.loopback.calls) == 1
                    assert fixture.show(task_id)["mailbox"]["requests"][0]["binding"] == binding
            tail_review()
            fresh("Specific action approval", "d\r" if static else "\r")
            shown(shell, "Review exact action")
            capture(shell, f"Exact approval choices default to Leave at {cols}x18; " + ("static" if static else "live"))
            fresh("Task actions" if static else "Task details", "\r")
            assert not marker.exists() and len(fixture.loopback.calls) == 1
            pending = fixture.show(task_id)
            assert pending["state"] == "waiting"
            assert pending["mailbox"]["requests"][0]["binding"] == binding

            open_review()
            tail_review()
            fresh("Specific action approval", "d\r" if static else "\r")
            # Return from the decision picker to complete context; this grants
            # no permission and retains the same task/request/action binding.
            fresh("Review exact action", "3\r" if static else "\x1b[A\r")
            assert not marker.exists() and len(fixture.loopback.calls) == 1
            fresh("Task actions" if static else "Task details", "b\r" if static else "\x1b")
            assert fixture.show(task_id)["mailbox"]["requests"][0]["binding"] == binding

            if not static:
                open_review()
                fresh("Task details", "\x04")
                assert not marker.exists() and len(fixture.loopback.calls) == 1
                assert fixture.show(task_id)["state"] == "waiting"
                assert fixture.show(task_id)["mailbox"]["requests"][0]["binding"] == binding

            open_review()
            tail_review()
            fresh("Specific action approval", "d\r" if static else "\r")
            shell.send("1\r" if static else "\x1b[H\r")
            wait_until(shell, marker.exists, "same explicitly approved exact action", 12)
            finished = fixture.finish(task_id)
            assert marker.read_text() == "NARROW   EXACT  SPACES\nACTION_TAIL_PROOF\n"
            assert finished["state"] == "completed" and finished["native_task_id"] == waiting["native_task_id"]
            consumed = finished["mailbox"]["requests"][0]
            assert consumed["id"] == request["id"] and consumed["binding"] == binding
            assert consumed["status"] == "consumed" and len(fixture.loopback.calls) == 3
            if static:
                assert "\x1b" not in shell.transcript, "static action review emitted ANSI controls"
            fixture.loopback.assert_ok()
        finally:
            fixture.close()
    print("  ok   complete 58/32x18 action review scrolls and returns safely; live/static defaults grant nothing; exact approval runs once", flush=True)


def recorded_results_show_checks_and_unresolved_work():
    def choose(index, payload):
        if index == 0:
            return response(tool("run_check", {"command": "printf 'CHECK_UI_PASS_PROOF\\n'; exit 0"}, "ui-check-pass"),
                            tool("run_check", {"command": "printf 'CHECK_UI_FAIL_PROOF\\n'; exit 9"}, "ui-check-fail"))
        assert index == 1, index
        return response(text="MODEL_UI_CLAIM_PROOF everything passed")

    fixture = UiFixture("results", choose)
    try:
        task_id = fixture.start("CHECK_UI_TASK_PROOF inspect evidence")
        finished = fixture.finish(task_id)
        checkpoint = fixture.checkpoint(finished)
        assert [row["exit_code"] for row in checkpoint["evidence"] if row["kind"] == "check"] == [0, 9], checkpoint
        shell = fixture.shell(cols=78, browser=task_id)
        shown(shell, "Task details")
        shown(shell, "Recorded checks:")
        shown(shell, "1 stale")
        shown(shell, "1 failed")
        shown(shell, "Unresolved")
        capture(shell, "Result with stale passing and current failed recorded checks")
        shell.send("e")
        shown(shell, "Recorded checks")
        shown(shell, "CHECK_UI_PASS_PROOF")
        shown(shell, "CHECK_UI_FAIL_PROOF")
        capture(shell, "Recorded command exits and output")
        assert "MODEL_UI_CLAIM_PROOF" not in json.dumps(checkpoint["evidence"]), "model prose became recorded evidence"
        shell.send("\x03")
        wait_until(shell, lambda: shell.proc.poll() is not None, "evidence browser exits")
        fixture.loopback.assert_ok()
        print("  ok   results distinguish model prose from observed checks and retain the failed check as unresolved", flush=True)
    finally:
        fixture.close()


def live_followup_shows_queued_then_received():
    holder = {}

    def choose(index, payload):
        if index == 0:
            return response(tool("run_command", {"command": holder["command"]}, "ui-followup-boundary"))
        assert index == 1, index
        transcript = input_text(payload)
        assert transcript.count("LIVE_UI_FOLLOWUP_PROOF") == 1, transcript
        return response(text="LIVE_UI_RESULT_PROOF follow-up received")

    fixture = UiFixture("followup", choose)
    try:
        started = fixture.work / "ui-followup-started"
        finished_effect = fixture.work / "ui-followup-ended"
        holder["command"] = ("touch " + shlex.quote(str(started))
                             + "; sleep 6; touch " + shlex.quote(str(finished_effect)))
        task_id = fixture.start("FOLLOWUP_UI_TASK_PROOF ongoing work")
        fixture.wait(task_id, lambda row: started.exists(), "follow-up fixture begins actual command")
        shell = fixture.shell(browser=task_id)
        shown(shell, "Task details")
        shell.send("f")
        shown(shell, "Follow-up")
        shell.send("LIVE_UI_FOLLOWUP_PROOF\r")
        wait_until(shell, lambda: len(fixture.show(task_id)["mailbox"].get("followups", [])) == 1,
                   "UI persists the follow-up")
        queued = fixture.show(task_id)["mailbox"]["followups"][0]
        assert queued["status"] == "queued" and not finished_effect.exists(), queued
        shown(shell, "Follow-ups: 1 queued")
        shown(shell, "0 received")
        capture(shell, "Live follow-up queued during command")
        finished = fixture.finish(task_id)
        entry = finished["mailbox"]["followups"][0]
        assert entry["status"] == "received" and entry.get("received_at_ms"), finished
        assert finished_effect.exists()
        shown(shell, "Follow-ups: 0 queued", timeout=8)
        shown(shell, "1 received")
        shell.send("\x1b[F")
        shown(shell, "LIVE_UI_RESULT_PROOF", timeout=8)
        capture(shell, "Live follow-up received after safe boundary")
        shell.send("\x03")
        wait_until(shell, lambda: shell.proc.poll() is not None, "follow-up browser closes")
        fixture.loopback.assert_ok()
        print("  ok   follow-up UI queues during a real command and shows received only after durable delivery", flush=True)
    finally:
        fixture.close()


def naming_pinning_archive_and_persistent_review_are_unobtrusive():
    def choose(index, payload):
        assert index == 0, index
        return response(text="METADATA_UI_RESULT_PROOF completed work")

    fixture = UiFixture("metadata", choose)
    try:
        task_id = fixture.start("METADATA_UI_ORIGINAL_PROOF immutable objective")
        finished = fixture.finish(task_id)
        original = (fixture.data / "aishe/background-tasks" / task_id / "request.txt").read_bytes()
        metadata_path = fixture.data / "aishe/background-tasks" / task_id / "metadata.json"
        first = fixture.shell()
        second = fixture.shell()
        assert first.ready() and second.ready()
        wait_until(first, lambda: "1 ready" in probe(first, fixture.home)[3], "unreviewed result badge", 10)
        first.send("print -r -- META_''DRAFT > \"$HOME/meta-draft-effect\"\x01\x06\x06")
        before = probe(first, fixture.home)
        first.send("\x18b\r")
        shown(first, "Task details")
        wait_until(first, lambda: metadata_path.exists() and json.loads(metadata_path.read_text()).get("seen_revision"),
                   "seen exact result is durable")
        assert not json.loads(metadata_path.read_text()).get("reviewed_revision"), "opening silently marked reviewed"
        first.send("v")
        wait_until(first, lambda: json.loads(metadata_path.read_text()).get("reviewed_revision"),
                   "explicit review action is durable")
        stamp = json.loads(metadata_path.read_text())["reviewed_revision"]
        first.send("n")
        shown(first, "Task name")
        # A rename field may be prefilled with the current title.
        first.send("\x15CALM_UI_NAME_PROOF\r")
        wait_until(first, lambda: json.loads(metadata_path.read_text()).get("title") == "CALM_UI_NAME_PROOF", "UI rename")
        first.send("i")
        wait_until(first, lambda: json.loads(metadata_path.read_text())["pinned"], "UI pin")
        assert json.loads(metadata_path.read_text())["reviewed_revision"] == stamp, "name/pin changed reviewed result stamp"
        assert fixture.show(task_id)["objective"] == finished["objective"], "name rewrote objective"
        first.send("h")
        shown(first, "Archive")
        first.send("\r")
        first.drain(.3)
        assert not json.loads(metadata_path.read_text())["archived"], "default archive confirmation hid work"
        first.send("h")
        shown(first, "Archive")
        first.send("y\r")
        wait_until(first, lambda: json.loads(metadata_path.read_text())["archived"], "confirmed archive")
        assert (fixture.data / "aishe/background-tasks" / task_id / "request.txt").read_bytes() == original
        assert close_drawer(first, fixture)[:2] == before[:2], "metadata controls changed draft/cursor"
        wait_until(second, lambda: "ready" not in probe(second, fixture.home)[3], "persistent review reaches second shell", 10)
        first.send("\x18b\x16")
        shown(first, "Task views")
        first.send("\x1b[F\r")
        shown(first, "Archived tasks")
        shown(first, "CALM_UI_NAME_PROOF")
        capture(first, "Archived work remains available without a badge")
        first.send("\r")
        shown(first, "Task details")
        first.send("h")
        wait_until(first, lambda: not json.loads(metadata_path.read_text())["archived"], "unarchive control")
        close_drawer(first, fixture)
        assert "ready" not in probe(first, fixture.home)[3], "unarchive resurrected an already reviewed result"
        fixture.loopback.assert_ok()
        print("  ok   names/pin/archive preserve immutable request and draft; review is persistent; archived work is recoverable", flush=True)
    finally:
        fixture.close()


def native_prompt_and_session_usage_preserve_provider_coverage():
    """Real HTTP responses establish absence, explicit zero, and known subtotal."""
    for cols in (100, 58):
        def choose(index, payload):
            answer = response(text=json.dumps({"type": "answer", "explanation": f"COVERAGE_{index}_PROOF"}))
            if index == 0:
                answer.pop("usage")
            elif index == 1:
                answer["usage"] = {"input_tokens": 0, "output_tokens": 0}
            else:
                assert index == 2, index
            return answer

        fixture = UiFixture(f"usage-coverage-{cols}", choose)
        try:
            status_order = ["session_cost", "last_cost", "requests"] if cols == 100 else ["last_cost", "session_cost", "requests"]
            fixture.config.write_text(fixture.config.read_text().replace(
                'budget_usd = 0.0', 'budget_usd = 0.0\nstatus_line_items = ' + json.dumps(status_order))
                .replace('"original-background-model"', '"cov"')
                + '\n[pricing."cov"]\ninput = 1.0\noutput = 2.0\n')
            leanrc = Path(fixture.env["AISHE_LEANRC"])
            leanrc.write_text(leanrc.read_text() + '\nbuiltin printf "%s\\n%s\\n" "$AISHE_USAGE_FILE" "$AISHE_STATUS_FILE" > "$HOME/usage-paths"\n')
            shell = fixture.shell(cols)
            assert shell.ready()
            paths = (fixture.home / "usage-paths").read_text().splitlines()
            assert len(paths) == 2 and all(paths), paths
            tally, status = map(Path, paths)

            for index in range(3):
                shell.send(f"? report coverage case {index}\r")
                wait_until(shell, lambda: tally.exists() and len(tally.read_text().splitlines()) == index + 1,
                           f"recorded provider coverage {index}", 12)
                wait_until(shell, lambda: status.exists() and f"{index + 1} req" in status.read_text(),
                           f"refreshed prompt usage {index}", 5)
                fields = dict(line.split("\t", 1) for line in status.read_text().splitlines())
                prompt = probe(shell, fixture.home)[2]
                if index == 0:
                    assert fields["last_cost"] == "last cost n/a", fields
                    assert fields["session_cost"] == "session cost n/a", fields
                    assert "cost n/a" in prompt and "$0.0000" not in prompt, prompt
                elif index == 1:
                    assert fields["last_cost"] == "last ~$0.0000", fields
                    assert fields["session_cost"] == "session ~$0.0000 (partial; 1 unknown)", fields
                    if cols == 100:
                        assert "partial" in prompt, prompt
                    else:
                        assert "last ~$0.0000" in prompt, prompt
                else:
                    assert fields["last_cost"] == "last ~$0.0001", fields
                    assert fields["session_cost"] == "session ~$0.0001 (partial; 1 unknown)", fields
                capture(shell, f"Native prompt truthful coverage {index} at {cols} columns")
            rows = [row.split("\t") for row in tally.read_text().splitlines()]
            assert all(row[0] == "v3" for row in rows), rows
            assert [row[6:] for row in rows] == [["1", "0", "0", "0", "0", "0"], ["0", "0", "0", "1", "0", "0"], ["0", "20", "30", "1", "20", "30"]], rows
            start = len(shell.plain())
            shell.send("/usage\r")
            shown(shell, "20 in · 30 out (partial) · 3 reqs · ~$0.0001 (partial; 1 unknown)", start, 8)
            document = json.loads(fixture.cli("usage", "--json").stdout)
            assert document["total"]["cost_usd"] is None, document
            assert abs(document["total"]["known_cost_subtotal_usd"] - .00008) < 1e-12, document
            assert len(fixture.loopback.calls) == 3, fixture.loopback.calls
            fixture.loopback.assert_ok()
        finally:
            fixture.close()
    print("  ok   native prompt/session/ledger preserve missing usage, explicit zero, and partial subtotal via real HTTP", flush=True)


def native_session_budget_blocks_missing_or_legacy_usage_before_more_http():
    for legacy in (False, True):
        def choose(index, payload):
            assert not legacy and index == 0, (legacy, index)
            answer = response(text=json.dumps({"type": "answer", "explanation": "BUDGET_UNKNOWN_USAGE_PROOF"}))
            answer.pop("usage")
            return answer

        fixture = UiFixture(f"budget-coverage-{legacy}", choose)
        try:
            fixture.config.write_text(fixture.config.read_text().replace('budget_usd = 0.0', 'budget_usd = 10.0')
                + '\n[pricing."original-background-model"]\ninput = 1.0\noutput = 2.0\n')
            leanrc = Path(fixture.env["AISHE_LEANRC"])
            leanrc.write_text(leanrc.read_text() + '\nbuiltin print -r -- "$AISHE_USAGE_FILE" > "$HOME/budget-usage-path"\n')
            shell = fixture.shell()
            assert shell.ready()
            tally = Path((fixture.home / "budget-usage-path").read_text().strip())
            if legacy:
                tally.write_text("v2\t0\t0\t1\toriginal-background-model\topenai\n")
                start = len(shell.plain())
                shell.send("/usage\r")
                shown(shell, "tokens n/a", start)
            else:
                shell.send("? produce one unreported usage response\r")
                wait_until(shell, lambda: tally.exists() and len(tally.read_text().splitlines()) == 1,
                           "unreported usage checkpoint", 12)
            start = len(shell.plain())
            shell.send("? this request must not reach the provider\r")
            shown(shell, "recorded usage coverage is unknown", start, 8)
            shell.drain(.2)
            assert len(fixture.loopback.calls) == (0 if legacy else 1), fixture.loopback.calls
            start = len(shell.plain())
            shell.send("/status\r")
            shown(shell, "incomplete usage blocks further AI work", start, 8)
            capture(shell, "Native budget rejects unknown " + ("legacy" if legacy else "provider") + " usage")
            fixture.loopback.assert_ok()
        finally:
            fixture.close()
    print("  ok   native session budgets reject missing and legacy usage before another HTTP request; local status works", flush=True)


def main():
    scenarios = [question_inbox_answer_preserves_the_editing_draft,
                 action_approval_is_specific_and_defaults_to_no,
                 narrow_exact_action_review_is_complete_and_defaults_to_leave,
                 recorded_results_show_checks_and_unresolved_work,
                 live_followup_shows_queued_then_received,
                 naming_pinning_archive_and_persistent_review_are_unobtrusive,
                 native_prompt_and_session_usage_preserve_provider_coverage,
                 native_session_budget_blocks_missing_or_legacy_usage_before_more_http]
    failed = []
    for scenario in scenarios:
        print("  run  " + scenario.__name__, flush=True)
        try:
            scenario()
        except Exception:
            failed.append(scenario.__name__)
            traceback.print_exc()
            print("  FAIL " + scenario.__name__, flush=True)
    if output := os.environ.get("AISHE_TEST_CAPTURE_PATH"):
        from background_tasks_pty import CAPTURES
        with open(BINARY, "rb") as binary_file:
            digest = hashlib.file_digest(binary_file, "sha256").hexdigest()
        version = subprocess.run([BINARY, "--version"], capture_output=True, text=True,
                                 check=True, timeout=3).stdout.strip()
        Path(output).write_text(json.dumps({"schema_version": 1,
                                          "binary": {"path": BINARY, "version": version, "sha256": digest},
                                          "captures": CAPTURES}, indent=2))
    if failed:
        raise SystemExit(f"FAIL: {len(failed)}/{len(scenarios)} task interaction UI scenarios: {', '.join(failed)}")
    print(f"PASS: {len(scenarios)}/{len(scenarios)} task interaction UI scenarios", flush=True)


if __name__ == "__main__":
    main()
