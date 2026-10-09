#!/usr/bin/env python3
"""Prove the native workspace's actual OS boundary, or report an explicit skip.

Linux CI sets AISHE_REQUIRE_FUNCTIONAL_SANDBOX=1: missing/unusable namespaces
are then a failure. Other hosts never count a capability skip as a qualified
sandbox. The loopback provider runs outside the sandbox; its tool attempts
ordinary Python writes and sockets inside the admitted workspace.
"""

from __future__ import annotations

import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import tempfile

from native_task_interactions import BINARY, Fixture, input_text, response, tool


def capability():
    if sys.platform != "linux":
        return False, "OS workspace isolation qualification requires Linux bubblewrap"
    executable = shutil.which("bwrap")
    if not executable:
        return False, "bubblewrap is missing"
    with tempfile.TemporaryDirectory(prefix="aishe-qualification-bwrap-") as directory:
        args = [executable, "--unshare-user", "--ro-bind", "/", "/", "--tmpfs", "/tmp", "--dev", "/dev",
                "--proc", "/proc", "--bind", directory, directory, "--chdir", directory,
                "--unshare-net", "--die-with-parent", "--", "/bin/sh", "-c",
                "set -eu; : > namespace-proof; test -r /etc/passwd"]
        result = subprocess.run(args, capture_output=True, text=True, timeout=10)
        if result.returncode or not (Path(directory) / "namespace-proof").exists():
            return False, "bubblewrap namespace probe failed: " + (result.stderr.strip() or str(result.returncode))
    return True, "functional Linux bubblewrap namespaces"


def actual_workspace_and_network_boundary():
    holder = {}

    def choose(index, payload):
        if index == 0:
            return response(tool("run_check", {"command": holder["command"]}, "real-sandbox-proof"))
        assert index == 1, index
        assert "exit code 0" in input_text(payload).lower(), input_text(payload)
        return response(text="Actual namespace qualification completed")

    fixture = Fixture("functional-sandbox", choose)
    outside = None
    try:
        fixture.config.write_text(fixture.config.read_text().replace('default_scope = "host"', 'default_scope = "workspace"')
                                  .replace('workspace_network = "allow"', 'workspace_network = "deny"')
                                  + "require_functional = true\n")
        secret = fixture.home / "hidden-home-proof"
        secret.write_text("PRIVATE_HOME_MUST_BE_MASKED")
        # /tmp is deliberately writable and private inside the namespace.
        # Put the outside proof on the host checkout instead, where the real
        # read-only bind must deny a write that our host user can ordinarily do.
        descriptor, filename = tempfile.mkstemp(prefix=".native-sandbox-outside-",
                                                dir=Path(__file__).resolve().parent.parent)
        os.close(descriptor)
        outside = Path(filename)
        outside.write_text("ORIGINAL_OUTSIDE_VALUE")
        # The fixture root may be in /tmp. Bind sources are opened before the
        # private /tmp mount; the workspace remains reachable, its parent does
        # not. A successful out-of-scope write must fail the check itself.
        script = "\n".join([
            "import json,pathlib,socket",
            "proof={'workspace_write':False,'outside_write_denied':False,'home_masked':False,'network_denied':False}",
            "pathlib.Path('inside-workspace-proof').write_text('inside')",
            "proof['workspace_write']=True",
            "try:",
            f" pathlib.Path({str(outside)!r}).write_text('ESCAPED')",
            "except OSError:",
            " proof['outside_write_denied']=True",
            f"proof['home_masked']=not pathlib.Path({str(secret)!r}).exists()",
            "try:",
            f" socket.create_connection(('127.0.0.1',{fixture.loopback.server.server_port}),timeout=.5).close()",
            "except OSError:",
            " proof['network_denied']=True",
            "pathlib.Path('namespace-result.json').write_text(json.dumps(proof))",
            "assert all(proof.values()),proof",
        ])
        holder["command"] = "python3 -c " + shlex.quote(script)
        task_id = fixture.start("Qualify actual namespace boundaries")
        finished = fixture.finish(task_id)
        assert finished["state"] == "completed", finished
        result = json.loads((fixture.work / "namespace-result.json").read_text())
        assert all(result.values()), result
        assert outside.read_text() == "ORIGINAL_OUTSIDE_VALUE", "tool changed a host path outside its workspace"
        assert secret.read_text() == "PRIVATE_HOME_MUST_BE_MASKED"
        checkpoint = fixture.checkpoint(finished)
        assert checkpoint["execution_scope"] == "workspace" and checkpoint["network_policy"] == "deny", checkpoint
        evidence = [row for row in checkpoint["evidence"] if row["kind"] == "check"]
        assert len(evidence) == 1 and evidence[0]["exit_code"] == 0, evidence
        fixture.loopback.assert_ok()
        print("PASS: native workspace writes remain inside the root; home is masked; actual tool sockets are denied", flush=True)
    finally:
        if outside is not None:
            outside.unlink(missing_ok=True)
        fixture.close()


def main():
    usable, reason = capability()
    if not usable:
        if os.environ.get("AISHE_REQUIRE_FUNCTIONAL_SANDBOX") == "1":
            raise SystemExit("FAIL: functional sandbox is required: " + reason)
        print("SKIP: sandbox not qualified: " + reason, flush=True)
        return
    print("  run  " + reason, flush=True)
    actual_workspace_and_network_boundary()


if __name__ == "__main__":
    main()
