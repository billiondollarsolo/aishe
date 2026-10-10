#!/usr/bin/env python3
"""Qualify conventional scripts and reversible native activation in isolated homes."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import pathlib
import shutil
import stat
import subprocess
import tempfile

from harness_identity import require_current_binary


def qualify(binary: pathlib.Path) -> dict:
    cases = []
    with tempfile.TemporaryDirectory(prefix="aishe-shell-adoption-") as directory:
        root = pathlib.Path(directory).resolve()
        home = root / "home"
        home.mkdir()
        env = dict(os.environ)
        for key in tuple(env):
            if key.startswith("AISHE_") or key.endswith("_API_KEY") or key in ("ZDOTDIR", "BASH_ENV", "ENV"):
                env.pop(key)
        env.update(
            HOME=str(home),
            XDG_CONFIG_HOME=str(root / "config"),
            XDG_DATA_HOME=str(root / "data"),
            XDG_RUNTIME_DIR=str(root / "runtime"),
            AISHE_CONFIG_DIR=str(root / "config"),
            AISHE_DATA_DIR=str(root / "data"),
            AISHE_RUNTIME_DIR=str(root / "runtime"),
            NO_COLOR="1",
        )

        def run(name, args=(), text=None, expected=0, output=None, custom_env=None):
            result = subprocess.run(
                [str(binary), *args],
                cwd=root,
                env=custom_env or env,
                input=text,
                capture_output=True,
                text=True,
                timeout=15,
                check=False,
            )
            if result.returncode != expected:
                raise AssertionError(
                    f"{name}: exit {result.returncode} != {expected}; "
                    f"stdout={result.stdout!r}; stderr={result.stderr!r}"
                )
            if output is not None and result.stdout != output:
                raise AssertionError(f"{name}: stdout {result.stdout!r} != {output!r}")
            cases.append(dict(name=name, exit_code=result.returncode, status="pass"))
            return result

        run(
            "multiline-stdin-preserves-one-shell",
            text="value=from-script\nif true; then\nprintf '%s\\n' \"$value\"\nfi\n"
            "emit() { cat <<'EOF'\nheredoc body\nEOF\n}\nemit\n",
            output="from-script\nheredoc body\n",
        )
        run("stdin-exit-status", text="exit 17\n", expected=17, output="")
        unknown = run(
            "unknown-script-command-never-routes-to-ai",
            text="aishe_missing_program_for_script_test\nprintf 'still shell\\n'\nexit 19\n",
            expected=19,
            output="still shell\n",
        )
        assert "command not found" in unknown.stderr
        assert "provider" not in unknown.stderr.lower()
        run("stdin-arguments", ["-s", "--", "argument with spaces"],
            "printf '%s\\n' \"$1\"\n", output="argument with spaces\n")
        run("command-positional-arguments", ["-c", "printf '%s:%s\\n' \"$0\" \"$1\"",
            "script-name", "argument with spaces"], output="script-name:argument with spaces\n")
        script = root / "script with spaces.sh"
        script.write_text("if true; then\nprintf '%s\\n' \"$1\"\nfi\nexit 23\n")
        run("script-filename-and-arguments", [str(script), "file argument"],
            expected=23, output="file argument\n")
        syntax = subprocess.run([str(binary)], input="if true; then\n", capture_output=True,
                                text=True, cwd=root, env=env, timeout=15, check=False)
        assert syntax.returncode != 0, "incomplete syntax incorrectly succeeded"
        assert "provider" not in syntax.stderr.lower()
        cases.append(dict(name="incomplete-script-is-a-shell-error", exit_code=syntax.returncode,
                          status="pass"))
        if shutil.which("zsh"):
            (home / ".zprofile").write_text("export AISHE_LOGIN_TEST=profile\n")
            (home / ".zlogin").write_text("export AISHE_LOGIN_TEST=\"${AISHE_LOGIN_TEST}:login\"\n")
            (home / ".zshrc").write_text("printf 'unexpected interactive startup\\n'\n")
            run("login-command-startup-and-arguments", ["-lc",
                "printf '%s:%s\\n' \"$AISHE_LOGIN_TEST\" \"$1\"", "name", "login argument"],
                output="profile:login:login argument\n")
            run("login-command-preserves-shell-negation", ["-lc", "! true"],
                expected=1, output="")
        bash = shutil.which("bash")
        if bash:
            fallback = root / "bash-only-bin"
            fallback.mkdir()
            (fallback / "bash").symlink_to(bash)
            bash_env = {**env, "PATH": str(fallback)}
            run("stdin-bash-fallback", text="if true; then printf 'bash script\\n'; fi\n",
                output="bash script\n", custom_env=bash_env)
        assert not (root / "config").exists(), "script input initialized configuration"
        assert not (root / "data").exists(), "script input initialized AI or history state"
        assert not (root / "runtime").exists(), "script input initialized a backend runtime"
        cases.append(dict(name="scripts-create-no-aishe-state", status="pass"))
        disconnected = run("unconnected-ai-is-actionable-failure",
            ["-c", "? Explain this directory"], expected=1)
        assert "auth.unavailable" in disconnected.stderr and "/setup" in disconnected.stderr
        run("ordinary-shell-recovers-after-unconnected-ai", ["-c", "printf 'shell still available\\n'"],
            output="shell still available\n")
        assert not (root / "config").exists(), "AI error or direct command wrote fresh config"

        config_dir = root / "config" / "aishe"
        config_dir.mkdir(parents=True)
        config_file = config_dir / "config.toml"
        config_file.write_text('[aishe]\nshell_profile = "bash"\n')
        bash_script = "value=BASH_SELECTED\nreference=value\nprintf '%s\\n' \"${!reference}\"\n"
        run("saved-bash-profile-controls-script-syntax", text=bash_script,
            output="BASH_SELECTED\n")
        run("saved-bash-profile-controls-command-syntax", ["-c",
            "value=BASH_SELECTED; reference=value; printf '%s\\n' \"${!reference}\""],
            output="BASH_SELECTED\n")
        config_file.write_text("invalid AI configuration = [\n")
        run("malformed-ai-config-does-not-block-scripts", text="printf 'shell available\\n'\n",
            output="shell available\n")
        assert config_file.read_text() == "invalid AI configuration = [\n"
        config_file.unlink()

        rcfile = home / ".zshrc"
        original = "alias keep_alias='printf preserved'\nexport KEEP_STARTUP=1"
        rcfile.write_text(original)
        rcfile.chmod(0o640)
        preview = run("activation-preview-is-read-only", ["activate", "zsh", "--json"])
        document = json.loads(preview.stdout)
        assert document["operation"] == "preview" and not document["changed"]
        assert rcfile.read_text() == original
        assert not list(home.glob("*.aishe-backup-*"))
        for shell in ("zsh", "bash"):
            executable = shutil.which(shell)
            if executable:
                parsed = subprocess.run([executable, "-n"], input=document["block"],
                                        capture_output=True, text=True, check=False, timeout=10)
                assert parsed.returncode == 0, parsed.stderr
        applied = json.loads(run("activation-private-backup", ["activate", "zsh", "--apply", "--json"]).stdout)
        backup = pathlib.Path(applied["backup"])
        assert backup.read_text() == original
        assert stat.S_IMODE(backup.stat().st_mode) == 0o600
        assert stat.S_IMODE(rcfile.stat().st_mode) == 0o640
        assert rcfile.read_text().startswith(document["block"])
        updated = rcfile.read_text()
        idempotent = json.loads(run("activation-apply-is-idempotent", ["activate", "zsh", "--apply", "--json"]).stdout)
        assert not idempotent["changed"] and idempotent["backup"] is None
        assert rcfile.read_text() == updated
        # The block must leave scripts alone even when the shell reads this file.
        executable = shutil.which("zsh") or bash
        guarded = subprocess.run([executable, "-c", document["block"] + "printf 'guard passed\\n'"],
                                 cwd=root, env=env, capture_output=True, text=True, timeout=10,
                                 check=False)
        assert guarded.returncode == 0 and guarded.stdout == "guard passed\n"
        cases.append(dict(name="activation-skips-noninteractive-shells", status="pass"))
        rcfile.write_text(updated + "\n# later user edit\n")
        run("activation-remove-preserves-user-edits", ["activate", "zsh", "--remove", "--json"])
        assert rcfile.read_text() == original + "\n# later user edit\n"
        run("activation-remove-is-idempotent", ["activate", "zsh", "--remove", "--json"])
        rcfile.write_text(original + "\n# >>> AIShe native shell >>>\n")
        malformed = subprocess.run([str(binary), "activate", "zsh", "--apply"],
                                   cwd=root, env=env, capture_output=True, text=True, timeout=10,
                                   check=False)
        assert malformed.returncode != 0
        assert rcfile.read_text() == original + "\n# >>> AIShe native shell >>>\n"
        cases.append(dict(name="malformed-activation-is-preserved", status="pass"))
        rcfile.unlink()
        target = home / "real-rc"
        target.write_text(original)
        rcfile.symlink_to(target)
        symlink = subprocess.run([str(binary), "activate", "zsh", "--apply"],
                                 cwd=root, env=env, capture_output=True, text=True, timeout=10,
                                 check=False)
        assert symlink.returncode != 0 and target.read_text() == original
        cases.append(dict(name="activation-does-not-overwrite-symlinks", status="pass"))
    return dict(schema_version=1, binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(),
                cases=cases, outcome="pass")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=pathlib.Path)
    parser.add_argument("--json", type=pathlib.Path)
    args = parser.parse_args()
    binary = pathlib.Path(require_current_binary(args.binary))
    report = qualify(binary)
    if args.json:
        args.json.parent.mkdir(parents=True, exist_ok=True)
        args.json.write_text(json.dumps(report, indent=2) + "\n")
    print(f"shell-adoption: PASS ({len(report['cases'])} cases)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
