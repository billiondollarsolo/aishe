#!/usr/bin/env python3
"""Exercise real native platform trust on a disposable macOS fixture.

The accepted endpoint is unpaid public HTTPS. A private self-signed localhost
server must be rejected; this fixture never adds certificates to a keychain.
"""

import argparse
import datetime
import hashlib
import http.server
import json
import os
from pathlib import Path
import platform
import shutil
import ssl
import subprocess
import tempfile
import threading
import time

from harness_identity import parse_binary_identity, require_current_binary
from native_startup_diagnostic import capture, checked, native_link_attributes


TEST_NAMES = ["platform_tls::tests::native_first_concurrent_https_and_untrusted_rejection",
              "platform_tls::tests::native_https_proxy_activates_for_http_target"]


class Handler(http.server.BaseHTTPRequestHandler):
    requests = 0

    def do_GET(self):
        type(self).requests += 1
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b"untrusted certificate was accepted\n")

    def log_message(self, *_args):
        pass

    def do_CONNECT(self):
        type(self).requests += 1
        self.send_error(501)


class Redirect(http.server.BaseHTTPRequestHandler):
    requests = 0

    def do_GET(self):
        type(self).requests += 1
        self.send_response(302)
        self.send_header("Location", "https://example.com/")
        self.end_headers()

    def log_message(self, *_args):
        pass


def sha256(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary")
    parser.add_argument("--output", type=Path,
                        default=Path("test-results/native-tls-activation-macOS.json"))
    args = parser.parse_args()
    if platform.system() != "Darwin":
        raise SystemExit("native TLS activation requires actual macOS")
    binary = require_current_binary(args.binary)
    repository = Path(__file__).resolve().parent.parent
    output = args.output.resolve()
    output.parent.mkdir(parents=True, exist_ok=True)
    evidence = output.with_suffix("")
    evidence.mkdir(parents=True, exist_ok=True)
    deadline = time.monotonic() + 720
    report = {"schema_version": 1, "kind": "aishe_native_tls_activation",
              "generated_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
              "binary": binary, "binary_sha256": sha256(binary),
              "public_https_url": "https://example.com/", "trust_store_modified": False,
              "paid_provider_used": False, "steps": {}, "status": "failed"}
    server = None
    redirect_server = None
    failure = None
    try:
        identity = checked(capture([binary, "--version"], os.environ, deadline), "binary identity")
        report["binary_identity"] = parse_binary_identity(identity["stdout"])
        source = checked(capture(["git", "-C", str(repository), "rev-parse", "HEAD"],
                                 os.environ, deadline), "source identity")
        report["source_commit"] = source["stdout"].strip()
        dirty = checked(capture(["git", "-C", str(repository), "status", "--porcelain"],
                                os.environ, deadline), "source status")
        report["source_dirty"] = bool(dirty["stdout"])
        if report["source_dirty"] or not report["source_commit"].startswith(report["binary_identity"]["commit"]):
            raise RuntimeError("native TLS proof requires matching clean source")
        cargo = shutil.which("cargo")
        openssl = shutil.which("openssl")
        if not cargo or not openssl:
            raise RuntimeError("native TLS proof requires Cargo and OpenSSL")
        compile_result = capture([cargo, "test", "--release", "--locked", "--lib", "platform_tls::tests::native_",
                                  "--no-run", "--message-format=json"], os.environ, deadline, 600)
        compile_log = evidence / "compile.jsonl"
        compile_log.write_text(compile_result["stdout"] + "\n" + compile_result["stderr"])
        report["steps"]["compile"] = {**compile_result, "stdout": "retained in compile.jsonl",
                                       "stderr": "retained in compile.jsonl", "log_sha256": sha256(compile_log)}
        checked(compile_result, "native TLS test compilation")
        executables = []
        for line in compile_result["stdout"].splitlines():
            try:
                row = json.loads(line)
            except ValueError:
                continue
            if row.get("reason") == "compiler-artifact" and row.get("executable") and row.get("profile", {}).get("test"):
                executables.append(row["executable"])
        if len(executables) != 1:
            raise RuntimeError(f"expected one actual library test executable, found {len(executables)}")
        executable = executables[0]
        report["test_executable"] = executable
        report["test_executable_sha256"] = sha256(executable)
        retained_executable = evidence / "native-tls-test-executable"
        shutil.copy2(executable, retained_executable)
        if sha256(retained_executable) != report["test_executable_sha256"]:
            raise RuntimeError("retained native test executable copy hash mismatch")
        report["artifact_test_executable_path"] = str(Path(evidence.name) / retained_executable.name)
        report["test_executable_bytes"] = retained_executable.stat().st_size
        report["test_native_link_attributes"] = native_link_attributes(executable)
        report["product_native_link_attributes"] = native_link_attributes(binary)
        retained_product = evidence / "aishe-candidate"
        shutil.copy2(binary, retained_product)
        if sha256(retained_product) != report["binary_sha256"]:
            raise RuntimeError("retained TLS product copy hash mismatch")
        report["artifact_binary_path"] = str(Path(evidence.name) / retained_product.name)
        report["binary_bytes"] = retained_product.stat().st_size
        with tempfile.TemporaryDirectory(prefix="aishe-native-tls-") as text:
            root = Path(text)
            cert_config = root / "certificate.conf"
            cert_config.write_text("""[req]
prompt = no
distinguished_name = dn
x509_extensions = extensions
[dn]
CN = localhost
[extensions]
subjectAltName = DNS:localhost,IP:127.0.0.1
basicConstraints = critical,CA:FALSE
keyUsage = critical,digitalSignature,keyEncipherment
extendedKeyUsage = serverAuth
""")
            cert = root / "server.pem"
            key = root / "server.key"
            certificate = capture([openssl, "req", "-x509", "-newkey", "rsa:2048", "-nodes",
                                   "-sha256", "-days", "1", "-config", str(cert_config),
                                   "-keyout", str(key), "-out", str(cert)], os.environ, deadline)
            report["steps"]["certificate"] = certificate
            checked(certificate, "disposable self-signed certificate")
            report["untrusted_certificate_sha256"] = sha256(cert)
            context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
            context.load_cert_chain(cert, key)
            server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
            server.socket = context.wrap_socket(server.socket, server_side=True)
            threading.Thread(target=server.serve_forever, daemon=True).start()
            redirect_server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Redirect)
            threading.Thread(target=redirect_server.serve_forever, daemon=True).start()
            env = {name: os.environ[name] for name in ("PATH", "TMPDIR", "LANG", "LC_ALL")
                   if name in os.environ}
            env.update(HOME=str(root), AISHE_CONFIG_DIR=str(root / "config"),
                       AISHE_DATA_DIR=str(root / "data"), AISHE_RUNTIME_DIR=str(root / "runtime"),
                       NO_PROXY="127.0.0.1,localhost", no_proxy="127.0.0.1,localhost",
                       AISHE_NATIVE_TLS_VALID_URL=report["public_https_url"],
                       AISHE_NATIVE_TLS_REDIRECT_URL=f"http://127.0.0.1:{redirect_server.server_port}/",
                       AISHE_NATIVE_TLS_UNTRUSTED_URL=f"https://127.0.0.1:{server.server_port}/")
            for number, test_name in enumerate(TEST_NAMES):
                test_result = capture([executable, "--exact", test_name, "--ignored", "--nocapture"],
                                      env, deadline, 35)
                test_log = evidence / f"test-{number}.log"
                test_log.write_text(test_result["stdout"] + "\n" + test_result["stderr"])
                report["steps"][test_name] = {**test_result, "log_sha256": sha256(test_log)}
                checked(test_result, "native TLS trust/activation contract")
                if "test result: ok. 1 passed; 0 failed;" not in test_result["stdout"]:
                    raise RuntimeError("native TLS test did not actually run exactly one passing test")
                records = [line.split("native startup link record: ", 1)[1]
                           for line in test_result["stdout"].splitlines()
                           if "native startup link record: " in line]
                if len(records) != 1:
                    raise RuntimeError("native TLS executable lacks one exact compiled link record")
                compiled = json.loads(records[0])
                report.setdefault("compiled_link_records", []).append(compiled)
                if compiled.get("source_commit") != report["source_commit"]:
                    raise RuntimeError("native TLS test compiled link record/source mismatch")
                for label in ("test_native_link_attributes", "product_native_link_attributes"):
                    attributes = report[label]
                    frameworks = attributes.get("frameworks", {})
                    if not attributes.get("supported") or len(frameworks) != 2 or not all(
                        value["delayed_init"] == compiled.get("delayed_frameworks")
                        for value in frameworks.values()
                    ):
                        raise RuntimeError(f"{label} does not match the tested framework mode")
                    minimum = attributes.get("minimum_macos")
                    if compiled.get("baseline_min_os") is not None and minimum != compiled["baseline_min_os"]:
                        raise RuntimeError(f"{label} changed the baseline deployment target")
                    if compiled.get("chained_fixups"):
                        headers = attributes.get("chained_fixup_headers", [])
                        formats = attributes.get("chain_pointer_formats", [])
                        if (attributes.get("cpu_type") != 0x0100000C
                                or minimum is None or minimum < (11 << 16)
                                or len(headers) != 1 or headers[0]["version"] != 0
                                or headers[0]["imports_format"] not in (1, 2, 3)
                                or headers[0]["imports_count"] >= 0xFFFF
                                or headers[0]["symbols_format"] != 0 or not formats
                                or any(value != 2 and not (minimum >= (12 << 16) and value == 6)
                                       for value in formats)):
                            raise RuntimeError(f"{label} lacks deployment-compatible chained fixups")
            report["untrusted_http_requests"] = Handler.requests
            if Handler.requests:
                raise RuntimeError("an HTTP request reached the untrusted TLS server")
            report["http_redirect_requests"] = Redirect.requests
            if Redirect.requests != 1:
                raise RuntimeError("HTTP-to-HTTPS redirect did not actually execute")
        report["status"] = "passed"
    except (AssertionError, OSError, subprocess.SubprocessError, ValueError, RuntimeError, TimeoutError) as error:
        failure = str(error)
        report["error"] = failure
    finally:
        if server:
            server.shutdown()
            server.server_close()
        if redirect_server:
            redirect_server.shutdown()
            redirect_server.server_close()
        report["finished_at"] = datetime.datetime.now(datetime.timezone.utc).isoformat()
        output.write_text(json.dumps(report, indent=2) + "\n")
    print(f"native TLS activation {report['status']}: {output}")
    if failure:
        raise SystemExit(failure)


if __name__ == "__main__":
    main()
