#!/usr/bin/env python3
"""Real-zsh PTY tests for right/below/off live statusline placement."""

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

BINARY = os.path.abspath(
    sys.argv[1] if len(sys.argv) > 1 else "target/release/aishe"
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
            'status_line_items = ["model", "mode", "last_tokens", "last_cost", '
            '"session_tokens", "session_cost", "requests"]\n\n'
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
    return home, env, model


def run_case(position, cols):
    home, env, model = environment(position)
    shell = Pty(env, cols)
    try:
        if not shell.ready():
            raise AssertionError("%s session never became ready" % position)
        shell.drain(0.5)
        if position == "off":
            if model in shell.transcript:
                raise AssertionError("off statusline rendered the model")
        elif model not in shell.transcript or "auto" not in shell.transcript:
            raise AssertionError(
                "%s statusline did not render identity:\n%s"
                % (position, shell.transcript[-2500:])
            )

        shell.send("print -r -- POSITION=$AISHE_STATUS_POSITION")
        if not shell.expect("POSITION=" + position):
            raise AssertionError("%s placement was not passed to zsh" % position)
        submitted = "? print the status test marker"
        submitted_start = len(shell.transcript)
        shell.send(submitted)
        if not shell.expect("STATUS_CALL_OK"):
            raise AssertionError("%s AI call did not complete" % position)
        if not submitted_line_remains_visible(
            shell.transcript[submitted_start:], submitted
        ):
            raise AssertionError(
                "%s statusline erased the submitted request:\n%s"
                % (position, shell.transcript[-2500:])
            )
        shell.drain(0.8)
        dynamic = ["last 123/45 tok", "session 123/45 tok", "1 req"]
        if position == "off":
            if any(value in shell.transcript for value in dynamic):
                raise AssertionError("off statusline rendered dynamic metrics")
        elif not all(value in shell.transcript for value in dynamic):
            raise AssertionError(
                "%s statusline did not refresh metrics:\n%s"
                % (position, shell.transcript[-2500:])
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
    command_payload = "$(touch %s)" % marker
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


def main():
    if shutil.which("zsh") is None:
        print("SKIP: zsh not on PATH")
        return
    if not os.path.exists(BINARY):
        raise SystemExit("FAIL: binary not found: " + BINARY)
    # A detailed right prompt intentionally follows native zsh behavior and is
    # hidden when it would collide with the input prompt, so exercise it wide.
    # The below/off cases cover narrow-terminal behavior.
    run_case("right", 180)
    run_case("below", 60)
    run_case("off", 42)
    prompt_substitution_is_inert()
    print("PASS: statusline placement and live metrics")


if __name__ == "__main__":
    main()
