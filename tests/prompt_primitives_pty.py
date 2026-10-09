#!/usr/bin/env python3
"""Exercise shared prompt primitives through real local PTYs.

Build AIShe first, then run:
    python3 tests/prompt_primitives_pty.py target/debug/aishe

The tiny fixture links the same profile's built library. It never constructs a
provider, starts a backend, or accesses an external network service.
"""

from __future__ import annotations

import fcntl
import os
import pathlib
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


ROOT = pathlib.Path(__file__).resolve().parent.parent
CSI = re.compile(r"\x1b\[[0-9;?]*[ -/]*[@-~]")
FIXTURE = r'''
use aishe::promptui;

fn main() {
    let kind = std::env::args().nth(1).unwrap_or_default();
    let options = (0..5)
        .map(|index| format!("Option {} with a deliberately long display label", index + 1))
        .collect::<Vec<_>>();
    match kind.as_str() {
        "picker" => println!("RESULT:{:?}", promptui::filter_picker("Fixture picker", &options, 1)),
        "menu" => println!("RESULT:{:?}", promptui::menu("Fixture menu", &options, 1, true, "Fixture help")),
        "text" => println!("RESULT:{:?}", promptui::text("Fixture text", "sample-default", |_| Ok(()))),
        "confirm" => println!("RESULT:{:?}", promptui::confirm("Fixture confirm", false)),
        "secret" => println!("RESULT:{:?}", promptui::secret("Fixture secret", 64)),
        _ => panic!("unknown fixture"),
    }
}
'''


def compile_fixture(binary: pathlib.Path, directory: pathlib.Path) -> pathlib.Path:
    library = binary.parent / "libaishe.rlib"
    if not library.is_file():
        raise AssertionError("missing %s; build AIShe's library in the same profile first" % library)
    relevant_sources = [ROOT / "src/promptui.rs", ROOT / "src/ui.rs", ROOT / "src/ui/render.rs"]
    if any(path.stat().st_mtime_ns > min(library.stat().st_mtime_ns, binary.stat().st_mtime_ns)
           for path in relevant_sources):
        raise AssertionError("prompt source is newer than the built library/binary; rebuild AIShe before this test")
    compiler = shutil.which("rustc")
    if compiler is None:
        raise AssertionError("rustc is required to compile the local prompt fixture")
    source = directory / "prompt_fixture.rs"
    source.write_text(FIXTURE, encoding="utf-8")
    fixture = directory / "prompt_fixture"
    # Release archives contain ThinLTO bitcode and must be linked by rustc's
    # matching LTO pipeline instead of being passed directly to the system ld.
    profile_flags = ["-C", "lto=thin", "-C", "opt-level=3"] if binary.parent.name == "release" else []
    subprocess.run(
        [compiler, "--edition=2021", *profile_flags, str(source), "--extern", "aishe=" + str(library),
         "-L", "dependency=" + str(binary.parent / "deps"), "-o", str(fixture)],
        check=True,
        cwd=ROOT,
    )
    return fixture


class Prompt:
    def __init__(self, fixture: pathlib.Path, kind: str, *, cols=100, static=False):
        self.master, self.slave = pty.openpty()
        self.initial_termios = termios.tcgetattr(self.slave)
        self.resize(cols)
        env = dict(os.environ)
        env.update(TERM="xterm-256color", NO_COLOR="1", AISHE_UNICODE="ascii",
                   AISHE_MOTION="static" if static else "live")
        self.process = subprocess.Popen(
            [str(fixture), kind], stdin=self.slave, stdout=self.slave, stderr=self.slave,
            env=env, close_fds=True,
            preexec_fn=lambda: (os.setsid(), fcntl.ioctl(0, termios.TIOCSCTTY, 0)),
        )
        self.transcript = ""
        try:
            self.expect("Fixture " + kind)
            if not static:
                end = time.monotonic() + 3
                while termios.tcgetattr(self.slave)[3] & termios.ICANON:
                    if time.monotonic() >= end:
                        raise AssertionError("prompt did not enter raw mode\n" + self.transcript)
                    self.drain(0.025)
            self.drain(0.1)
        except Exception:
            self.close()
            raise

    def resize(self, cols, rows=30):
        fcntl.ioctl(self.slave, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))

    def drain(self, duration=0.1):
        end = time.monotonic() + duration
        while time.monotonic() < end:
            ready, _, _ = select.select([self.master], [], [], min(0.025, max(0, end - time.monotonic())))
            if ready:
                try:
                    chunk = os.read(self.master, 65536)
                except OSError:
                    return
                if not chunk:
                    return
                self.transcript += chunk.decode("utf-8", "replace")

    def expect(self, expected, timeout=3):
        end = time.monotonic() + timeout
        while expected not in self.transcript:
            if time.monotonic() >= end:
                raise AssertionError("did not see %r\n%s" % (expected, self.transcript[-2000:]))
            self.drain(0.05)

    def send(self, data):
        os.write(self.master, data)
        self.drain(0.1)

    def finished(self, expected):
        self.expect("RESULT:" + expected)
        self.process.wait(timeout=3)
        self.drain()
        if self.process.returncode != 0:
            raise AssertionError("prompt failed: %s\n%s" % (self.process.returncode, self.transcript))
        if termios.tcgetattr(self.slave) != self.initial_termios:
            raise AssertionError("prompt did not restore terminal attributes\n" + self.transcript)

    def close(self):
        if self.process.poll() is None:
            os.killpg(os.getpgid(self.process.pid), signal.SIGKILL)
            self.process.wait(timeout=3)
        os.close(self.slave)
        os.close(self.master)


def cancellation_and_restoration(fixture):
    for kind in ("picker", "menu", "text", "confirm", "secret"):
        for name, key in (("Esc", b"\x1b"), ("Ctrl-C", b"\x03"), ("Ctrl-D", b"\x04"), ("Enter", b"\r")):
            prompt = Prompt(fixture, kind)
            try:
                prompt.send(key)
                if name == "Enter":
                    expected = {"picker": "Ok(Use(1))", "menu": "Ok(Selected(1))",
                                "text": 'Ok(Some("sample-default"))', "confirm": "Ok(Some(false))",
                                "secret": 'Ok(Some(""))'}[kind]
                else:
                    expected = "Ok(Cancel)" if kind in ("picker", "menu") else "Ok(None)"
                prompt.finished(expected)
            except Exception as error:
                raise AssertionError("%s %s: %s" % (kind, name, error)) from error
            finally:
                prompt.close()
    print("  ok   Esc/Ctrl-C/Ctrl-D/Enter restore terminal mode for all shared prompts")


def menu_resize(fixture):
    prompt = Prompt(fixture, "menu", cols=100)
    try:
        offset = len(prompt.transcript)
        prompt.resize(20)
        prompt.send(b"\x1b[B")
        redraw = CSI.sub("", prompt.transcript[offset:])
        rows = [row for row in re.split(r"[\r\n]", redraw) if row]
        if not rows or not any(">" in row for row in rows):
            raise AssertionError("resized menu lost its visible selection\n" + repr(redraw))
        if any(len(row) > 20 for row in rows):
            raise AssertionError("menu redraw exceeded its new 20-column width\n" + repr(redraw))
        prompt.send(b"\r")
        prompt.finished("Ok(Selected(2))")
    finally:
        prompt.close()
    print("  ok   menu redraw honors a 100-to-20-column resize and keeps selection")


def text_editing(fixture):
    value = "https://example.invalid/v1/a-long-custom-endpoint"
    for entered, expected in ((value.encode() + b"x\x7f\r", 'Ok(Some("%s"))' % value),
                              (b":back\r", 'Ok(Some(":back"))'),
                              (b":cancel\r", "Ok(None)")):
        prompt = Prompt(fixture, "text")
        try:
            prompt.send(entered)
            prompt.finished(expected)
        finally:
            prompt.close()
    print("  ok   text editing preserves long endpoints, backspace, and line commands")


def static_picker(fixture):
    prompt = Prompt(fixture, "picker", cols=20, static=True)
    try:
        for command in (b":prev\r", b":next\r"):
            prompt.send(command)
        plain = CSI.sub("", prompt.transcript)
        if "0 matches" in plain or "filter: :prev" in plain or "filter: :next" in plain:
            raise AssertionError("page-boundary commands changed the filter\n" + plain)
        if "Up/Down" in plain or "Esc" in plain or "\x1b" in prompt.transcript:
            raise AssertionError("static picker advertised live controls or emitted ANSI\n" + plain)
        prompt.send(b"\r")
        prompt.finished("Ok(Use(1))")
    finally:
        prompt.close()
    for kind in ("picker", "menu"):
        for key in (b"\x03", b"\x04"):
            prompt = Prompt(fixture, kind, static=True)
            try:
                prompt.send(key)
                prompt.finished("Ok(Cancel)")
            finally:
                prompt.close()
    print("  ok   static controls, boundary paging, default selection, and Ctrl-C/EOF cancellation")


def setup_text_cancellation(binary):
    # Reuse the maintained navigation helper; stop before model discovery or
    # credentials, so this observes setup's real state transition without API IO.
    saved_argv = sys.argv
    sys.argv = [sys.argv[0], str(binary)]
    try:
        import setup_pty
    finally:
        sys.argv = saved_argv

    root = tempfile.mkdtemp(prefix="aishe-prompt-primitives-setup-")
    env = setup_pty.isolated_env(root)
    env.update(AISHE_MOTION="live", AISHE_UNICODE="unicode", LC_ALL="C.UTF-8")
    shell = setup_pty.Pty([str(binary), "setup"], env)
    try:
        setup_pty.setup_to_provider(shell)
        shell.menu(9)  # Other / custom endpoint.
        shell.expect("API endpoint")
        shell.send("\x03")
        shell.expect("Setup paused", timeout=5)
        setup_pty.expect_setup_exit(shell, "text-prompt Ctrl-C")
        draft = pathlib.Path(root) / "data/aishe/setup-draft.json"
        if not draft.is_file():
            raise AssertionError("text-prompt Ctrl-C did not save the resumable setup draft")
    finally:
        shell.close()
        setup_pty.cleanup_isolated_root(root)
    print("  ok   setup text Ctrl-C pauses with exit 2 and a resumable draft")


def main():
    binary = pathlib.Path(require_current_binary(sys.argv[1] if len(sys.argv) > 1 else "target/debug/aishe"))
    with tempfile.TemporaryDirectory(prefix="aishe-prompt-primitives-") as root:
        fixture = compile_fixture(binary, pathlib.Path(root))
        cancellation_and_restoration(fixture)
        menu_resize(fixture)
        text_editing(fixture)
        static_picker(fixture)
        setup_text_cancellation(binary)
    print("PASS: shared prompt primitive PTY regressions")


if __name__ == "__main__":
    main()
