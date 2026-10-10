#!/usr/bin/env python3
"""Account-free startup, persistent shell choices, and safe login adoption."""

import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import tomllib

from setup_pty import BINARY, Pty, native_env


def environment(root):
    env = native_env(str(root))
    for name in list(env):
        if name.startswith("AISHE_") and name not in {
            "AISHE_CONFIG_DIR", "AISHE_DATA_DIR", "AISHE_RUNTIME_DIR",
            "AISHE_LEGACY_OPENCODE", "AISHE_LEAN", "AISHE_SPY_OPENCODE",
        }:
            env.pop(name)
    for name in ("ANTHROPIC_API_KEY", "OPENAI_API_KEY", "XAI_API_KEY"):
        env.pop(name, None)
    env.update({
        "PATH": str(Path(BINARY).parent) + os.pathsep + env["PATH"],
        "NO_COLOR": "1",
        "AISHE_POLICY_FILE": str(root / "no-policy.toml"),
        "AISHE_CREDENTIALS_FILE": str(root / "config/aishe/credentials.toml"),
    })
    return env


def config_path(root):
    return root / "config/aishe/config.toml"


def run_setup_later(root, profile, existing=False):
    env = environment(root)
    shell = Pty([BINARY, "setup"], env, cols=32)
    try:
        if existing:
            shell.expect("Review or change setup")
            shell.menu(2)
        shell.expect("Shell experience")
        shell.menu(profile)
        shell.expect("AI connection")
        shell.line()  # connect later is the actual default
        shell.expect("Review shell setup")
        shell.expect("check run")  # narrow terminals wrap the explanatory label
        shell.expect("Save these shell defaults")
        shell.line()
        shell.expect("Shell ready")
        assert shell.finish() == 0, shell.transcript
        next_steps = shell.transcript.split("Inside AIShe:", 1)[1]
        next_steps = re.sub(r"\x1b\[[0-?]*[ -/]*[@-~]", "", next_steps).replace("\r", "")
        assert all(len(line) <= 32 for line in next_steps.splitlines()), next_steps
        assert "? install kubectl please" in next_steps, next_steps
        assert "runs in your shell" in next_steps, next_steps
        assert "asks the agent" in next_steps, next_steps
        assert "Available models" not in shell.transcript, shell.transcript
        assert not (root / "managed-start-spy").exists()
        return tomllib.loads(config_path(root).read_text())
    finally:
        shell.close()


def account_free_commands_and_startup():
    with tempfile.TemporaryDirectory(prefix="aishe-account-free-") as directory:
        root = Path(directory)
        env = environment(root)
        for line in ("true", "printf 'ACCOUNT_FREE_COMMAND\\n'", "!printf 'FORCED_COMMAND\\n'"):
            result = subprocess.run([BINARY, "-c", line], env=env, capture_output=True, text=True, timeout=10)
            assert result.returncode == 0, (line, result.stdout, result.stderr)
            assert "AIShe setup" not in result.stdout + result.stderr
            assert not config_path(root).exists(), "ordinary commands created a config"
        (root / ".zshrc").write_text("PROMPT='MY_ZSH> '\nalias adoption_alias='printf PERSONAL_ALIAS_OK\\\\n'\n")
        shell = Pty([BINARY], env, cols=58)
        try:
            shell.expect("MY_ZSH>")
            shell.line("printf 'ACCOUNT_FREE_%s\\n' SHELL")
            shell.expect("ACCOUNT_FREE_SHELL")
            shell.line("exit")
            assert shell.finish() == 0
            assert "AIShe setup" not in shell.transcript
        finally:
            shell.close()
        saved = tomllib.loads(config_path(root).read_text())
        assert saved["aishe"]["shell_profile"] == "personal"
        assert saved["aishe"]["safety_profile"] == "conservative"
        assert saved["backend"]["engine"] == "native"
        assert not (root / "managed-start-spy").exists()
    print("  ok   account-free direct commands and fresh native shell")


def persistent_shell_profiles():
    for choice, value in ((1, "personal"), (2, "clean"), (3, "bash")):
        with tempfile.TemporaryDirectory(prefix="aishe-profile-%s-" % value) as directory:
            root = Path(directory)
            env = environment(root)
            (root / ".zshrc").write_text("PROMPT='MY_ZSH> '\nprintf 'ZSH_%s\\n' SETTINGS\n")
            (root / ".bashrc").write_text("PS1='MY_BASH> '\nprintf 'BASH_%s\\n' SETTINGS\n")
            saved = run_setup_later(root, choice)
            assert saved["aishe"]["shell_profile"] == value
            shell = Pty([BINARY], env, cols=58)
            try:
                shell.expect("MY_BASH>" if value == "bash" else "MY_ZSH>" if value == "personal" else "ask")
                shell.line("printf 'PERSISTED_%s\\n' PROFILE")
                shell.expect("PERSISTED_PROFILE")
                assert ("ZSH_SETTINGS" in shell.transcript) == (value == "personal")
                assert ("BASH_SETTINGS" in shell.transcript) == (value == "bash")
                shell.line("exit")
                assert shell.finish() == 0
            finally:
                shell.close()
    print("  ok   reviewed Connect later saves and launches all three shell experiences")


def fresh_settings_without_account():
    with tempfile.TemporaryDirectory(prefix="aishe-fresh-settings-") as directory:
        root = Path(directory)
        shell = Pty([BINARY, "settings"], environment(root), cols=32)
        try:
            shell.expect("Choose a section")
            shell.menu(9)  # Done; inspecting defaults is read-only
            assert shell.finish() == 0, shell.transcript
            assert not config_path(root).exists()
        finally:
            shell.close()
    print("  ok   fresh Settings opens without an account and exits without state writes")


def preserve_existing_connection_and_credentials():
    with tempfile.TemporaryDirectory(prefix="aishe-profile-preserve-") as directory:
        root = Path(directory)
        config_path(root).parent.mkdir(parents=True)
        config_path(root).write_text('''version = 7
[aishe]
connection = "work"
connection_fallback = "work"
mode = "auto"
[backend]
engine = "native"
[connections.work]
provider = "openai"
label = "My saved account"
base_url = "http://127.0.0.1:1"
model = "saved-model"
auth_required = false
transport = "chat"
[connections.work.auth]
type = "none"
''')
        credentials = config_path(root).with_name("credentials.toml")
        prior = b'version = 1\n[profiles.saved]\napi_key = "retained-private-value"\n'
        credentials.write_bytes(prior)
        credentials.chmod(0o600)
        saved = run_setup_later(root, 1, existing=True)
        assert saved["connections"]["work"]["model"] == "saved-model"
        assert saved["aishe"]["connection"] == "work"
        assert saved["aishe"]["mode"] == "auto"
        assert saved["aishe"]["shell_profile"] == "personal"
        assert credentials.read_bytes() == prior
        shell = Pty([BINARY, "settings"], environment(root), cols=58)
        try:
            shell.expect("Choose a section")
            shell.menu(2)
            shell.expect("Terminal & history")
            shell.menu(8)
            shell.expect("Shell experience")
            shell.menu(3)
            shell.expect("Terminal & history")
            shell.menu(9)
            shell.expect("Choose a section")
            shell.menu(8)
            shell.expect("Save these defaults")
            shell.line()
            shell.expect("Settings saved for new shells")
            assert shell.finish() == 0, shell.transcript
        finally:
            shell.close()
        saved = tomllib.loads(config_path(root).read_text())
        assert saved["aishe"]["shell_profile"] == "bash"
        assert saved["connections"]["work"]["model"] == "saved-model"
        assert credentials.read_bytes() == prior
    print("  ok   Connect later preserves existing account, mode and exact credential bytes")


def personal_login_and_bash_refusal():
    with tempfile.TemporaryDirectory(prefix="aishe-personal-login-") as directory:
        root = Path(directory)
        env = environment(root)
        config_path(root).parent.mkdir(parents=True)
        config_path(root).write_text('version = 7\n[aishe]\nshell_profile = "personal"\n[backend]\nengine = "native"\n')
        for name in (".zshenv", ".zprofile", ".zshrc", ".zlogin", ".zlogout"):
            (root / name).write_text("printf '%s\\n' %s >> \"$HOME/login-order\"\n" % ("%s", name))
        shell = Pty([BINARY, "-il"], env)
        try:
            shell.drain(.5)
            shell.line("printf 'LOGIN_%s\\n' READY")
            shell.expect("LOGIN_READY")
            shell.line("exit")
            assert shell.finish() == 0, shell.transcript
        finally:
            shell.close()
        assert (root / "login-order").read_text().splitlines() == [".zshenv", ".zprofile", ".zshrc", ".zlogin", ".zlogout"]
        config_path(root).write_text('version = 7\n[aishe]\nshell_profile = "bash"\n[backend]\nengine = "native"\n')
        rejected = Pty([BINARY, "-il"], env)
        try:
            rejected.expect("not login shells")
            assert rejected.finish() != 0
        finally:
            rejected.close()
    print("  ok   native personal login startup order and explicit Bash login refusal")


def actual_reversible_activation():
    with tempfile.TemporaryDirectory(prefix="aishe-activate-native-") as directory:
        root = Path(directory)
        env = environment(root)
        original = "printf 'loaded\\n' >> \"$HOME/rc-counter\"\nPROMPT='ACTIVATED> '\nalias activation_alias='printf ALIAS_ACTIVATION_OK'\n"
        (root / ".zshrc").write_text(original)
        applied = subprocess.run([BINARY, "activate", "zsh", "--apply"], env=env, capture_output=True, text=True, timeout=10)
        assert applied.returncode == 0, applied.stdout + applied.stderr
        shell = Pty(["zsh", "-i"], env)
        try:
            shell.expect("ACTIVATED>")
            shell.line("printf 'ACTIVATED_%s\\n' \"$AISHE_LEAN\"")
            shell.expect("ACTIVATED_1")
            shell.line("activation_alias")
            shell.expect("ALIAS_ACTIVATION_OK")
            assert (root / "rc-counter").read_text().splitlines() == ["loaded"]
            shell.line("exit")
            assert shell.finish() == 0, shell.transcript
        finally:
            shell.close()
        removed = subprocess.run([BINARY, "activate", "zsh", "--remove"], env=env, capture_output=True, text=True, timeout=10)
        assert removed.returncode == 0, removed.stdout + removed.stderr
        assert (root / ".zshrc").read_text() == original
    print("  ok   actual zsh activation loads one native shell and removes exactly")


def internal_controls_outside_path():
    for profile in ("clean", "personal", "bash"):
        with tempfile.TemporaryDirectory(prefix="aishe-outside-path-%s-" % profile) as directory:
            root = Path(directory)
            env = environment(root)
            tools = root / "tools"
            tools.mkdir()
            # System utilities remain available without exposing any installed
            # aishe executable through PATH, including on packaged CI machines.
            for name in (
                "zsh", "bash", "sh", "dash", "uname", "env", "mktemp",
                "mkdir", "chmod", "rm", "rmdir", "cat", "sed", "grep",
                "awk", "tr", "cut", "head", "tail", "date", "wc", "sleep",
                "find", "readlink", "dirname", "basename", "sort", "cp",
                "stty", "tput", "mv", "od", "getent",
            ):
                path = shutil.which(name)
                if path:
                    (tools / name).symlink_to(path)
            env["PATH"] = str(tools)
            env["AISHE_MOTION"] = "static"
            assert shutil.which("aishe", path=env["PATH"]) is None
            installed = root / "installed application/aishe"
            installed.parent.mkdir()
            shutil.copy2(BINARY, installed)
            config_path(root).parent.mkdir(parents=True)
            config_path(root).write_text(
                'version = 7\n[aishe]\nshell_profile = "%s"\n[backend]\nengine = "native"\n' % profile
            )
            before = config_path(root).read_bytes()
            function = "aishe() { printf 'USER_CLI_%s\\n' FUNCTION; }\n"
            (root / ".zshrc").write_text("PROMPT='OUTSIDE_ZSH> '\n" + function)
            (root / ".bashrc").write_text("PS1='OUTSIDE_BASH> '\n" + function)
            (root / ".aishe").mkdir()
            (root / ".aishe/leanrc").write_text(function)
            shell = Pty([str(installed), "-i"], env, cols=58)
            try:
                shell.line("printf 'OUTSIDE_PATH_%s\\n' READY")
                shell.expect("OUTSIDE_PATH_READY")
                shell.line("export AISHE_CLI_BIN=/missing-cli-after-startup")
                shell.line("aishe preserved")
                shell.expect("USER_CLI_FUNCTION")
                if profile != "bash":
                    # Setup is a native zsh slash control. Bash's declared
                    # reduced slash surface offers Settings instead.
                    shell.line("/setup")
                    shell.expect("Continue setup")
                    shell.line()
                    shell.expect("Shell experience")
                    shell.send("\x03")
                    shell.expect("Setup paused")
                shell.line("/settings")
                shell.expect("Choose a section")
                shell.menu(9)
                shell.expect("No settings changed.")
                shell.line("/context --json")
                shell.expect('"total_estimated_tokens"')
                if profile == "bash":
                    shell.line("/auth")
                    shell.expect("connection: anthropic")
                    shell.expect("auth: auto")
                shell.line("printf 'OUTSIDE_PATH_%s\\n' COMPLETE")
                shell.expect("OUTSIDE_PATH_COMPLETE")
                shell.line("exit")
                assert shell.finish() == 0, shell.transcript
            finally:
                shell.close()
            assert config_path(root).read_bytes() == before
            assert not (root / "config/aishe/credentials.toml").exists()
            assert not (root / "managed-start-spy").exists()
    print("  ok   absolute launch controls work outside PATH and preserve user aishe functions")


if __name__ == "__main__":
    account_free_commands_and_startup()
    persistent_shell_profiles()
    fresh_settings_without_account()
    preserve_existing_connection_and_credentials()
    personal_login_and_bash_refusal()
    actual_reversible_activation()
    internal_controls_outside_path()
    print("PASS: shell adoption and account-free onboarding")
