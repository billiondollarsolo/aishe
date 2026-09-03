#!/usr/bin/env python3
"""End-to-end scenario tests for the zsh-PTY front-end.

Drives the real `aishe zsh` wrapper through a pseudo-terminal and exercises the
classes of edge cases that have bitten interactive use: NL routing, the `?`/`#`
force-NL sigil (including a trailing `?` that zsh would otherwise glob), questions
whose first word is a real command, auto-mode never eval'ing a non-command, and
up-arrow history. The model is replaced by a deterministic fake (AISHE_FAKE_LLM),
so every scenario is reproducible with no network and no API key.

A scenario fails if its expected marker does not appear, or if any "forbidden"
string (a parse error, a glob error, an eval error) shows up anywhere.

Usage: pty_scenarios.py [path-to-aishe]   (defaults to target/release/aishe)
Exit 0 on success, non-zero on the first failure. Skips if zsh is absent.
"""

import os
import sys
import pty
import re
import time
import select
import signal
import shutil
import tempfile
import subprocess

from harness_identity import require_current_binary

BINARY = require_current_binary(
    sys.argv[1] if len(sys.argv) > 1 else "target/release/aishe"
)
TIMEOUT = 30.0

# Strings that must never appear in the transcript: they mean a non-command was
# sent to the shell, or the sigil/routing leaked into zsh's parser/globber.
FORBIDDEN = [
    "parse error",
    "(eval):",
    "no matches found",
    "command not found: #",
    "command not found: ?",
]

SGR = re.compile(r"\x1b\[[0-9;]*m")
CSI = re.compile(r"\x1b\[[0-9;?]*[ -/]*[@-~]")
TERMINAL_MODE = re.compile(r"\x1b[=>]")


def submitted_line_remains_visible(segment, line):
    """The final ZLE redraw before accept must contain the submitted NL line.

    Raw PTY output contains every intermediate edit, including text that ZLE
    later erases. Strip color and inspect the display immediately before ZLE
    disables bracketed paste: the submitted line must be the final rendered
    content, not an earlier terminal echo followed by an erase-to-spaces redraw.
    """
    plain = SGR.sub("", segment)
    before_accept = plain.rsplit("\x1b[?2004l", 1)[0]
    if line not in before_accept:
        return False
    after_final_copy = before_accept.rsplit(line, 1)[1]
    after_final_copy = TERMINAL_MODE.sub("", CSI.sub("", after_final_copy))
    after_final_copy = after_final_copy.replace("\r", "").replace("\n", "")
    return after_final_copy == ""


class Pty:
    def __init__(self, argv, env):
        self.master, slave = pty.openpty()
        self.proc = subprocess.Popen(
            argv, stdin=slave, stdout=slave, stderr=slave,
            env=env, preexec_fn=os.setsid, close_fds=True,
        )
        os.close(slave)
        self.buf = ""          # unconsumed output (for expect)
        self.transcript = ""   # everything ever seen (for forbidden-string checks)

    def _drain(self, deadline):
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            return False
        r, _, _ = select.select([self.master], [], [], min(remaining, 0.2))
        if not r:
            return True
        try:
            chunk = os.read(self.master, 4096)
        except OSError:
            return False
        if not chunk:
            return False
        text = chunk.decode("utf-8", "replace")
        self.buf += text
        self.transcript += text
        return True

    def expect(self, needle, timeout=TIMEOUT):
        deadline = time.monotonic() + timeout
        while True:
            idx = self.buf.find(needle)
            if idx != -1:
                self.buf = self.buf[idx + len(needle):]
                return True
            if not self._drain(deadline):
                return False

    def send(self, line):
        os.write(self.master, (line + "\r").encode("utf-8"))

    def expect_prompt(self, timeout=TIMEOUT):
        """Wait until the prompt is visible and ZLE has entered input mode."""
        return self.expect("ZP> ", timeout) and self.expect("\x1b[?2004h", timeout)

    def wait_ready(self, timeout=20):
        """Block until zsh's line editor is really accepting input.

        The prompt appearing is not enough: ZLE enables bracketed paste after
        printing it, and input typed in that window arrives mangled (`echo` as
        `ccho`) on a slow runner, which then reads as a shell-wrapper bug. Send
        a marker through a full round trip first.
        """
        marker = "PTY_READY_MARKER"
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            self.send("print -r -- %s" % marker)
            if self.expect(marker, timeout=2) and self.expect(marker, timeout=2):
                return True
        return False

    def reset_editor(self, timeout=20):
        """Clear the current ZLE buffer and prove a fresh prompt accepts input."""
        self.buf = ""
        self.raw(b"\x03")
        return self.wait_ready(timeout) and self.expect_prompt(timeout)

    def raw(self, data):
        os.write(self.master, data)

    def settle(self, seconds=1.0):
        deadline = time.monotonic() + seconds
        while self._drain(deadline) and time.monotonic() < deadline:
            pass

    def close(self):
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


def make_env(binary):
    home = tempfile.mkdtemp(prefix="aishe-scen-")
    cfgdir = os.path.join(home, ".config", "aishe")
    os.makedirs(cfgdir, exist_ok=True)
    with open(os.path.join(cfgdir, "config.toml"), "w") as f:
        f.write(
            "[aishe]\n"
            'mode = "auto"\n'              # auto: exercises the eval path
            'provider = "anthropic"\n'
            'front_end = "zsh-pty"\n'
            "pty_prompt = false\n"         # use the plain zsh prompt for stable matching
            "status_line = false\n"        # and no right-prompt status either
            '\n[backend]\n'
            'engine = "native"\n'
            'default_scope = "host"\n'
            '\n[sandbox]\n'
            'allow_host_yolo = true\n'
        )
    # A minimal, deterministic zsh config with working history.
    with open(os.path.join(home, ".zshrc"), "w") as f:
        f.write(
            "HISTFILE=~/.zsh_history\nHISTSIZE=1000\nSAVEHIST=1000\n"
            "setopt INC_APPEND_HISTORY\nPROMPT='ZP> '\n"
        )
    # aishe must be callable as `aishe` inside the wrapped zsh.
    bindir = os.path.join(home, "bin")
    os.makedirs(bindir, exist_ok=True)
    os.symlink(os.path.abspath(binary), os.path.join(bindir, "aishe"))
    env = dict(os.environ)
    env.update({
        "HOME": home,
        "XDG_CONFIG_HOME": os.path.join(home, ".config"),
        # macOS ignores XDG_*; these are honored on every platform.
        "AISHE_CONFIG_DIR": os.path.join(home, ".config"),
        "AISHE_DATA_DIR": os.path.join(home, ".local", "share"),
        "XDG_DATA_HOME": os.path.join(home, ".local", "share"),
        "ZDOTDIR": home,
        # GitHub runners ship group-writable zsh completion dirs, so compinit
        # stops with an interactive "insecure directories" prompt that swallows
        # a keystroke and desynchronises every later expect().
        "ZSH_DISABLE_COMPFIX": "true",
        "TERM": "xterm-256color",
        "PATH": bindir + ":" + os.environ.get("PATH", ""),
        "ANTHROPIC_API_KEY": "",
        "OPENAI_API_KEY": "",
    })
    return home, env


PASSED = []
FAKE_GENERATION = 0


def check(sh, name, ok):
    if ok:
        PASSED.append(name)
        sys.stdout.write("  ok   %s\n" % name)
    else:
        sys.stderr.write("\nFAIL: %s\n---- recent output ----\n%s\n-----------------------\n"
                         % (name, sh.transcript[-2500:]))
        sh.close()
        sys.exit(1)


def set_fake(sh, payload):
    # Export the canned model response into the wrapped zsh; the per-call
    # `aishe --auto-line` inherits it. Single-quote-safe payloads only.
    #
    # A fixed sleep here races a slow runner. Keep the assignment, comparison,
    # and acknowledgement in one command so the second marker can only appear
    # after zsh has both exported the exact payload and verified its value.
    # A generation suffix prevents stale terminal output from satisfying a
    # later handshake.
    global FAKE_GENERATION
    for _ in range(3):
        FAKE_GENERATION += 1
        marker = "FAKE_SET_MARKER_%d" % FAKE_GENERATION
        sh.send(
            "export AISHE_FAKE_LLM='%s'; "
            "[[ \"$AISHE_FAKE_LLM\" == '%s' ]] && "
            "print -r -- 'FAKE_SET_''MARKER_%d'"
            % (payload, payload, FAKE_GENERATION)
        )
        # The full marker is deliberately split by shell quotes in the typed
        # command. It can therefore occur only in command output, never in one
        # or more ZLE redraws of the input line.
        if sh.expect(marker, timeout=3):
            sh.buf = ""  # drop anything trailing so the next expect() is clean
            return
        # Input can arrive in the narrow redraw window after a foreground
        # `aishe` child exits. Reset ZLE and retry with a fresh marker.
        sh.raw(b"\x03")
        sh.expect_prompt(timeout=3)
    raise RuntimeError(
        "failed to install deterministic fake provider\n" + sh.transcript[-2500:]
    )


def answer(text):
    return '{"type":"answer","explanation":"%s"}' % text


def command(cmd, expl="does a thing"):
    return '{"type":"command","command":"%s","explanation":"%s"}' % (cmd, expl)


def main():
    if shutil.which("zsh") is None:
        sys.stderr.write("SKIP: zsh not on PATH\n")
        sys.exit(0)
    if not os.path.exists(BINARY):
        sys.stderr.write("FAIL: binary not found: %s\n" % BINARY)
        sys.exit(1)

    home, env = make_env(BINARY)
    sh = Pty([os.path.abspath(BINARY), "zsh"], env)
    try:
        sh.expect_prompt()  # first prompt and live line editor
        sh.wait_ready()

        # A user's existing zsh/Oh My Zsh history configuration must win over
        # aishe's fallback.
        sh.send("print -r -- USERHIST=$HISTFILE")
        check(
            sh,
            "user-configured HISTFILE is preserved",
            sh.expect("USERHIST=%s" % os.path.join(home, ".zsh_history")),
        )
        sh.send("print -r -- HISTMANAGED=${AISHE_MANAGED_HISTFILE:-0}")
        check(sh, "user history is not marked aishe-managed", sh.expect("HISTMANAGED=0"))

        # 1. A plain shell command runs normally.
        sh.send("echo SCEN1_$((20 + 22))")
        check(sh, "plain command runs", sh.expect("SCEN1_42"))

        # A minimal account has no syntax-highlighting plugin. Aishe supplies a
        # narrow fallback that marks a recognized first command word green.
        sh.send(
            "_aishe_test_highlight_dump() { "
            'zle -M "AISHE_RH=${(j:|:)region_highlight}"; }; '
            "zle -N _aishe_test_highlight_dump; "
            "bindkey '^X^H' _aishe_test_highlight_dump"
        )
        sh.expect_prompt()
        sh.buf = ""
        sh.raw(b"echo")
        sh.settle(0.3)
        sh.raw(b"\x18\x08")  # Ctrl-X Ctrl-H: dump region_highlight via zle -M
        check(
            sh,
            "recognized command is highlighted green",
            sh.expect("AISHE_RH=0 4 fg=green"),
        )
        if not sh.reset_editor():
            raise RuntimeError("line editor did not recover after highlight probe")

        # The first token can be a real command and initially green, then the
        # completed buffer can become a natural-language question. Define the
        # three collision names on every platform so the test does not depend
        # on whether the host happens to ship /usr/bin/what or a `where` helper.
        sh.send(
            "what() { return 77; }; "
            "where() { return 77; }; "
            "who() { return 77; }"
        )
        sh.expect_prompt()
        sh.buf = ""
        sh.raw(b"what")
        sh.settle(0.3)
        sh.raw(b"\x18\x08")
        check(
            sh,
            "ambiguous bare command starts green",
            sh.expect("AISHE_RH=0 4 fg=green"),
        )
        if not sh.reset_editor():
            raise RuntimeError("line editor did not recover after bare-command probe")

        for line, name in [
            ("what is the capital of France", "what-question turns AI-colored"),
            ("where is the ssh config", "where-question turns AI-colored"),
            ("who am i logged in as", "who-question turns AI-colored"),
        ]:
            sh.buf = ""
            sh.raw(line.encode("utf-8"))
            sh.settle(0.3)
            sh.raw(b"\x18\x08")
            check(
                sh,
                name,
                sh.expect("AISHE_RH=0 %d fg=magenta" % len(line)),
            )
            if not sh.reset_editor():
                raise RuntimeError("line editor did not recover after question probe")

        # Color is supplemental. The on-demand widget exposes the exact same
        # predicate as bounded text without taking over POSTDISPLAY or prompts.
        sh.buf = ""
        sh.raw(b"what is the active route")
        sh.raw(b"\x18?")
        check(
            sh,
            "agent route has a non-color text cue",
            sh.expect("aishe route: agent"),
        )
        if not sh.reset_editor():
            raise RuntimeError("line editor did not recover after agent route cue")
        sh.buf = ""
        sh.raw(b"echo route")
        sh.raw(b"\x18?")
        check(
            sh,
            "shell route has a non-color text cue",
            sh.expect("aishe route: shell/local"),
        )
        if not sh.reset_editor():
            raise RuntimeError("line editor did not recover after shell route cue")

        # Bare/ordinary commands remain shell-colored even after the collision
        # grammar is enabled.
        sh.buf = ""
        sh.raw(b"who")
        sh.settle(0.3)
        sh.raw(b"\x18\x08")
        check(sh, "bare who remains a shell command", sh.expect("AISHE_RH=0 3 fg=green"))
        if not sh.reset_editor():
            raise RuntimeError("line editor did not recover after who probe")
        sh.buf = ""
        sh.raw(b"ls -la")
        sh.settle(0.3)
        sh.raw(b"\x18\x08")
        check(sh, "Linux command with arguments stays green", sh.expect("AISHE_RH=0 2 fg=green"))
        if not sh.reset_editor():
            raise RuntimeError("line editor did not recover after command probe")

        # Suggest mode is a two-step native editing contract. The first Enter
        # submits natural language and stages the proposed command in BUFFER;
        # it must not execute until a second Enter. While staged, ordinary ZLE
        # highlighting, cursor editing, and history behavior remain available.
        sh.send("export AISHE_MODE=suggest")
        sh.expect_prompt()
        staged_a = os.path.join(home, "suggestion-stage-A")
        staged_b = os.path.join(home, "suggestion-stage-B")
        staged_command = "touch %s" % staged_a
        set_fake(sh, command(staged_command, "stage for native editing"))
        sh.expect_prompt()
        sh.buf = ""
        sh.send("? prepare the staged file command")
        check(
            sh,
            "suggest first Enter stages a native zsh buffer",
            sh.expect(staged_command),
        )
        sh.settle(0.3)
        check(
            sh,
            "staged suggestion does not execute on first Enter",
            not os.path.exists(staged_a) and not os.path.exists(staged_b),
        )
        sh.buf = ""
        sh.raw(b"\x18\x08")  # existing route-highlight dump widget
        check(
            sh,
            "staged suggestion keeps native route highlighting",
            sh.expect("AISHE_RH=0 5 fg=green"),
        )
        # Cursor remains at the end of the staged BUFFER. Edit A -> B with the
        # native line editor, then use the second Enter to execute it.
        sh.raw(b"\x7fB\r")
        check(sh, "edited staged command returns to a prompt", sh.expect_prompt())
        check(
            sh,
            "second Enter executes only the edited suggestion",
            os.path.exists(staged_b) and not os.path.exists(staged_a),
        )
        sh.buf = ""
        sh.raw(b"\x1b[A")
        check(
            sh,
            "edited staged command enters native zsh history",
            # Route highlighting inserts SGR bytes inside the command head;
            # the exact edited path remains contiguous and distinguishes A/B.
            sh.expect(staged_b),
        )
        if not sh.reset_editor():
            raise RuntimeError("line editor did not recover after staged history probe")

        # Native completion remains active because the proposal is real BUFFER
        # state, not plain printed text. Complete a deliberately partial
        # function name, then execute the completed function on Enter.
        completion_name = "aishe-stage-completion-target"
        completion_partial = "aishe-stage-completion-targ"
        sh.send(
            "%s() { print -r -- STAGED_COMPLETION_OK; }; "
            "autoload -Uz compinit; compinit -D; print -r -- COMPLETION_READY"
            % completion_name
        )
        check(sh, "native completion fixture is ready", sh.expect("COMPLETION_READY"))
        sh.expect_prompt()
        set_fake(sh, command(completion_partial, "complete this staged command"))
        sh.expect_prompt()
        sh.buf = ""
        sh.send("? prepare a command for completion")
        check(
            sh,
            "partial suggestion is staged before completion",
            sh.expect(completion_partial),
        )
        sh.buf = ""
        sh.raw(b"\t")
        sh.settle(0.4)
        sh.raw(b"\r")
        check(
            sh,
            "native completion edits and executes a staged suggestion",
            sh.expect("STAGED_COMPLETION_OK"),
        )
        sh.expect_prompt()

        # A staged proposal canceled with Ctrl-C never executes and never enters
        # zsh history. This is the safe review path for an unwanted suggestion.
        canceled_path = os.path.join(home, "suggestion-canceled")
        canceled_command = "touch %s" % canceled_path
        set_fake(sh, command(canceled_command, "cancel this staged command"))
        sh.expect_prompt()
        sh.buf = ""
        sh.send("? prepare a command I will cancel")
        check(
            sh,
            "cancel scenario stages before Ctrl-C",
            sh.expect(canceled_command),
        )
        cancel_start = len(sh.transcript)
        sh.raw(b"\x03")
        check(sh, "Ctrl-C cancels a staged suggestion", sh.expect_prompt())
        sh.settle(0.3)
        check(
            sh,
            "staged Ctrl-C does not emit a failure hint",
            "aishe: exit" not in sh.transcript[cancel_start:],
        )
        history_text = ""
        history_path = os.path.join(home, ".zsh_history")
        if os.path.exists(history_path):
            with open(history_path, encoding="utf-8", errors="replace") as history:
                history_text = history.read()
        history_commands = []
        for history_line in history_text.splitlines():
            if history_line.startswith(": ") and ";" in history_line:
                history_line = history_line.split(";", 1)[1]
            history_commands.append(history_line)
        check(
            sh,
            "canceled suggestion is neither executed nor recorded",
            not os.path.exists(canceled_path)
            and canceled_command not in history_commands,
        )
        if not sh.wait_ready() or not sh.expect_prompt():
            raise RuntimeError("line editor did not recover after staged Ctrl-C")

        # The remaining scenarios intentionally exercise auto execution.
        sh.send("export AISHE_MODE=auto")
        sh.expect_prompt()

        # Enter uses the same grammar as highlighting. This is the regression
        # assertion: a valid `what` command must not run once the full line is a
        # recognizable question.
        set_fake(sh, answer("ANSWER_COLLISION_42"))
        collision_line = "what is the capital of France"
        collision_start = len(sh.transcript)
        sh.send(collision_line)
        check(
            sh,
            "question collision routes consistently with its color",
            sh.expect("ANSWER_COLLISION_42"),
        )
        check(
            sh,
            "auto-routed question remains visible after Enter",
            submitted_line_remains_visible(
                sh.transcript[collision_start:], collision_line
            ),
        )

        # Keep later assertions about literal terminal echoes independent of
        # ANSI color sequences; the fallback itself has now been verified. Wait
        # for Ctrl-C to finish before sending the assignment, and put the marker
        # in the same command so observing it proves the assignment ran.
        sh.expect_prompt()
        sh.send("export AISHE_COMMAND_HIGHLIGHT=0; print -r -- HIGHLIGHT_TEST_DONE")
        sh.expect("HIGHLIGHT_TEST_DONE")
        sh.expect("HIGHLIGHT_TEST_DONE")

        # 2. `?` sigil routes a question to the AI (and the trailing text with a
        #    glob char does not reach zsh's globber).
        set_fake(sh, answer("ANSWER_MOON_42"))
        sigil_line = "? what is the moon?"
        sigil_start = len(sh.transcript)
        sh.send(sigil_line)
        check(sh, "? sigil routes question (trailing ?)", sh.expect("ANSWER_MOON_42"))
        check(
            sh,
            "? sigil request remains visible after Enter",
            submitted_line_remains_visible(sh.transcript[sigil_start:], sigil_line),
        )

        # Alt-Enter forces even a valid shell-looking line through Aishe. It
        # uses a separate ZLE widget and must preserve the submitted line too.
        sh.expect_prompt()
        set_fake(sh, answer("ANSWER_FORCE_NL_42"))
        sh.expect_prompt()
        forced_line = "echo this is a natural-language request"
        forced_start = len(sh.transcript)
        sh.raw(forced_line.encode("utf-8"))
        sh.raw(b"\x1b\r")
        check(
            sh,
            "Alt-Enter forces a valid-command-shaped request",
            sh.expect("ANSWER_FORCE_NL_42"),
        )
        check(
            sh,
            "Alt-Enter request remains visible after submission",
            submitted_line_remains_visible(sh.transcript[forced_start:], forced_line),
        )

        # 3. `#` sigil, also with a trailing `?`.
        set_fake(sh, answer("ANSWER_HASH_42"))
        sh.send("# is the theme enabled?")
        check(sh, "# sigil routes question (trailing ?)", sh.expect("ANSWER_HASH_42"))

        # 4. A question whose first word is a real command (`who`) still reaches
        #    the AI when forced with the sigil.
        set_fake(sh, answer("ANSWER_PRES_42"))
        sh.send("? who is the president")
        check(sh, "sigil beats command-name collision", sh.expect("ANSWER_PRES_42"))

        # 5. A natural-language line ending in `?` must be intercepted before
        #    zsh's NOMATCH option treats the punctuation as an unmatched glob.
        set_fake(sh, answer("ANSWER_UNSIGILED_42"))
        sh.send("cna you add a new admin using the same ssh key as root?")
        check(
            sh,
            "unknown question bypasses zsh NOMATCH",
            sh.expect("ANSWER_UNSIGILED_42"),
        )

        # A real command keeps native glob behavior; the pre-route is deliberately
        # limited to an unknown first word.
        native_match = os.path.join(home, "native1")
        sh.send("touch %s" % native_match)
        sh.expect_prompt()
        set_fake(sh, answer("MUST_NOT_ROUTE_42"))
        sh.send("print -r -- %s" % os.path.join(home, "native?"))
        check(sh, "real commands keep native glob expansion", sh.expect(native_match))

        # Shell syntax that happens to end in the special `$?` parameter is not
        # an English question and must remain in zsh.
        sh.send("false; print -r -- STATUS_$?")
        check(sh, "trailing shell status parameter stays native", sh.expect("STATUS_1"))

        # 6. THE reported bug: in auto mode the model answers a question with a
        #    malformed "command". It must be surfaced as an answer, never eval'd.
        set_fake(sh, command("the sun is a star > ", "PROSE_SUN_42"))
        sh.send("? tell me about the sun")
        check(sh, "auto mode shows prose, not a parse error", sh.expect("PROSE_SUN_42"))

        # 7. Auto mode runs a *valid* safe command the model returns.
        set_fake(sh, command("echo RANCMD_42", "prints a marker"))
        sh.send("? print the marker")
        check(sh, "auto mode runs a valid command", sh.expect("RANCMD_42"))

        # 8. Up-arrow recalls the previous real command.
        sh.send("export AISHE_FAKE_LLM=")  # stop faking; back to plain shell
        sh.settle(0.3)
        sh.send("echo HISTMARK_42")
        sh.expect("HISTMARK_42")
        sh.buf = ""
        sh.raw(b"\x1b[A")  # up arrow
        check(sh, "up-arrow recalls previous command", sh.expect("echo HISTMARK_42"))
        if not sh.reset_editor():  # Ctrl-C to clear the recalled line
            raise RuntimeError("line editor did not recover after history probe")

        # 9. Shift-Tab cycles the interaction mode for the session (the config
        #    starts in auto, so one press lands on yolo). Entering yolo requires
        #    one explicit scope acceptance per shell; use host scope so this
        #    terminal-I/O contract does not depend on whether the CI runner's
        #    kernel permits unprivileged bubblewrap namespaces. After acceptance,
        #    the widget reports the new mode via `zle -M`.
        sh.settle(0.3)
        sh.buf = ""
        sh.raw(b"\x1b[Z")  # Shift-Tab
        check(
            sh,
            "Shift-Tab requests one yolo-host scope acceptance",
            sh.expect("Type yolo-host to continue:"),
        )
        sh.send("yolo-host")
        check(
            sh,
            "yolo-host acceptance phrase is visible, not secret input",
            sh.expect("yolo-host\r\n"),
        )
        # With the prompt function present the cycle refreshes the prompt rather
        # than printing a message, so assert the shell state itself. This must
        # run before reset_editor: its Ctrl-C races the widget.
        mode_start = len(sh.transcript)
        sh.send("print -r -- MODE_''NOW=$AISHE_MODE")
        sh.expect("MODE_NOW=")
        check(
            sh,
            "Shift-Tab cycles the mode",
            "MODE_NOW=yolo" in SGR.sub("", sh.transcript[mode_start:]),
        )
        check(
            sh,
            "mode switch returns to a ready prompt",
            sh.reset_editor(),
        )

        # Primary slash commands are handled locally and never sent to the model.
        sh.send("/help")
        check(
            sh,
            "/help exposes the primary command surface",
            sh.expect("AIShe"),
        )
        check(sh, "/help includes /connection", sh.expect("/connection"))
        check(sh, "/help includes live status", sh.expect("/status"))
        sh.expect_prompt()
        sh.send("/status")
        check(sh, "/status is available at the prompt", sh.expect("AIShe status"))
        check(sh, "/status shows output density", sh.expect("output: focus"))
        sh.expect_prompt()

        # 10. Ctrl-O switches to the detailed agent transcript for this shell
        #     without changing the persistent preference.
        sh.raw(b"\x0f")
        check(
            sh,
            "Ctrl-O changes agent detail for the current shell",
            sh.expect("details: compact (this shell)"),
        )
        sh.raw(b"\x0f")
        check(
            sh,
            "Ctrl-O keeps cycling the density",
            sh.expect("details: detailed (this shell)"),
        )
        sh.raw(b"\x0f")
        check(
            sh,
            "Ctrl-O returns to focus output",
            sh.expect("details: focus (this shell)"),
        )
        # The detail toggle repaints ZLE asynchronously. Discard its prior
        # prompt bytes, interrupt the empty line, then prove a fresh command can
        # complete before typing the persistent-setting command. This avoids a
        # slow remote runner matching a stale prompt and losing the first byte.
        check(
            sh,
            "Ctrl-O toggle leaves the line editor ready",
            sh.reset_editor(),
        )
        sh.send("aishe output detailed")
        check(
            sh,
            "persistent output setting is saved",
            sh.expect("output = detailed"),
        )
        sh.expect_prompt()
        sh.send("print -r -- OUTPUT_MODE=$AISHE_AGENT_OUTPUT")
        check(
            sh,
            "persistent output setting reaches the current shell",
            sh.expect("OUTPUT_MODE=detailed"),
        )
        sh.expect_prompt()
        sh.send("aishe output focus")
        sh.expect("output = focus")

        # 11. Fix-the-last-command: after a real command fails, Ctrl-X Ctrl-F asks
        #    the model for a correction and pre-fills it on the line (never runs
        #    it). `false` is a real command that exits non-zero (an *unknown*
        #    command would route to the AI instead, so use a real failing one).
        sh.send("")  # land on a clean, fresh prompt after the prior raw keys
        sh.settle(0.3)
        sh.buf = ""
        set_fake(sh, command("echo FIXED_SCEN9_42", "corrected"))
        hint_start = len(sh.transcript)
        sh.send("false")
        check(
            sh,
            "failed command prints one recovery hint",
            sh.expect("aishe: exit 1"),
        )
        sh.settle(0.4)
        hint_segment = sh.transcript[hint_start:]
        check(
            sh,
            "failure hint is not repeated on prompt redraw",
            hint_segment.count("aishe: exit 1") == 1,
        )
        sh.buf = ""
        sh.raw(b"\x18\x06")  # Ctrl-X Ctrl-F
        check(sh, "fix key pre-fills a correction", sh.expect("echo FIXED_SCEN9_42"))
        if not sh.reset_editor():  # clear the pre-filled line without running it
            raise RuntimeError("line editor did not recover after fix probe")

        # Success, Ctrl-C, and an explicit opt-out must remain quiet.
        sh.send("export AISHE_FAILURE_HINTS=0")
        quiet_start = len(sh.transcript)
        sh.send("false")
        sh.send("print -r -- HINTS_DISABLED_DONE")
        check(sh, "disabled failure hints stay quiet", sh.expect("HINTS_DISABLED_DONE"))
        sh.settle(0.3)
        check(
            sh,
            "disabled failure produced no recovery hint",
            "aishe: exit 1" not in sh.transcript[quiet_start:],
        )
        sh.send("export AISHE_FAILURE_HINTS=1")

        success_start = len(sh.transcript)
        sh.send("true")
        sh.send("print -r -- SUCCESS_HINT_DONE")
        check(sh, "successful command stays hint-free", sh.expect("SUCCESS_HINT_DONE"))
        sh.settle(0.3)
        check(
            sh,
            "success produced no recovery hint",
            "aishe: exit" not in sh.transcript[success_start:],
        )

        interrupt_start = len(sh.transcript)
        sh.send("sleep 10")
        sh.settle(0.3)
        if not sh.reset_editor():
            raise RuntimeError("line editor did not recover after Ctrl-C")
        sh.send("print -r -- INTERRUPT_HINT_DONE")
        check(sh, "Ctrl-C returns to the prompt", sh.expect("INTERRUPT_HINT_DONE"))
        sh.settle(0.3)
        check(
            sh,
            "Ctrl-C produced no recovery hint",
            "aishe: exit 130" not in sh.transcript[interrupt_start:],
        )

        # 10. Per-session usage summary: with token recording on (AISHE_FAKE_USAGE),
        #     an NL call tallies its tokens, and the PTY prints a one-line cost
        #     summary to its own stderr when zsh exits. Use auto mode so the call
        #     *executes* (no line-editor pre-fill to clean up before exit).
        sh.send("")  # land on a clean prompt
        sh.settle(0.3)
        sh.send("export AISHE_MODE=auto")
        sh.send("export AISHE_FAKE_USAGE='123,45'")
        sh.settle(0.3)
        set_fake(sh, command("echo USAGE_SCEN_42", "marker"))
        sh.send("? give me the usage marker")  # `?` forces NL; auto runs + records usage
        check(sh, "usage-scenario NL call ran", sh.expect("USAGE_SCEN_42"))
        sh.send("export AISHE_FAKE_LLM=")  # stop faking so `exit` can't route to NL
        sh.settle(0.3)

        # Global invariant: no parse/glob/eval errors anywhere in the session.
        leaked = [s for s in FORBIDDEN if s in sh.transcript]
        check(sh, "no parse/glob/eval errors leaked", not leaked)
        if leaked:
            sys.stderr.write("leaked: %r\n" % leaked)

        sh.send("exit")
        # The parent prints the post-session summary to its own stderr after zsh
        # exits; wait for it (draining) rather than a fixed settle, which can miss
        # it under load.
        check(sh, "session usage summary printed on exit",
              sh.expect("aishe session:", timeout=8))
        sys.stdout.write("\nAll %d scenarios passed.\n" % len(PASSED))
    finally:
        sh.close()
        shutil.rmtree(home, ignore_errors=True)


if __name__ == "__main__":
    main()
