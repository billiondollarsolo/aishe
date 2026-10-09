#!/usr/bin/env python3
"""Settings drafts, direct edits, navigation, review, and terminal policy."""

from pathlib import Path
import tempfile
import tomllib

# Reuse the real-PTY transport; each scenario owns isolated configuration.
from setup_pty import BINARY, Pty, isolated_env


def fixture(root):
    env = isolated_env(root)
    env.update({"NO_COLOR": "1", "AISHE_UNICODE": "ascii", "AISHE_MOTION": "static"})
    env.pop("AISHE_LEGACY_OPENCODE", None)
    env["AISHE_POLICY_FILE"] = str(Path(root) / "no-policy.toml")
    env["AISHE_CREDENTIALS_FILE"] = str(Path(root) / "config" / "aishe" / "credentials.toml")
    config = Path(root) / "config" / "aishe" / "config.toml"
    config.parent.mkdir(parents=True)
    config.write_text('''version = 7
[aishe]
mode = "suggest"
connection = "work"
connection_fallback = "work"
status_line = true
status_line_position = "right"
hook_timeout_secs = 30
[connections.work]
provider = "openai"
label = "Work"
base_url = "http://127.0.0.1:1"
model = "work-model"
auth_required = false
transport = "chat"
[connections.work.auth]
type = "none"
[connections.personal]
provider = "openai"
label = "Personal"
base_url = "http://127.0.0.1:2"
model = "personal-model"
auth_required = false
transport = "chat"
[connections.personal.auth]
type = "none"
[backend]
engine = "native"
[ui]
unicode = "ascii"
motion = "static"
''')
    return env, config


def expect_menu(shell, title):
    shell.expect(title)
    # Both static and live menus have a complete footer before consuming input.
    shell.expect("  > ")


def choose(shell, title, number):
    expect_menu(shell, title)
    shell.menu(number)


def finish(shell):
    assert shell.finish() == 0, shell.transcript[-5000:]


def clean_exit_and_narrow():
    with tempfile.TemporaryDirectory(prefix="aishe-settings-narrow-") as root:
        env, config = fixture(root)
        before = config.read_bytes()
        shell = Pty([BINARY, "settings"], env, cols=32)
        try:
            choose(shell, "Choose a section", 8)
            shell.expect("No changes to apply.")
            choose(shell, "Choose a section", 9)
            finish(shell)
            assert config.read_bytes() == before
            assert "\x1b[" not in shell.transcript, "static/plain settings redraws or colors"
            assert all(ord(c) < 128 for c in shell.transcript), shell.transcript
            for line in shell.transcript.replace("\r", "").splitlines():
                assert len(line) <= 32, repr(line)
        finally:
            shell.close()
    print("  ok   32-column ASCII/static hub, empty review, clean exit")


def cancel_provider_transaction():
    with tempfile.TemporaryDirectory(prefix="aishe-settings-cancel-") as root:
        env, config = fixture(root)
        before = config.read_bytes()
        shell = Pty([BINARY, "settings"], env)
        try:
            choose(shell, "Choose a section", 1)
            choose(shell, "Connection & model", 3)
            choose(shell, "Provider", 2)  # OpenAI resets defaults in the candidate
            shell.expect("Endpoint")
            shell.line("http://127.0.0.1:3")
            expect_menu(shell, "Authentication method")
            shell.line(":cancel")
            choose(shell, "Connection & model", 4)
            choose(shell, "Choose a section", 8)
            shell.expect("No changes to apply.")
            choose(shell, "Choose a section", 9)
            finish(shell)
            assert config.read_bytes() == before
            assert "Connection check" not in shell.transcript, "editing triggered network validation"
        finally:
            shell.close()
    print("  ok   cancelled provider candidate leaves default/endpoint/model unchanged")


def direct_model_and_persistent_sections():
    with tempfile.TemporaryDirectory(prefix="aishe-settings-apply-") as root:
        env, config = fixture(root)
        before = config.read_bytes()
        shell = Pty([BINARY, "settings"], env)
        try:
            choose(shell, "Choose a section", 1)
            choose(shell, "Connection & model", 1)
            choose(shell, "Default connection", 1)  # BTree order: personal, work
            choose(shell, "Connection & model", 2)
            shell.expect("Model")
            shell.line("personal-new")
            choose(shell, "Connection & model", 4)
            choose(shell, "Choose a section", 2)
            choose(shell, "Terminal & history", 6)
            shell.expect("AI hook timeout seconds")
            shell.line("75")
            choose(shell, "Terminal & history", 3)
            choose(shell, "Agent transcript density", 3)
            choose(shell, "Terminal & history", 8)
            choose(shell, "Choose a section", 5)
            choose(shell, "Usage & logging", 2)
            shell.expect("Session budget USD")
            shell.line("12.5")
            choose(shell, "Usage & logging", 4)  # logging toggles and remains here
            choose(shell, "Usage & logging", 6)
            choose(shell, "Choose a section", 6)
            choose(shell, "Response tuning", 1)
            choose(shell, "Reasoning effort", 5)
            choose(shell, "Response tuning", 6)
            choose(shell, "Choose a section", 8)
            shell.expect("Saved values -> draft values.")
            shell.expect("Save these defaults")
            shell.line("y")
            shell.expect("Settings saved for new shells.")
            finish(shell)
            data = tomllib.loads(config.read_text())
            assert data["aishe"]["connection"] == "personal"
            assert data["connections"]["personal"]["model"] == "personal-new"
            assert data["connections"]["personal"]["reasoning_effort"] == "high"
            assert data["connections"]["work"]["model"] == "work-model"
            assert data["connections"]["personal"]["auth"] == {"type": "none"}
            assert data["connections"]["personal"]["base_url"] == "http://127.0.0.1:2"
            assert data["aishe"]["hook_timeout_secs"] == 75
            assert data["backend"]["output"] == "detailed"
            assert data["aishe"]["budget_usd"] == 12.5
            backups = list(config.parent.glob("config.toml.setup.*.bak"))
            assert backups and backups[-1].read_bytes() == before
            assert "Authentication method" not in shell.transcript, "model-only edit forced auth wizard"
            assert "Connection check" not in shell.transcript, "model-only edit triggered network calls"
            assert "Use /model or /connection to change this shell." in shell.transcript
            assert all(ord(c) < 128 for c in shell.transcript), "ASCII settings contains fixed Unicode punctuation"
        finally:
            shell.close()
    print("  ok   direct named-model edit, persistent sections, review/apply, exact backup")


def declined_review_preserves_draft():
    with tempfile.TemporaryDirectory(prefix="aishe-settings-discard-") as root:
        env, config = fixture(root)
        before = config.read_bytes()
        shell = Pty([BINARY, "settings"], env)
        try:
            choose(shell, "Choose a section", 1)
            choose(shell, "Connection & model", 2)
            shell.expect("Model")
            shell.line("draft-model")
            choose(shell, "Connection & model", 4)
            choose(shell, "Choose a section", 8)
            shell.expect("Save these defaults")
            shell.line("n")
            choose(shell, "Choose a section", 1)
            expect_menu(shell, "Connection & model")
            assert "draft-model" in shell.transcript
            shell.menu(4)
            expect_menu(shell, "Choose a section")
            shell.line(":cancel")
            shell.expect("Discard unsaved changes and exit")
            shell.line("n")
            choose(shell, "Choose a section", 9)
            shell.expect("Unsaved changes discarded.")
            finish(shell)
            assert config.read_bytes() == before
            assert not list(config.parent.glob("*.bak"))
        finally:
            shell.close()
    print("  ok   declined review and cancel keep draft; explicit discard preserves saved bytes")


def model_only_preserves_explicit_api_identity():
    with tempfile.TemporaryDirectory(prefix="aishe-settings-api-identity-") as root:
        env, config = fixture(root)
        config.write_text(config.read_text().replace(
            '[connections.work.auth]\ntype = "none"',
            '[connections.work.auth]\ntype = "api_key"\ncredential = "work-secret-profile"\napi_key_env = "WORK_ACCOUNT_API_KEY"',
        ))
        before = tomllib.loads(config.read_text())
        shell = Pty([BINARY, "settings"], env)
        try:
            choose(shell, "Choose a section", 1)
            choose(shell, "Connection & model", 2)
            shell.expect("Model")
            shell.line("work-new")
            choose(shell, "Connection & model", 4)
            choose(shell, "Choose a section", 8)
            shell.expect("Save these defaults")
            shell.line("y")
            finish(shell)
            data = tomllib.loads(config.read_text())
            assert data["connections"]["work"]["model"] == "work-new"
            assert data["connections"]["work"]["auth"] == before["connections"]["work"]["auth"]
            assert data["connections"]["work"]["base_url"] == before["connections"]["work"]["base_url"]
            assert data["connections"]["personal"]["model"] == "personal-model"
            assert "Authentication method" not in shell.transcript
            assert not (Path(root) / "config" / "aishe" / "credentials.toml").exists()
        finally:
            shell.close()
    print("  ok   model-only edit preserves explicit API profile/variable and unused key fields")


def mode_scope_defaults_are_reviewed():
    with tempfile.TemporaryDirectory(prefix="aishe-settings-authority-") as root:
        env, config = fixture(root)
        shell = Pty([BINARY, "settings"], env)
        try:
            choose(shell, "Choose a section", 3)
            choose(shell, "Mode & safety", 1)
            choose(shell, "Startup mode", 3)
            choose(shell, "Mode & safety", 2)
            choose(shell, "Agent scope", 2)
            choose(shell, "Mode & safety", 5)
            choose(shell, "Choose a section", 8)
            shell.expect("Agent scope")
            shell.expect("workspace -> host")
            shell.expect("Save these defaults")
            shell.line("y")
            finish(shell)
            data = tomllib.loads(config.read_text())
            assert data["aishe"]["mode"] == "yolo"
            assert data["backend"]["default_scope"] == "host"
            assert "require a fresh grant in each shell." in shell.transcript
        finally:
            shell.close()
    print("  ok   mode/scope are editable defaults with explicit per-shell grant reminder")


def managed_defaults_are_explicit_in_review():
    with tempfile.TemporaryDirectory(prefix="aishe-settings-policy-") as root:
        env, config = fixture(root)
        policy = Path(root) / "policy.toml"
        policy.write_text('version = 1\nallow_host_yolo = false\nrequire_audit_logging = true\nrequire_redaction = true\nmax_budget_usd = 5.0\n')
        env["AISHE_POLICY_FILE"] = str(policy)
        shell = Pty([BINARY, "settings"], env)
        try:
            choose(shell, "Choose a section", 3)
            choose(shell, "Mode & safety", 2)
            choose(shell, "Agent scope", 2)
            choose(shell, "Mode & safety", 5)
            choose(shell, "Choose a section", 8)
            shell.expect("Managed by organization")
            shell.expect("host -> workspace")
            shell.expect("Save these defaults")
            shell.line("y")
            finish(shell)
            data = tomllib.loads(config.read_text())
            assert data["backend"]["default_scope"] == "workspace"
            assert data["logging"]["enabled"]
            assert data["logging"]["redact"] and data["aishe"]["redact_secrets"]
            assert data["aishe"]["budget_usd"] == 5.0
        finally:
            shell.close()
    print("  ok   managed policy changes are visible before the exact constrained defaults save")


if __name__ == "__main__":
    clean_exit_and_narrow()
    cancel_provider_transaction()
    direct_model_and_persistent_sections()
    declined_review_preserves_draft()
    model_only_preserves_explicit_api_identity()
    mode_scope_defaults_are_reviewed()
    managed_defaults_are_explicit_in_review()
    print("PASS: settings UI and transaction contracts")
