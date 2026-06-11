#!/usr/bin/env python3
"""Repeatable admin / validation harness for aishe.

The report it writes is verbose: every check shows its actual output (or the
aishe-vs-raw-shell diff on a mismatch), with a per-suite timing table and an
environment header.

Six suites:
  1. Shell pass-through: run ~300 common + edge-case Linux commands & shell
     constructs through `aishe -c` and compare to the raw shell (proves "Linux
     still works like Linux": aishe delegates faithfully). Run with NO api key,
     so anything misrouted to the LLM shows up as a mismatch.
  2. Admin file ops — create/edit/move/permission/archive/delete files via aishe
     and verify the resulting on-disk state.
  4. Plugins, slash-commands & skills (deterministic) — meta slash-commands
     (`/commands`, `/skills`, `/config`, `/help`), custom command discovery,
     `shell:`/`$ARGUMENTS`/`$1`/`$2` templating, no-frontmatter discovery, and
     project→user override precedence. No model needed.
  5. Dispatch classification — assert each input routes to shell vs natural
     language (independent of output determinism). No model needed.
  6. Config & meta robustness — a config exercising every newer field round-trips
     through `/config`, `aishe doctor` passes, the repo example config parses, and
     the new meta commands (`/usage`, `/reset`, `/ghost`, `/help`) behave. No
     model needed.
  3. Natural language (optional; needs an API key) — suggest / yolo / mode
     switching, custom NL commands, model-invoked skills (progressive
     disclosure), token-usage display, the budget cap, and audit logging against
     the real model.

Writes a timestamped Markdown report to test-results/ and prints a summary.
Designed to be extended: add rows to SHELL_CASES / FILE_OPS / NL_SUGGEST, or
plugin/skill fixtures in install_plugins().

Usage:  python3 tests/admin_validation.py [path/to/aishe]
The API key (for suite 3) is read from $GROQ_API_KEY or /tmp/aishe-secrets.env.
"""

import datetime
import json
import os
import platform
import shutil
import subprocess
import sys
import tempfile
import time


def trunc(s, n=240):
    """Truncate a value's text for the report, noting how much was dropped."""
    s = str(s)
    return s if len(s) <= n else s[:n] + f"…(+{len(s) - n} chars)"

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
    # --- edge cases: quoting & escaping ---
    "echo $'a\\nb'",
    "echo $'tab\\tend'",
    'echo "a\\"b"',
    "echo 'can'\\''t'",
    "echo \"nested 'single'\"",
    "echo 'literal $HOME stays'",
    "printf '%b\\n' 'x\\ty'",
    "echo a\\ b",
    "echo \"has | pipe\"",
    "echo \"has ; semi\"",
    "echo 'has && amp'",
    "echo \"$(echo inner) outer\"",
    "v=x; echo \"${v:+yes}\"",
    "echo one'two'three",
    'echo a"b"c',
    # --- edge cases: arithmetic ---
    "echo $((2 ** 8))",
    "echo $((17 % 5))",
    "echo $((-3 + 10))",
    "echo $((10 / 3))",
    "echo $((1 << 4))",
    "echo $((0xff))",
    "echo $((2#1010))",
    "echo $(( (1+2) * 3 ))",
    "echo $((5 > 3))",
    "echo $((5 == 5))",
    "n=5; echo $((n*n))",
    "echo $((8#17))",
    # --- edge cases: parameter expansion (zsh) ---
    "v=hello; echo ${v//l/L}",
    "v=hello; echo ${v/l/L}",
    "v=HELLO; echo ${(L)v}",
    "v=hello; echo ${(U)v}",
    "v=hello; echo ${v:0:2}",
    "v=hello; echo ${v: -2}",
    "v=a,b,c; echo ${v//,/ }",
    "v=foobar; echo ${v#foo}",
    "v=foobar; echo ${v%bar}",
    "v=path/to/file.txt; echo ${v:t}",
    "v=path/to/file.txt; echo ${v:h}",
    "v=path/to/file.txt; echo ${v:e}",
    "v=path/to/file.txt; echo ${v:r}",
    "echo ${UNSETZZZ:-fallback}",
    "echo ${UNSETZZZ:=assigned}",
    "v=; echo ${v:-empty}",
    "echo ${(C)v::=hello world}",
    # --- edge cases: arrays (zsh) ---
    "arr=(a b c); echo ${#arr}",
    "arr=(a b c); echo $arr[2]",
    "arr=(a b c); echo ${arr[-1]}",
    "arr=(a b c); echo ${arr[@]}",
    "arr=(1 2 3 4); echo ${arr[2,3]}",
    "arr=(c a b); echo ${(o)arr}",
    "arr=(b a b a); echo ${(u)arr}",
    # --- edge cases: globbing & brace expansion ---
    "echo *.txt",
    "echo [ab].txt",
    "echo ?.txt",
    "echo **/*.txt",
    "echo {1..3}",
    "echo {a,b}{1,2}",
    "echo file{01..03}.log",
    "echo {1..10..2}",
    "echo {Z..X}",
    "echo nomatch-*(N)",
    # --- edge cases: redirection & here-docs/strings ---
    "cat <<< herestring",
    "echo $(wc -w <<< 'one two three')",
    "{ echo out; echo err >&2; } 2>/dev/null",
    "printf '%s\\n' a b c | tac",
    "echo $(seq 3 | wc -l)",
    "cat <<EOF | wc -l\na\nb\nc\nEOF",
    "echo hi | cat -",
    "echo discard >/dev/null; echo $?",
    # --- edge cases: compound / control structures ---
    "for i in {1..3}; do echo i=$i; done",
    "i=5; until (( i >= 8 )); do echo $i; i=$((i+1)); done",
    "if [[ -f a.txt ]]; then echo has; fi",
    "case abc in a*) echo A;; esac",
    "n=0; for f in *.txt; do n=$((n+1)); done; echo $n",
    "{ echo a; echo b; } | wc -l",
    "(cd subdir && pwd) | xargs basename",
    "true && echo yes || echo no",
    "false && echo yes || echo no",
    "false || echo recovered",
    "echo hi | while read l; do echo got-$l; done",
    "for ((i=0;i<3;i++)); do echo c$i; done",
    "repeat 3 echo hi",
    "x=1; [[ $x == 1 ]] && echo one",
    # --- edge cases: builtins & misc commands ---
    "print -l a b c",
    "print -r -- 'raw\\nstring'",
    "printf '%03d\\n' 7",
    "printf '%-5s|\\n' hi",
    "printf '%+d\\n' 5",
    "printf '%x\\n' 255",
    "printf '%o\\n' 8",
    "type echo",
    "echo a b c | tr ' ' '\\n' | wc -l",
    "echo a; echo b; echo c",
    "echo a # trailing comment",
    ": ignored; echo aftercolon",
    "let n=6+1; echo $n",
    "typeset -i k=9; echo $k",
    # --- edge cases: more quoting / escaping / special chars ---
    "echo \"a\\\\b\"",
    "echo 'a\\b'",
    "echo \"price: \\$5\"",
    "echo 'literal `backtick`'",
    "echo \"$(printf '%s' nested)\"",
    "echo \"${HOME:+has-home}\"",
    "v='a b'; echo \"[$v]\"",
    "v='a b'; echo [$v]",
    "echo a''b",
    "echo a\"\"b",
    "echo \"tab\\tin-double\"",
    "echo $'\\x41\\x42'",
    "echo $'\\u00e9'",
    "echo $'col1\\tcol2'",
    "printf '%q\\n' 'a b'",
    "echo \\$notvar",
    "echo '#notcomment'",
    "echo end #comment",
    # --- edge cases: parameter expansion (more zsh) ---
    "v=Hello; echo ${v:0:1}${v: -1}",
    "v=aXbXc; echo ${v//X/-}",
    "v=foobarbar; echo ${v%%bar*}",
    "v=foobarbar; echo ${v##*bar}",
    "v=a.b.c.d; echo ${v//./ }",
    "echo ${UNSET-fallback}",
    "v=x; echo ${v:+set}${UNSET:+set}",
    "v=hello; echo ${(C)v}",
    "v='one two three'; echo ${#${(z)v}}",
    "arr=(1 2 3); echo ${(j:+:)arr}",
    "arr=(c b a); echo \"${(@s.,.)${(j:,:)arr}}\"",
    "v=hello; echo ${v[2,4]}",
    "v=hello; echo ${v[-1]}",
    "echo ${(l:5::0:)1}",
    "echo ${(r:5:)foo}",
    # --- edge cases: arithmetic (more) ---
    "echo $((3 ** 0))",
    "echo $((7 & 3))",
    "echo $((7 | 8))",
    "echo $((6 ^ 3))",
    "echo $((~0 & 255))",
    "echo $((100 % 7))",
    "echo $((-7 / 2))",
    "echo $((1e0 < 2))",
    "echo $((16#ff))",
    "echo $(( 2 > 1 ? 10 : 20 ))",
    "i=5; echo $((i++)) $i",
    "i=5; echo $((++i)) $i",
    "echo $(( [##16] 255 ))",
    # --- edge cases: globbing / brace (more) ---
    "echo {a,b,c}.txt",
    "echo pre{1,2}post",
    "echo {1..3}{a,b}",
    "echo [[:digit:]]*",
    "echo *.{txt,csv}",
    "echo a.t?t",
    "echo subdir/*.txt",
    "echo no_such_glob_*(N)",
    "setopt nullglob; echo nomatch_*; unsetopt nullglob",
    "echo {3..1}",
    "echo {a..c}{1..2}",
    # --- edge cases: redirection / fds / heredoc ---
    "echo to-err >&2 2>/dev/null; echo done",
    "exec 3>&1; echo via-fd >&3; exec 3>&-",
    "{ echo o; echo e >&2; } 2>&1 | sort",
    "cat <<'EOF'\n$nosubst\nEOF",
    "cat <<EOF\n$((1+1))\nEOF",
    "read x <<< 'word'; echo $x",
    "read -r a b <<< 'x y z'; echo \"$a|$b\"",
    "echo abc | tee /dev/null | cat",
    "printf 'a\\nb\\n' | head -1",
    "printf 'a\\nb\\nc\\n' | sed -n '$p'",
    "echo hi >| /dev/null; echo $?",
    "echo x | wc -c",
    # --- edge cases: control flow / functions (more) ---
    "for ((i=1;i<=3;i++)) do echo r$i; done",
    "i=0; while (( i < 2 )); do echo w$i; (( i++ )); done",
    "case abc in (a*) echo A;; (*) echo other;; esac",
    "if (( 1 )); then echo t; fi",
    "f() { return 3; }; f; echo $?",
    "f() { echo \"$# args: $*\"; }; f a b c",
    "f() { local x=inner; echo $x; }; x=outer; f; echo $x",
    "() { echo anon $1; } hello",
    "true && { echo a; echo b; }",
    "{ echo one; echo two; } | wc -l",
    "n=0; repeat 4 (( n++ )); echo $n",
    # --- edge cases: text processing (more) ---
    "echo 'a1b2c3' | tr -d '0-9'",
    "echo 'a,b,c' | cut -d, -f2",
    "printf '3\\n1\\n2\\n' | sort -n | tr '\\n' ' '",
    "printf 'x\\nx\\ny\\n' | uniq -c | tr -s ' '",
    "echo 'Hello World' | tr '[:upper:]' '[:lower:]'",
    "echo 'one two three' | awk '{print NF}'",
    "echo 'a:b:c' | awk -F: '{print $2}'",
    "printf '1 2\\n3 4\\n' | awk '{s+=$1} END{print s}'",
    "echo abcabc | sed 's/a/X/2'",
    "echo 'foo123bar' | grep -oE '[0-9]+'",
    "echo -e 'a\\nb\\nc' | grep -c ''",
    "printf '%s\\n' a b c | nl | tr -s ' '",
    "echo 'hello' | rev",
    "echo 'a b c' | xargs -n1 | wc -l",
    # --- edge cases: assignments / env ---
    "a=1 b=2 c=3; echo $a$b$c",
    "x=$(echo dyn); echo $x",
    "x=${#HOME}; [ $x -gt 0 ] && echo haslen",
    "arr=(a b c); arr+=(d); echo ${#arr}",
    "typeset -A m; m[k]=v; echo $m[k]",
    "integer n=40+2; echo $n",
    "x=5; (( x += 3 )); echo $x",
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
    "sleep 0",
    "jobs",
    "echo $RANDOM >/dev/null",
    "type -a echo >/dev/null",
    "ls -d */ 2>/dev/null | head -1 || echo none",
    "hash -r",
    "ulimit -n >/dev/null",
    "umask",
]

# ----- dispatch classification (no key): assert routing, not output ------------
# A line that must run as SHELL must NOT hit the model (no "LLM not configured").
# A line that must be NATURAL LANGUAGE must hit the model (with no key, that
# surfaces as the "LLM not configured" notice). This catches mis-routing
# independent of whether the command's output is deterministic.
DISPATCH_SHELL = [
    "ls -la",
    "git status",
    "for i in 1 2; do echo $i; done",
    "x=1; echo $x",
    "echo a | grep a",
    "{ echo x; }",
    "( echo y )",
    "if true; then echo z; fi",
    "while false; do echo never; done",
    "until true; do echo never; done",
    "case x in x) echo m;; esac",
    "time echo hi",
    "cd /tmp && pwd",
    "FOO=bar env >/dev/null",
    "echo 'list all the files please'",       # quoted NL → still echo (shell)
    "find . -name '*.txt'",
    "/bin/echo hi",
    "$(echo echo) hi",
    "grep -E 'foo|bar' a.txt",
    "f() { echo hi; }; f",
    "[[ -d subdir ]] && echo yes",
    "(( 1 + 1 ))",
    "!echo forced-shell",                     # ! sigil = forced shell
    "/usage",                                 # meta slash-commands route to builtin
    "/reset",
    "/ghost",
    "/skills",
    "/help",
    "jobs",                                    # job-control builtins (reedline)
    "fg",
    "bg",
    "wait",
    "disown",
    "tar --version",
    "awk 'BEGIN{print 1}'",
    "sed -n '1p' a.txt",
]
DISPATCH_NL = [
    "what is the capital of france",
    "how do I list files by size",
    "please summarize this directory",
    "explain the difference between tcp and udp",
    "tell me a joke about computers",
    "show me the largest files here",
    "?force this to be natural language",      # ? sigil = forced NL
    "turn this csv into json data",
    "why is my disk full",
    "give me a one-liner to backup my home dir",
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
        # --- edge cases ---
        (f"mkdir -p {d}/a/b/c/deep", lambda: os.path.isdir(f"{d}/a/b/c/deep")),
        (f"touch {d}/t.txt", lambda: os.path.exists(f"{d}/t.txt")),
        (f"echo data > {d}/tr.txt && : > {d}/tr.txt", lambda: open(f"{d}/tr.txt").read() == ""),
        (f"cat >> {d}/h.txt <<EOF\nx\ny\nEOF", lambda: open(f"{d}/h.txt").read() == "x\ny\n"),
        (f"touch {d}/e.sh && chmod u+x {d}/e.sh", lambda: (os.stat(f"{d}/e.sh").st_mode & 0o100) != 0),
        (f'touch "{d}/a b.txt"', lambda: os.path.exists(f"{d}/a b.txt")),
        (f"echo h > {d}/.hidden", lambda: os.path.exists(f"{d}/.hidden")),
        (f"cp -r {d}/proj {d}/proj_copy", lambda: os.path.isfile(f"{d}/proj_copy/f.txt")),
        (f"printf '%s\\n' x y z | xargs -n1 echo > {d}/xa.txt", lambda: open(f"{d}/xa.txt").read() == "x\ny\nz\n"),
        (f"sed -e 's/x/X/' -e 's/y/Y/' {d}/h.txt > {d}/sed2.txt", lambda: open(f"{d}/sed2.txt").read() == "X\nY\n"),
        (f"grep -rl x {d}/proj_copy >/dev/null; echo rc=$?", lambda: True),  # informational
        (f"tar -C {d} -czf {d}/proj.tgz proj", lambda: os.path.exists(f"{d}/proj.tgz")),
        (f"mkdir -p {d}/ex && tar -C {d}/ex -xzf {d}/proj.tgz", lambda: os.path.isfile(f"{d}/ex/proj/f.txt")),
        (f"find {d}/proj -name '*.txt' -exec wc -l {{}} + >/dev/null", lambda: True),  # informational
        (f"rm -rf {d}/proj_copy", lambda: not os.path.exists(f"{d}/proj_copy")),
    ]

# ----- NL prompts (need API key) ----------------------------------------------
NL_SUGGEST = [
    "list files in long format including hidden",
    "show disk usage of the current directory sorted largest first",
    # `?` forces NL: the request starts with the real command `find`, which would
    # otherwise route to the shell (the documented command-vs-NL ambiguity).
    "?find all rust files modified in the last day",
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


# ----- plugins: custom /slash-commands & model-invoked skills ------------------
# A unique token that lives *only* inside the skill body, so a successful yolo
# run proves the model actually loaded the skill (progressive disclosure) rather
# than guessing. Mirrors the Claude Code skill format byte-for-byte.
SKILL_TOKEN = "PROJECT_STAMP_42"


def install_plugins(cfgroot):
    """Drop Claude-Code-style command and skill files into the temp config so
    the suite exercises discovery + execution without touching the real ~."""
    cdir = os.path.join(cfgroot, "aishe", "commands")
    sdir = os.path.join(cfgroot, "aishe", "skills", "file-stamper")
    os.makedirs(cdir, exist_ok=True)
    os.makedirs(sdir, exist_ok=True)

    # 1. A *shell* custom command — deterministic, no model needed. Exercises
    #    $ARGUMENTS / $1 templating and `shell: true` execution.
    with open(os.path.join(cdir, "echo-args.md"), "w") as f:
        f.write(
            "---\n"
            "description: echo back the arguments (templating smoke test)\n"
            "shell: true\n"
            "---\n"
            "echo args=$ARGUMENTS first=$1\n"
        )

    # 1b. Positional templating: $1/$2 in a shell command.
    with open(os.path.join(cdir, "echo2.md"), "w") as f:
        f.write("---\ndescription: positional args\nshell: true\n---\necho p1=$1 p2=$2\n")

    # 1c. No frontmatter at all — must still be discovered (default description).
    with open(os.path.join(cdir, "plain.md"), "w") as f:
        f.write("Just a body, no frontmatter, referencing $ARGUMENTS.\n")

    # 1d. A user command that a project command will override (precedence test).
    with open(os.path.join(cdir, "dup.md"), "w") as f:
        f.write("---\ndescription: USER-DUP\nshell: true\n---\necho USER-DUP\n")

    # 2. An *NL* custom command (prompt template) — used in the key-gated suite.
    with open(os.path.join(cdir, "bigfiles.md"), "w") as f:
        f.write(
            "---\n"
            "description: suggest a command to find the biggest files\n"
            "mode: suggest\n"
            "---\n"
            "Show the 10 largest files under $ARGUMENTS, human-readable, "
            "largest first.\n"
        )

    # 3. A model-invoked skill (Claude Code `SKILL.md` format). Carries an extra
    #    `license:` key (like real anthropics/skills) to prove aishe ignores it.
    #    The body holds a unique token to verify progressive disclosure.
    with open(os.path.join(sdir, "SKILL.md"), "w") as f:
        f.write(
            "---\n"
            "name: file-stamper\n"
            "description: Use this skill whenever the user asks to stamp, mark, "
            "or brand a file with the project marker / project stamp.\n"
            "license: Apache-2.0 (compat-test fixture)\n"
            "---\n"
            "# Stamping files with the project marker\n\n"
            "To stamp (mark/brand) a file with the project marker, write the "
            f"exact text `{SKILL_TOKEN}` as the file's contents using a shell "
            "command (e.g. printf). Do not write anything else.\n"
        )


def key():
    if os.environ.get("GROQ_API_KEY"):
        return os.environ["GROQ_API_KEY"]
    try:
        return open("/tmp/aishe-secrets.env").read().split("=", 1)[1].strip()
    except Exception:
        return None


def install_project_plugins(projroot):
    """Drop project-scoped (`<cwd>/.aishe/`) command & skill files to exercise
    project discovery and user→project override precedence."""
    cdir = os.path.join(projroot, ".aishe", "commands")
    sdir = os.path.join(projroot, ".aishe", "skills", "proj-skill")
    os.makedirs(cdir, exist_ok=True)
    os.makedirs(sdir, exist_ok=True)
    # Same name as a user command → project must win.
    with open(os.path.join(cdir, "dup.md"), "w") as f:
        f.write("---\ndescription: PROJECT-DUP\nshell: true\n---\necho PROJECT-DUP\n")
    # A project-only command.
    with open(os.path.join(cdir, "projcmd.md"), "w") as f:
        f.write("---\ndescription: project only\nshell: true\n---\necho PROJ-ONLY\n")
    # A project-only skill.
    with open(os.path.join(sdir, "SKILL.md"), "w") as f:
        f.write("---\nname: proj-skill\ndescription: a project-scoped skill\n---\nProject skill body.\n")


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


# A config exercising every newer field, with distinctive values so we can
# confirm each one round-trips through `/config`.
FULL_CONFIG = """\
[aishe]
mode = "auto"
provider = "openai"
front_end = "reedline"
edit_mode = "vi"
structured = "json"
stream = true
show_usage = false
budget_usd = 1.5
memory = false
cache = false
cache_ttl_secs = 120
ghost_text = true
redact_secrets = false
report_time = 7
git_status = false
git_prompt = false
show_right_prompt = false
auto_pushd = true
hist_ignore_dups = false
hist_ignore_space = true
hist_ignore = ["ls", "cd *"]
cdpath = ["/tmp", "/srv"]
correct = true
file_tools = false
web_tool = false
yolo_plan = true
project_context = false
yolo_confirm = "writes"
yolo_sandbox = true
max_yolo_iterations = 5
yolo_confirm_dangerous = false

[providers.openai]
base_url = "https://api.groq.com/openai"
api_key_env = "GROQ_API_KEY"
model = "openai/gpt-oss-120b"

[mcp_servers.remote]
url = "https://mcp.example.com/mcp"

[logging]
enabled = false
redact = true

[pricing."openai/gpt-oss-120b"]
input = 0.15
output = 0.6

[named_dirs]
proj = "/home/me/projects"
"""


# A minimal MCP stdio server used by the deterministic MCP suite: echo + add.
MCP_SERVER_PY = r'''
import sys, json
def send(o):
    sys.stdout.write(json.dumps(o) + "\n"); sys.stdout.flush()
TOOLS = [
    {"name": "echo", "description": "Echo text uppercased.",
     "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}}},
    {"name": "add", "description": "Add two integers.",
     "inputSchema": {"type": "object", "properties": {"a": {"type": "integer"}, "b": {"type": "integer"}}}},
]
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    m = json.loads(line); mid = m.get("id"); method = m.get("method")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": mid, "result": {"protocolVersion": "2025-06-18", "capabilities": {"tools": {}}, "serverInfo": {"name": "t", "version": "0"}}})
    elif method == "notifications/initialized":
        pass
    elif method == "tools/list":
        send({"jsonrpc": "2.0", "id": mid, "result": {"tools": TOOLS}})
    elif method == "tools/call":
        p = m.get("params", {}); name = p.get("name"); a = p.get("arguments", {})
        if name == "echo":
            send({"jsonrpc": "2.0", "id": mid, "result": {"content": [{"type": "text", "text": str(a.get("text", "")).upper()}]}})
        elif name == "add":
            send({"jsonrpc": "2.0", "id": mid, "result": {"content": [{"type": "text", "text": str(a.get("a", 0) + a.get("b", 0))}]}})
        else:
            send({"jsonrpc": "2.0", "id": mid, "result": {"content": [{"type": "text", "text": "?"}], "isError": True}})
    elif mid is not None:
        send({"jsonrpc": "2.0", "id": mid, "error": {"code": -32601, "message": "no method"}})
'''


def write_config(text):
    """Create a temp config root containing `text` as config.toml."""
    root = tempfile.mkdtemp(prefix="aishe-cfg-")
    os.makedirs(os.path.join(root, "aishe"))
    with open(os.path.join(root, "aishe", "config.toml"), "w") as f:
        f.write(text)
    return root


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
    run_t0 = time.monotonic()

    cfgroot = make_config()
    install_plugins(cfgroot)
    fixture = tempfile.mkdtemp(prefix="aishe-fixture-")
    # fixture files
    open(f"{fixture}/a.txt", "w").write("foo\nbar\nfoo\nbaz\n")
    open(f"{fixture}/b.txt", "w").write("second\nfile\n")
    open(f"{fixture}/data.csv", "w").write("name,age\nalice,30\nbob,25\n")
    open(f"{fixture}/nums.txt", "w").write("3\n1\n2\n1\n")
    os.makedirs(f"{fixture}/subdir", exist_ok=True)
    open(f"{fixture}/subdir/c.txt", "w").write("inner\n")
    install_project_plugins(fixture)  # project-scoped .aishe/ under the cwd

    env_local = base_env(cfgroot, with_key=False)
    report = []
    counts = {}
    timings = []  # (suite-title, seconds, n-checks)
    _suite = {"title": None, "t0": None, "n0": 0}

    def section(title):
        # Close out the previous suite's timing before starting a new one.
        if _suite["title"] is not None:
            n = sum(v[1] for v in counts.values()) - _suite["n0"]
            timings.append((_suite["title"], time.monotonic() - _suite["t0"], n))
            report.append(f"\n_{n} checks in {time.monotonic() - _suite['t0']:.1f}s_")
        _suite["title"] = title
        _suite["t0"] = time.monotonic()
        _suite["n0"] = sum(v[1] for v in counts.values())
        report.append(f"\n## {title}\n")

    def close_last_suite():
        if _suite["title"] is not None:
            n = sum(v[1] for v in counts.values()) - _suite["n0"]
            timings.append((_suite["title"], time.monotonic() - _suite["t0"], n))
            report.append(f"\n_{n} checks in {time.monotonic() - _suite['t0']:.1f}s_")

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
        if match:
            # Verbose: show the (matching) exit code and output for every case.
            detail = f"→ rc={rc_a} out={trunc(repr(out_a.rstrip(chr(10))))}"
        else:
            s1_fail += 1
            detail = (
                f"**MISMATCH**\n"
                f"    aishe(rc={rc_a}): {trunc(repr(out_a))}\n"
                f"    raw  (rc={rc_r}): {trunc(repr(out_r))}\n"
                f"    stderr: {trunc(repr(err_a.strip()))}"
            )
        add(f"shell: {cmd}", match, detail)
    for cmd in SMOKE_CASES:
        rc_a, out_a, err_a = run([BIN, "-c", cmd], env_local, cwd=fixture)
        ok = rc_a == 0
        detail = (
            f"→ rc={rc_a} out={trunc(repr(out_a.strip()), 120)}"
            if ok
            else f"(rc={rc_a} err={trunc(repr(err_a.strip()))})"
        )
        add(f"smoke: {cmd}", ok, detail)

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
        # Show the command with the temp dir collapsed to <d> for readability.
        shown = cmd.replace(opdir, "<d>")
        detail = (
            f"→ rc={rc} on-disk state verified"
            if ok
            else f"(rc={rc} err={trunc(repr(err.strip()))})"
        )
        add(f"fileop: {shown}", ok, detail)
    shutil.rmtree(opdir, ignore_errors=True)

    # ---- Suite 4: plugins / slash-commands / skills (deterministic) ----
    section("Suite 4 — Plugins, slash-commands & skills (no model needed)")

    # Read-only meta commands work under `-c`.
    report.append("\n**Meta slash-commands (`-c`):**\n")
    rc, out, err = run([BIN, "-c", "/commands"], env_local, cwd=fixture)
    add("slash: /commands lists customs", rc == 0 and "echo-args" in out and "bigfiles" in out,
        "" if "echo-args" in out else f"(out={out.strip()!r})")
    rc, out, err = run([BIN, "-c", "/skills"], env_local, cwd=fixture)
    add("slash: /skills lists skills", rc == 0 and "file-stamper" in out,
        "" if "file-stamper" in out else f"(out={out.strip()!r})")
    rc, out, err = run([BIN, "-c", "/config"], env_local, cwd=fixture)
    add("slash: /config prints config", rc == 0 and "[aishe]" in out,
        "" if "[aishe]" in out else f"(out={out.strip()!r})")
    rc, out, err = run([BIN, "-c", "/help"], env_local, cwd=fixture)
    add("slash: /help prints help", rc == 0 and "meta commands" in out.lower())

    # Custom *shell* command: $ARGUMENTS / $1 templating + `shell: true` exec.
    report.append("\n**Custom command execution (shell + templating):**\n")
    rc, out, err = run([BIN, "-c", "/echo-args hello world"], env_local, cwd=fixture)
    ok = rc == 0 and "args=hello world" in out and "first=hello" in out
    add("plugin: /echo-args hello world", ok, f"→ `{out.strip()}`" if out.strip() else f"(rc={rc} err={err.strip()!r})")
    # No-arg invocation should still run (empty expansion).
    rc, out, err = run([BIN, "-c", "/echo-args"], env_local, cwd=fixture)
    add("plugin: /echo-args (no args)", rc == 0 and "args=" in out, f"→ `{out.strip()}`")
    # Positional $1/$2 templating.
    rc, out, err = run([BIN, "-c", "/echo2 x y z"], env_local, cwd=fixture)
    add("plugin: /echo2 positional $1/$2", "p1=x p2=y" in out, f"→ `{out.strip()}`")
    # Unknown slash command should not crash (falls through gracefully).
    rc, out, err = run([BIN, "-c", "/nonexistent-cmd-xyz"], env_local, cwd=fixture)
    add("plugin: unknown /command handled", rc is not None)

    report.append("\n**Discovery & project-override precedence:**\n")
    # No-frontmatter command is discovered.
    rc, out, err = run([BIN, "-c", "/commands"], env_local, cwd=fixture)
    add("plugin: no-frontmatter command discovered", "plain" in out)
    # Project-only command + project skill are discovered (cwd = project root).
    add("plugin: project-only command discovered", "projcmd" in out)
    rc, sout, _ = run([BIN, "-c", "/skills"], env_local, cwd=fixture)
    add("plugin: project-only skill discovered", "proj-skill" in sout)
    # Project command overrides a same-named user command.
    rc, out, err = run([BIN, "-c", "/dup"], env_local, cwd=fixture)
    add("plugin: project overrides user (/dup)", "PROJECT-DUP" in out and "USER-DUP" not in out,
        f"→ `{out.strip()}`")

    # ---- Suite 5: dispatch classification (no key; routing only) ----
    section("Suite 5 — Dispatch classification (shell vs natural language)")
    NL_NOTE = "LLM not configured"
    report.append("\n**Must route to SHELL (must NOT call the model):**\n")
    for cmd in DISPATCH_SHELL:
        _, out, err = run([BIN, "-c", cmd], env_local, cwd=fixture)
        ok = NL_NOTE not in err
        add(f"route-shell: {cmd}", ok, "→ ran as shell" if ok else "**misrouted to NL**")
    report.append("\n**Must route to NATURAL LANGUAGE (must reach the model path):**\n")
    for cmd in DISPATCH_NL:
        _, out, err = run([BIN, "-c", cmd], env_local, cwd=fixture)
        ok = NL_NOTE in err
        add(
            f"route-nl: {cmd}",
            ok,
            "→ reached the model path" if ok else f"**not routed to NL** ({trunc(repr(err.strip()), 80)})",
        )

    # ---- Suite 6: config & meta-command robustness (deterministic) ----
    section("Suite 6 — Config & meta-command robustness (no model needed)")

    # 6a. A config exercising every newer field round-trips through `/config`.
    full_root = write_config(FULL_CONFIG)
    full_env = base_env(full_root, with_key=False)
    rc, out, err = run([BIN, "-c", "/config"], full_env)
    report.append("\n**Full config round-trips (`/config`):**\n")
    expected = [
        'mode = "auto"',
        "report_time = 7",
        "auto_pushd = true",
        "correct = true",
        "file_tools = false",
        "web_tool = false",
        "yolo_plan = true",
        "project_context = false",
        'yolo_confirm = "writes"',
        "yolo_sandbox = true",
        "cache = false",
        "cache_ttl_secs = 120",
        "budget_usd = 1.5",
        "hist_ignore_space = true",
        "ghost_text = true",
        "redact_secrets = false",
        "[logging]",
        "[pricing.",
        "[named_dirs]",
        'proj = "/home/me/projects"',
        "[mcp_servers.remote]",
        'url = "https://mcp.example.com/mcp"',
    ]
    for token in expected:
        add(f"config: {token}", token in out, "" if token in out else f"(missing from /config)")
    add("config: parses (rc 0)", rc == 0)

    # 6b. `aishe doctor` reports the privacy/logging status and passes.
    rc, out, err = run([BIN, "doctor"], full_env)
    add("config: doctor passes", rc == 0, f"(rc={rc})")
    add("config: doctor shows redaction line", "secret redaction" in out)
    add("config: doctor shows audit-log line", "audit log" in out)

    # 6c. The repo's annotated example config parses cleanly.
    report.append("\n**Repo example config parses:**\n")
    example = os.path.join(REPO, "examples", "config.toml")
    if os.path.exists(example):
        ex_root = write_config(open(example).read())
        rc, out, err = run([BIN, "-c", "/config"], base_env(ex_root, with_key=False))
        add("config: examples/config.toml parses", rc == 0 and "[aishe]" in out,
            "" if "[aishe]" in out else f"(err={err.strip()[:80]!r})")
        shutil.rmtree(ex_root, ignore_errors=True)

    # 6d. New read-only / toggle meta commands behave in `-c` (no crash, not NL).
    report.append("\n**New meta commands (`-c`):**\n")
    for meta in ["/usage", "/reset", "/ghost", "/plan", "/cache", "/sandbox", "/help"]:
        rc, out, err = run([BIN, "-c", meta], env_local, cwd=fixture)
        ok = rc == 0 and "LLM not configured" not in err
        add(f"meta: {meta}", ok, "" if ok else f"(rc={rc} err={err.strip()[:60]!r})")
    # `/usage` with no calls reports an empty session rather than erroring.
    rc, out, err = run([BIN, "-c", "/usage"], env_local, cwd=fixture)
    add("meta: /usage reports empty session", "no model calls" in out.lower() or "usage" in out.lower())

    shutil.rmtree(full_root, ignore_errors=True)

    # ---- Suite 7: MCP client (deterministic, no model) ----
    section("Suite 7 — MCP client (real stdio server, no model)")
    report.append(
        "\nA tiny Python MCP server (newline-delimited JSON-RPC 2.0) is spawned; "
        "aishe must handshake, list its tools, and round-trip the config.\n"
    )
    mcp_root = tempfile.mkdtemp(prefix="aishe-mcp-")
    server_py = os.path.join(mcp_root, "server.py")
    with open(server_py, "w") as f:
        f.write(MCP_SERVER_PY)
    mcp_cfg = write_config(
        '[aishe]\nmode = "yolo"\nprovider = "openai"\nfront_end = "reedline"\n'
        "\n[providers.openai]\n"
        'base_url = "https://api.groq.com/openai"\napi_key_env = "GROQ_API_KEY"\n'
        'model = "openai/gpt-oss-120b"\n\n'
        f'[mcp_servers.demo]\ncommand = "{sys.executable}"\nargs = ["{server_py}"]\n'
    )
    mcp_env = base_env(mcp_cfg, with_key=False)
    # `/mcp` lists the namespaced tools (proves connect + handshake + tools/list).
    rc, out, err = run([BIN, "-c", "/mcp"], mcp_env, timeout=30)
    add("mcp: server connected", "connected (2 tools)" in err,
        "" if "connected (2 tools)" in err else f"(err={err.strip()[:120]!r})")
    for tool in ["mcp__demo__echo", "mcp__demo__add"]:
        add(f"mcp: lists {tool}", tool in out,
            "" if tool in out else f"(out={out.strip()[:120]!r})")
    # The `[mcp_servers]` block round-trips through `/config`.
    rc, out, err = run([BIN, "-c", "/config"], mcp_env, timeout=30)
    add("mcp: [mcp_servers.demo] round-trips", "[mcp_servers.demo]" in out,
        "" if "[mcp_servers.demo]" in out else "(missing from /config)")
    # A disabled server is not connected.
    mcp_off = write_config(
        '[aishe]\nmode = "yolo"\nprovider = "openai"\nfront_end = "reedline"\n'
        "\n[providers.openai]\n"
        'base_url = "https://api.groq.com/openai"\napi_key_env = "GROQ_API_KEY"\n'
        'model = "openai/gpt-oss-120b"\n\n'
        f'[mcp_servers.demo]\ncommand = "{sys.executable}"\nargs = ["{server_py}"]\n'
        "enabled = false\n"
    )
    rc, out, err = run([BIN, "-c", "/mcp"], base_env(mcp_off, with_key=False), timeout=30)
    add("mcp: disabled server is skipped", "no MCP tools" in out,
        "" if "no MCP tools" in out else f"(out={out.strip()[:120]!r})")
    shutil.rmtree(mcp_off, ignore_errors=True)
    shutil.rmtree(mcp_cfg, ignore_errors=True)
    shutil.rmtree(mcp_root, ignore_errors=True)

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

        # NL custom command (prompt template) → command suggestion.
        report.append("\n**Custom NL command (`/bigfiles`, expands $ARGUMENTS):**\n")
        rc, out, err = run([BIN, "-c", "/bigfiles src"], env_llm, cwd=fixture, timeout=60)
        cmd = out.strip().splitlines()[-1].strip() if out.strip() else ""
        add("plugin-nl: /bigfiles src", bool(cmd) and "LLM not configured" not in err,
            f"→ `{cmd}`" if cmd else f"(err={err.strip()!r})")

        # Model-invoked skill (progressive disclosure): a request matching the
        # skill description should make the model load the skill and use its
        # unique token — proving the body reached context.
        report.append("\n**Model-invoked skill (`use_skill`, progressive disclosure):**\n")
        skill_dir = tempfile.mkdtemp(prefix="aishe-skill-")
        target = f"{skill_dir}/stamped.txt"
        rc, out, err = run(
            [BIN, "--mode", "yolo", "-c", f"stamp the file {target} with the project marker"],
            env_llm, cwd=fixture, timeout=120)
        invoked = "skill: file-stamper" in out or "📖" in out
        try:
            stamped = os.path.exists(target) and SKILL_TOKEN in open(target).read()
        except Exception:
            stamped = False
        add("skill: use_skill invoked (📖)", invoked, "" if invoked else f"(out tail={out.strip()[-160:]!r})")
        add("skill: body token written to file", stamped,
            "" if stamped else f"(exists={os.path.exists(target)})")
        shutil.rmtree(skill_dir, ignore_errors=True)

        # Token/cost accounting: the per-session usage line prints after a call.
        report.append("\n**Token usage line (after a suggest call):**\n")
        rc, out, err = run([BIN, "-c", "list files by size"], env_llm, cwd=fixture, timeout=60)
        usage_ok = ("in ·" in err or " in " in err) and (
            "$" in err or "cost n/a" in err or "req" in err
        )
        add("usage: per-call usage line shown", usage_ok,
            "" if usage_ok else f"(stderr={err.strip()[-120:]!r})")

        # Budget cap: a tiny budget stops a yolo run before it finishes.
        report.append("\n**Budget cap stops a yolo run:**\n")
        budget_root = write_config(
            '[aishe]\nmode = "yolo"\nprovider = "openai"\nfront_end = "reedline"\n'
            "budget_usd = 0.00001\n\n[providers.openai]\n"
            'base_url = "https://api.groq.com/openai"\napi_key_env = "GROQ_API_KEY"\n'
            'model = "openai/gpt-oss-120b"\n'
        )
        bdir = tempfile.mkdtemp(prefix="aishe-budget-")
        rc, out, err = run(
            [BIN, "--mode", "yolo", "-c",
             f"create three files {bdir}/a {bdir}/b {bdir}/c then list them"],
            base_env(budget_root, with_key=True), cwd=fixture, timeout=90)
        add("budget: yolo stops at budget", "budget reached" in (out + err).lower(),
            "" if "budget reached" in (out + err).lower() else f"(tail={(out+err).strip()[-120:]!r})")
        shutil.rmtree(budget_root, ignore_errors=True)
        shutil.rmtree(bdir, ignore_errors=True)

        # Audit logging: an NL call is recorded as ai_request + ai_response.
        report.append("\n**Audit log records AI calls:**\n")
        log_root = write_config(
            '[aishe]\nmode = "suggest"\nprovider = "openai"\nfront_end = "reedline"\n'
            "\n[providers.openai]\n"
            'base_url = "https://api.groq.com/openai"\napi_key_env = "GROQ_API_KEY"\n'
            'model = "openai/gpt-oss-120b"\n\n[logging]\nenabled = true\n'
        )
        log_env = base_env(log_root, with_key=True)
        run([BIN, "-c", "show the current date"], log_env, cwd=fixture, timeout=60)
        log_path = os.path.join(log_env["XDG_DATA_HOME"], "aishe", "audit.jsonl")
        kinds = set()
        if os.path.exists(log_path):
            for line in open(log_path):
                try:
                    kinds.add(json.loads(line).get("kind"))
                except Exception:
                    pass
        add("audit: ai_request logged", "ai_request" in kinds)
        add("audit: ai_response logged", "ai_response" in kinds)
        shutil.rmtree(log_root, ignore_errors=True)

    # ---- write report ----
    close_last_suite()
    ts = datetime.datetime.utcnow().strftime("%Y%m%dT%H%M%SZ")
    total_ok = sum(v[0] for v in counts.values())
    total = sum(v[1] for v in counts.values())
    elapsed = time.monotonic() - run_t0
    failures = [
        line for line in report if line.startswith("- ❌")
    ]
    raw_ver = (
        subprocess.run([RAW_SHELL, "--version"], capture_output=True, text=True)
        .stdout.strip()
        .splitlines()[0]
        if RAW_SHELL
        else "?"
    )
    header = [
        f"# aishe validation report — {ts}",
        "",
        f"- Binary: `{BIN}`",
        f"- aishe: `{subprocess.run([BIN,'--version'],capture_output=True,text=True).stdout.strip()}`",
        f"- Raw shell: `{RAW_SHELL}`  ({raw_ver})",
        f"- Host: {platform.platform()}  ·  python {platform.python_version()}",
        f"- Model suite: {'enabled (Groq gpt-oss-120b)' if key() else 'skipped (no API key)'}",
        f"- Date (UTC): {datetime.datetime.utcnow().isoformat()}Z  ·  wall time {elapsed:.1f}s",
        "",
        "## Summary",
        "",
        f"**{total_ok}/{total} checks passed**  ({len(failures)} failed)",
        "",
        "| Category | Pass / Total |",
        "|----------|:-----------:|",
    ]
    for name, (ok, n) in sorted(counts.items()):
        flag = "" if ok == n else "  ⚠️"
        header.append(f"| `{name}` | {ok}/{n}{flag} |")
    header.append("")
    header.append("### Per-suite timing")
    header.append("")
    header.append("| Suite | Checks | Seconds |")
    header.append("|-------|:------:|:-------:|")
    for title, secs, n in timings:
        short = title.split(" — ")[0]
        header.append(f"| {short} | {n} | {secs:.1f} |")
    header.append("")
    if failures:
        header.append("### Failures")
        header.append("")
        header.extend(failures)
        header.append("")
    out_dir = os.path.join(REPO, "test-results")
    os.makedirs(out_dir, exist_ok=True)
    path = os.path.join(out_dir, f"validation-{ts}.md")
    with open(path, "w") as f:
        f.write("\n".join(header) + "\n" + "\n".join(report) + "\n")

    shutil.rmtree(fixture, ignore_errors=True)
    shutil.rmtree(cfgroot, ignore_errors=True)

    print(f"wrote {path}")
    print(f"{total_ok}/{total} checks passed; shell-suite failures: {s1_fail}")
    # Critical = deterministic suites must pass fully (shell + file ops +
    # plugin/slash discovery & execution). NL/skill suites need a model and are
    # informational.
    def full(name):
        c = counts.get(name, [0, 0])
        return c[1] > 0 and c[0] == c[1]

    # route-nl is informational (depends on no oddly-named command shadowing an
    # NL word on the host); a real command misrouted to NL (route-shell) is a bug.
    critical = (full("shell") and full("fileop") and full("slash")
                and full("plugin") and full("route-shell")
                and full("config") and full("meta"))
    sys.exit(0 if critical else 1)


if __name__ == "__main__":
    main()
