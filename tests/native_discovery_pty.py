#!/usr/bin/env python3
"""Persist a launch hint only after native zsh actually presented it."""

import json
from pathlib import Path
import shutil
import time

from pty_helper import Pty, environment


HINT = '/help | ? ask | Shift-Tab mode'


def seen(root):
    path = root / '.local/share/aishe/discovery-hints.json'
    return path.exists() and json.loads(path.read_text()).get('launch_hint_seen', False)


def wait_seen(shell, root):
    end = time.monotonic() + 3
    while not seen(root) and time.monotonic() < end:
        shell.drain(.05)
    assert seen(root), 'displayed hint was not acknowledged'


def main():
    for profile in ('clean', 'personal'):
        home, env = environment('native-discovery-' + profile, mode='ask', extra={
            'AISHE_ZSH_PROFILE': profile, 'AISHE_LEAN': '1',
            'NO_COLOR': '1', 'AISHE_UNICODE': 'ascii',
        }, config_extra='discovery_hints = true')
        env.pop('AISHE_COMMAND_HINT_SHOWN', None)
        for key in ('AISHE_FAKE_LLM', 'AISHE_FAKE_LLM_FILE', 'UNUSED_FAKE_KEY'):
            env.pop(key, None)
        root = Path(home)
        shell = None
        try:
            shell = Pty(env, cols=32, rows=18)
            assert shell.ready(), shell.plain()[-4000:]
            assert HINT in shell.plain(), 'first shell did not show controls'
            wait_seen(shell, root)
            if profile == 'clean':
                shell.send('? where do I connect?\r')
                assert shell.expect('AI is not connected.', 4), shell.plain()[-4000:]
                assert 'Run /setup to connect an account' in shell.plain()
                assert 'AISHE_FAKE_LLM' not in shell.plain(), 'test controls leaked into product guidance'
                shell.send("print -r -- SHELL_''AFTER_REQUEST\r")
                assert shell.expect('SHELL_AFTER_REQUEST', 4), 'unconnected AI request broke shell commands'
            shell.close()
            shell = Pty(env, cols=32, rows=18)
            assert shell.ready(), shell.plain()[-4000:]
            assert HINT not in shell.plain(), 'seen hint recurred'
        finally:
            if shell is not None:
                shell.close()
            shutil.rmtree(home, ignore_errors=True)

    home, env = environment('native-discovery-disabled', mode='ask', extra={
        'AISHE_LEAN': '1', 'NO_COLOR': '1', 'AISHE_UNICODE': 'ascii',
    }, config_extra='discovery_hints = false')
    env.pop('AISHE_COMMAND_HINT_SHOWN', None)
    root = Path(home)
    shell = Pty(env, cols=32, rows=18)
    try:
        assert shell.ready(), shell.plain()[-4000:]
        assert HINT not in shell.plain(), 'disabled hint was shown'
        assert not seen(root), 'suppressed hint was marked seen'
    finally:
        shell.close()
        shutil.rmtree(home, ignore_errors=True)

    # A valid child spawn is insufficient evidence of presentation. Personal
    # startup can terminate before AIShe installs or renders its shared hook.
    home, env = environment('native-discovery-aborted', zshrc='exit 0\n', mode='ask', extra={
        'AISHE_ZSH_PROFILE': 'personal', 'AISHE_LEAN': '1',
        'NO_COLOR': '1', 'AISHE_UNICODE': 'ascii',
    }, config_extra='discovery_hints = true')
    env.pop('AISHE_COMMAND_HINT_SHOWN', None)
    root = Path(home)
    shell = Pty(env, cols=32, rows=18)
    try:
        shell.drain(.5)
        assert HINT not in shell.plain(), 'startup fixture unexpectedly rendered a hint'
        assert not seen(root), 'aborted startup consumed the unseen hint'
        shell.close()
        (root / '.zshrc').write_text('PROMPT="PERSONAL> "\n')
        shell = Pty(env, cols=32, rows=18)
        assert shell.ready(), shell.plain()[-4000:]
        assert HINT in shell.plain(), 'next working shell lost the launch hint'
        wait_seen(shell, root)
    finally:
        shell.close()
        shutil.rmtree(home, ignore_errors=True)
    print('PASS: native launch hints, narrow width, persistent once-state, disabled/aborted presentation')


if __name__ == '__main__':
    main()
