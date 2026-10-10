#!/usr/bin/env python3
"""Run reproducible AIShe qualification profiles and emit one JSON report.

This is deliberately a small Python orchestration layer over the commands that
already define the project in CONTRIBUTING.md and CI.  Commands are always
passed to subprocess as argv arrays.  In particular, no profile text is ever
evaluated by a shell.
"""

from __future__ import annotations

import argparse
import dataclasses
import datetime
import hashlib
import json
import os
import pathlib
import platform
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
import time
from collections.abc import Callable, Mapping, Sequence
from typing import Protocol

from harness_identity import cargo_version, parse_binary_identity, require_current_binary


SCHEMA_VERSION = 1
PROFILE_REVISION = "2026-10-10.1"
THREAT_MODEL_VERSION = "2026-07-31.1"
THREAT_MODEL_REVIEWED = "2026-07-31"
BINARY = "{release_binary}"
NATIVE_CLEAN_ENV = (("AISHE_LEAN", "1"), ("AISHE_LEGACY_OPENCODE", "0"), ("AISHE_ZSH_PROFILE", "clean"))
NATIVE_PERSONAL_ENV = (("AISHE_LEAN", "1"), ("AISHE_LEGACY_OPENCODE", "0"), ("AISHE_ZSH_PROFILE", "personal"))
LEGACY_ENV = (("AISHE_LEAN", "0"), ("AISHE_LEGACY_OPENCODE", "1"), ("AISHE_ZSH_PROFILE", "clean"))


@dataclasses.dataclass(frozen=True)
class Gate:
    id: str
    label: str
    command: tuple[str, ...]
    timeout_seconds: int = 600
    external_harness: bool = False
    identity_check: bool = False
    required: bool = True
    platforms: frozenset[str] | None = None
    credential_env: str | None = None
    required_tools: tuple[str, ...] = ()
    execution_env: tuple[tuple[str, str], ...] = NATIVE_CLEAN_ENV


@dataclasses.dataclass(frozen=True)
class Profile:
    name: str
    description: str
    gates: tuple[Gate, ...]


@dataclasses.dataclass(frozen=True)
class CommandResult:
    returncode: int
    stdout: str = ""
    stderr: str = ""


class Runner(Protocol):
    def run(
        self,
        command: Sequence[str],
        *,
        cwd: pathlib.Path,
        env: Mapping[str, str],
        timeout: int,
    ) -> CommandResult: ...


class SubprocessRunner:
    """Production command runner.  It intentionally has no shell mode."""

    def run(
        self,
        command: Sequence[str],
        *,
        cwd: pathlib.Path,
        env: Mapping[str, str],
        timeout: int,
    ) -> CommandResult:
        try:
            completed = subprocess.run(
                list(command),
                cwd=cwd,
                env=dict(env),
                stdin=subprocess.DEVNULL,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
                timeout=timeout,
                check=False,
                shell=False,
            )
            return CommandResult(completed.returncode, completed.stdout, completed.stderr)
        except subprocess.TimeoutExpired as error:
            stdout = error.stdout.decode(errors="replace") if isinstance(error.stdout, bytes) else (error.stdout or "")
            stderr = error.stderr.decode(errors="replace") if isinstance(error.stderr, bytes) else (error.stderr or "")
            return CommandResult(124, stdout, f"{stderr}\nqualification timeout after {timeout}s".strip())
        except OSError as error:
            return CommandResult(127, "", f"could not start command: {error}")


FORMAT = Gate(
    "rust-format",
    "Rust formatting",
    ("cargo", "fmt", "--all", "--", "--check"),
    timeout_seconds=180,
    required_tools=("cargo",),
)
CLIPPY = Gate(
    "rust-clippy",
    "Strict Rust linting",
    (
        "cargo",
        "clippy",
        "--all-targets",
        "--all-features",
        "--locked",
        "--",
        "-D",
        "warnings",
    ),
    timeout_seconds=1800,
    required_tools=("cargo",),
)
RUST_TESTS = Gate(
    "rust-tests",
    "Rust unit and integration tests",
    ("cargo", "test", "--all-targets", "--locked"),
    timeout_seconds=1800,
    required_tools=("cargo",),
)
CARGO_DENY = Gate(
    "dependency-policy",
    "Dependency advisories, bans, licenses, and sources",
    (
        "cargo",
        "deny",
        "--all-features",
        "check",
        "advisories",
        "bans",
        "licenses",
        "sources",
    ),
    timeout_seconds=900,
    required_tools=("cargo",),
)
RELEASE_BUILD = Gate(
    "release-build",
    "Build the checked-out release binary",
    ("cargo", "build", "--release", "--locked"),
    timeout_seconds=1800,
    required_tools=("cargo",),
)
IDENTITY = Gate(
    "release-identity",
    "Verify release binary version and commit",
    (BINARY, "--version"),
    timeout_seconds=30,
    identity_check=True,
)
def python_gate(
    gate_id: str,
    label: str,
    script: str,
    *arguments: str,
    timeout: int = 600,
    required_tools: tuple[str, ...] = (),
    platforms: frozenset[str] | None = None,
    credential_env: str | None = None,
    required: bool = True,
    execution_env: tuple[tuple[str, str], ...] = NATIVE_CLEAN_ENV,
) -> Gate:
    return Gate(
        gate_id,
        label,
        ("python3", script, *arguments),
        timeout_seconds=timeout,
        external_harness=True,
        required=required,
        platforms=platforms,
        credential_env=credential_env,
        required_tools=("python3", *required_tools),
        execution_env=execution_env,
    )


SHELL_CONTRACT = python_gate(
    "shell-contract",
    "Static shellcheck and generated zsh/Bash syntax",
    "tests/shell_contract.py",
    BINARY,
    timeout=180,
    required_tools=("shellcheck", "zsh", "bash"),
)
LIVE_CONTRACT = python_gate(
    "live-contract-unit",
    "Machine-readable live response contract tests",
    "tests/live_contract_test.py",
    timeout=120,
)
DOCS_CONTRACT = python_gate(
    "docs-contract",
    "Documentation lifecycle plus relative path and anchor links",
    "tests/docs_contract_test.py",
    timeout=120,
)
PTY_SMOKE = python_gate(
    "pty-smoke", "Native interactive display and handoff smoke", "tests/lean_ui_pty.py", BINARY, required_tools=("zsh",)
)
PTY_SCENARIOS = python_gate(
    "pty-scenarios",
    "Native slash discovery, completion, and local routing",
    "tests/lean_slash_pty.py",
    BINARY,
    required_tools=("zsh",),
)
NATIVE_TASK_CANCEL = python_gate(
    "native-cancel-pty", "Native provider and command cancellation", "tests/lean_cancel_pty.py", BINARY,
    required_tools=("zsh",), timeout=180,
)
NATIVE_MODE_GRANTS = python_gate(
    "native-mode-grants-pty", "Native shell mode and scoped grants", "tests/lean_modes_pty.py", BINARY,
    required_tools=("zsh",), timeout=180,
)
NATIVE_PICKERS = python_gate(
    "native-picker-pty", "Native connection/model selection and shell isolation", "tests/lean_picker_pty.py", BINARY,
    required_tools=("zsh",), timeout=180,
)
NATIVE_SETTINGS = python_gate(
    "native-settings-pty", "Reviewed settings transactions and draft isolation", "tests/settings_pty.py", BINARY,
    required_tools=("zsh",), timeout=300,
)
NATIVE_ADOPTION = python_gate(
    "native-adoption-pty", "Account-free startup, saved shell profiles, and connection preservation",
    "tests/adoption_pty.py", BINARY, required_tools=("zsh", "bash"), timeout=300,
)
NATIVE_DISCOVERY = python_gate(
    "native-discovery-pty", "Rendered one-time discovery and failed/suppressed display acknowledgment",
    "tests/native_discovery_pty.py", BINARY, required_tools=("zsh",), timeout=180,
)
DIRECT_SHELL_CONTRACTS = python_gate(
    "direct-shell-contracts", "One-shot rc, environment, status, and after-completion history",
    "tests/direct_shell_startup_test.py", BINARY, required_tools=("zsh",), timeout=180,
)
SHELL_ADOPTION = python_gate(
    "shell-adoption-contracts", "Whole-program stdin, script arguments, and reversible terminal activation",
    "tests/shell_adoption.py", BINARY, required_tools=("zsh", "bash"), timeout=300,
)
RELEASE_GATE_CONTRACT = python_gate(
    "release-gate-contract", "Exact-source CI, retained performance evidence, and release dispositions",
    "tests/release_gate_test.py", timeout=180,
)
NATIVE_PROMPTS = python_gate(
    "native-prompts-pty", "Shared prompt controls and exact terminal restoration", "tests/prompt_primitives_pty.py", BINARY,
    required_tools=("zsh", "rustc"), timeout=180,
)
NATIVE_PROFILE = python_gate(
    "native-personal-profile-pty", "Native personal startup files, widgets, prompts, and completion cache",
    "tests/native_zsh_profile_pty.py", BINARY,
    required_tools=("zsh",), timeout=300, execution_env=NATIVE_PERSONAL_ENV,
)
NATIVE_SHELL_STATE = python_gate(
    "native-live-shell-state-pty", "Native agents use live shell exports and fail closed on invalid state",
    "tests/live_shell_state_pty.py", BINARY, required_tools=("zsh",), timeout=180,
)
NATIVE_BACKGROUND_LIFECYCLE = python_gate(
    "native-background-lifecycle", "Detached native cancellation, truthful outcomes, checkpoint resume, and rework",
    "tests/native_background_lifecycle.py", BINARY, timeout=180,
)
NATIVE_BACKGROUND_UI = python_gate(
    "native-background-ui", "Quiet task indicators, browser, controls, and terminal restoration",
    "tests/background_tasks_pty.py", BINARY, required_tools=("zsh", "git"), timeout=300,
)
NATIVE_TASK_INTERACTIONS = python_gate(
    "native-task-interactions", "Durable background questions, approvals, steering, evidence, and metadata",
    "tests/native_task_interactions.py", BINARY, timeout=300,
)
NATIVE_TASK_INTERACTIONS_UI = python_gate(
    "native-task-interactions-ui", "Needs you, live follow-ups, checks, and history with terminal restoration",
    "tests/task_interactions_pty.py", BINARY, required_tools=("zsh", "git"), timeout=300,
)
AGENTIC_WORKFLOWS = python_gate(
    "agentic-workflows", "Dependency snapshots, restart, cancellation, selective changes, and factual timeline",
    "tests/agentic_workflows.py", BINARY, required_tools=("git",), timeout=600,
)
TASK_HANDOFF_UI = python_gate(
    "task-handoff-ui", "Exclusive foreground/background continuation with real terminal input",
    "tests/task_handoff_pty.py", BINARY, required_tools=("zsh", "git"), timeout=300,
)
TASK_WORKFLOWS_UI = python_gate(
    "task-workflows-ui", "Workflow launcher, selected review, timeline, and bounded busy history",
    "tests/task_workflows_pty.py", BINARY, required_tools=("zsh", "git"), timeout=600,
)
NATIVE_SANDBOX_FUNCTIONAL = python_gate(
    "native-sandbox-functional", "Actual workspace and denied-network enforcement by native tools",
    "tests/native_sandbox_qualification.py", BINARY, required_tools=("bwrap",), platforms=frozenset({"Linux"}), timeout=180,
)
BASH_HOOK_CURRENT = python_gate(
    "bash-hook-current",
    "Native Bash hook declared-tier matrix for the current Bash",
    "tests/bash_hook.py",
    BINARY,
    "--bash",
    "bash",
    "--require-current-family",
    timeout=180,
    required_tools=("bash",),
)

ADVISORY_POLICY = python_gate(
    "advisory-policy-metadata",
    "Owned advisory exceptions and review deadlines",
    "tests/advisory_policy_test.py",
    timeout=120,
)
LAZY_LOADING = python_gate(
    "lazy-loading",
    "Local surfaces start no provider, backend, or network listener",
    "tests/lazy_loading_test.py",
    BINARY,
    "--output",
    "test-results/lazy-loading.json",
    timeout=300,
    required_tools=("cc",),
)
PERFORMANCE_EVIDENCE = python_gate(
    "performance-evidence",
    "Versioned shell, route, picker, render, RSS, and size budgets",
    "tests/performance_benchmark.py",
    BINARY,
    "--samples",
    "60",
    "--commands",
    "100",
    "--warmup",
    "10",
    "--output",
    "test-results/performance.json",
    timeout=3600,
    required_tools=("cargo", "zsh"),
)
TERMINAL_CONTRACT_UNIT = python_gate(
    "terminal-contract-unit",
    "Terminal compatibility harness semantics",
    "tests/terminal_compat_test.py",
    timeout=120,
)
TERMINAL_LOCAL = python_gate(
    "terminal-local-latency",
    "Local PTY ESC latency, resize, routing, and staging contract",
    "tests/terminal_compat.py",
    BINARY,
    "--capability",
    "local-latency",
    "--require-capability",
    "local-latency",
    "--json",
    "test-results/terminal-compat-local.json",
    timeout=300,
    required_tools=("zsh",),
)
TERMINAL_LINUX_MULTIPLEXERS = python_gate(
    "terminal-linux-multiplexers",
    "Required tmux and GNU screen terminal transports",
    "tests/terminal_compat.py",
    BINARY,
    "--capability",
    "tmux",
    "--require-capability",
    "tmux",
    "--capability",
    "screen",
    "--require-capability",
    "screen",
    "--json",
    "test-results/terminal-compat-linux-multiplexers.json",
    timeout=480,
    platforms=frozenset({"Linux"}),
    required_tools=("zsh", "tmux", "screen"),
)


QUICK_GATES = (
    FORMAT,
    CLIPPY,
    RUST_TESTS,
    RELEASE_BUILD,
    IDENTITY,
    SHELL_CONTRACT,
    DOCS_CONTRACT,
    LIVE_CONTRACT,
    PTY_SMOKE,
    PTY_SCENARIOS,
    NATIVE_TASK_CANCEL,
    NATIVE_MODE_GRANTS,
    BASH_HOOK_CURRENT,
)

LOCAL_FULL_GATES = (
    FORMAT,
    CLIPPY,
    RUST_TESTS,
    CARGO_DENY,
    RELEASE_BUILD,
    IDENTITY,
    SHELL_CONTRACT,
    DOCS_CONTRACT,
    RELEASE_GATE_CONTRACT,
    DIRECT_SHELL_CONTRACTS,
    SHELL_ADOPTION,
    ADVISORY_POLICY,
    LAZY_LOADING,
    PERFORMANCE_EVIDENCE,
    TERMINAL_CONTRACT_UNIT,
    TERMINAL_LOCAL,
    python_gate(
        "direct-shell-slo",
        "Direct-shell isolation and startup SLO",
        "tests/direct_shell_benchmark.py",
        BINARY,
        "--commands",
        "100",
        "--warmup",
        "10",
        timeout=900,
        required_tools=("zsh",),
    ),
    Gate(
        "runtime-install",
        "Install the pinned managed runtime",
        (BINARY, "backend", "install"),
        timeout_seconds=900,
        external_harness=True,
    ),
    Gate(
        "runtime-live-verify",
        "Live-verify the pinned managed runtime",
        (BINARY, "backend", "verify", "--live"),
        timeout_seconds=300,
        external_harness=True,
    ),
    Gate(
        "installer-runtime-transaction",
        "Installer runtime transaction and fault contract",
        ("sh", "tests/installer_runtime_transaction.sh"),
        timeout_seconds=900,
        external_harness=True,
        required_tools=("sh",),
    ),
    Gate(
        "installer-upgrade-linux",
        "Installer upgrade preserves config and data",
        ("sh", "tests/installer_upgrade.sh", BINARY),
        timeout_seconds=900,
        external_harness=True,
        platforms=frozenset({"Linux"}),
        required_tools=("sh",),
    ),
    python_gate(
        "provider-unauthenticated",
        "Unauthenticated loopback provider",
        "tests/provider_unauthenticated.py",
        BINARY,
    ),
    python_gate(
        "credentials-linux",
        "Private shared credential precedence",
        "tests/credentials_linux.py",
        BINARY,
        platforms=frozenset({"Linux"}),
    ),
    LIVE_CONTRACT,
    PTY_SMOKE,
    PTY_SCENARIOS,
    NATIVE_TASK_CANCEL,
    NATIVE_MODE_GRANTS,
    NATIVE_PICKERS,
    NATIVE_SETTINGS,
    NATIVE_ADOPTION,
    NATIVE_DISCOVERY,
    NATIVE_PROMPTS,
    NATIVE_PROFILE,
    NATIVE_SHELL_STATE,
    NATIVE_BACKGROUND_LIFECYCLE,
    NATIVE_BACKGROUND_UI,
    NATIVE_TASK_INTERACTIONS,
    NATIVE_TASK_INTERACTIONS_UI,
    AGENTIC_WORKFLOWS,
    TASK_HANDOFF_UI,
    TASK_WORKFLOWS_UI,
    NATIVE_SANDBOX_FUNCTIONAL,
    python_gate(
        "legacy-pty-smoke", "Explicit legacy zsh smoke", "tests/pty_smoke.py", BINARY,
        required_tools=("zsh",), execution_env=LEGACY_ENV,
    ),
    python_gate(
        "legacy-pty-scenarios", "Explicit legacy routing and history", "tests/pty_scenarios.py", BINARY,
        required_tools=("zsh",), execution_env=LEGACY_ENV,
    ),
    BASH_HOOK_CURRENT,
    python_gate(
        "statusline-pty", "Statusline placement and live metrics", "tests/statusline_pty.py", BINARY, required_tools=("zsh",),
        execution_env=LEGACY_ENV,
    ),
    python_gate(
        "model-picker-pty",
        "Connection/model picker and concurrent selection",
        "tests/model_picker_pty.py",
        BINARY,
        required_tools=("zsh",),
        execution_env=LEGACY_ENV,
    ),
    python_gate(
        "setup-pty", "Interactive setup state machine", "tests/setup_pty.py", BINARY, required_tools=("zsh",)
    ),
    python_gate(
        "in-shell-menus-pty",
        "Menus launched from inside the AIShe shell read keys",
        "tests/in_shell_menus_pty.py",
        BINARY,
        required_tools=("zsh",),
    ),
    python_gate(
        "yolo-consent-pty",
        "Declining yolo consent is a cancel, not an error",
        "tests/yolo_consent_pty.py",
        BINARY,
        required_tools=("zsh",),
        execution_env=LEGACY_ENV,
    ),
    python_gate(
        "palette-pty",
        "Palette repaints the prompt and fills slash forms",
        "tests/palette_pty.py",
        BINARY,
        required_tools=("zsh",),
        execution_env=LEGACY_ENV,
    ),
    python_gate(
        "mode-handoff-pty",
        "aishe mode and /mode agree inside the shell",
        "tests/mode_handoff_pty.py",
        BINARY,
        required_tools=("zsh",),
        execution_env=LEGACY_ENV,
    ),
    python_gate(
        "bare-words-pty",
        "Bare reset and details stay the user's commands",
        "tests/bare_words_pty.py",
        BINARY,
        required_tools=("zsh",),
        execution_env=LEGACY_ENV,
    ),
    python_gate(
        "theme-prompt-pty",
        "A prompt theme survives; a stock prompt gets the glyph",
        "tests/theme_prompt_pty.py",
        BINARY,
        required_tools=("zsh",),
        execution_env=LEGACY_ENV,
    ),
    python_gate(
        "keys-pty",
        "Shift-Tab cycles the mode only on an empty line",
        "tests/keys_pty.py",
        BINARY,
        required_tools=("zsh",),
        execution_env=LEGACY_ENV,
    ),
    python_gate(
        "usage-report",
        "Usage report reads the content-free ledger",
        "tests/usage_report_pty.py",
        BINARY,
        execution_env=LEGACY_ENV,
    ),
    python_gate(
        "slash-highlight-pty",
        "A registered /command does not read as an error",
        "tests/slash_highlight_pty.py",
        BINARY,
        required_tools=("zsh",),
        execution_env=LEGACY_ENV,
    ),
    python_gate(
        "picker-arrows-pty",
        "Arrow keys move the selection in in-shell pickers",
        "tests/picker_arrows_pty.py",
        BINARY,
        required_tools=("zsh",),
    ),
    python_gate(
        "statusline-width-pty",
        "The statusline shortens instead of vanishing",
        "tests/statusline_width_pty.py",
        BINARY,
        required_tools=("zsh",),
        execution_env=LEGACY_ENV,
    ),
    python_gate(
        "docs-cli-block",
        "docs/commands.md CLI table matches the clap tree",
        "tests/docs_cli_block_test.py",
        BINARY,
    ),
    python_gate(
        "opencode-runtime-contract",
        "Pinned OpenCode provider and tool bridge",
        "tests/opencode_runtime_contract.py",
        BINARY,
        timeout=900,
        execution_env=LEGACY_ENV,
    ),
    python_gate(
        "connection-isolation",
        "Same-provider credential and runtime isolation",
        "tests/opencode_connection_isolation.py",
        BINARY,
        timeout=900,
        execution_env=LEGACY_ENV,
    ),
    python_gate(
        "host-scope",
        "Workspace-to-host authority and focus output",
        "tests/opencode_host_scope.py",
        BINARY,
        timeout=900,
        execution_env=LEGACY_ENV,
    ),
    python_gate(
        "opencode-soak",
        "Managed startup, reconnect, and memory qualification",
        "tests/opencode_soak.py",
        BINARY,
        "--turns",
        "20",
        "--cold-cycles",
        "3",
        "--warm-probes",
        "20",
        "--reconnect-every",
        "10",
        timeout=1800,
        execution_env=LEGACY_ENV,
    ),
    python_gate(
        "opencode-concurrency",
        "Concurrent managed-session isolation",
        "tests/opencode_concurrency.py",
        BINARY,
        "--sessions",
        "8",
        timeout=900,
        execution_env=LEGACY_ENV,
    ),
    python_gate(
        "durable-task-resume",
        "Durable task interruption and resume",
        "tests/durable_task_resume.py",
        BINARY,
        timeout=900,
    ),
    python_gate(
        "pty-fuzz", "Generated PTY and adversarial response fuzz", "tests/pty_fuzz.py", BINARY, required_tools=("zsh",),
        execution_env=LEGACY_ENV,
    ),
    python_gate(
        "zsh-features", "zsh feature matrix", "tests/zsh_features.py", BINARY, "--profile", "clean", required_tools=("zsh",), timeout=180
    ),
    python_gate(
        "pty-signals", "PTY signals and resize behavior", "tests/pty_signals.py", BINARY, "--profile", "clean", required_tools=("zsh",), timeout=180
    ),
    python_gate(
        "zsh-features-personal", "Native personal zsh feature matrix", "tests/zsh_features.py", BINARY,
        "--profile", "personal", required_tools=("zsh",), timeout=180, execution_env=NATIVE_PERSONAL_ENV,
    ),
    python_gate(
        "pty-signals-personal", "Native personal jobs, signals, paste, editors, and terminal restoration",
        "tests/pty_signals.py", BINARY, "--profile", "personal",
        required_tools=("zsh",), timeout=180, execution_env=NATIVE_PERSONAL_ENV,
    ),
    python_gate(
        "zsh-features-legacy", "Explicit legacy zsh feature matrix", "tests/zsh_features.py", BINARY,
        "--profile", "legacy", required_tools=("zsh",), timeout=180, execution_env=LEGACY_ENV,
    ),
    python_gate(
        "pty-signals-legacy", "Explicit legacy jobs and terminal restoration", "tests/pty_signals.py", BINARY,
        "--profile", "legacy", required_tools=("zsh",), timeout=180, execution_env=LEGACY_ENV,
    ),
    python_gate(
        "admin-validation",
        "Deterministic admin, shell, dispatch, config, and MCP validation",
        "tests/admin_validation.py",
        BINARY,
        timeout=1800,
    ),
    python_gate(
        "real-model",
        "Paid live-model classification",
        "tests/real_model.py",
        BINARY,
        timeout=1800,
        credential_env="AISHE_REALTEST_KEY",
        required=False,
        execution_env=LEGACY_ENV,
    ),
    python_gate(
        "real-model-fuzz",
        "Paid live-model robustness fuzz",
        "tests/real_fuzz.py",
        BINARY,
        timeout=1800,
        credential_env="AISHE_REALTEST_KEY",
        required=False,
        execution_env=LEGACY_ENV,
    ),
)

LINUX_FULL_GATES = LOCAL_FULL_GATES + (TERMINAL_LINUX_MULTIPLEXERS,)

# Paid calls are explicit release dispositions rather than hidden skips in the
# deterministic profiles. The paid-live profile clones them as required.
DETERMINISTIC_RELEASE_GATES = tuple(
    gate for gate in LOCAL_FULL_GATES if gate.credential_env is None
) + (TERMINAL_LINUX_MULTIPLEXERS,)
PAID_RELEASE_GATES = tuple(
    dataclasses.replace(gate, required=True)
    for gate in LOCAL_FULL_GATES
    if gate.credential_env is not None
) + (
    python_gate(
        "paid-live-release",
        "Paid live release contract across configured provider",
        "tests/live_release.py",
        BINARY,
        timeout=2400,
        credential_env="AISHE_REALTEST_KEY",
        required=True,
    ),
)

PROFILES = {
    "quick": Profile(
        "quick",
        "Rust gates plus a freshly built binary's core contract and PTY smoke tests.",
        QUICK_GATES,
    ),
    "local-full": Profile(
        "local-full",
        "All deterministic local CI gates, with platform and paid-live gates explicitly classified.",
        LOCAL_FULL_GATES,
    ),
    "linux-full": Profile(
        "linux-full",
        "All deterministic Linux gates, including required bubblewrap, tmux, and screen evidence.",
        LINUX_FULL_GATES,
    ),
    "release": Profile(
        "release",
        "Deterministic release evidence for the current supported platform; paid gates are a separate disposition.",
        DETERMINISTIC_RELEASE_GATES,
    ),
    "paid-live": Profile(
        "paid-live",
        "Release evidence plus required credentialed live-model, fuzz, and end-to-end gates.",
        DETERMINISTIC_RELEASE_GATES + PAID_RELEASE_GATES,
    ),
}


def _sha256(path: pathlib.Path) -> str | None:
    if not path.is_file():
        return None
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def _git(root: pathlib.Path, *arguments: str) -> str | None:
    if not (root / ".git").exists():
        return None
    try:
        completed = subprocess.run(
            ["git", *arguments],
            cwd=root,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            timeout=10,
            check=True,
            shell=False,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    return completed.stdout.strip()


def _runtime_metadata(root: pathlib.Path) -> dict[str, object]:
    manifest_path = root / "assets/backend/opencode/runtime-manifest.json"
    plugin_path = root / "assets/backend/opencode/aishe-plugin.mjs"
    metadata: dict[str, object] = {
        "name": "opencode",
        "manifest": str(manifest_path.relative_to(root)),
        "manifest_sha256": _sha256(manifest_path),
        "trusted_plugin": str(plugin_path.relative_to(root)),
        "trusted_plugin_sha256": _sha256(plugin_path),
    }
    try:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        metadata["pinned_version"] = manifest.get("version")
    except (OSError, ValueError):
        metadata["pinned_version"] = None
    return metadata


def _corpora(root: pathlib.Path) -> list[dict[str, object]]:
    paths = (
        ("safety", "tests/safety_corpus.rs"),
        ("routing", "tests/fixtures/routing/v1.json"),
        ("routing-typo-assistance", "tests/fixtures/routing/typo-assistance-v1.json"),
        ("boundary-fuzz", "tests/boundary_fuzz.rs"),
        ("live-model-classification", "tests/real_model.py"),
        ("live-model-fuzz", "tests/real_fuzz.py"),
        ("opencode-events", "tests/fixtures/opencode/v1.18.27/events.jsonl"),
        ("opencode-api-contract", "tests/fixtures/opencode/v1.18.27/openapi-contract.json"),
    )
    return [
        {"id": corpus_id, "path": relative, "sha256": _sha256(root / relative)}
        for corpus_id, relative in paths
    ]


def _evidence_artifacts(root: pathlib.Path) -> list[dict[str, object]]:
    evidence_root = root / "test-results"
    if not evidence_root.is_dir():
        return []
    artifacts = []
    for path in sorted(evidence_root.rglob("*")):
        if path.is_file():
            artifacts.append(
                {
                    "path": str(path.relative_to(root)),
                    "bytes": path.stat().st_size,
                    "sha256": _sha256(path),
                }
            )
    return artifacts


def collect_metadata(
    root: pathlib.Path,
    profile: Profile,
    env: Mapping[str, str],
    *,
    platform_name: str,
) -> dict[str, object]:
    cargo_toml = root / "Cargo.toml"
    try:
        checkout_version = cargo_version(cargo_toml.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        checkout_version = None
    status = _git(root, "status", "--porcelain")
    shell = env.get("SHELL")
    return {
        "profile": {
            "name": profile.name,
            "revision": PROFILE_REVISION,
            "description": profile.description,
            "gate_count": len(profile.gates),
        },
        "source": {
            "repository": str(root),
            "version": checkout_version,
            "commit": _git(root, "rev-parse", "HEAD"),
            "dirty": bool(status) if status is not None else None,
            "cargo_lock_sha256": _sha256(root / "Cargo.lock"),
        },
        "host": {
            "os": {
                "system": platform_name,
                "release": platform.release(),
                "machine": platform.machine(),
            },
            "python": platform.python_version(),
            "shell": {
                "configured": shell,
                "resolved": shutil.which(shell) if shell else None,
                "zsh": shutil.which("zsh"),
                "bash": shutil.which("bash"),
            },
            "sandbox": {
                "kind": "bubblewrap" if platform_name == "Linux" else "policy-only",
                "status": (
                    "available"
                    if platform_name == "Linux" and shutil.which("bwrap")
                    else "unavailable"
                    if platform_name == "Linux"
                    else "unsupported-platform"
                ),
                "executable": shutil.which("bwrap") if platform_name == "Linux" else None,
            },
        },
        "runtime": _runtime_metadata(root),
        "corpora": _corpora(root),
        "security": {
            "threat_model_version": THREAT_MODEL_VERSION,
            "threat_model_reviewed": THREAT_MODEL_REVIEWED,
            "document": "SECURITY.md",
            "document_sha256": _sha256(root / "SECURITY.md"),
            "safety_matcher_role": "defense_in_depth",
            "known_limitations": [
                "macOS workspace policy is not an OS sandbox",
                "Linux host scope is intentionally unsandboxed",
                "text safety classification cannot prove a command safe",
                "prompt injection can influence model proposals",
            ],
            "sandbox_functional_evidence": {
                "required_linux_backend": "bubblewrap",
                "availability": (
                    "present" if platform_name == "Linux" and shutil.which("bwrap") else "absent"
                    if platform_name == "Linux" else "unsupported_platform"
                ),
                "qualification_gates": ["native-sandbox-functional", "rust-tests", "host-scope", "admin-validation"],
            },
        },
        "credentials": {
            "paid_live_configured": bool(env.get("AISHE_REALTEST_KEY")),
            "environment_variable": "AISHE_REALTEST_KEY",
        },
    }


def _resolved_command(gate: Gate, binary: pathlib.Path) -> list[str]:
    return [str(binary) if argument == BINARY else argument for argument in gate.command]


def _skip_reason(
    gate: Gate,
    *,
    platform_name: str,
    env: Mapping[str, str],
    tool_finder: Callable[[str], str | None],
) -> str | None:
    if gate.platforms is not None and platform_name not in gate.platforms:
        supported = ", ".join(sorted(gate.platforms))
        return f"not applicable on {platform_name}; supported platform: {supported}"
    if gate.credential_env and not env.get(gate.credential_env):
        return f"credential not configured: {gate.credential_env}"
    missing = [tool for tool in gate.required_tools if tool_finder(tool) is None]
    if missing:
        return f"required tool unavailable: {', '.join(missing)}"
    return None


def _gate_record(gate: Gate, binary: pathlib.Path) -> dict[str, object]:
    return {
        "id": gate.id,
        "label": gate.label,
        "command": _resolved_command(gate, binary),
        "status": "skip",
        "required": gate.required,
        "external_harness": gate.external_harness,
        "execution_env": dict(gate.execution_env),
        "duration_ms": 0,
        "returncode": None,
        "skip_reason": None,
    }


def reported_skip(output: str) -> str | None:
    """A harness's zero exit is not proof that its required gate ran."""
    plain = re.sub(r"\x1b\[[0-9;?]*[ -/]*[@-~]", "", output)
    match = re.search(r"(?im)^\s*SKIP(?:\s|[:(])[^\n]*", plain)
    return match.group().strip() if match else None


def _write_report(output: pathlib.Path, report: dict[str, object]) -> None:
    output.parent.mkdir(parents=True, exist_ok=True)
    data = json.dumps(report, indent=2, sort_keys=True) + "\n"
    descriptor, temporary = tempfile.mkstemp(prefix=f".{output.name}.", dir=output.parent)
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8") as handle:
            handle.write(data)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, output)
    finally:
        try:
            os.unlink(temporary)
        except FileNotFoundError:
            pass


def run_qualification(
    profile: Profile,
    output: pathlib.Path,
    *,
    root: pathlib.Path,
    keep_going: bool = False,
    runner: Runner | None = None,
    env: Mapping[str, str] | None = None,
    platform_name: str | None = None,
    identity_verifier: Callable[..., str] = require_current_binary,
    tool_finder: Callable[[str], str | None] | None = None,
    announce: Callable[[str], None] = print,
) -> dict[str, object]:
    """Run a profile.  Dependency injection keeps orchestration tests cheap."""

    root = root.resolve()
    output = output.resolve()
    binary = (root / "target/release/aishe").resolve()
    command_runner = runner or SubprocessRunner()
    run_env = dict(os.environ if env is None else env)
    find_tool = tool_finder or (
        lambda tool: shutil.which(tool, path=run_env.get("PATH"))
    )
    system = platform_name or platform.system()
    if system == "Linux" and profile.name in {"linux-full", "release", "paid-live"}:
        # Those profiles claim functional Linux isolation, so host-scope must
        # exercise bubblewrap rather than the policy-only compatibility path.
        run_env.setdefault("AISHE_TEST_REQUIRE_BWRAP", "1")
    started_wall = datetime.datetime.now(datetime.timezone.utc)
    started = time.monotonic_ns()
    metadata = collect_metadata(root, profile, run_env, platform_name=system)
    results: list[dict[str, object]] = []
    stopped_by: str | None = None
    release_built = False
    binary_verified = False
    binary_metadata: dict[str, object] = {
        "path": str(binary),
        "identity": None,
        "sha256": None,
        "verified_against_checkout": False,
    }

    with tempfile.TemporaryDirectory(prefix="aishe-qualification-runtime-") as runtime_dir:
        run_env.setdefault("AISHE_RUNTIME_DIR", runtime_dir)
        for gate in profile.gates:
            gate_env = {**run_env, **dict(gate.execution_env)}
            record = _gate_record(gate, binary)
            reason = _skip_reason(
                gate,
                platform_name=system,
                env=gate_env,
                tool_finder=find_tool,
            )
            if reason:
                if gate.platforms is not None and system not in gate.platforms:
                    # Required when applicable; an explicit cross-platform
                    # not-applicable record is not a release hold.
                    record["required"] = False
                record["skip_reason"] = reason
                results.append(record)
                announce(f"SKIP {gate.id}: {reason}")
                continue
            if stopped_by is not None:
                record["skip_reason"] = f"not run after failure: {stopped_by}"
                results.append(record)
                continue
            if gate.identity_check and not release_built:
                record["skip_reason"] = "release build did not pass"
                results.append(record)
                announce(f"SKIP {gate.id}: release build did not pass")
                continue
            if gate.external_harness and not binary_verified:
                record["skip_reason"] = "release binary identity was not verified"
                results.append(record)
                announce(f"SKIP {gate.id}: release binary identity was not verified")
                continue

            command = record["command"]
            announce(f"RUN  {gate.id}: {shlex.join(command)}")
            gate_started = time.monotonic_ns()
            if gate.identity_check:
                try:
                    verified_path = identity_verifier(binary, root=root, announce=False)
                    if pathlib.Path(verified_path).resolve() != binary:
                        raise ValueError(
                            f"identity verifier returned {verified_path}, expected {binary}"
                        )
                    completed = command_runner.run(
                        command,
                        cwd=root,
                        env=gate_env,
                        timeout=gate.timeout_seconds,
                    )
                    if completed.returncode == 0:
                        identity = parse_binary_identity(completed.stdout)
                        binary_metadata.update(
                            {
                                "identity": identity,
                                "sha256": _sha256(binary),
                                "verified_against_checkout": True,
                            }
                        )
                        binary_verified = True
                except (Exception, SystemExit) as error:
                    completed = CommandResult(1, "", str(error))
            else:
                completed = command_runner.run(
                    command,
                    cwd=root,
                    env=gate_env,
                    timeout=gate.timeout_seconds,
                )
            record["duration_ms"] = round((time.monotonic_ns() - gate_started) / 1_000_000, 3)
            record["returncode"] = completed.returncode
            record["skip_reason"] = None
            skipped = reported_skip(completed.stdout + "\n" + completed.stderr)
            if completed.returncode == 0 and skipped:
                record["status"] = "skip"
                record["skip_reason"] = "harness did not qualify the gate: " + skipped
                announce(f"SKIP {gate.id}: {skipped}")
            elif completed.returncode == 0:
                record["status"] = "pass"
                if gate.id == RELEASE_BUILD.id:
                    release_built = True
                announce(f"PASS {gate.id} ({record['duration_ms']:.1f} ms)")
            else:
                record["status"] = "fail"
                record["failure"] = (
                    completed.stderr.strip().splitlines()[-1]
                    if completed.stderr.strip()
                    else f"command exited {completed.returncode}"
                )
                announce(f"FAIL {gate.id} (exit {completed.returncode})")
                if not keep_going:
                    stopped_by = gate.id
            results.append(record)

    counts = {status: sum(result["status"] == status for result in results) for status in ("pass", "fail", "skip")}
    required_skips = sum(result["status"] == "skip" and result["required"] for result in results)
    if counts["fail"]:
        outcome = "failed"
    elif required_skips:
        outcome = "incomplete"
    elif counts["skip"]:
        outcome = "passed_with_skips"
    else:
        outcome = "passed"
    finished_wall = datetime.datetime.now(datetime.timezone.utc)
    report: dict[str, object] = {
        "schema_version": SCHEMA_VERSION,
        "kind": "aishe_qualification",
        "generated_at": finished_wall.isoformat(),
        "started_at": started_wall.isoformat(),
        **metadata,
        "binary": binary_metadata,
        "artifacts": _evidence_artifacts(root),
        "summary": {
            "outcome": outcome,
            "counts": counts,
            "required_skips": required_skips,
            "keep_going": keep_going,
            "stopped_after_failure": stopped_by,
            "duration_ms": round((time.monotonic_ns() - started) / 1_000_000, 3),
        },
        "gates": results,
    }
    _write_report(output, report)
    announce(
        f"qualification {profile.name}: {outcome.upper()} · "
        f"{counts['pass']} pass, {counts['fail']} fail, {counts['skip']} skip · {output}"
    )
    return report


def list_profiles(profile_name: str | None = None) -> None:
    selected = [PROFILES[profile_name]] if profile_name else list(PROFILES.values())
    for profile in selected:
        print(f"{profile.name}: {profile.description}")
        if profile_name:
            binary = pathlib.Path("target/release/aishe")
            for gate in profile.gates:
                qualifiers = []
                if gate.platforms:
                    qualifiers.append("platform=" + ",".join(sorted(gate.platforms)))
                if gate.credential_env:
                    qualifiers.append("credential=" + gate.credential_env)
                suffix = f" [{' '.join(qualifiers)}]" if qualifiers else ""
                print(f"  {gate.id:<31} {shlex.join(_resolved_command(gate, binary))}{suffix}")


def parse_arguments(argv: Sequence[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("profile", nargs="?", choices=sorted(PROFILES))
    parser.add_argument("--output", type=pathlib.Path, help="required JSON report path")
    parser.add_argument("--keep-going", action="store_true", help="run independent gates after a failure")
    parser.add_argument("--list", action="store_true", help="list profiles or the selected profile's commands")
    args = parser.parse_args(argv)
    if args.list:
        return args
    if not args.profile:
        parser.error("a profile is required unless --list is used")
    if args.output is None:
        parser.error("--output is required when running qualification")
    return args


def main(argv: Sequence[str] | None = None) -> int:
    args = parse_arguments(sys.argv[1:] if argv is None else argv)
    if args.list:
        list_profiles(args.profile)
        return 0
    root = pathlib.Path(__file__).resolve().parent.parent
    report = run_qualification(
        PROFILES[args.profile],
        args.output,
        root=root,
        keep_going=args.keep_going,
    )
    return 0 if report["summary"]["outcome"] in {"passed", "passed_with_skips"} else 1


if __name__ == "__main__":
    raise SystemExit(main())
