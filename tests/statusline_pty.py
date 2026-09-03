#!/usr/bin/env python3
"""Real-zsh PTY tests for the native right-prompt/off live status."""

import fcntl
import os
import pty
import re
import select
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import time

from harness_identity import require_current_binary

BINARY = require_current_binary(
    os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else "target/release/aishe")
)
SGR = re.compile(r"\x1b\[[0-9;]*m")
CSI = re.compile(r"\x1b\[[0-9;?]*[ -/]*[@-~]")
TERMINAL_MODE = re.compile(r"\x1b[=>]")


def submitted_line_remains_visible(segment, line):
    plain = SGR.sub("", segment)
    before_accept = plain.rsplit("\x1b[?2004l", 1)[0]
    if line not in before_accept:
        return False
    after_final_copy = before_accept.rsplit(line, 1)[1]
    after_final_copy = TERMINAL_MODE.sub("", CSI.sub("", after_final_copy))
    after_final_copy = after_final_copy.replace("\r", "").replace("\n", "")
    return after_final_copy == ""


class Pty:
    def __init__(self, env, cols):
        self.master, slave = pty.openpty()
        fcntl.ioctl(
            self.master,
            termios.TIOCSWINSZ,
            struct.pack("HHHH", 24, cols, 0, 0),
        )
        self.proc = subprocess.Popen(
            [BINARY, "zsh"],
            stdin=slave,
            stdout=slave,
            stderr=slave,
            env=env,
            preexec_fn=os.setsid,
            close_fds=True,
        )
        os.close(slave)
        self.transcript = ""

    def drain(self, seconds=0.2):
        end = time.monotonic() + seconds
        while time.monotonic() < end:
            ready, _, _ = select.select([self.master], [], [], 0.1)
            if not ready:
                continue
            try:
                chunk = os.read(self.master, 8192)
            except OSError:
                return
            if not chunk:
                return
            self.transcript += chunk.decode("utf-8", "replace")

    def expect(self, text, timeout=20):
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            if text in self.transcript:
                return True
            self.drain()
        return text in self.transcript

    def send(self, line):
        os.write(self.master, (line + "\r").encode())

    def ready(self):
        marker = "STATUS_PTY_READY"
        # Keep the exact marker out of the input line: a PTY normally echoes
        # typed input before zsh is ready, and treating that echo as command
        # output makes this check race on a loaded host.
        self.send("print -r -- STATUS_PTY_''READY")
        return self.expect(marker)

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


def environment(position):
    home = tempfile.mkdtemp(prefix="aishe-status-")
    config_dir = os.path.join(home, ".config", "aishe")
    os.makedirs(config_dir)
    enabled = "false" if position == "off" else "true"
    model = "status-model-" + position
    with open(os.path.join(config_dir, "config.toml"), "w", encoding="utf-8") as file:
        file.write(
            "version = 2\n"
            "[aishe]\n"
            'mode = "auto"\n'
            'provider = "anthropic"\n'
            "pty_prompt = true\n"
            "show_usage = false\n"
            "status_line = %s\n"
            'status_line_position = "%s"\n'
            'status_line_items = ["model", "mode", "scope", "session_tokens", '
            '"requests", "plan"]\n\n'
            "[providers.anthropic]\n"
            'base_url = "https://api.anthropic.com"\n'
            'api_key_env = "UNUSED_FAKE_KEY"\n'
            'model = "%s"\n\n'
            '[pricing."%s"]\n'
            "input = 1.0\n"
            "output = 2.0\n\n"
            "[backend]\n"
            'engine = "native"\n'
            % (enabled, position, model, model)
        )
    with open(os.path.join(home, ".zshrc"), "w", encoding="utf-8") as file:
        file.write("unset HISTFILE\nPROMPT='USER_PROMPT> '\n")
    bin_dir = os.path.join(home, "bin")
    os.makedirs(bin_dir)
    os.symlink(BINARY, os.path.join(bin_dir, "aishe"))
    env = dict(os.environ)
    env.pop("NO_COLOR", None)
    env.update(
        {
            "HOME": home,
            "AISHE_CONFIG_DIR": os.path.join(home, ".config"),
            "AISHE_DATA_DIR": os.path.join(home, ".local", "share"),
            "XDG_CONFIG_HOME": os.path.join(home, ".config"),
            "XDG_DATA_HOME": os.path.join(home, ".local", "share"),
            "ZDOTDIR": home,
            "ZSH_DISABLE_COMPFIX": "true",
            "TERM": "xterm-256color",
            "PATH": bin_dir + ":" + os.environ.get("PATH", ""),
            "AISHE_FAKE_LLM": (
                '{"type":"command","command":"echo STATUS_CALL_OK",'
                '"explanation":"status test"}'
            ),
            "AISHE_FAKE_USAGE": "123,45",
        }
    )
    if position == "off":
        env["AISHE_UNICODE"] = "ascii"
    return home, env, model


def run_case(position, cols, expected_position=None):
    expected_position = expected_position or position
    home, env, model = environment(position)
    shell = Pty(env, cols)
    try:
        if not shell.ready():
            raise AssertionError("%s session never became ready" % position)
        shell.drain(0.5)
        if position == "off":
            if model in shell.transcript:
                raise AssertionError("off statusline rendered the model")
            if "▄▄▄" in shell.transcript or "AI Shell" not in shell.transcript:
                raise AssertionError("ASCII mode rendered a font-dependent logo")
            if not re.search(r"\x1b\[[^\n]*m>{1,2}\x1b", shell.transcript):
                raise AssertionError("ASCII mode did not render an ASCII prompt glyph")
        elif model not in shell.transcript or "auto" not in shell.transcript:
            raise AssertionError(
                "%s statusline did not render identity:\n%s"
                % (position, shell.transcript[-2500:])
            )
        elif "^[" in CSI.sub("", shell.transcript):
            raise AssertionError("statusline rendered visible ANSI escape text")
        elif "\x1b[33m" not in shell.transcript or "\x1b[36m" not in shell.transcript:
            raise AssertionError("statusline did not apply semantic model/mode colors")

        if position != "off":
            shell.send(
                "AISHE_STATUS_ITEMS=identity,model; aishe_set_prompt status-only; "
                "print -r -- BEGIN_''STATUS; print -r -- \"$_AISHE_STATUS_TEXT\"; "
                "print -r -- \"RPROMPT=$RPROMPT\"; print -r -- \"POSTDISPLAY=$POSTDISPLAY\"; "
                "print -r -- END_''STATUS; "
                "AISHE_STATUS_ITEMS=model,mode,scope,session_tokens,requests,plan"
            )
            if not shell.expect("END_STATUS"):
                raise AssertionError("could not inspect rendered status text")
            plain = CSI.sub("", shell.transcript).rsplit("BEGIN_STATUS", 1)[1]
            rendered = plain.split("END_STATUS", 1)[0]
            if rendered.count(model) != 1 or "Auto (legacy)" in rendered:
                raise AssertionError("composite identity repeated model/auth details")
            if "RPROMPT=" not in rendered or "%90v" not in rendered:
                raise AssertionError("status was not installed in native RPROMPT")
            if re.search(r"POSTDISPLAY=.*" + re.escape(model), rendered):
                raise AssertionError("status polluted plugin-owned POSTDISPLAY")

        shell.send("print -r -- POSITION=$AISHE_STATUS_POSITION")
        if not shell.expect("POSITION=" + expected_position):
            raise AssertionError(
                "%s resolved to the wrong placement" % position
            )
        submitted = "? print the status test marker"
        submitted_start = len(shell.transcript)
        shell.send(submitted)
        if not shell.expect("STATUS_CALL_OK", timeout=60):
            raise AssertionError("%s AI call did not complete" % position)
        if not submitted_line_remains_visible(
            shell.transcript[submitted_start:], submitted
        ):
            raise AssertionError(
                "%s statusline erased the submitted request:\n%s"
                % (position, shell.transcript[-2500:])
            )
        shell.drain(0.8)
        dynamic = ["session 123/45 tok", "1 req"]
        if position == "off":
            if any(value in shell.transcript for value in dynamic):
                raise AssertionError("off statusline rendered dynamic metrics")
        else:
            shell.send("print -r -- DYNAMIC=$_AISHE_STATUS_TEXT")
            if not all(shell.expect(value) for value in dynamic):
                raise AssertionError(
                    "%s statusline did not refresh metrics:\n%s"
                    % (position, shell.transcript[-2500:])
                )
            shell.send(
                "! print -r -- $'plan\\tweek 82% left' >> $AISHE_STATUS_FILE; "
                "export AISHE_AUTH_KIND=oauth; AISHE_STATUS_ITEMS+=,plan; aishe_set_prompt; "
                "[[ $_AISHE_STATUS_TEXT == *'week 82% left'* && "
                "$_AISHE_STATUS_TEXT != *'session '* && "
                "$_AISHE_STATUS_TEXT != *' req'* ]] && "
                "print -r -- QUOTA_FILTER_''OK"
            )
            if not shell.expect("QUOTA_FILTER_OK"):
                raise AssertionError(
                    "subscription quota did not replace token/request noise:\n%s"
                    % shell.transcript[-2500:]
                )
        shell.send("export AISHE_FAKE_LLM=; exit")
        shell.drain(1)
        print("  ok   statusline %s (%d columns)" % (position, cols))
    finally:
        shell.close()
        shutil.rmtree(home, ignore_errors=True)


def prompt_substitution_is_inert():
    home, env, original_model = environment("right")
    marker = os.path.join(home, "prompt-substitution-must-not-run")
    command_payload = "$(touch $HOME/prompt-substitution-must-not-run)"
    toml_payload = command_payload + "\\u001b[2Kspoof"
    rendered_payload = command_payload + "\\x1b[2Kspoof"
    config = os.path.join(home, ".config", "aishe", "config.toml")
    with open(config, encoding="utf-8") as file:
        text = file.read()
    with open(config, "w", encoding="utf-8") as file:
        file.write(text.replace(original_model, toml_payload))
    with open(os.path.join(home, ".zshrc"), "a", encoding="utf-8") as file:
        file.write("setopt PROMPT_SUBST\n")

    shell = Pty(env, 180)
    try:
        if not shell.ready():
            raise AssertionError("prompt-substitution session never became ready")
        shell.drain(0.5)
        if os.path.exists(marker):
            raise AssertionError(
                "PROMPT_SUBST executed model-name command substitution"
            )
        if rendered_payload not in shell.transcript:
            raise AssertionError(
                "model prompt payload was not rendered inert and control-safe:\n%s"
                % shell.transcript[-2500:]
            )
        print("  ok   model text is control-safe and inert with PROMPT_SUBST")
    finally:
        shell.close()
        shutil.rmtree(home, ignore_errors=True)


def theme_survives_mode_cycle():
    home, env, _ = environment("right")
    capture = os.path.join(home, "prompt-state")
    buffer_capture = os.path.join(home, "buffer-state")
    with open(os.path.join(home, ".zshrc"), "a", encoding="utf-8") as file:
        file.write(
            "PROMPT='THEME> '; RPROMPT='THEME-RIGHT'\n"
            "_original_tab_test() { BUFFER+='-tab'; CURSOR=${#BUFFER}; }\n"
            "zle -N _original_tab_test\n"
            "bindkey '^I' _original_tab_test\n"
            "autoload -Uz add-zle-hook-widget\n"
            "_fake_suggestion() { POSTDISPLAY='ghost'; }\n"
            "add-zle-hook-widget line-pre-redraw _fake_suggestion\n"
            "_capture_display() { { print -rn -- \"$POSTDISPLAY\"; "
            "print -rn -- $'\\n---\\n'; print -rn -- \"$RPROMPT\"; } > "
            + capture
            + "; }\n"
            "zle -N _capture_display\n"
            "bindkey '^X^T' _capture_display\n"
            "_capture_buffer() { print -rn -- \"$BUFFER\" > "
            + buffer_capture
            + "; }\n"
            "zle -N _capture_buffer\n"
            "bindkey '^X^B' _capture_buffer\n"
            "export AISHE_MODE=suggest\n"
        )
    shell = Pty(env, 180)
    try:
        if not shell.ready():
            raise AssertionError("theme-preservation session never became ready")
        shell.send('print -r -- "THEME_READY=$PROMPT|$RPROMPT"')
        if not shell.expect("THEME_READY=") or not shell.expect("THEME-RIGHT"):
            raise AssertionError(
                "test prompt theme did not activate:\n%s" % shell.transcript[-2500:]
            )
        os.write(shell.master, b"plain\t\x18\x02")
        shell.drain(0.5)
        with open(buffer_capture, encoding="utf-8") as file:
            if file.read() != "plain-tab":
                raise AssertionError("ordinary Tab did not delegate to the prior widget")
        os.write(shell.master, b"\x15/\t")
        if not shell.expect("AIShe command palette", timeout=5):
            raise AssertionError("slash-Tab did not open the command palette")
        os.write(shell.master, b"\x1b")
        time.sleep(0.05)
        os.write(shell.master, b"OB")
        if not shell.expect("selected 2/", timeout=5):
            raise AssertionError("SS3 Down did not move the palette selection")
        os.write(shell.master, b"\x1b")
        shell.drain(0.5)
        os.write(shell.master, b"\x15")
        shell.transcript = ""
        os.write(shell.master, b"\x1b[Z")
        shell.drain(1)
        repaint = CSI.sub("", shell.transcript)
        os.write(shell.master, b"hello")
        shell.drain(1)
        os.write(shell.master, b"\x18\x14")
        shell.drain(1)
        with open(capture, encoding="utf-8") as file:
            prompt_state = file.read()
        postdisplay, rprompt = prompt_state.split("\n---\n", 1)
        if postdisplay != "ghost":
            raise AssertionError(
                "Shift-Tab changed plugin-owned POSTDISPLAY: %r\n%s"
                % (postdisplay, shell.transcript[-2500:])
            )
        if "THEME-RIGHT" not in rprompt or "%90v" not in rprompt:
            raise AssertionError("AIShe did not compose with the theme RPROMPT")
        if "auto" not in repaint:
            raise AssertionError("Shift-Tab did not visibly emit the new mode")
        os.write(shell.master, b"\x15")
        shell.send(
            'print -r -- "THEME_CHECK=$AISHE_MODE|$PROMPT|$RPROMPT|$_AISHE_STATUS_TEXT"'
        )
        if not shell.expect("THEME_CHECK=auto|") or not shell.expect("THEME-RIGHT"):
            raise AssertionError(
                "Shift-Tab corrupted the composed prompt state:\n%s"
                % shell.transcript[-2500:]
            )
        print("  ok   Shift-Tab coexists with prompt themes and autosuggestions")
    finally:
        shell.close()
        shutil.rmtree(home, ignore_errors=True)


def main():
    if shutil.which("zsh") is None:
        print("SKIP: zsh not on PATH")
        return
    if not os.path.exists(BINARY):
        raise SystemExit("FAIL: binary not found: " + BINARY)
    # Legacy `below` configs migrate to the one stable native placement.
    run_case("right", 180)
    run_case("below", 100, "right")
    run_case("off", 42)
    prompt_substitution_is_inert()
    theme_survives_mode_cycle()
    print("PASS: statusline placement and live metrics")


if __name__ == "__main__":
    main()
