#!/usr/bin/env python3
"""Render the default lean prompt in real zsh, without a provider or PTY."""

import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


HOOK = Path(__file__).resolve().parents[1] / "src/lean/assets/hook.zsh"
SCRIPT = r'''
source "$1"
setopt promptsubst
aishe_set_prompt
print -P -- "LEFT=$PROMPT"
print -P -- "RIGHT=$RPROMPT"
print -r -- "MODEL=$AISHE_MODEL"
print -r -- "CONNECTION=$AISHE_CONNECTION"
print -r -- "SCOPE=$AISHE_SCOPE"
print -r -- "OUTPUT=$AISHE_AGENT_OUTPUT"
'''


@unittest.skipUnless(shutil.which("zsh"), "zsh is required")
class LeanPromptTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="aishe-prompt-")
        self.root = Path(self.temp.name)
        self.addCleanup(self.temp.cleanup)

    def render(self, extra=None, cwd=None):
        env = {k: v for k, v in os.environ.items() if not k.startswith(("AISHE_", "_AISHE_"))}
        env.update(
            HOME=str(self.root),
            AISHE_MODE="ask",
            AISHE_SCOPE="workspace",
            AISHE_UNICODE="ascii",
            AISHE_STYLE="none",
            AISHE_MODEL="grok-4.5",
            AISHE_CONNECTION="grok",
            AISHE_CONNECTION_LABEL="Grok",
            AISHE_STATUS_ITEMS="mode,model,scope,session_tokens,session_cost,requests",
            COLUMNS="120",
        )
        env.update(extra or {})
        result = subprocess.run(
            ["zsh", "-f", "-c", SCRIPT, "prompt-test", str(HOOK)],
            cwd=cwd or self.root,
            env=env,
            text=True,
            capture_output=True,
            check=True,
            timeout=5,
        )
        self.assertEqual(result.stderr, "")
        fields = dict(line.split("=", 1) for line in result.stdout.splitlines())
        self.assertNotIn("\x1b", fields["LEFT"] + fields["RIGHT"])
        return fields

    def test_mode_is_left_and_context_is_right_without_duplicate_mode(self):
        result = self.render()
        self.assertIn("ask >", result["LEFT"])
        self.assertEqual(result["RIGHT"], "grok-4.5 | Grok")
        self.assertNotIn("ask", result["RIGHT"])

    def test_grant_and_host_scope_are_visible_before_first_turn(self):
        pending = self.render({"AISHE_MODE": "agent", "AISHE_SCOPE": "host"})
        self.assertIn("agent:host [grant needed] *", pending["LEFT"])
        accepted = self.render(
            {"AISHE_MODE": "agent", "AISHE_SCOPE": "host", "_AISHE_AGENT_HOST_GRANTED": "1"}
        )
        self.assertIn("agent:host *", accepted["LEFT"])
        self.assertNotIn("grant needed", accepted["LEFT"])
        allow = self.render({"AISHE_MODE": "allow"})
        self.assertIn("allow [grant needed] >>", allow["LEFT"])

    def test_workspace_grant_does_not_look_valid_after_leaving_its_root(self):
        workspace = self.root / "workspace"
        workspace.mkdir()
        env = {
            "AISHE_MODE": "agent",
            "_AISHE_AGENT_WORKSPACE_GRANTED": "1",
            "_AISHE_AGENT_WORKSPACE_ROOT": str(workspace),
        }
        self.assertNotIn("grant needed", self.render(env, workspace)["LEFT"])
        self.assertIn("grant needed", self.render(env, self.root)["LEFT"])

    def test_narrow_terminal_drops_context_before_mode(self):
        long_path = self.root / ("a" * 100)
        long_path.mkdir()
        result = self.render(
            {"COLUMNS": "20", "AISHE_MODEL": "very-long-model-" * 10}, long_path
        )
        self.assertIn("ask >", result["LEFT"])
        self.assertLessEqual(len(result["LEFT"]), 20)
        self.assertEqual(result["RIGHT"], "")
        pending = self.render({"COLUMNS": "20", "AISHE_MODE": "agent", "AISHE_SCOPE": "host"})
        self.assertIn("agent:host? *", pending["LEFT"])
        self.assertEqual(pending["RIGHT"], "")

    def test_pending_authority_fits_24_and_32_columns(self):
        for columns in (24, 32):
            for scope in ("host", "workspace"):
                with self.subTest(columns=columns, scope=scope):
                    result = self.render(
                        {"COLUMNS": str(columns), "AISHE_MODE": "agent", "AISHE_SCOPE": scope}
                    )
                    self.assertIn(f"agent:{scope}", result["LEFT"])
                    self.assertTrue("grant" in result["LEFT"] or "?" in result["LEFT"])
                    self.assertLessEqual(len(result["LEFT"]), columns)
                    self.assertEqual(result["RIGHT"], "")

    def test_prompt_strings_are_literal_and_controls_cannot_paint(self):
        marker = self.root / "executed"
        model = f"%F{{red}}$(touch {marker})\x1b\x07"
        result = self.render({"AISHE_MODEL": model, "COLUMNS": "250"})
        self.assertIn("%F{red}$(touch", result["RIGHT"])
        self.assertFalse(marker.exists())
        self.assertNotIn("\x07", result["RIGHT"])

    def test_session_file_handoffs_update_the_actual_display(self):
        selection = self.root / "selection"
        selection.write_text("new\nNew provider\nprovider\nendpoint\nAPI key\nnew-model\nauto\nshell\n")
        scope = self.root / "scope"
        scope.write_text("host")
        output = self.root / "output"
        output.write_text("detailed")
        status = self.root / "status"
        status.write_text("session_tokens\tsession 123/45 tok\nsession_cost\tsession ~$0.0040\nrequests\t1 req\n")
        result = self.render(
            {
                "AISHE_MODE": "agent",
                "_AISHE_AGENT_HOST_GRANTED": "1",
                "AISHE_SELECTION_FILE": str(selection),
                "AISHE_SCOPE_FILE": str(scope),
                "AISHE_OUTPUT_FILE": str(output),
                "AISHE_STATUS_FILE": str(status),
            }
        )
        self.assertIn("agent:host *", result["LEFT"])
        self.assertEqual(result["MODEL"], "new-model")
        self.assertEqual(result["CONNECTION"], "new")
        self.assertEqual(result["SCOPE"], "host")
        self.assertEqual(result["OUTPUT"], "detailed")
        for value in ("new-model", "New provider", "123/45 tok", "~$0.0040", "1 req"):
            self.assertIn(value, result["RIGHT"])

    def test_recent_activity_fields_preserve_unknown_cost(self):
        status = self.root / "recent-status"
        status.write_text("task\ttask release checks\nlast_tokens\tlast 123/45 tok\n"
                          "last_cost\tlast cost n/a\n")
        result = self.render({
            "AISHE_STATUS_FILE": str(status),
            "AISHE_STATUS_ITEMS": "task,last_tokens,last_cost",
            "AISHE_MODEL": "model",
            "AISHE_CONNECTION_LABEL": "Work",
        })
        for value in ("task release checks", "last 123/45 tok", "last cost n/a"):
            self.assertIn(value, result["RIGHT"])
        self.assertNotIn("$0.0000", result["RIGHT"])

    def test_status_off_and_ascii_policy(self):
        result = self.render({"AISHE_STATUS_POSITION": "off"})
        self.assertEqual(result["RIGHT"], "")
        self.assertTrue(result["LEFT"].isascii())


if __name__ == "__main__":
    unittest.main()
