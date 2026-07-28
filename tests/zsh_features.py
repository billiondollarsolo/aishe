#!/usr/bin/env python3
"""zsh feature-matrix test for the aishe zsh-PTY front-end.

In the zsh-PTY front-end aishe runs the user's real zsh and only injects a few
hooks (a command_not_found handler, an accept-line wrapper for the ?/# sigil, a
precmd, and a force-NL key). This test drives the *wrapped* zsh through a broad
matrix of real zsh functionality -- pipes, redirects, here-docs, process
substitution, command substitution, globbing, brace expansion, parameter
expansion, arrays, multi-line control structures and functions, aliases, the
directory stack, job control, history expansion, quoting, arithmetic -- and
asserts each behaves exactly as plain zsh would, so the injected hooks never
break normal shell use. Multi-line cases are typed line-by-line, which is the
real stress test for the accept-line wrapper (it must not eat continuation).

Writes a markdown report to test-results/zsh-features-<ts>.md.
Usage: zsh_features.py [path-to-aishe]   Exit 0 on success. Skips if zsh absent.
"""

import os
import re
import sys
import pty
import time
import select
import signal
import shutil
import tempfile
import datetime
import subprocess

BINARY = sys.argv[1] if len(sys.argv) > 1 else "target/release/aishe"
REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
REPORT_DIR = os.path.join(REPO_ROOT, "test-results")
TIMEOUT = 20.0
FORBIDDEN = ["parse error", "(eval):", "command not found: #", "command not found: ?"]


class Pty:
    def __init__(self, argv, env, cwd=None):
        self.master, slave = pty.openpty()
        self.proc = subprocess.Popen(argv, stdin=slave, stdout=slave, stderr=slave,
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
        text = chunk.decode("utf-8", "replace")
        self.buf += text
        self.transcript += text
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

    def settle(self, secs=0.3):
        deadline = time.monotonic() + secs
        while time.monotonic() < deadline:
            self._drain(deadline)

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
    home = tempfile.mkdtemp(prefix="aishe-feat-")
    cfgdir = os.path.join(home, ".config", "aishe")
    os.makedirs(cfgdir, exist_ok=True)
    with open(os.path.join(cfgdir, "config.toml"), "w") as f:
        f.write('[aishe]\nmode = "auto"\nprovider = "anthropic"\n'
                'front_end = "zsh-pty"\npty_prompt = false\n')
    with open(os.path.join(home, ".zshrc"), "w") as f:
        f.write("HISTFILE=~/.zsh_history\nHISTSIZE=2000\nSAVEHIST=2000\n"
                "setopt INTERACTIVE_COMMENTS\nPROMPT='ZP> '\nPROMPT2='> '\n")
    bindir = os.path.join(home, "bin")
    os.makedirs(bindir, exist_ok=True)
    os.symlink(os.path.abspath(binary), os.path.join(bindir, "aishe"))
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
    })
    return home, env


# Each case: (name, [lines to type], expected substring in output).
def cases():
    return [
        # --- pipes & redirection ---
        ("pipe", ["echo hi | tr a-z A-Z"], "HI"),
        ("redirect out+in", ["echo REDIR > f1 && cat f1"], "REDIR"),
        ("append", ["echo A1 > f2; echo A2 >> f2; cat f2 | tr '\\n' ,"], "A1,A2,"),
        ("stderr redirect", ["echo E 1>&2 2>/dev/null; echo OUT"], "OUT"),
        ("pipe count", ["printf 'a\\nb\\nc\\n' | wc -l | tr -d ' '"], "3"),
        ("here-string", ["tr a-z A-Z <<< heredoc"], "HEREDOC"),
        ("process substitution", ["diff <(echo s) <(echo s) && echo SAME"], "SAME"),
        # --- here-doc (multi-line) ---
        ("here-doc", ["cat <<EOF", "HD_LINE_1", "EOF"], "HD_LINE_1"),
        # --- command substitution ---
        ("dollar-paren", ['echo "cs=$(echo VAL)"'], "cs=VAL"),
        ("backtick", ["echo BT_`echo X`_END"], "BT_X_END"),
        # --- globbing & expansion ---
        ("brace list", ["echo {a,b,c}"], "a b c"),
        ("brace range", ["echo {1..4}"], "1 2 3 4"),
        ("glob match", ["touch g1.zf g2.zf; echo *.zf"], "g1.zf g2.zf"),
        ("tilde", ["echo ~"], "/"),
        # --- parameter / arrays ---
        ("var", ["V=hello; echo $V"], "hello"),
        ("braced var", ["W=wor; echo ${W}ld"], "world"),
        ("substring", ["s=abcdef; echo ${s:1:3}"], "bcd"),
        ("length", ["t=12345; echo ${#t}"], "5"),
        ("array index", ["arr=(x y z); echo $arr[2]"], "y"),
        ("array join", ["arr=(p q r); echo ${(j:-:)arr}"], "p-q-r"),
        # --- arithmetic ---
        ("arith expansion", ["echo $((3 ** 4))"], "81"),
        ("arith command", ["(( n = 6 * 7 )); echo $n"], "42"),
        # --- control structures (multi-line: stresses accept-line) ---
        ("for loop", ["for i in 1 2 3; do", "  printf L$i", "done; echo ."], "L1L2L3."),
        ("if/elif/else", ["if false; then", "  echo NO", "elif true; then", "  echo ELIF", "fi"], "ELIF"),
        ("while loop", ["c=0; while (( c < 3 )); do", "  printf W; (( c++ ))", "done; echo ."], "WWW."),
        ("case", ["x=b; case $x in", "  a) echo CA ;;", "  b) echo CB ;;", "esac"], "CB"),
        # --- functions (multi-line) ---
        ("function def+call", ["greet() {", '  echo "hi $1"', "}", "greet bob"], "hi bob"),
        # --- aliases ---
        ("alias", ["alias zz='echo aliased42'", "zz"], "aliased42"),
        # --- directory stack ---
        ("cd + pwd", ["cd /tmp && pwd"], "/tmp"),
        ("pushd/popd", ["pushd /etc >/dev/null && pwd && popd >/dev/null"], "/etc"),
        ("cd dash", ["cd /tmp; cd /etc; cd - >/dev/null; pwd"], "/tmp"),
        # --- exit codes & logic ---
        ("exit code", ["false; echo rc=$?"], "rc=1"),
        ("and-list", ["true && echo ANDOK"], "ANDOK"),
        ("or-list", ["false || echo OROK"], "OROK"),
        # --- history expansion ---
        ("bang-bang", ["echo HB_marker", "!!"], "HB_marker"),
        ("bang-dollar", ["echo one two three_$$X", "echo last=!$"], "three_"),
        # --- quoting ---
        ("single quote literal", ["q=Z; echo 'lit $q'"], "lit $q"),
        ("double quote expand", ["q=Z; echo \"exp $q\""], "exp Z"),
        ("multiline quote", ['echo "ml1', 'ml2"'], "ml1"),
        # --- misc builtins ---
        ("printf", ["printf '%s-%d\\n' hi 7"], "hi-7"),
        ("read from heredoc", ["read a b <<< 'p q'; echo $b$a"], "qp"),
        ("comment ignored", ["echo CMT # this is a comment"], "CMT"),
        # --- job control ---
        ("background job", ["sleep 0.2 & echo BG_$!"], "BG_"),
        ("jobs + wait", ["sleep 0.2 & jobs >/dev/null; wait; echo WAITED"], "WAITED"),
    ]


PASSED = []
FAILED = []


def run():
    home, env = make_env(BINARY)
    sh = Pty([os.path.abspath(BINARY), "zsh"], env, cwd=home)
    started = time.monotonic()
    try:
        sh.expect("ZP> ", timeout=30)
        sh.wait_ready()
        for name, lines, want in cases():
            sh.buf = ""
            before = len(sh.transcript)
            for ln in lines:
                sh.send(ln)
                sh.settle(0.15)
            ok = sh.expect(want, timeout=12)
            sh.settle(0.2)
            seg = sh.transcript[before:]
            forbidden = [s for s in FORBIDDEN if s in seg]
            if ok and not forbidden:
                PASSED.append(name)
                sys.stdout.write("  ok   %s\n" % name)
            else:
                why = ("expected %r not seen" % want) if not ok else ("leaked %r" % forbidden)
                FAILED.append((name, lines, want, why, re.sub(r"\x1b\[[0-9;?]*[A-Za-z]", "", seg)[-600:]))
                sys.stdout.write("  FAIL %s  (%s)\n" % (name, why))
            # reset to a clean prompt in case a case left a partial line
            os.write(sh.master, b"\x03")
            sh.settle(0.15)
        sh.send("exit")
        sh.settle(0.4)
    finally:
        dur = time.monotonic() - started
        sh.close()
        shutil.rmtree(home, ignore_errors=True)
    return dur


def write_report(dur):
    os.makedirs(REPORT_DIR, exist_ok=True)
    ts = datetime.datetime.utcnow().strftime("%Y%m%dT%H%M%SZ")
    path = os.path.join(REPORT_DIR, "zsh-features-%s.md" % ts)
    total = len(PASSED) + len(FAILED)
    status = "PASS" if not FAILED else "FAIL"
    out = ["# aishe zsh feature-matrix report", "",
           "- Date: %s" % ts, "- Binary: `%s`" % BINARY,
           "- Duration: %.1fs" % dur,
           "- Result: **%s** (%d/%d passed)" % (status, len(PASSED), total), "",
           "Drives the real `aishe zsh` front-end (the user's zsh + aishe's hooks) "
           "through a matrix of zsh features and asserts each behaves like plain zsh, "
           "so the injected hooks never break normal shell use. Multi-line cases are "
           "typed line-by-line to stress the accept-line wrapper.", "",
           "## Features verified", ""]
    for n in PASSED:
        out.append("- [x] %s" % n)
    for n, *_ in FAILED:
        out.append("- [ ] %s (FAILED)" % n)
    if FAILED:
        out += ["", "## Failures", ""]
        for name, lines, want, why, seg in FAILED:
            out += ["### %s" % name, "", "- input: `%s`" % " ⏎ ".join(lines),
                    "- expected: `%s`" % want, "- why: %s" % why, "",
                    "```", seg, "```", ""]
    with open(path, "w") as f:
        f.write("\n".join(out) + "\n")
    sys.stdout.write("report: %s\n" % os.path.relpath(path, REPO_ROOT))


def main():
    if shutil.which("zsh") is None:
        sys.stderr.write("SKIP: zsh not on PATH\n")
        sys.exit(0)
    if not os.path.exists(BINARY):
        sys.stderr.write("FAIL: binary not found: %s\n" % BINARY)
        sys.exit(1)
    dur = run()
    write_report(dur)
    sys.stdout.write("\n%d/%d zsh features OK\n" % (len(PASSED), len(PASSED) + len(FAILED)))
    sys.exit(1 if FAILED else 0)


if __name__ == "__main__":
    main()
