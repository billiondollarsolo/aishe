#!/usr/bin/env python3
"""Linux/shared-credentials contract with real local HTTP requests.

Uses synthetic keys and isolated config/data roots. Safe to run on a test node:
it never reads or writes the user's actual Aishe state.
"""

import hashlib
import json
import os
import shutil
import socket
import stat
import subprocess
import sys
import tempfile
import threading


BINARY = os.path.abspath(
    sys.argv[1] if len(sys.argv) > 1 else "target/release/aishe"
)


def run(env, *args, stdin=None):
    return subprocess.run(
        [BINARY, *args],
        input=stdin,
        text=True,
        capture_output=True,
        env=env,
        timeout=30,
        check=False,
    )


def one_request_server():
    listener = socket.socket()
    listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    listener.bind(("127.0.0.1", 0))
    listener.listen(1)
    port = listener.getsockname()[1]
    result = {}

    def serve():
        connection, _ = listener.accept()
        with connection:
            request = connection.recv(16384)
            result["request"] = request.decode("utf-8", "replace")
            body = b'{"data":[{"id":"linux-credential-model"}]}'
            connection.sendall(
                b"HTTP/1.1 200 OK\r\n"
                b"Content-Type: application/json\r\n"
                + ("Content-Length: %d\r\n" % len(body)).encode()
                + b"Connection: close\r\n\r\n"
                + body
            )
        listener.close()

    thread = threading.Thread(target=serve, daemon=True)
    thread.start()
    return "http://127.0.0.1:%d" % port, result, thread


def write_config(path, endpoint):
    text = """version = 3

[aishe]
mode = "suggest"
provider = "openai"

[providers.openai]
base_url = "%s"
credential = "openai"
api_key_env = "AISHE_LINUX_CREDENTIAL_OVERRIDE"
model = "linux-credential-model"
transport = "chat"
auth_required = true
""" % endpoint
    with open(path, "w", encoding="utf-8") as file:
        file.write(text)


def sha256(path):
    return hashlib.sha256(open(path, "rb").read()).hexdigest()


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def main():
    require(os.path.isfile(BINARY), "binary not found: " + BINARY)
    root = tempfile.mkdtemp(prefix="aishe-credentials-linux-")
    try:
        config_root = os.path.join(root, "config")
        data_root = os.path.join(root, "data")
        config_dir = os.path.join(config_root, "aishe")
        os.makedirs(config_dir)
        config_path = os.path.join(config_dir, "config.toml")
        endpoint, first_request, first_thread = one_request_server()
        write_config(config_path, endpoint)
        config_hash = sha256(config_path)
        env = dict(os.environ)
        env.update(
            {
                "HOME": root,
                "AISHE_CONFIG_DIR": config_root,
                "AISHE_DATA_DIR": data_root,
                "XDG_CONFIG_HOME": config_root,
                "XDG_DATA_HOME": data_root,
            }
        )
        env.pop("AISHE_LINUX_CREDENTIAL_OVERRIDE", None)
        stored = "synthetic-linux-stored-key"
        saved = run(env, "auth", "set", "openai", "--stdin", stdin=stored + "\n")
        require(saved.returncode == 0, saved.stderr)
        require(stored not in saved.stdout + saved.stderr, "auth set echoed the key")
        credentials = os.path.join(config_dir, "credentials.toml")
        require(stat.S_IMODE(os.stat(config_dir).st_mode) == 0o700, "config dir is not 0700")
        require(stat.S_IMODE(os.stat(credentials).st_mode) == 0o600, "credentials are not 0600")
        require(sha256(config_path) == config_hash, "auth set changed ordinary config")

        status_result = run(env, "auth", "status", "openai", "--json")
        require(status_result.returncode == 0, status_result.stderr)
        status_data = json.loads(status_result.stdout)
        require(status_data["source"]["type"] == "credentials_file", status_result.stdout)
        require(stored not in status_result.stdout + status_result.stderr, "status exposed key")

        models = run(env, "models", "--provider", "openai", "--json")
        require(models.returncode == 0, models.stderr)
        first_thread.join(timeout=5)
        require(not first_thread.is_alive(), "stored-key server did not complete")
        require(
            "Authorization: Bearer " + stored in first_request.get("request", ""),
            "provider did not use the stored key",
        )

        second_endpoint, second_request, second_thread = one_request_server()
        write_config(config_path, second_endpoint)
        file_before_override = open(credentials, "rb").read()
        overridden_env = dict(env)
        override = "synthetic-linux-environment-override"
        overridden_env["AISHE_LINUX_CREDENTIAL_OVERRIDE"] = override
        models = run(overridden_env, "models", "--provider", "openai", "--json")
        require(models.returncode == 0, models.stderr)
        second_thread.join(timeout=5)
        require(
            "Authorization: Bearer " + override in second_request.get("request", ""),
            "environment did not override the stored key",
        )
        require(
            open(credentials, "rb").read() == file_before_override,
            "environment override modified credentials",
        )

        os.chmod(credentials, 0o644)
        doctor = run(env, "doctor", "--fix", "--json")
        require(doctor.returncode == 0, doctor.stderr)
        require(stored not in doctor.stdout + doctor.stderr, "Doctor exposed key")
        require(stat.S_IMODE(os.stat(credentials).st_mode) == 0o600, "Doctor did not repair mode")
        require(
            open(credentials, "rb").read() == file_before_override,
            "Doctor changed credential contents",
        )

        removed = run(env, "auth", "remove", "openai", "--yes")
        require(removed.returncode == 0, removed.stderr)
        require(stored not in removed.stdout + removed.stderr, "remove exposed key")
        missing = run(env, "auth", "status", "openai", "--json")
        require(missing.returncode == 1, "removed profile still resolves")

        target = os.path.join(root, "symlink-target.toml")
        with open(target, "w", encoding="utf-8") as file:
            file.write('version = 1\n[profiles.openai]\napi_key = "symlink-secret"\n')
        os.chmod(target, 0o600)
        os.unlink(credentials)
        os.symlink(target, credentials)
        rejected = run(env, "auth", "status", "openai", "--json")
        require(rejected.returncode == 1, "symlinked credential file was accepted")
        require("symlink" in rejected.stderr.lower(), rejected.stderr)
        require("symlink-secret" not in rejected.stdout + rejected.stderr, "symlink key leaked")

        print(
            "PASS: Linux shared credentials, private modes, file-only auth, "
            "environment precedence, Doctor repair, removal, and symlink refusal"
        )
    finally:
        shutil.rmtree(root, ignore_errors=True)


if __name__ == "__main__":
    main()
