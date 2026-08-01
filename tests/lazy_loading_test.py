#!/usr/bin/env python3
"""Prove local AIShe surfaces do not eagerly start network/provider/backend work.

The harness combines three independent signals:

* a missing required provider credential makes provider construction fatal;
* a loopback provider canary records every attempted HTTP connection;
* a validated preload shim records connect/bind/listen syscalls in the AIShe
  process tree.

Backend startup is additionally checked through AIShe's durable backend-state
marker. The shim is supported on the project's macOS/Linux target platforms and
is self-tested before any product assertion is accepted.
"""

from __future__ import annotations

import argparse
import datetime
import json
import os
import pathlib
import platform
import shutil
import socket
import subprocess
import tempfile
import threading

from harness_identity import require_current_binary


SCHEMA_VERSION = 1
AUDITED_OPERATIONS = frozenset({"connect", "bind", "listen"})


SHIM_SOURCE = r"""
#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/socket.h>
#include <sys/types.h>
#include <unistd.h>

static void record_event(const char *operation, int descriptor, const struct sockaddr *address) {
    const char *path = getenv("AISHE_TEST_NETWORK_AUDIT_LOG");
    if (path == NULL || path[0] == '\0') return;
    int output = open(path, O_WRONLY | O_CREAT | O_APPEND, 0600);
    if (output < 0) return;
    int family = address == NULL ? -1 : address->sa_family;
    char line[160];
    int length = snprintf(line, sizeof(line),
        "{\"pid\":%ld,\"operation\":\"%s\",\"fd\":%d,\"family\":%d}\n",
        (long)getpid(), operation, descriptor, family);
    if (length > 0) write(output, line, (size_t)length);
    close(output);
}

static int audit_denies_network(void) {
    const char *path = getenv("AISHE_TEST_NETWORK_AUDIT_LOG");
    if (path == NULL || path[0] == '\0') return 0;
    errno = EPERM;
    return 1;
}

static int aishe_connect(int descriptor, const struct sockaddr *address, socklen_t length) {
    typedef int (*function_type)(int, const struct sockaddr *, socklen_t);
    static function_type next = NULL;
    record_event("connect", descriptor, address);
    if (audit_denies_network()) return -1;
    if (next == NULL) next = (function_type)dlsym(RTLD_NEXT, "connect");
    return next == NULL ? -1 : next(descriptor, address, length);
}

static int aishe_bind(int descriptor, const struct sockaddr *address, socklen_t length) {
    typedef int (*function_type)(int, const struct sockaddr *, socklen_t);
    static function_type next = NULL;
    record_event("bind", descriptor, address);
    if (audit_denies_network()) return -1;
    if (next == NULL) next = (function_type)dlsym(RTLD_NEXT, "bind");
    return next == NULL ? -1 : next(descriptor, address, length);
}

static int aishe_listen(int descriptor, int backlog) {
    typedef int (*function_type)(int, int);
    static function_type next = NULL;
    record_event("listen", descriptor, NULL);
    if (audit_denies_network()) return -1;
    if (next == NULL) next = (function_type)dlsym(RTLD_NEXT, "listen");
    return next == NULL ? -1 : next(descriptor, backlog);
}

#ifdef __APPLE__
#define AISHE_INTERPOSE(replacement, replacee) \
    __attribute__((used)) static struct { const void *replacement; const void *replacee; } \
    aishe_interpose_##replacee __attribute__((section("__DATA,__interpose"))) = \
        { (const void *)(unsigned long)&replacement, (const void *)(unsigned long)&replacee };
AISHE_INTERPOSE(aishe_connect, connect)
AISHE_INTERPOSE(aishe_bind, bind)
AISHE_INTERPOSE(aishe_listen, listen)
#else
int connect(int descriptor, const struct sockaddr *address, socklen_t length) {
    return aishe_connect(descriptor, address, length);
}
int bind(int descriptor, const struct sockaddr *address, socklen_t length) {
    return aishe_bind(descriptor, address, length);
}
int listen(int descriptor, int backlog) {
    return aishe_listen(descriptor, backlog);
}
#endif
"""


PROBE_SOURCE = r"""
#include <arpa/inet.h>
#include <sys/socket.h>
#include <unistd.h>

int main(void) {
    int descriptor = socket(AF_INET, SOCK_STREAM, 0);
    struct sockaddr_in address = {0};
    address.sin_family = AF_INET;
    address.sin_port = htons(9);
    address.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    connect(descriptor, (struct sockaddr *)&address, sizeof(address));
    close(descriptor);
    return 0;
}
"""


def parse_audit(path: pathlib.Path) -> list[dict]:
    if not path.exists():
        return []
    events = []
    for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        try:
            event = json.loads(line)
        except json.JSONDecodeError as error:
            raise AssertionError(f"invalid network audit event on line {number}: {error}")
        if event.get("operation") not in AUDITED_OPERATIONS:
            raise AssertionError(f"unexpected network audit operation: {event!r}")
        events.append(event)
    return events


class NetworkAudit:
    def __init__(self, root: pathlib.Path):
        system = platform.system()
        if system not in {"Darwin", "Linux"}:
            raise AssertionError(f"network syscall audit is unsupported on {system}")
        compiler = shutil.which("cc")
        if not compiler:
            raise AssertionError("cc is required for the network syscall audit")
        extension = "dylib" if system == "Darwin" else "so"
        self.library = root / f"libaishe_network_audit.{extension}"
        source = root / "network_audit.c"
        probe_source = root / "network_probe.c"
        self.probe = root / "network_probe"
        source.write_text(SHIM_SOURCE, encoding="utf-8")
        probe_source.write_text(PROBE_SOURCE, encoding="utf-8")
        library_command = [compiler, "-O2", "-fPIC"]
        if system == "Darwin":
            library_command.extend(["-dynamiclib", str(source), "-o", str(self.library)])
        else:
            library_command.extend(
                ["-shared", str(source), "-o", str(self.library), "-ldl"]
            )
        self._compile(library_command)
        self._compile([compiler, "-O2", str(probe_source), "-o", str(self.probe)])
        self.system = system
        self._self_test(root / "network-audit-self-test.jsonl")

    @staticmethod
    def _compile(command: list[str]) -> None:
        result = subprocess.run(
            command,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            timeout=30,
            check=False,
        )
        if result.returncode != 0:
            raise AssertionError(
                f"could not build network audit fixture: {command!r}\n{result.stderr}"
            )

    def environment(self, log: pathlib.Path) -> dict[str, str]:
        values = {"AISHE_TEST_NETWORK_AUDIT_LOG": str(log)}
        if self.system == "Darwin":
            values["DYLD_INSERT_LIBRARIES"] = str(self.library)
            values["DYLD_FORCE_FLAT_NAMESPACE"] = "1"
        else:
            values["LD_PRELOAD"] = str(self.library)
        return values

    def _self_test(self, log: pathlib.Path) -> None:
        environment = os.environ.copy()
        environment.update(self.environment(log))
        result = subprocess.run(
            [str(self.probe)],
            env=environment,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
            text=True,
            timeout=10,
            check=False,
        )
        if result.returncode != 0:
            raise AssertionError(f"network audit self-test failed: {result.stderr}")
        operations = {event["operation"] for event in parse_audit(log)}
        if "connect" not in operations:
            raise AssertionError("network audit self-test did not intercept connect()")
        log.unlink(missing_ok=True)


class ProviderCanary:
    def __init__(self):
        self.listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self.listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self.listener.bind(("127.0.0.1", 0))
        self.listener.listen(8)
        self.listener.settimeout(0.1)
        self.port = self.listener.getsockname()[1]
        self.requests = 0
        self.running = True
        self.thread = threading.Thread(target=self._serve, daemon=True)
        self.thread.start()

    def _serve(self) -> None:
        while self.running:
            try:
                connection, _ = self.listener.accept()
            except socket.timeout:
                continue
            except OSError:
                return
            with connection:
                self.requests += 1
                try:
                    connection.recv(16 * 1024)
                    connection.sendall(
                        b"HTTP/1.1 500 Internal Server Error\r\n"
                        b"Content-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                except OSError:
                    pass

    def close(self) -> None:
        self.running = False
        self.listener.close()
        self.thread.join(timeout=2)


def write_config(root: pathlib.Path, provider_port: int) -> None:
    config = root / "config" / "aishe" / "config.toml"
    config.parent.mkdir(parents=True)
    config.write_text(
        f"""version = 7

[aishe]
mode = "suggest"
provider = "openai"
connection = "lazy"
connection_fallback = "lazy"

[backend]
engine = "opencode"
fallback = "none"

[providers.openai]
base_url = "http://127.0.0.1:{provider_port}"
credential = "lazy"
api_key_env = "AISHE_LAZY_PROVIDER_KEY"
model = "lazy-model"
transport = "chat"
auth_required = true

[connections.lazy]
provider = "openai"
label = "Lazy loading canary"
base_url = "http://127.0.0.1:{provider_port}"
credential = "lazy"
api_key_env = "AISHE_LAZY_PROVIDER_KEY"
model = "lazy-model"
transport = "chat"
auth_required = true

[connections.lazy.auth]
type = "api_key"
credential = "lazy"
""",
        encoding="utf-8",
    )


def assert_surface(name: str, command: list[str], env: dict[str, str], root: pathlib.Path,
                   audit: NetworkAudit, canary: ProviderCanary) -> dict:
    log = root / f"network-{name}.jsonl"
    child_env = env.copy()
    child_env.update(audit.environment(log))
    before_requests = canary.requests
    result = subprocess.run(
        command,
        env=child_env,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        timeout=20,
        check=False,
    )
    if result.returncode != 0:
        raise AssertionError(
            f"{name} failed with {result.returncode}\n"
            f"stdout:\n{result.stdout}\nstderr:\n{result.stderr}"
        )
    events = parse_audit(log)
    if events:
        raise AssertionError(f"{name} performed network syscalls: {events}")
    provider_requests = canary.requests - before_requests
    if provider_requests:
        raise AssertionError(f"{name} made {provider_requests} provider request(s)")
    backend = root / "data" / "aishe" / "backend"
    if backend.exists():
        paths = [str(path.relative_to(root)) for path in backend.rglob("*")]
        raise AssertionError(f"{name} materialized backend state: {paths[:20]}")

    if name == "shell" and result.stdout != "lazy-shell-ok\n":
        raise AssertionError(f"shell output changed: {result.stdout!r}")
    if name == "help" and "Usage: aishe" not in result.stdout:
        raise AssertionError("help output did not contain the root usage")
    if name == "route":
        payload = json.loads(result.stdout)
        if payload.get("kind") != "shell" or payload.get("schema_version") != 1:
            raise AssertionError(f"route output changed: {payload!r}")
    if name == "status":
        payload = json.loads(result.stdout)
        if payload.get("schema_version") != 1:
            raise AssertionError(f"status output changed: {payload!r}")

    return {
        "exit_code": result.returncode,
        "provider_started": False,
        "provider_requests": 0,
        "backend_started": False,
        "backend_state_materialized": False,
        "network_connect_calls": 0,
        "network_bind_calls": 0,
        "network_listen_calls": 0,
        "pass": True,
    }


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("binary")
    parser.add_argument("--output", type=pathlib.Path)
    args = parser.parse_args()
    binary = require_current_binary(args.binary)

    with tempfile.TemporaryDirectory(prefix="aishe-lazy-loading-") as text:
        root = pathlib.Path(text)
        audit = NetworkAudit(root)
        canary = ProviderCanary()
        try:
            write_config(root, canary.port)
            env = os.environ.copy()
            env.update(
                {
                    "HOME": str(root),
                    "AISHE_CONFIG_DIR": str(root / "config"),
                    "AISHE_DATA_DIR": str(root / "data"),
                    "AISHE_RUNTIME_DIR": str(root / "runtime"),
                    "XDG_CONFIG_HOME": str(root / "config"),
                    "XDG_DATA_HOME": str(root / "data"),
                    "NO_COLOR": "1",
                    "TERM": "dumb",
                }
            )
            env.pop("AISHE_LAZY_PROVIDER_KEY", None)
            surfaces = {
                "shell": [binary, "-c", "printf 'lazy-shell-ok\\n'"],
                "help": [binary, "--help"],
                "route": [binary, "route", "--json", "--", "printf lazy-route-ok"],
                "status": [binary, "status", "--json"],
            }
            records = {
                name: assert_surface(name, command, env, root, audit, canary)
                for name, command in surfaces.items()
            }
        finally:
            canary.close()

    report = {
        "schema_version": SCHEMA_VERSION,
        "kind": "aishe_lazy_loading",
        "generated_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "binary": binary,
        "platform": platform.system(),
        "instrumentation": {
            "network": "validated dynamic-library syscall interposition",
            "audited_operations": sorted(AUDITED_OPERATIONS),
            "provider": "missing required credential plus loopback request canary",
            "backend": "durable backend-state marker",
        },
        "surfaces": records,
        "pass": all(record["pass"] for record in records.values()),
    }
    if args.output:
        output = args.output.resolve()
    else:
        stamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
        output = pathlib.Path("test-results") / f"lazy-loading-{stamp}.json"
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report, indent=2))
    print(f"report: {output}")
    print("PASS: local shell/help/route/status stayed provider/backend/network lazy")


if __name__ == "__main__":
    main()
