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
PROFILE_REVISION = "2026-07-31.5"
THREAT_MODEL_VERSION = "2026-07-31.1"
THREAT_MODEL_REVIEWED = "2026-07-31"
BINARY = "{release_binary}"


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
    "pty-smoke", "Interactive zsh PTY smoke", "tests/pty_smoke.py", BINARY, required_tools=("zsh",)
)
PTY_SCENARIOS = python_gate(
    "pty-scenarios",
    "Routing, sigil, auto-mode, and history scenarios",
    "tests/pty_scenarios.py",
    BINARY,
    required_tools=("zsh",),
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
    BASH_HOOK_CURRENT,
    python_gate(
        "statusline-pty", "Statusline placement and live metrics", "tests/statusline_pty.py", BINARY, required_tools=("zsh",)
    ),
    python_gate(
        "model-picker-pty",
        "Connection/model picker and concurrent selection",
        "tests/model_picker_pty.py",
        BINARY,
        required_tools=("zsh",),
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
    ),
    python_gate(
        "palette-pty",
        "Palette repaints the prompt and fills slash forms",
        "tests/palette_pty.py",
        BINARY,
        required_tools=("zsh",),
    ),
    python_gate(
        "mode-handoff-pty",
        "aishe mode and /mode agree inside the shell",
        "tests/mode_handoff_pty.py",
        BINARY,
        required_tools=("zsh",),
    ),
    python_gate(
        "bare-words-pty",
        "Bare reset and details stay the user's commands",
        "tests/bare_words_pty.py",
        BINARY,
        required_tools=("zsh",),
    ),
    python_gate(
        "theme-prompt-pty",
        "A prompt theme survives; a stock prompt gets the glyph",
        "tests/theme_prompt_pty.py",
        BINARY,
        required_tools=("zsh",),
    ),
    python_gate(
        "keys-pty",
        "Shift-Tab cycles the mode only on an empty line",
        "tests/keys_pty.py",
        BINARY,
        required_tools=("zsh",),
    ),
    python_gate(
        "slash-highlight-pty",
        "A registered /command does not read as an error",
        "tests/slash_highlight_pty.py",
        BINARY,
        required_tools=("zsh",),
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
    ),
    python_gate(
        "connection-isolation",
        "Same-provider credential and runtime isolation",
        "tests/opencode_connection_isolation.py",
        BINARY,
        timeout=900,
    ),
    python_gate(
        "host-scope",
        "Workspace-to-host authority and focus output",
        "tests/opencode_host_scope.py",
        BINARY,
        timeout=900,
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
    ),
    python_gate(
        "opencode-concurrency",
        "Concurrent managed-session isolation",
        "tests/opencode_concurrency.py",
        BINARY,
        "--sessions",
        "8",
        timeout=900,
    ),
    python_gate(
        "durable-task-resume",
        "Durable task interruption and resume",
        "tests/durable_task_resume.py",
        BINARY,
        timeout=900,
    ),
    python_gate(
        "pty-fuzz", "Generated PTY and adversarial response fuzz", "tests/pty_fuzz.py", BINARY, required_tools=("zsh",)
    ),
    python_gate(
        "zsh-features", "zsh feature matrix", "tests/zsh_features.py", BINARY, required_tools=("zsh",), timeout=900
    ),
    python_gate(
        "pty-signals", "PTY signals and resize behavior", "tests/pty_signals.py", BINARY, required_tools=("zsh",)
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
    ),
    python_gate(
        "real-model-fuzz",
        "Paid live-model robustness fuzz",
        "tests/real_fuzz.py",
        BINARY,
        timeout=1800,
        credential_env="AISHE_REALTEST_KEY",
        required=False,
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
                "qualification_gates": ["rust-tests", "host-scope", "admin-validation"],
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
        "duration_ms": 0,
        "returncode": None,
        "skip_reason": None,
    }


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
            record = _gate_record(gate, binary)
            reason = _skip_reason(
                gate,
                platform_name=system,
                env=run_env,
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
                        env=run_env,
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
                    env=run_env,
                    timeout=gate.timeout_seconds,
                )
            record["duration_ms"] = round((time.monotonic_ns() - gate_started) / 1_000_000, 3)
            record["returncode"] = completed.returncode
            record["skip_reason"] = None
            if completed.returncode == 0:
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
