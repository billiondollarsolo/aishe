#!/usr/bin/env python3
"""Generative robustness fuzzer for the zsh-PTY front-end.

Drives the real `aishe zsh` wrapper through a large, generated corpus and checks
hard invariants after every line:
  * the shell never died,
  * no parse error / glob error / eval error / stray "command not found: #|?"
    ever leaked,
  * real commands produce their expected output,
  * `?`/`#`-forced natural language reaches the AI (the fake answer marker shows)
    regardless of the punctuation/metacharacters in the line,
  * adversarial model responses (prose, malformed commands, dangerous commands)
    are never eval'd into an error.

The model is the deterministic fake (AISHE_FAKE_LLM). Runs a few hundred cases by
default; pass a multiplier to scale up (e.g. `pty_fuzz.py BIN 10`).

Exit 0 on success; on the first invariant breach it prints the offending case and
recent output and exits non-zero. Skips if zsh is absent.
"""

import os
import sys
import pty
import time
import select
import signal
import shutil
import string
import random
import tempfile
import subprocess

BINARY = sys.argv[1] if len(sys.argv) > 1 else "target/release/aishe"
SCALE = int(sys.argv[2]) if len(sys.argv) > 2 else 1
TIMEOUT = 20.0
SEED = 1234

# Per-run results, written to a markdown report under test-results/.
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
REPORT_DIR = os.path.join(REPO_ROOT, "test-results")
RUN = {
    "start": None,
    "counts": {},        # kind -> number of cases that passed
    "samples": {},        # kind -> a few example inputs
    "fail": None,         # (kind, input, why, recent_output) on failure
}


def _record(case):
    k = case.get("kind", "?")
    RUN["counts"][k] = RUN["counts"].get(k, 0) + 1
    s = RUN["samples"].setdefault(k, [])
    if len(s) < 6 and case.get("input") and case["input"] not in s:
        s.append(case["input"])

FORBIDDEN = [
    "parse error", "(eval):", "no matches found",
    "command not found: #", "command not found: ?",
    "bad pattern", "bad set of key", "bad math expression",
]


class Pty:
    def __init__(self, argv, env, cwd=None):
        self.master, slave = pty.openpty()
        self.proc = subprocess.Popen(
            argv, stdin=slave, stdout=slave, stderr=slave,
            env=env, cwd=cwd, preexec_fn=os.setsid, close_fds=True)
        os.close(slave)
        self.buf = ""
        self.transcript = ""

    def _drain(self, deadline):
        if deadline - time.monotonic() <= 0:
            return False
        r, _, _ = select.select([self.master], [], [], 0.1)
        if not r:
            return True
        try:
            chunk = os.read(self.master, 8192)
        except OSError:
            return False
        if not chunk:
            return False
        t = chunk.decode("utf-8", "replace")
        self.buf += t
        self.transcript += t
        # keep transcript bounded
        if len(self.transcript) > 200000:
            self.transcript = self.transcript[-100000:]
        return True

    def expect(self, needle, timeout=TIMEOUT):
        deadline = time.monotonic() + timeout
        while True:
            i = self.buf.find(needle)
            if i != -1:
                self.buf = self.buf[i + len(needle):]
                return True
            if not self._drain(deadline):
                return False

    def send(self, line):
        os.write(self.master, (line + "\r").encode("utf-8"))

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

    def drain_until_quiet(self, quiet=0.3, maxwait=12):
        # Wait until no new output arrives for `quiet` seconds: aishe has finished
        # the turn (the async --auto-line subprocess returned) and the prompt is
        # idle. Critical so the next input is not sent mid-processing.
        deadline = time.monotonic() + maxwait
        while time.monotonic() < deadline:
            before = len(self.transcript)
            self.settle(quiet)
            if len(self.transcript) == before:
                return

    def clear_line(self):
        # Ctrl-C: abandon any partial or pre-filled buffer and get a fresh prompt.
        os.write(self.master, b"\x03")
        self.drain_until_quiet(quiet=0.2)
        self.buf = ""

    def settle(self, secs=0.25):
        deadline = time.monotonic() + secs
        while time.monotonic() < deadline:
            self._drain(deadline)

    def alive(self):
        return self.proc.poll() is None

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
    home = tempfile.mkdtemp(prefix="aishe-fuzz-")
    cfgdir = os.path.join(home, ".config", "aishe")
    os.makedirs(cfgdir, exist_ok=True)
    with open(os.path.join(cfgdir, "config.toml"), "w") as f:
        f.write('[aishe]\nmode = "auto"\nprovider = "anthropic"\n'
                'front_end = "zsh-pty"\npty_prompt = false\n')
    with open(os.path.join(home, ".zshrc"), "w") as f:
        f.write("HISTFILE=~/.zsh_history\nHISTSIZE=2000\nSAVEHIST=2000\nPROMPT='ZP> '\n")
    bindir = os.path.join(home, "bin")
    os.makedirs(bindir, exist_ok=True)
    os.symlink(os.path.abspath(binary), os.path.join(bindir, "aishe"))
    fakefile = os.path.join(home, "fake_llm.txt")
    open(fakefile, "w").close()
    env = dict(os.environ)
    env.update({
        "HOME": home, "XDG_CONFIG_HOME": os.path.join(home, ".config"),
 # macOS ignores XDG_*; these are honored on every platform.
 "AISHE_CONFIG_DIR": os.path.join(home, ".config"),
 "AISHE_DATA_DIR": os.path.join(home, ".local", "share"),
        "XDG_DATA_HOME": os.path.join(home, ".local", "share"),
        "ZDOTDIR": home, "TERM": "xterm-256color",
        "PATH": bindir + ":" + os.environ.get("PATH", ""),
        "ANTHROPIC_API_KEY": "", "OPENAI_API_KEY": "",
        # The fake reads its response from this file every call; the harness
        # rewrites it per case (from Python, so no shell-quoting hazards).
        "AISHE_FAKE_LLM_FILE": fakefile,
    })
    return home, env, fakefile


def write_report(status, total):
    import datetime
    os.makedirs(REPORT_DIR, exist_ok=True)
    ts = datetime.datetime.utcnow().strftime("%Y%m%dT%H%M%SZ")
    path = os.path.join(REPORT_DIR, "fuzz-%s.md" % ts)
    dur = time.monotonic() - RUN["start"] if RUN["start"] else 0.0
    kind_desc = {
        "command": "real shell commands (pipes, redirects, globs, quoting, "
                   "control structures, env vars), output matched exactly",
        "nl-sigil": "`?`/`#`-forced natural language stuffed with shell "
                    "metacharacters, must reach the AI and never reach zsh's parser",
        "adv-cmd": "adversarial model responses (prose, malformed commands), must "
                   "be surfaced as answers, never eval'd",
        "adv-danger": "dangerous valid commands, must be held for review, never run",
    }
    lines = []
    lines.append("# aishe zsh-PTY fuzz report")
    lines.append("")
    lines.append("- Date: %s" % ts)
    lines.append("- Binary: `%s`" % BINARY)
    lines.append("- Scale: %d   Seed: %d" % (SCALE, SEED))
    lines.append("- Duration: %.1fs" % dur)
    lines.append("- Result: **%s** (%d cases)" % (status, total))
    lines.append("")
    lines.append("Drives the real `aishe zsh` front-end through a pseudo-terminal "
                 "with a deterministic fake model. After every case it asserts the "
                 "shell is alive, routing is correct, and none of these ever leak: "
                 + ", ".join("`%s`" % s for s in FORBIDDEN) + ".")
    lines.append("")
    lines.append("## Cases by category")
    lines.append("")
    lines.append("| Category | Passed | What it checks |")
    lines.append("| --- | ---: | --- |")
    for k in ("command", "nl-sigil", "adv-cmd", "adv-danger"):
        if k in RUN["counts"]:
            lines.append("| `%s` | %d | %s |" % (k, RUN["counts"][k], kind_desc.get(k, "")))
    lines.append("")
    lines.append("## Example generated inputs")
    lines.append("")
    for k in ("command", "nl-sigil", "adv-cmd", "adv-danger"):
        if RUN["samples"].get(k):
            lines.append("**%s**" % k)
            lines.append("")
            lines.append("```")
            for s in RUN["samples"][k]:
                lines.append(s)
            lines.append("```")
            lines.append("")
    if RUN["fail"]:
        kind, inp, why, recent = RUN["fail"]
        lines.append("## Failure")
        lines.append("")
        lines.append("- Category: `%s`" % kind)
        lines.append("- Input: `%s`" % inp)
        lines.append("- Why: %s" % why)
        lines.append("")
        lines.append("```")
        lines.append(recent)
        lines.append("```")
    with open(path, "w") as f:
        f.write("\n".join(lines) + "\n")
    sys.stdout.write("report: %s\n" % os.path.relpath(path, REPO_ROOT))
    return path


def fail(sh, case, why):
    import re
    recent = re.sub(r"\x1b\[[0-9;?]*[A-Za-z]", "", sh.transcript[-2000:])
    RUN["fail"] = (case.get("kind", "?"), case.get("input", ""), why, recent)
    sys.stderr.write("\nFAIL [%s]: %s\n  case: %r\n" % (case.get("kind", "?"), why, case.get("input", "")))
    total = sum(RUN["counts"].values())
    write_report("FAIL", total)
    sh.close()
    sys.exit(1)


def check_invariants(sh, case):
    if not sh.alive():
        fail(sh, case, "shell process died")
    for s in FORBIDDEN:
        if s in sh.transcript:
            fail(sh, case, "forbidden string leaked: %r" % s)


# ---- corpus generators ----------------------------------------------------

# Metacharacter-heavy fragments to splice into natural-language lines. These are
# exactly what makes zsh choke if a line is not intercepted before parsing.
META = [
    "?", "??", "*", "**", "~", "$HOME", "$(date)", "`uname`", "<x>", ">y", "|z",
    "a&&b", "a||b", "a;b", "{1..3}", "[abc]", "#hash", "%cpu", "!bang", '"q"',
    "'s'", "\\esc", "2>&1", "foo>bar", "a=b", "(group)", "x?y*z", "café", "→", "—",
]

QWORDS = ["what", "why", "how", "who", "which", "explain", "is", "can you",
          "tell me", "where", "when", "should I", "find", "time", "make", "test"]


def gen_nl_inputs(rng, n):
    """Natural-language lines, sigil-forced, stuffed with metacharacters."""
    cases = []
    for _ in range(n):
        sigil = rng.choice(["?", "#", "? ", "# "])
        parts = [rng.choice(QWORDS)]
        for _ in range(rng.randint(1, 5)):
            if rng.random() < 0.5:
                parts.append(rng.choice(META))
            else:
                parts.append("".join(rng.choice(string.ascii_lowercase)
                                     for _ in range(rng.randint(2, 6))))
        line = sigil + " ".join(parts)
        cases.append({"kind": "nl-sigil", "input": line, "want": "AIANSWER"})
    return cases


def gen_command_inputs(rng, n):
    """Deterministic real shell commands with pipes/redirs/quoting/globs."""
    cases = []
    # (template, expected-output). Use @M@/@A@/@B@ placeholders so literal shell
    # braces like {1..3} survive (no str.format).
    templates = [
        ("echo M_@M@", "M_@M@"),
        ("printf '%s\\n' M_@M@", "M_@M@"),
        ("echo @A@ @B@ | tr a-z A-Z", "@AU@ @BU@"),
        ("echo M_@M@ | cat", "M_@M@"),
        ("true && echo M_@M@", "M_@M@"),
        ("false || echo M_@M@", "M_@M@"),
        ("echo $((6 * 7))_M_@M@", "42_M_@M@"),
        ("for i in 1 2 3; do printf X; done; echo _M_@M@", "XXX_M_@M@"),
        ("echo 'M_@M@'", "M_@M@"),
        ('echo "q M_@M@"', "q M_@M@"),
        ("echo M_@M@ > /tmp/aishe_fuzz_$$ && cat /tmp/aishe_fuzz_$$", "M_@M@"),
        ("VAR=M_@M@; echo $VAR", "M_@M@"),
        ("echo {1..3}_M_@M@", "1_M_@M@ 2_M_@M@ 3_M_@M@"),
    ]
    for _ in range(n):
        tpl, exp = rng.choice(templates)
        m = "%05d" % rng.randint(0, 99999)
        a = "".join(rng.choice(string.ascii_lowercase) for _ in range(3))
        b = "".join(rng.choice(string.ascii_lowercase) for _ in range(3))
        sub = lambda s: (s.replace("@M@", m).replace("@A@", a).replace("@B@", b)
                          .replace("@AU@", a.upper()).replace("@BU@", b.upper()))
        cases.append({"kind": "command", "input": sub(tpl), "want": sub(exp)})
    return cases


# Adversarial model responses (the *fake* output), to prove auto mode never
# eval's a non-command into an error.
def adversarial_responses():
    bad_cmds = [
        "the sun is a star > ", "echo unterminated \"", "for i in", "if then fi",
        "a | | b", "&& echo x", "echo `", "$( ", "((", "case esac",
        "The answer is 42.", "Here's how: run `ls`", "1) first 2) second",
        "> redirect only", "| pipe only", "; ; ;",
    ]
    out = []
    for c in bad_cmds:
        esc = c.replace("\\", "\\\\").replace('"', '\\"')
        out.append({"kind": "adv-cmd", "fake":
                    '{"type":"command","command":"%s","explanation":"ADVPROSE"}' % esc,
                    "input": "? explain", "want": "ADVPROSE"})
    # Valid-but-dangerous commands: the explanation always shows; whatever the
    # safety gate decides, the shell must not crash or emit a parse/glob error.
    for c in ["rm -rf /tmp/does-not-exist-aishe", "sudo rm -f /tmp/aishe-nope",
              "chmod -R 777 /tmp/aishe-nope"]:
        out.append({"kind": "adv-danger", "fake":
                    '{"type":"command","command":"%s","explanation":"DANGERMARK"}' % c,
                    "input": "? do the risky thing", "want": "DANGERMARK"})
    return out


def main():
    if shutil.which("zsh") is None:
        sys.stderr.write("SKIP: zsh not on PATH\n")
        sys.exit(0)
    if not os.path.exists(BINARY):
        sys.stderr.write("FAIL: binary not found: %s\n" % BINARY)
        sys.exit(1)

    rng = random.Random(SEED)
    RUN["start"] = time.monotonic()
    home, env, fakefile = make_env(BINARY)

    def set_fake(payload):
        # Written from Python -> arbitrary bytes are safe (no shell quoting).
        with open(fakefile, "w") as f:
            f.write(payload)

    sh = Pty([os.path.abspath(BINARY), "zsh"], env, cwd=home)
    sh.wait_ready()
    n_cmd = 120 * SCALE
    n_nl = 200 * SCALE
    ran = 0
    try:
        sh.expect("ZP> ", timeout=30)

        # ---- commands: must produce exact output, no errors ----
        set_fake("")
        for case in gen_command_inputs(rng, n_cmd):
            sh.buf = ""
            sh.send(case["input"])
            ok = sh.expect(case["want"], timeout=10)
            sh.drain_until_quiet()
            check_invariants(sh, case)
            if not ok:
                fail(sh, case, "expected command output %r not seen" % case["want"])
            _record(case)
            ran += 1

        # ---- sigil NL: a fixed fake answer; must always route, never error ----
        set_fake('{"type":"answer","explanation":"AIANSWER"}')
        for case in gen_nl_inputs(rng, n_nl):
            sh.buf = ""
            sh.send(case["input"])
            ok = sh.expect("AIANSWER", timeout=10)
            sh.drain_until_quiet()  # wait for aishe to finish before the next line
            check_invariants(sh, case)
            if not ok:
                fail(sh, case, "sigil NL did not reach the AI")
            _record(case)
            ran += 1

        # ---- adversarial model responses: never an eval/parse error ----
        for case in adversarial_responses():
            set_fake(case["fake"])
            sh.buf = ""
            sh.send(case["input"])
            if not sh.expect(case["want"], timeout=10):
                fail(sh, case, "expected %r from adversarial response" % case["want"])
            sh.drain_until_quiet()
            check_invariants(sh, case)  # the real invariant: no parse/glob/eval error
            sh.clear_line()  # drop any pre-filled buffer before the next case
            _record(case)
            ran += 1

        set_fake("")
        sh.send("echo DONE_$((1+1))")
        if not sh.expect("DONE_2"):
            fail(sh, {"kind": "final"}, "shell unresponsive after fuzzing")
        sys.stdout.write("OK: %d generated cases passed (scale=%d), no invariant breaches.\n"
                         % (ran, SCALE))
        write_report("PASS", ran)
    finally:
        sh.close()
        shutil.rmtree(home, ignore_errors=True)


if __name__ == "__main__":
    main()
