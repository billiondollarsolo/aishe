#!/usr/bin/env python3
"""Interactive setup/settings/tour PTY coverage."""

import json
import fcntl
import http.server
import os
import pty
import select
import shutil
import signal
import struct
import subprocess
import sys
import tempfile
import termios
import threading
import time
from contextlib import contextmanager

BINARY = os.path.abspath(
    sys.argv[1] if len(sys.argv) > 1 else "target/release/aishe"
)


class Pty:
    def __init__(self, argv, env, cols=100):
        self.master, slave = pty.openpty()
        fcntl.ioctl(
            self.master,
            termios.TIOCSWINSZ,
            struct.pack("HHHH", 30, cols, 0, 0),
        )
        self.proc = subprocess.Popen(
            argv,
            stdin=slave,
            stdout=slave,
            stderr=slave,
            env=env,
            preexec_fn=os.setsid,
            close_fds=True,
        )
        os.close(slave)
        self.buffer = ""
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
            text = chunk.decode("utf-8", "replace")
            self.buffer += text
            self.transcript += text

    def expect(self, text, timeout=20):
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            position = self.buffer.find(text)
            if position >= 0:
                self.buffer = self.buffer[position + len(text) :]
                return
            self.drain()
        raise AssertionError(
            "did not see %r\n---- transcript ----\n%s"
            % (text, self.transcript[-4000:])
        )

    def send(self, text):
        os.write(self.master, text.encode())

    def line(self, text=""):
        self.send(text + "\r")

    def menu(self, number):
        self.send(str(number) + "\r")

    def finish(self, timeout=20):
        end = time.monotonic() + timeout
        while time.monotonic() < end:
            if self.proc.poll() is not None:
                self.drain()
                return self.proc.returncode
            self.drain()
        raise AssertionError("process did not exit\n" + self.transcript[-4000:])

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


@contextmanager
def model_server(models, accepted_keys=None):
    accepted_keys = set(accepted_keys or [])

    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, _format, *_args):
            pass

        def send_json(self, status, payload):
            body = json.dumps(payload).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def authorized(self):
            if not accepted_keys:
                return True
            header = self.headers.get("Authorization", "")
            return header.startswith("Bearer ") and header[7:] in accepted_keys

        def do_GET(self):
            if self.path != "/v1/models":
                self.send_json(404, {"error": {"message": "not found"}})
            elif not self.authorized():
                self.send_json(401, {"error": {"message": "invalid API key"}})
            else:
                self.send_json(200, {"data": [{"id": model} for model in models]})

        def do_POST(self):
            length = int(self.headers.get("Content-Length", "0"))
            payload = json.loads(self.rfile.read(length) or b"{}")
            model = payload.get("model", "")
            if not self.authorized():
                self.send_json(401, {"error": {"message": "invalid API key"}})
            elif model not in models:
                self.send_json(
                    404,
                    {"error": {"message": "model does not exist or is unavailable"}},
                )
            elif self.path == "/v1/responses":
                self.send_json(
                    200,
                    {
                        "id": "setup-model-check",
                        "output": [
                            {
                                "type": "message",
                                "content": [
                                    {"type": "output_text", "text": "setup-ok"}
                                ],
                            }
                        ],
                        "usage": {"input_tokens": 1, "output_tokens": 1},
                    },
                )
            else:
                self.send_json(
                    200,
                    {
                        "choices": [
                            {"message": {"role": "assistant", "content": "setup-ok"}}
                        ],
                        "usage": {"prompt_tokens": 1, "completion_tokens": 1},
                    },
                )

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield "http://127.0.0.1:%d" % server.server_port
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)


def isolated_env(root):
    env = dict(os.environ)
    env.pop("NO_COLOR", None)
    env.update(
        {
            "HOME": root,
            "AISHE_CONFIG_DIR": os.path.join(root, "config"),
            "AISHE_DATA_DIR": os.path.join(root, "data"),
            "XDG_CONFIG_HOME": os.path.join(root, "config"),
            "XDG_DATA_HOME": os.path.join(root, "data"),
            "TERM": "xterm-256color",
        }
    )
    return env


def cleanup_isolated_root(root):
    """Stop any managed backend started by setup before deleting its state."""
    if os.path.isdir(root):
        subprocess.run(
            [BINARY, "backend", "stop"],
            cwd=root,
            env=isolated_env(root),
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            timeout=15,
            check=False,
        )
    shutil.rmtree(root, ignore_errors=True)


def setup_to_provider(shell):
    shell.expect("Continue setup")
    shell.line()  # Step 1: continue; platform/runtime/sandbox auto-verify in CI.
    shell.expect("Provider service", timeout=60)


def expect_setup_exit(shell, label):
    code = shell.finish()
    if code != 2:
        raise AssertionError("%s returned %s, expected stable pause code 2" % (label, code))


def setup_visual_style_and_alignment():
    root = tempfile.mkdtemp(prefix="aishe-setup-visual-")
    try:
        env = isolated_env(root)
        shell = Pty([BINARY, "setup"], env, cols=58)
        try:
            setup_to_provider(shell)
            shell.drain(0.3)
            if "\x1b[1;36mProvider service\x1b[0m" not in shell.transcript:
                raise AssertionError("setup menu title was not color styled")
            if "\x1b[1;36;7m› " not in shell.transcript:
                raise AssertionError("current setup selection was not highlighted")
            if "…\x1b[0m" not in shell.transcript:
                raise AssertionError("narrow focus row was not kept to one line")
            if "       reasoning and tools." not in shell.transcript:
                raise AssertionError("long provider help did not word-wrap with indentation")

            shell.menu(2)
            shell.expect("API endpoint")
            shell.drain(0.2)
            aligned = "\x1b[0m\r\n  \x1b[1;36mAPI endpoint\x1b[0m"
            if aligned not in shell.transcript:
                raise AssertionError(
                    "text prompt did not return to the left margin after raw menu mode\n"
                    + shell.transcript[-2500:]
                )
            shell.line(":cancel")
            shell.expect("Setup paused")
            expect_setup_exit(shell, "visual setup probe")
        finally:
            shell.close()
        print("  ok   colored focus, narrow wrapping, and prompt alignment")
    finally:
        cleanup_isolated_root(root)


def setup_width_and_no_color_matrix():
    for cols in (40, 80, 120, 200):
        root = tempfile.mkdtemp(prefix="aishe-setup-width-%d-" % cols)
        try:
            env = isolated_env(root)
            shell = Pty([BINARY, "setup"], env, cols=cols)
            try:
                setup_to_provider(shell)
                shell.drain(0.2)
                if "\x1b[?1049h" in shell.transcript:
                    raise AssertionError(
                        "setup entered an alternate-screen UI at %d columns" % cols
                    )
                if "\x1b[1;36;7m› " not in shell.transcript:
                    raise AssertionError(
                        "setup lost its visible focus row at %d columns" % cols
                    )
                shell.menu(2)
                shell.expect("API endpoint")
                shell.line(":cancel")
                shell.expect("Setup paused")
                expect_setup_exit(shell, "setup width %d" % cols)
            finally:
                shell.close()
        finally:
            cleanup_isolated_root(root)

    root = tempfile.mkdtemp(prefix="aishe-setup-no-color-")
    try:
        env = isolated_env(root)
        env["NO_COLOR"] = "1"
        shell = Pty([BINARY, "setup"], env, cols=80)
        try:
            setup_to_provider(shell)
            shell.drain(0.2)
            if "\x1b[1;36m" in shell.transcript or "\x1b[1;36;7m" in shell.transcript:
                raise AssertionError("NO_COLOR setup emitted color styling")
            if "› " not in shell.transcript or "Provider service" not in shell.transcript:
                raise AssertionError("NO_COLOR setup lost its focus marker or title")
            shell.send("\x1b")
            shell.expect("Setup paused")
            expect_setup_exit(shell, "NO_COLOR setup")
        finally:
            shell.close()
    finally:
        cleanup_isolated_root(root)
    print("  ok   40/80/120/200-column and NO_COLOR setup matrix")


def setup_checks_catalog_credential_and_manual_model():
    root = tempfile.mkdtemp(prefix="aishe-setup-model-check-")
    good_key = "setup-catalog-good-key"
    try:
        env = isolated_env(root)
        catalog = ["catalog-valid-model"] + [
            "catalog-model-%02d" % number for number in range(1, 12)
        ]
        with model_server(catalog, [good_key]) as endpoint:
            rejected = Pty([BINARY, "setup"], env)
            try:
                setup_to_provider(rejected)
                rejected.menu(7)
                rejected.expect("API endpoint")
                rejected.line(endpoint)
                rejected.expect("Credential profile 'custom'")
                rejected.menu(1)
                rejected.expect("API key for 'custom'")
                rejected.line("wrong-key")
                rejected.expect("Could not load /v1/models (InvalidCredential)")
                rejected.expect("Model discovery needs attention")
                rejected.line()  # default: back to credential
                rejected.expect("Credential profile 'custom'")
                rejected.send("\x1b")
                rejected.expect("Setup paused")
                expect_setup_exit(rejected, "invalid-credential setup probe")
            finally:
                rejected.close()

            restarted = Pty([BINARY, "setup", "--restart"], env)
            try:
                setup_to_provider(restarted)
                restarted.menu(7)
                restarted.expect("API endpoint")
                restarted.line(endpoint)
                restarted.expect("Credential profile 'custom'")
                restarted.menu(1)
                restarted.expect("API key for 'custom'")
                restarted.line(good_key)
                restarted.expect("Credential accepted; /v1/models returned 12 model(s)")
                restarted.expect("Available models (refreshed from /v1/models)")
                restarted.menu(1)
                restarted.expect("Model ID")
                restarted.line("catalog-missing-model")
                restarted.expect("Validate it with one minimal request?")
                restarted.line()
                restarted.expect("making one minimal request")
                restarted.expect("Model validation failed (ModelNotFound)")
                restarted.expect("Model ID")
                restarted.line("catalog-valid-model")
                restarted.expect("present in the current endpoint catalog")
                restarted.expect("Safety profile")
                restarted.send("b")
                restarted.expect("Available models (refreshed from /v1/models)")
                restarted.menu(10)
                restarted.expect(
                    "Model 'catalog-model-08' is present in the current endpoint catalog"
                )
                restarted.expect("Safety profile")
                restarted.send("\x1b")
                restarted.expect("Setup paused")
                expect_setup_exit(restarted, "manual-model setup probe")
            finally:
                restarted.close()
        print("  ok   /models rejects bad keys and validates selected or typed models")
    finally:
        cleanup_isolated_root(root)


def complete_setup(root, env, endpoint):
    shell = Pty([BINARY, "setup"], env)
    try:
        setup_to_provider(shell)
        shell.menu(6)  # Ollama

        shell.expect("API endpoint")
        shell.line("ftp://bad")
        shell.expect("endpoint must use http:// or https://")
        shell.line(endpoint)

        shell.expect("Available models (refreshed from /v1/models)")
        shell.menu(1)  # type a model ID
        shell.expect("Model ID")
        shell.line(":back")
        # Back from typed entry returns to the catalog picker; the menu's own
        # back action returns to the previous setup step.
        shell.expect("Available models (refreshed from /v1/models)")
        shell.menu(1)
        shell.expect("Model ID")
        shell.line("setup-local-model")
        shell.expect("present in the current endpoint catalog")

        shell.expect("Safety profile")
        shell.menu(2)  # balanced
        shell.expect("Default execution scope")
        shell.menu(1)  # workspace
        shell.expect("Workspace network")
        shell.menu(1)  # deny
        shell.expect("No price is known")
        shell.menu(1)  # enter exact rates
        shell.expect("Input price")
        shell.line("-1")
        shell.expect("price must be a finite non-negative number")
        shell.line("0.125")
        shell.expect("Output price")
        shell.line("0.5")

        shell.expect("Agent transcript density")
        shell.menu(1)  # focus
        shell.expect("Live status-line placement")
        shell.menu(2)  # below
        shell.expect("Status-line contents")
        shell.menu(2)  # detailed
        shell.expect("preview (below)")
        shell.expect("Enable the private redacted audit log?")
        shell.line("n")

        shell.expect("Run live text/structured/tool/streaming checks?", timeout=60)
        shell.line("n")
        shell.expect("Provider check:")
        shell.expect("Review")
        shell.expect("Apply this configuration")
        shell.line("y")
        shell.expect("Setup complete", timeout=60)
        shell.expect("Run the guided first-session tour now")
        shell.line("n")
        shell.expect("Run `aishe tour` when you are ready")
        if shell.finish() != 0:
            raise AssertionError("setup returned nonzero\n" + shell.transcript[-4000:])
    finally:
        shell.close()

    config = os.path.join(root, "config", "aishe", "config.toml")
    text = open(config, encoding="utf-8").read()
    required = [
        'model = "setup-local-model"',
        'safety_profile = "balanced"',
        'status_line_position = "below"',
        "input = 0.125",
        "output = 0.5",
    ]
    for value in required:
        if value not in text:
            raise AssertionError("saved config missing %r\n%s" % (value, text))
    draft = os.path.join(root, "data", "aishe", "setup-draft.json")
    if os.path.exists(draft):
        raise AssertionError("successful setup left a resumable draft")
    print("  ok   full setup flow, invalid input, back, pricing, status, apply")
    return config


def cancel_preserves_active_config(root, env, config):
    before = open(config, "rb").read()
    shell = Pty([BINARY, "setup"], env)
    try:
        setup_to_provider(shell)
        shell.send("\x1b")
        shell.expect("Setup paused")
        expect_setup_exit(shell, "cancel")
    finally:
        shell.close()
    after = open(config, "rb").read()
    if before != after:
        raise AssertionError("cancel changed the active config")
    draft = os.path.join(root, "data", "aishe", "setup-draft.json")
    if not os.path.isfile(draft):
        raise AssertionError("cancel did not save a resumable draft")

    resumed = Pty([BINARY, "setup", "--resume"], env)
    try:
        resumed.expect("Provider service")
        resumed.send("\x03")
        resumed.expect("Setup paused")
        expect_setup_exit(resumed, "Ctrl-C cancel")
    finally:
        resumed.close()
    if before != open(config, "rb").read():
        raise AssertionError("resumed Ctrl-C changed the active config")

    # Force the saved draft to a later valid state, then prove --restart
    # discards only that draft and begins at the first step.
    with open(draft, encoding="utf-8") as file:
        draft_state = json.load(file)
    draft_state["step"] = "review"
    with open(draft, "w", encoding="utf-8") as file:
        json.dump(draft_state, file)
    restarted = Pty([BINARY, "setup", "--restart"], env)
    try:
        setup_to_provider(restarted)
        restarted.send("\x1b")
        restarted.expect("Setup paused")
        expect_setup_exit(restarted, "restarted setup cancel")
    finally:
        restarted.close()
    if before != open(config, "rb").read():
        raise AssertionError("setup --restart changed the active config")
    print("  ok   cancel/Ctrl-C/resume/restart preserve active config")


def settings_are_transactional(root, env, config):
    before = open(config, "rb").read()
    shell = Pty([BINARY, "settings"], env)
    try:
        shell.expect("Choose a section")
        shell.menu(1)  # provider transaction
        shell.expect("Provider service")
        shell.menu(2)  # OpenAI
        shell.expect("Endpoint")
        shell.line(":cancel")
        shell.expect("Choose a section")
        shell.menu(9)  # exit without changes
        if shell.finish() != 0:
            raise AssertionError("settings cancel returned nonzero")
    finally:
        shell.close()
    if before != open(config, "rb").read():
        raise AssertionError("cancelled provider transaction changed config")

    apply = Pty([BINARY, "settings"], env)
    try:
        apply.expect("Choose a section")
        apply.menu(2)  # shell/history/status
        apply.expect("Shell, history & statusline")
        apply.menu(4)  # hook timeout
        apply.expect("AI hook timeout seconds")
        apply.line("75")
        apply.expect("Shell, history & statusline")
        apply.menu(5)  # placement
        apply.expect("Statusline placement")
        apply.menu(1)  # right
        apply.expect("preview (right)")
        apply.expect("Shell, history & statusline")
        apply.menu(7)  # transcript density
        apply.expect("Agent transcript density")
        apply.menu(3)  # detailed
        apply.expect("Shell, history & statusline")
        apply.menu(9)  # back
        apply.expect("Choose a section")
        apply.menu(8)  # review/apply
        apply.expect("Apply these settings")
        apply.line("y")
        apply.expect("saved:")
        if apply.finish() != 0:
            raise AssertionError("settings apply returned nonzero")
    finally:
        apply.close()
    text = open(config, encoding="utf-8").read()
    if 'status_line_position = "right"' not in text:
        raise AssertionError("settings did not apply selected placement")
    if "hook_timeout_secs = 75" not in text:
        raise AssertionError("settings did not apply selected hook timeout")
    if 'output = "detailed"' not in text:
        raise AssertionError("settings did not apply selected transcript density")
    print("  ok   settings provider cancel is transactional; reviewed apply works")


def hidden_auth_and_staged_setup_are_secret_safe():
    root = tempfile.mkdtemp(prefix="aishe-credential-pty-")
    secret = "pty-secret-must-never-echo"
    server_context = model_server(["custom-credential-model"], [secret])
    endpoint = server_context.__enter__()
    try:
        env = isolated_env(root)
        shell = Pty([BINARY, "setup"], env)
        try:
            setup_to_provider(shell)
            shell.menu(7)  # custom, with authentication required
            shell.expect("API endpoint")
            shell.line(endpoint)
            shell.expect("Credential profile 'custom'")
            shell.menu(1)  # enter and save locally
            shell.expect("API key for 'custom'")
            shell.line(secret)
            shell.expect("Credential accepted; /v1/models returned 1 model(s)")
            shell.expect("Available models (refreshed from /v1/models)")

            draft = os.path.join(root, "data", "aishe", "setup-draft.json")
            draft_text = open(draft, encoding="utf-8").read()
            credentials = os.path.join(
                root, "config", "aishe", "credentials.toml"
            )
            if secret in draft_text:
                raise AssertionError("setup serialized its staged secret")
            if os.path.exists(credentials):
                raise AssertionError("setup wrote credentials before Apply")
            if secret in shell.transcript:
                raise AssertionError("hidden setup input appeared in the PTY transcript")

            shell.send("\x1b")
            shell.expect("Setup paused")
            expect_setup_exit(shell, "staged setup cancel")
        finally:
            shell.close()

        if os.path.exists(
            os.path.join(root, "config", "aishe", "credentials.toml")
        ):
            raise AssertionError("cancelled setup persisted a credential")

        resumed = Pty([BINARY, "setup", "--resume"], env)
        try:
            resumed.expect("returning to the Credential step")
            resumed.expect("Credential profile 'custom'")
            resumed.menu(1)
            resumed.expect("API key for 'custom'")
            resumed.line(secret)
            resumed.expect("Available models (refreshed from /v1/models)")
            resumed.menu(1)
            resumed.expect("Model ID")
            resumed.line("custom-credential-model")
            resumed.expect("present in the current endpoint catalog")
            resumed.expect("Safety profile")
            resumed.menu(1)
            resumed.expect("Default execution scope")
            resumed.menu(1)
            resumed.expect("Workspace network")
            resumed.menu(1)
            resumed.expect("No price is known")
            resumed.menu(2)
            resumed.expect("Agent transcript density")
            resumed.menu(1)
            resumed.expect("Live status-line placement")
            resumed.menu(3)
            resumed.expect("Enable the private redacted audit log?")
            resumed.line("n")
            resumed.expect(
                "Run live text/structured/tool/streaming checks?", timeout=60
            )
            resumed.line("n")
            resumed.expect("Review")
            resumed.expect("will save locally on Apply")
            resumed.expect("Apply this configuration")
            resumed.line("y")
            resumed.expect("Setup complete", timeout=60)
            resumed.expect("Run the guided first-session tour now")
            resumed.line("n")
            if resumed.finish() != 0:
                raise AssertionError(
                    "resumed credential setup returned nonzero\n"
                    + resumed.transcript[-4000:]
                )
            if secret in resumed.transcript:
                raise AssertionError("resumed hidden input appeared in transcript")
        finally:
            resumed.close()

        credentials = os.path.join(root, "config", "aishe", "credentials.toml")
        saved = open(credentials, encoding="utf-8").read()
        if secret not in saved or "[profiles.custom]" not in saved:
            raise AssertionError("Apply did not save the staged credential")
        if os.stat(credentials).st_mode & 0o777 != 0o600:
            raise AssertionError("credentials file is not mode 0600")

        # Exercise the standalone hidden prompt as well. The secret may exist in
        # the private file, but must not appear in output or process arguments.
        replacement = "auth-replacement-secret-never-echo"
        auth = Pty([BINARY, "auth", "set", "custom"], env)
        try:
            auth.expect("API key for credential profile 'custom'")
            auth.line(replacement)
            auth.expect("credential profile 'custom' saved")
            if auth.finish() != 0:
                raise AssertionError("interactive auth set returned nonzero")
            if replacement in auth.transcript:
                raise AssertionError("hidden auth input appeared in transcript")
        finally:
            auth.close()
        if replacement not in open(credentials, encoding="utf-8").read():
            raise AssertionError("interactive auth replacement was not saved")
        print("  ok   hidden setup/auth input, cancel, resume, and Apply are secret-safe")
    finally:
        server_context.__exit__(None, None, None)
        cleanup_isolated_root(root)


def tour_pause_resume_skip_restart_and_complete(root, env):
    paused = Pty([BINARY, "tour"], env)
    try:
        paused.expect("1. Normal shell commands")
        paused.expect("Lesson 1 of 7")
        paused.menu(3)  # exit and resume later
        paused.expect("Tour paused")
        if paused.finish() != 0:
            raise AssertionError("tour pause returned nonzero")
    finally:
        paused.close()

    resumed = Pty([BINARY, "tour"], env)
    try:
        resumed.expect("1. Normal shell commands")
        resumed.expect("Lesson 1 of 7")
        resumed.menu(2)  # skip lesson one
        resumed.expect("2. Natural-language routing")
        resumed.expect("not verified; lesson stays offline")
        resumed.expect("Lesson 2 of 7")
        resumed.menu(3)  # pause on lesson two
        resumed.expect("Tour paused")
        if resumed.finish() != 0:
            raise AssertionError("resumed tour pause returned nonzero")
    finally:
        resumed.close()

    restarted = Pty([BINARY, "tour", "--restart"], env)
    lessons = [
        "1. Normal shell commands",
        "2. Natural-language routing",
        "3. Suggest mode",
        "4. Recover from failures",
        "5. File change and undo",
        "6. Modes and safety",
        "7. Your persistent state",
    ]
    try:
        for index, lesson in enumerate(lessons, start=1):
            restarted.expect(lesson)
            restarted.expect("Lesson %d of 7" % index)
            restarted.menu(1)
        restarted.expect("Tour complete")
        if restarted.finish() != 0:
            raise AssertionError("restarted tour returned nonzero")
    finally:
        restarted.close()

    state_path = os.path.join(root, "data", "aishe", "tour", "state.json")
    with open(state_path, encoding="utf-8") as file:
        state = json.load(file)
    if not state.get("completed") or state.get("next_lesson") != 7:
        raise AssertionError("tour did not persist completed state: %r" % state)
    undo_demo = os.path.join(root, "data", "aishe", "tour", "workspace", "undo-demo.txt")
    if os.path.exists(undo_demo):
        raise AssertionError("tour undo lesson left its demo file behind")
    print("  ok   tour pause/resume/skip/restart/offline/undo flow")


def main():
    if not os.path.exists(BINARY):
        raise SystemExit("FAIL: binary not found: " + BINARY)
    root = tempfile.mkdtemp(prefix="aishe-setup-pty-")
    try:
        setup_visual_style_and_alignment()
        setup_width_and_no_color_matrix()
        setup_checks_catalog_credential_and_manual_model()
        env = isolated_env(root)
        with model_server(["setup-local-model"]) as endpoint:
            config = complete_setup(root, env, endpoint)
            cancel_preserves_active_config(root, env, config)
            settings_are_transactional(root, env, config)
        hidden_auth_and_staged_setup_are_secret_safe()
        tour_pause_resume_skip_restart_and_complete(root, env)
        print("PASS: interactive setup/settings/tour PTY")
    finally:
        cleanup_isolated_root(root)


if __name__ == "__main__":
    main()
