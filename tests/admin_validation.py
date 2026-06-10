#!/usr/bin/env python3
"""Repeatable admin / validation harness for aishe.

Three suites:
  1. Shell pass-through — run ~100 common Linux commands & shell constructs
     through `aishe -c` and compare to the raw shell (proves "Linux still works
     like Linux": aishe delegates faithfully). Run with NO api key, so anything
     misrouted to the LLM shows up as a mismatch.
  2. Admin file ops — create/edit/move/permission/delete files via aishe and
     verify the resulting on-disk state.
  3. Natural language (optional; needs an API key) — suggest / yolo / mode
     switching against the real model.

Writes a timestamped Markdown report to test-results/ and prints a summary.
Designed to be extended: add rows to SHELL_CASES / FILE_OPS / NL_SUGGEST / etc.

Usage:  python3 tests/admin_validation.py [path/to/aishe]
The API key (for suite 3) is read from $GROQ_API_KEY or /tmp/aishe-secrets.env.
"""

import datetime
import os
import shutil
import subprocess
import sys
import tempfile

BIN = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else "target/release/aishe")
RAW_SHELL = shutil.which("zsh") or shutil.which("bash")
REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


# ----- commands compared verbatim against the raw shell (deterministic) -------
SHELL_CASES = [
    # basics
    "echo hello world",
    "printf '%s\\n' one two three",
    "echo $((6 * 7))",
    "echo {1..5}",
    "echo a{b,c}d",
    "true; echo $?",
    "false; echo $?",
    "seq 1 5",
    "seq 1 2 9",
    "seq -s, 1 4",
    "expr 6 + 7",
    "echo -n nonewline",
    "echo 'single $quotes'",
    'echo "double $((1+1))"',
    "printf '%d-%d\\n' 3 4",
    # text files (fixture)
    "cat a.txt",
    "cat -n a.txt",
    "head -n 2 a.txt",
    "tail -n 1 a.txt",
    "head -c 3 a.txt",
    "wc -l a.txt",
    "wc -w a.txt",
    "wc -c a.txt",
    "wc < a.txt",
    "nl a.txt",
    "tac a.txt",
    "rev a.txt",
    "cat a.txt b.txt",
    "grep foo a.txt",
    "grep -c foo a.txt",
    "grep -n foo a.txt",
    "grep -v foo a.txt",
    "grep -i FOO a.txt",
    "grep -o foo a.txt",
    "grep -E 'foo|bar' a.txt",
    "egrep 'ba.' a.txt",
    "sort a.txt",
    "sort -r a.txt",
    "sort -u a.txt",
    "sort nums.txt",
    "sort -n nums.txt",
    "sort -n nums.txt | uniq",
    "sort -n nums.txt | uniq -c",
    "cut -d, -f1 data.csv",
    "cut -d, -f2 data.csv",
    "cut -c1-3 a.txt",
    "awk -F, '{print $1}' data.csv",
    "awk 'NR==2' data.csv",
    "awk -F, '{s+=$2} END{print s}' data.csv",
    "sed 's/o/0/g' a.txt",
    "sed -n '2p' a.txt",
    "sed '1d' a.txt",
    "tr 'a-z' 'A-Z' < a.txt",
    "tr -d aeiou < a.txt",
    "tr -s ' ' < a.txt",
    "fold -w 2 a.txt",
    "paste -sd, nums.txt",
    "tee /dev/null < a.txt",
    "comm <(sort a.txt) <(sort a.txt)",
    "diff <(echo x) <(echo x); echo rc=$?",
    "base64 a.txt | base64 -d",
    "md5sum a.txt | cut -d' ' -f1",
    "sha1sum a.txt | cut -d' ' -f1",
    "sha256sum a.txt | cut -d' ' -f1",
    "cksum a.txt | cut -d' ' -f1",
    "od -An -c a.txt | head -1",
    "xargs echo < nums.txt",
    "yes ok | head -3",
    # filesystem (fixture)
    "ls",
    "ls -1",
    "ls -a",
    "ls subdir",
    "find . -type f -name '*.txt' | sort",
    "find . -maxdepth 1 -type d | sort",
    "find . -name 'a.txt' -printf '%f\\n' 2>/dev/null || find . -name a.txt",
    "stat -c '%n %s' a.txt 2>/dev/null || stat -f '%N' a.txt",
    "file a.txt",
    "test -f a.txt && echo isfile",
    "[ -d subdir ] && echo isdir",
    "basename /usr/local/bin/tool",
    "dirname /usr/local/bin/tool",
    "realpath a.txt",
    "readlink -f a.txt",
    # pipes / redirection / substitution
    "cat data.csv | cut -d, -f2 | tail -n +2 | sort -n | head -1",
    "echo hi > out.tmp && cat out.tmp && rm out.tmp",
    "echo one > out.tmp && echo two >> out.tmp && cat out.tmp && rm out.tmp",
    "cat <<EOF\nheredoc line\nEOF",
    "echo $(echo nested)",
    "echo `echo backtick`",
    "wc -l < a.txt",
    "grep foo a.txt | wc -l",
    # control structures
    "for i in 1 2 3; do echo n$i; done",
    "i=0; while [ $i -lt 3 ]; do echo w$i; i=$((i+1)); done",
    "if [ 3 -gt 2 ]; then echo yes; else echo no; fi",
    "case x in x) echo matched;; *) echo no;; esac",
    "greet() { echo hi-$1; }; greet bob",
    # parameter expansion
    "v=hello; echo ${#v}",
    "v=hello; echo ${v:1:3}",
    "echo ${UNSET_VAR:-default}",
    "v=a.b.c; echo ${v%.*}",
    "v=a.b.c; echo ${v#*.}",
    # env / identity (stable within a run)
    "whoami",
    "id -u",
    "uname -s",
    "test $((2**10)) -eq 1024 && echo pow",
]

# Non-deterministic: just require exit 0 (output not compared).
SMOKE_CASES = [
    "date +%Y",
    "ps -o pid= -p $$",
    "df -h / | tail -1",
    "uptime",
    "env | wc -l",
    "ls -l /usr/bin | head -3",
    "free -m 2>/dev/null | head -1 || echo nofree",
]

# ----- admin file-editing scenario (run via aishe; verified on disk) ----------
def file_ops_script(d):
    return [
        (f"mkdir -p {d}/proj/src", lambda: os.path.isdir(f"{d}/proj/src")),
        (f"echo 'line1' > {d}/proj/f.txt", lambda: open(f"{d}/proj/f.txt").read() == "line1\n"),
        (f"echo 'line2' >> {d}/proj/f.txt", lambda: open(f"{d}/proj/f.txt").read() == "line1\nline2\n"),
        (f"sed -i 's/line1/LINE1/' {d}/proj/f.txt", lambda: open(f"{d}/proj/f.txt").read().startswith("LINE1")),
        (f"cp {d}/proj/f.txt {d}/proj/src/g.txt", lambda: os.path.exists(f"{d}/proj/src/g.txt")),
        (f"mv {d}/proj/src/g.txt {d}/proj/src/h.txt", lambda: os.path.exists(f"{d}/proj/src/h.txt") and not os.path.exists(f"{d}/proj/src/g.txt")),
        (f"ln -s {d}/proj/f.txt {d}/proj/link.txt", lambda: os.path.islink(f"{d}/proj/link.txt")),
        (f"chmod 600 {d}/proj/f.txt", lambda: (os.stat(f'{d}/proj/f.txt').st_mode & 0o777) == 0o600),
        (f"printf 'a\\nb\\nc\\n' > {d}/proj/list.txt && sort -r {d}/proj/list.txt > {d}/proj/sorted.txt",
         lambda: open(f"{d}/proj/sorted.txt").read() == "c\nb\na\n"),
        (f"rm -f {d}/proj/src/h.txt", lambda: not os.path.exists(f"{d}/proj/src/h.txt")),
        (f"find {d}/proj -type f | wc -l", lambda: True),  # informational
    ]

# ----- NL prompts (need API key) ----------------------------------------------
NL_SUGGEST = [
    "list files in long format including hidden",
    "show disk usage of the current directory sorted largest first",
    "find all rust files modified in the last day",
    "count the number of lines in all text files here",
    "show the running processes for the current user",
    "create a gzip archive of the src directory",
    "show the last 20 lines of a log file named app.log",
    "what is my current git branch",
    "recursively search for the word TODO in source files",
    "show the top 5 largest files under the current directory",
]
NL_QUESTIONS = [
    "?in one sentence, what does the chmod command do",
    "?what is the difference between grep and egrep in one sentence",
]


def key():
    if os.environ.get("GROQ_API_KEY"):
        return os.environ["GROQ_API_KEY"]
    try:
        return open("/tmp/aishe-secrets.env").read().split("=", 1)[1].strip()
    except Exception:
        return None


def make_config():
    cfgroot = tempfile.mkdtemp(prefix="aishe-val-")
    cdir = os.path.join(cfgroot, "aishe")
    os.makedirs(cdir)
    with open(os.path.join(cdir, "config.toml"), "w") as f:
        f.write(
            '[aishe]\nmode = "suggest"\nprovider = "openai"\nfront_end = "reedline"\n'
            'structured = "schema"\n\n[providers.openai]\n'
            'base_url = "https://api.groq.com/openai"\napi_key_env = "GROQ_API_KEY"\n'
            'model = "openai/gpt-oss-120b"\n'
        )
    return cfgroot


def base_env(cfgroot, with_key=False):
    env = dict(os.environ)
    env["XDG_CONFIG_HOME"] = cfgroot
    env["XDG_DATA_HOME"] = os.path.join(cfgroot, "data")
    env["HOME"] = cfgroot  # avoid sourcing a real ~/.aishrc
    env["GROQ_API_KEY"] = key() if with_key else ""
    return env


def run(argv, env, cwd=None, timeout=120):
    p = subprocess.run(argv, env=env, cwd=cwd, capture_output=True, text=True, timeout=timeout)
    return p.returncode, p.stdout, p.stderr


def main():
    if not os.path.exists(BIN):
        sys.exit(f"binary not found: {BIN}")
    if not RAW_SHELL:
        sys.exit("no zsh/bash on PATH")

    cfgroot = make_config()
    fixture = tempfile.mkdtemp(prefix="aishe-fixture-")
    # fixture files
    open(f"{fixture}/a.txt", "w").write("foo\nbar\nfoo\nbaz\n")
    open(f"{fixture}/b.txt", "w").write("second\nfile\n")
    open(f"{fixture}/data.csv", "w").write("name,age\nalice,30\nbob,25\n")
    open(f"{fixture}/nums.txt", "w").write("3\n1\n2\n1\n")
    os.makedirs(f"{fixture}/subdir", exist_ok=True)
    open(f"{fixture}/subdir/c.txt", "w").write("inner\n")

    env_local = base_env(cfgroot, with_key=False)
    report = []
    counts = {}

    def section(title):
        report.append(f"\n## {title}\n")

    def add(name, ok, detail=""):
        counts[name.split(":")[0]] = counts.get(name.split(":")[0], [0, 0])
        counts[name.split(":")[0]][1] += 1
        if ok:
            counts[name.split(":")[0]][0] += 1
        mark = "✅" if ok else "❌"
        report.append(f"- {mark} `{name}` {detail}")
        return ok

    # ---- Suite 1: shell pass-through (compare to raw shell) ----
    section("Suite 1 — Shell pass-through (aishe -c vs raw shell)")
    s1_fail = 0
    for cmd in SHELL_CASES:
        rc_a, out_a, err_a = run([BIN, "-c", cmd], env_local, cwd=fixture)
        rc_r, out_r, _ = run([RAW_SHELL, "-c", cmd], env_local, cwd=fixture)
        match = (out_a.rstrip("\n") == out_r.rstrip("\n")) and (rc_a == rc_r)
        if not match:
            s1_fail += 1
            detail = f"\n    aishe(rc={rc_a}): {out_a!r}\n    raw(rc={rc_r}): {out_r!r}\n    stderr: {err_a.strip()!r}"
        else:
            detail = ""
        add(f"shell: {cmd}", match, detail)
    for cmd in SMOKE_CASES:
        rc_a, out_a, err_a = run([BIN, "-c", cmd], env_local, cwd=fixture)
        add(f"smoke: {cmd}", rc_a == 0, "" if rc_a == 0 else f"(rc={rc_a} err={err_a.strip()!r})")

    # ---- Suite 2: admin file ops ----
    section("Suite 2 — Admin file-editing operations (verified on disk)")
    opdir = tempfile.mkdtemp(prefix="aishe-ops-")
    for cmd, check in file_ops_script(opdir):
        rc, out, err = run([BIN, "-c", cmd], env_local, cwd=opdir)
        try:
            ok = (rc == 0) and check()
        except Exception as e:
            ok = False
            err = f"{err} check-exc: {e}"
        add(f"fileop: {cmd}", ok, "" if ok else f"(rc={rc} err={err.strip()!r})")
    shutil.rmtree(opdir, ignore_errors=True)

    # ---- Suite 3: natural language (needs key) ----
    section("Suite 3 — Natural language (real model)")
    k = key()
    if not k:
        report.append("- ⏭️  skipped (no API key)")
    else:
        env_llm = base_env(cfgroot, with_key=True)
        report.append(f"\n_Model: openai/gpt-oss-120b via Groq_\n")
        # suggest: expect a non-empty, non-error command line
        report.append("\n**Suggest (`-c`, structured schema) — NL → command:**\n")
        for nl in NL_SUGGEST:
            rc, out, err = run([BIN, "-c", nl], env_llm, cwd=fixture, timeout=60)
            cmd = out.strip().splitlines()[-1].strip() if out.strip() else ""
            ok = bool(cmd) and "LLM not configured" not in err
            add(f"nl-suggest: {nl}", ok, f"→ `{cmd}`" if ok else f"(err={err.strip()!r})")
        # questions
        report.append("\n**Questions (`?`) — NL → answer:**\n")
        for q in NL_QUESTIONS:
            rc, out, err = run([BIN, "-c", q], env_llm, cwd=fixture, timeout=60)
            ans = (out or err).strip().replace("\n", " ")
            ok = len(ans) > 0
            add(f"nl-question: {q}", ok, f"→ {ans[:160]}")
        # yolo: verifiable side effects
        report.append("\n**Yolo (`--mode yolo`) — agentic, verified side effects:**\n")
        yolo_dir = tempfile.mkdtemp(prefix="aishe-yolo-")
        yolo_tasks = [
            (f"create a file at {yolo_dir}/hello.txt containing the text yolo-works then show it",
             lambda: os.path.exists(f"{yolo_dir}/hello.txt") and "yolo-works" in open(f"{yolo_dir}/hello.txt").read()),
            (f"count how many .txt files are in {fixture} and write just the number to {yolo_dir}/count.txt",
             lambda: os.path.exists(f"{yolo_dir}/count.txt")),
        ]
        for task, check in yolo_tasks:
            rc, out, err = run([BIN, "--mode", "yolo", "-c", task], env_llm, cwd=fixture, timeout=120)
            try:
                ok = check()
            except Exception:
                ok = False
            add(f"yolo: {task[:60]}…", ok, "" if ok else f"(rc={rc})")
        shutil.rmtree(yolo_dir, ignore_errors=True)
        # mode switching is easy?
        report.append("\n**Mode switching (`--mode` flag):**\n")
        for mode in ["suggest", "auto", "yolo"]:
            rc, out, err = run([BIN, "--mode", mode, "-c", "echo mode-ok"], env_llm, cwd=fixture)
            add(f"mode-flag: --mode {mode}", rc == 0 and "mode-ok" in out)

    # ---- write report ----
    ts = datetime.datetime.utcnow().strftime("%Y%m%dT%H%M%SZ")
    total_ok = sum(v[0] for v in counts.values())
    total = sum(v[1] for v in counts.values())
    header = [
        f"# aishe validation report — {ts}",
        "",
        f"- Binary: `{BIN}`",
        f"- Raw shell: `{RAW_SHELL}`  ({subprocess.run([RAW_SHELL,'--version'],capture_output=True,text=True).stdout.strip().splitlines()[0] if RAW_SHELL else '?'})",
        f"- aishe: `{subprocess.run([BIN,'--version'],capture_output=True,text=True).stdout.strip()}`",
        f"- Date (UTC): {datetime.datetime.utcnow().isoformat()}Z",
        "",
        "## Summary",
        "",
        f"**{total_ok}/{total} checks passed**",
        "",
    ]
    for name, (ok, n) in sorted(counts.items()):
        header.append(f"- {name}: {ok}/{n}")
    out_dir = os.path.join(REPO, "test-results")
    os.makedirs(out_dir, exist_ok=True)
    path = os.path.join(out_dir, f"validation-{ts}.md")
    with open(path, "w") as f:
        f.write("\n".join(header) + "\n" + "\n".join(report) + "\n")

    shutil.rmtree(fixture, ignore_errors=True)
    shutil.rmtree(cfgroot, ignore_errors=True)

    print(f"wrote {path}")
    print(f"{total_ok}/{total} checks passed; shell-suite failures: {s1_fail}")
    # Critical = shell + file suites must pass; NL is informational.
    critical = counts.get("shell", [0, 0])[0] == counts.get("shell", [0, 0])[1] and \
               counts.get("fileop", [0, 0])[0] == counts.get("fileop", [0, 0])[1]
    sys.exit(0 if critical else 1)


if __name__ == "__main__":
    main()
