#!/usr/bin/env python3
"""Slash discovery uses real ZLE buffers, preserves arguments, and stays local.

Run: python3 tests/lean_slash_pty.py target/debug/aishe
The probe writes BUFFER to a private file, so input echo cannot fake success.
"""

from pathlib import Path
import shutil
import time

from pty_helper import Pty, environment


def wait_file(shell, path, timeout=4):
    end = time.monotonic() + timeout
    while not path.exists() and time.monotonic() < end:
        shell.drain(.05)
    assert path.exists(), shell.plain()[-3000:]
    return path.read_text()


def wait_new(shell, text, start, timeout=4):
    end = time.monotonic() + timeout
    while text not in shell.plain()[start:] and time.monotonic() < end:
        shell.drain(.05)
    assert text in shell.plain()[start:], shell.plain()[-4000:]


def main():
    home, env = environment("lean-slash", mode="ask", extra={
        "AISHE_LEAN": "1", "AISHE_LEGACY_OPENCODE": "0", "NO_COLOR": "1",
        "AISHE_UNICODE": "ascii", "AISHE_COMMAND_HINT_SHOWN": "1",
    })
    root = Path(home)
    buffer_file = root / "zle-buffer"
    execution = root / "custom-executed"
    mcp_started = root / "mcp-started"
    provider_started = root / "provider-started"
    env["AISHE_SPY_PROVIDER_MAKE"] = str(provider_started)
    env.pop("AISHE_FAKE_LLM_FILE", None)
    leanrc = root / "leanrc"
    leanrc.write_text(
        '_slash_buffer_probe() { print -r -- "$BUFFER" > "$HOME/zle-buffer"; }; '
        "zle -N _slash_buffer_probe; bindkey '^X^B' _slash_buffer_probe\n"
    )
    env["AISHE_LEANRC"] = str(leanrc)
    commands = root / ".config/aishe/commands"
    commands.mkdir()
    (commands / "slashproof.md").write_text(
        "---\ndescription: Write the private test marker\nshell: true\n---\n"
        f"touch {execution}\n"
    )
    config = root / ".config/aishe/config.toml"
    config.write_text(config.read_text() + '\n[mcp_servers.slash_canary]\n'
                      'command = "sh"\n'
                      'args = ["-c", "touch \\\"$HOME/mcp-started\\\"; exit 1"]\n')
    directory = root / "path-completion"
    directory.mkdir()
    (directory / "unique-proof.txt").write_text("test")
    shell = Pty(env, cols=100, rows=30)
    try:
        assert shell.ready(), "shell did not start"
        shell.drain(.2)

        def probe(keys):
            buffer_file.unlink(missing_ok=True)
            shell.send(keys)
            shell.drain(.3)
            shell.send("\x18\x02")
            value = wait_file(shell, buffer_file)
            shell.send("\x03")
            shell.drain(.15)
            return value.rstrip("\n")

        # First Tab discovers user commands without executing them or warming MCP.
        assert probe("/slap\t") == "/slap", "unknown prefix unexpectedly changed"
        value = probe("/sla\t")
        assert value.strip() == "/slashproof", (value, shell.plain()[-3000:])
        assert not execution.exists(), "completion executed a custom command"
        assert not mcp_started.exists(), "completion started MCP"
        assert not provider_started.exists(), "completion constructed a provider"

        start = len(shell.plain())
        value = probe("/\t\x1b")
        menu = shell.plain()[start:]
        assert "Commands 1/" in menu and "Search:" in menu, menu
        assert "Choose a model for this shell" in menu, menu
        assert "Tab/arrows | Enter stages | Esc" in menu, menu
        assert value == "/", "cancelling discovery changed the input"
        first = probe("/\t\r")
        assert first == "/help ", first
        down = probe("/\t\x1b[B\r")
        assert down == "/mode ", (first, down)
        back = probe("/\t\x1b[B\x1b[A\r")
        assert back == first, (first, down, back)
        assert probe("/\tAI provider\r") == "/setup ", "description search failed"
        assert probe("/\tslashproof\r") == "/slashproof ", "custom search failed"
        assert not execution.exists(), "Enter executed a picker selection"
        assert probe("/\tno-such-command\x1b") == "/", "empty search cancellation lost input"
        assert probe("/\tconnectiox\x7fn\r") == "/connection ", "search backspace failed"
        assert probe("/\t\x1b[200~connection\x1b[201~\r") == "/connection ", "paste search failed"

        # Small terminals retain a bounded, searchable list and Enter only
        # stages the choice. Prefix/argument/path completion below stays native.
        for columns in (32, 58):
            shell.resize(columns, rows=18)
            shell.drain(.3)
            assert probe("/\tbackground work\r") == "/tasks ", "narrow description search failed"
            assert not execution.exists(), "narrow discovery executed a command"
        shell.resize(100, rows=30)
        shell.drain(.3)

        value = probe("/model original-argument\t")
        assert "original-argument" in value and value.startswith("/model "), value
        # Move the cursor into the command head: replacing it must preserve the tail.
        value = probe("/conn original-argument\x01" + "\x1b[C" * 5 + "\t")
        assert value.startswith("/connection ") and "original-argument" in value, value
        value = probe(str(directory / "unique-") + "\t")
        assert "unique-proof.txt" in value, value
        value = probe("/tm\t")
        assert value.strip() == "/tmp/", value
        value = probe("/help conn\t")
        assert value.strip() == "/help connection", value
        value = probe("/mode as\t")
        assert value.strip() == "/mode ask", value

        start = len(shell.plain())
        shell.send("/\r")
        wait_new(shell, "AIShe · quick guide", start)
        start = len(shell.plain())
        shell.send("/help mode\r")
        wait_new(shell, "/mode ask|allow|agent|agent-host", start)
        start = len(shell.plain())
        shell.send("/commands\r")
        wait_new(shell, "Browse all slash commands", start)
        start = len(shell.plain())
        shell.send("/conection\r")
        wait_new(shell, "Try /connection", start)
        shell.send("/sla\t\r")
        wait_file(shell, execution)
        shell.send("print -r -- SLASH_''MODE=$AISHE_MODE\r")
        assert shell.expect("SLASH_MODE=ask", 4), "discovery changed authority"
        assert not mcp_started.exists(), "local slashes started MCP"
        assert not provider_started.exists(), "local slashes constructed a provider"
        print("PASS: bounded searchable slash picker, staging/cancel, narrow widths, custom discovery, native completion, help, paths, and local execution")
    finally:
        shell.close()
        shutil.rmtree(home, ignore_errors=True)


if __name__ == "__main__":
    main()
