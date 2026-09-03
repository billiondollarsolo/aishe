#!/usr/bin/env python3
"""Portable live-process check for an unauthenticated loopback model endpoint."""

import json
import os
import shutil
import subprocess
import sys
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from harness_identity import require_current_binary

BINARY = require_current_binary(
    os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else "target/release/aishe")
)


class ModelsHandler(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path != "/v1/models":
            self.send_error(404)
            return
        body = json.dumps(
            {"data": [{"id": "local-model-b"}, {"id": "local-model-a"}]}
        ).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, _format, *_args):
        return


def main():
    root = tempfile.mkdtemp(prefix="aishe-provider-local-")
    server = ThreadingHTTPServer(("127.0.0.1", 0), ModelsHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        config_dir = os.path.join(root, "config", "aishe")
        os.makedirs(config_dir)
        with open(os.path.join(config_dir, "config.toml"), "w", encoding="utf-8") as file:
            file.write(
                "version = 2\n\n"
                "[aishe]\n"
                'provider = "openai"\n\n'
                "[providers.openai]\n"
                f'base_url = "http://127.0.0.1:{server.server_port}"\n'
                'api_key_env = "LOCAL_UNUSED_KEY"\n'
                'model = "local-model-a"\n'
                'transport = "chat"\n'
                "auth_required = false\n"
            )
        env = dict(os.environ)
        env.pop("LOCAL_UNUSED_KEY", None)
        env["AISHE_CONFIG_DIR"] = os.path.join(root, "config")
        env["AISHE_DATA_DIR"] = os.path.join(root, "data")

        # `provider test` was a duplicate spelling of `aishe test`, which nests
        # the same capability report under "provider".
        tested = subprocess.run(
            [BINARY, "test", "--json"],
            env=env,
            capture_output=True,
            text=True,
            timeout=20,
        )
        if tested.returncode != 0:
            raise AssertionError(tested.stderr or tested.stdout)
        report = json.loads(tested.stdout)["provider"]
        assert report["credential_required"] is False
        assert report["credential"]["state"] == "pass"
        assert report["model_available"]["state"] == "pass"

        listed = subprocess.run(
            [BINARY, "models", "--provider", "openai", "--json"],
            env=env,
            capture_output=True,
            text=True,
            timeout=20,
        )
        if listed.returncode != 0:
            raise AssertionError(listed.stderr or listed.stdout)
        listing = json.loads(listed.stdout)
        assert listing["schema_version"] == 1
        assert listing["models"] == ["local-model-a", "local-model-b"]
        print("PASS: unauthenticated loopback provider needs no dummy API key")
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)
        shutil.rmtree(root, ignore_errors=True)


if __name__ == "__main__":
    main()
