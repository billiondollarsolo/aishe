#!/usr/bin/env python3
"""Bounded native macOS startup attribution, separate from release qualification.

The three minimal-main controls expose native image/runtime costs. AIShe's
--version still parses full Clap; no control is a substitute startup SLO.
"""

import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import selectors
import shutil
import signal
import subprocess
import tempfile
import time

from direct_shell_benchmark import write_config
from direct_shell_profile import measured, summarize
from harness_identity import parse_binary_identity, require_current_binary


COMMANDS = 100
WARMUP = 10
OUTPUT_LIMIT = 8 * 1024 * 1024
DEADLINE_SECONDS = 150
C_SOURCE = "int main(void) { return 0; }\n"
RUST_SOURCE = "fn main() {}\n"


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def capture(argv, env, deadline, timeout=15):
    """Cap elapsed time and combined pipe bytes; terminate the whole probe group."""
    remaining = min(timeout, deadline - time.monotonic())
    if remaining <= 0:
        raise TimeoutError("native diagnostic total deadline exceeded")
    started = time.monotonic()
    process = subprocess.Popen(argv, env=env, stdin=subprocess.DEVNULL,
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                               start_new_session=True)
    streams = {"stdout": bytearray(), "stderr": bytearray()}
    reason = None
    with selectors.DefaultSelector() as selector:
        selector.register(process.stdout, selectors.EVENT_READ, "stdout")
        selector.register(process.stderr, selectors.EVENT_READ, "stderr")
        while selector.get_map():
            if time.monotonic() - started >= remaining:
                reason = "timeout"
                break
            for key, _ in selector.select(timeout=0.05):
                chunk = os.read(key.fileobj.fileno(), 65536)
                if not chunk:
                    selector.unregister(key.fileobj)
                    continue
                room = OUTPUT_LIMIT - sum(map(len, streams.values()))
                streams[key.data].extend(chunk[:room])
                if len(chunk) > room:
                    reason = "output_limit"
                    break
            if reason:
                break
    if reason:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
    try:
        code = process.wait(timeout=max(0.1, remaining - (time.monotonic() - started)))
    except subprocess.TimeoutExpired:
        reason = "timeout"
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        code = process.wait(timeout=3)
    process.stdout.close()
    process.stderr.close()
    return {"argv": [str(value) for value in argv], "returncode": code,
            "bounded_failure": reason, "elapsed_seconds": time.monotonic() - started,
            **{name: value.decode("utf-8", errors="replace")
               for name, value in streams.items()}}


def checked(result, description):
    if result["returncode"] or result["bounded_failure"]:
        raise RuntimeError(f"{description} failed: {result['returncode']}, "
                           f"{result['bounded_failure']}: {result.get('stderr', '')[:500]}")
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary")
    parser.add_argument("--output", type=Path,
                        default=Path("test-results/native-startup-diagnostic-macOS.json"))
    args = parser.parse_args()
    if platform.system() != "Darwin":
        raise SystemExit("native startup diagnostic requires an actual macOS host")
    deadline = time.monotonic() + DEADLINE_SECONDS
    binary = Path(require_current_binary(args.binary)).resolve()
    output = args.output.resolve()
    output.parent.mkdir(parents=True, exist_ok=True)
    evidence = output.with_suffix("")
    evidence.mkdir(parents=True, exist_ok=True)
    repository = Path(__file__).resolve().parent.parent
    report = {"schema_version": 1, "kind": "aishe_native_startup_diagnostic",
              "qualification_substitute": False, "commands": COMMANDS, "warmup": WARMUP,
              "generated_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
              "platform": platform.platform(), "machine": platform.machine(),
              "binary": str(binary), "binary_sha256": digest(binary),
              "binary_bytes": binary.stat().st_size, "tools": {}, "probes": {},
              "macho": {}, "loader_logs": {}, "cases": {}, "raw_rows": [],
              "limitations": [
                  "Minimal controls have empty mains; AIShe version includes full Clap and output.",
                  "CPU of a waited AIShe command includes its backing-shell child.",
                  "Controls isolate whole linked-image/runtime differences, not individual dyld phases.",
                  "Forced framework control loads both frameworks but makes no CF/Security API call.",
                  "Loader logging perturbs execution and is never a timing measurement.",
                  "Apple-protected backing shells may discard DYLD variables under SIP.",
                  "No unsupported legacy DYLD_PRINT_STATISTICS flags or Instruments attachment.",
              ]}
    failure = None
    try:
        identity = checked(capture([str(binary), "--version"], os.environ, deadline), "identity")
        report["binary_identity"] = parse_binary_identity(identity["stdout"])
        for label, command in (("source_commit", ["git", "-C", str(repository), "rev-parse", "HEAD"]),
                               ("source_status", ["git", "-C", str(repository), "status", "--porcelain"])):
            result = checked(capture(command, os.environ, deadline), label)
            if label == "source_commit":
                report[label] = result["stdout"].strip()
            else:
                report["source_dirty"] = bool(result["stdout"])
        if not report["source_commit"].startswith(report["binary_identity"]["commit"]):
            raise RuntimeError("diagnostic binary/source identity mismatch")
        if report["source_dirty"]:
            raise RuntimeError("native diagnostic requires a clean source checkout")

        tools = {name: shutil.which(name) for name in ("clang", "rustc", "rustup", "otool", "dyld_info", "file", "zsh")}
        # rustc can be a rustup shim whose lookup depends on HOME. Resolve its
        # current compiler before changing HOME, preserving the CI toolchain.
        if tools["rustup"]:
            current = capture([tools["rustup"], "which", "rustc"], os.environ, deadline)
            report["tools"]["rustup_current_compiler"] = current
            if current["returncode"] == 0 and not current["bounded_failure"]:
                tools["rustc"] = current["stdout"].strip()
        for name, path in tools.items():
            report["tools"][name] = {"available": bool(path), "path": path}
        for name in ("clang", "rustc", "otool", "zsh"):
            if not tools[name]:
                raise RuntimeError(f"required diagnostic tool unavailable: {name}")
        for name, flags in (("clang", ["--version"]), ("rustc", ["-vV"])):
            report["tools"][name]["identity"] = checked(
                capture([tools[name], *flags], os.environ, deadline), name + " identity")
        xcrun = shutil.which("xcrun")
        objdump = shutil.which("llvm-objdump")
        if xcrun and not objdump:
            discovery = capture([xcrun, "--find", "llvm-objdump"], os.environ, deadline)
            report["tools"]["llvm_objdump_discovery"] = discovery
            if discovery["returncode"] == 0 and not discovery["bounded_failure"]:
                objdump = discovery["stdout"].strip()
        report["tools"]["llvm_objdump"] = {"available": bool(objdump), "path": objdump}

        with tempfile.TemporaryDirectory(prefix="aishe-native-startup-") as text:
            root = Path(text).resolve()
            write_config(root)
            # An allowlist keeps provider credentials, DYLD overrides and other
            # AIShe session state out of measured child environments.
            env = {name: os.environ[name] for name in
                   ("PATH", "TMPDIR", "LANG", "LC_ALL", "SDKROOT", "MACOSX_DEPLOYMENT_TARGET")
                   if name in os.environ}
            env.update(HOME=str(root), AISHE_CONFIG_DIR=str(root / "config"),
                       AISHE_DATA_DIR=str(root / "data"), AISHE_RUNTIME_DIR=str(root / "runtime"),
                       XDG_CONFIG_HOME=str(root / "config"), XDG_DATA_HOME=str(root / "data"),
                       AISHE_SHELL_ID="nativestartupdiagnostic012345", NO_COLOR="1", TERM="dumb")
            c_path = evidence / "minimal.c"
            rust_path = evidence / "minimal.rs"
            c_path.write_text(C_SOURCE)
            rust_path.write_text(RUST_SOURCE)
            builds = [
                ("c_minimal", c_path, [tools["clang"], "-Oz", str(c_path)]),
                ("c_frameworks", c_path, [tools["clang"], "-Oz", str(c_path),
                 "-Wl,-needed_framework,Security", "-Wl,-needed_framework,CoreFoundation"]),
                ("rust_minimal", rust_path, [tools["rustc"], str(rust_path), "--edition=2021",
                 "-C", "opt-level=z", "-C", "lto=fat", "-C", "codegen-units=1",
                 "-C", "panic=unwind", "-C", "strip=symbols"]),
            ]
            executables = {"aishe": binary}
            for name, source, command in builds:
                executable = evidence / name
                result = capture([*command, "-o", str(executable)], env, deadline, 30)
                report["probes"][name] = {"source_sha256": digest(source), "compile": result,
                    "purpose": "minimal main, return status 0, no output"}
                checked(result, name + " compile")
                report["probes"][name].update(binary_sha256=digest(executable),
                                              binary_bytes=executable.stat().st_size)
                executables[name] = executable
            for name, executable in executables.items():
                imports = checked(capture([tools["otool"], "-L", str(executable)], env, deadline), name + " imports")
                commands = checked(capture([tools["otool"], "-l", str(executable)], env, deadline), name + " load commands")
                report["macho"][name] = {"imports": imports, "load_commands": commands,
                    "has_chained_fixups": "LC_DYLD_CHAINED_FIXUPS" in commands["stdout"]}
                report["macho"][name]["headers"] = checked(
                    capture([tools["otool"], "-hv", str(executable)], env, deadline), name + " headers")
                if tools["file"]:
                    report["macho"][name]["file"] = capture([tools["file"], str(executable)], env, deadline)
                if tools["dyld_info"]:
                    report["macho"][name]["fixups"] = capture([tools["dyld_info"], "-fixups", str(executable)], env, deadline)
                    report["macho"][name]["exports"] = capture([tools["dyld_info"], "-exports", str(executable)], env, deadline)
            imported = report["macho"]["c_frameworks"]["imports"]["stdout"]
            frameworks_verified = all(f"/{name}.framework/" in imported
                                      for name in ("Security", "CoreFoundation"))
            report["probes"]["c_frameworks"]["forced_imports_verified"] = frameworks_verified
            if not frameworks_verified:
                raise RuntimeError("forced framework control lacks verified Security/CoreFoundation imports")
            base_imports = report["macho"]["c_minimal"]["imports"]["stdout"]
            if any(f"/{name}.framework/" in base_imports for name in ("Security", "CoreFoundation")):
                raise RuntimeError("minimal C control unexpectedly links the compared frameworks")
            # LC_MAIN contains a file offset. Convert it through __TEXT to the
            # virtual entry address even when stripping removed _main symbols.
            load_commands = report["macho"]["aishe"]["load_commands"]["stdout"]
            entry = re.search(r"cmd LC_MAIN\s+cmdsize \d+\s+entryoff (\d+)", load_commands)
            segment = re.search(r"segname __TEXT\s+vmaddr (0x[0-9a-fA-F]+).*?fileoff (\d+)",
                                load_commands, re.DOTALL)
            if objdump and entry and segment:
                address = int(segment[1], 16) + int(entry[1]) - int(segment[2])
                report["macho"]["aishe"]["entry_disassembly"] = capture(
                    [objdump, "--disassemble", f"--start-address={address:#x}",
                     f"--stop-address={address + 512:#x}", str(binary)], env, deadline)
                report["macho"]["aishe"]["entry_address"] = hex(address)
            else:
                report["macho"]["aishe"]["entry_disassembly"] = {
                    "available": False, "reason": "llvm-objdump or LC_MAIN/__TEXT metadata unavailable"}

            active = dict(env, AISHE_HISTFILE=str(root / "active-history.ext"), AISHE_ZSH_PROFILE="clean")
            version_output = identity["stdout"].encode()
            cases = [(name, [str(executables[name])], env, b"")
                     for name in ("c_minimal", "c_frameworks", "rust_minimal")]
            cases += [("raw_zsh_noop", [tools["zsh"], "-c", ":"], env, b""),
                      ("aishe_direct_noop", [str(binary), "-c", ":"], env, b""),
                      ("aishe_direct_active_history_noop", [str(binary), "-c", ":"], active, b""),
                      ("aishe_version_full_clap", [str(binary), "--version"], env, version_output)]
            rows = {name: [] for name, *_ in cases}
            for number in range(WARMUP + COMMANDS):
                offset = number % len(cases)
                for order, (name, argv, case_env, expected) in enumerate(cases[offset:] + cases[:offset]):
                    if deadline - time.monotonic() < 15:
                        raise TimeoutError("native measurement deadline reached")
                    row = measured(argv, case_env, expected)
                    if number >= WARMUP:
                        row.update(case=name, round=number - WARMUP, order=order,
                                   status=0, exact_stdout=True, empty_stderr=True)
                        rows[name].append(row)
                        report["raw_rows"].append(row)
            report["cases"] = {name: {"samples": len(values), **summarize(values)}
                               for name, values in rows.items()}
            backend = root / "data" / "aishe" / "backend"
            report["backend_materialized"] = backend.exists()
            if backend.exists():
                raise RuntimeError("diagnostic direct commands materialized backend state")

            # Log captures are separate from the uninstrumented timings above.
            logged_env = dict(env, DYLD_PRINT_LIBRARIES="1", DYLD_PRINT_LOADERS="1",
                              DYLD_PRINT_INITIALIZERS="1", DYLD_PRINT_BINDINGS="1")
            log_cases = [(name, [str(executable)], b"") for name, executable in executables.items()
                         if name != "aishe"]
            log_cases += [("aishe_version_full_clap", [str(binary), "--version"], version_output),
                          ("aishe_direct_noop", [str(binary), "-c", ":"], b"")]
            for name, argv, expected in log_cases:
                result = capture(argv, logged_env, deadline)
                log = evidence / f"{name}-loader.log"
                log.write_text(result.pop("stderr"))
                result.update(log_path=str(log), log_sha256=digest(log), log_bytes=log.stat().st_size,
                              logging_observed=log.stat().st_size > 0,
                              exact_stdout=result["stdout"].encode() == expected)
                report["loader_logs"][name] = result
                checked(result, name + " loader capture")
                if not result["exact_stdout"]:
                    raise RuntimeError(name + " loader capture changed command output")
            report["status"] = "recorded"
    except (AssertionError, OSError, subprocess.SubprocessError, ValueError, RuntimeError, TimeoutError) as error:
        failure = str(error)
        report.update(status="failed", error=failure)
    finally:
        report["finished_at"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
        output.write_text(json.dumps(report, indent=2) + "\n")
    print(f"native startup diagnostic {report['status']}: {output}")
    if failure:
        raise SystemExit(failure)
    for name, values in report["cases"].items():
        print(f"  {name}: wall p95 {values['p95_ms']:.3f} ms; "
              f"mean child CPU {values['mean_child_cpu_ms']:.3f} ms")
    print("Recorded diagnostics only; original startup qualification is unchanged.")


if __name__ == "__main__":
    main()
