#!/usr/bin/env python3
"""Native zsh profiles preserve customization without selecting OpenCode.

Exercise real keymap bindings and buffer effects, not echoed key sequences.
Run: python3 tests/native_zsh_profile_pty.py target/debug/aishe
"""

from pathlib import Path
import os
import shutil
import subprocess
import tempfile
import time

from pty_helper import Pty, binary, environment


RC = r'''
eval "$(command aishe init zsh)"
unset HISTFILE
HISTSIZE=777
SAVEHIST=0
unsetopt SHARE_HISTORY
PROMPT='PERSONAL> '
RPROMPT='personal-right'
alias profile_alias='print -r -- ALIAS_WORKS'
profile_function() { print -r -- FUNCTION_WORKS; }
_profile_enter() {
  print -r -- "enter:$KEYMAP:$WIDGET" >> "$HOME/widget-trace"
  zle accept-line
}
_profile_tab_emacs() { [[ "$WIDGET" == _profile_tab_emacs ]] || return 1; BUFFER+='EMACS_TAB'; CURSOR=${#BUFFER}; }
_profile_tab_viins() { [[ "$WIDGET" == _profile_tab_viins ]] || return 1; BUFFER+='VI_TAB'; CURSOR=${#BUFFER}; }
_profile_details() { print -r -- details >> "$HOME/widget-trace"; }
_profile_probe() { print -r -- "$BUFFER" > "$HOME/buffer-proof"; }
zle -N _profile_enter
zle -N _profile_tab_emacs
zle -N _profile_tab_viins
zle -N _profile_details
zle -N _profile_probe
bindkey -e
bindkey -M emacs '^M' _profile_enter
bindkey -M viins '^M' _profile_enter
bindkey -M emacs '^I' _profile_tab_emacs
bindkey -M viins '^I' _profile_tab_viins
bindkey -M emacs '^O' _profile_details
bindkey -M viins '^O' _profile_details
bindkey -M emacs '^X^B' _profile_probe
bindkey -M viins '^X^B' _profile_probe
'''


def wait_file(shell, path, timeout=4):
    end = time.monotonic() + timeout
    while not path.exists() and time.monotonic() < end:
        shell.drain(.05)
    assert path.exists(), shell.plain()[-4000:]
    return path.read_text()


def effects(shell, keys, root):
    path = root / 'buffer-proof'
    path.unlink(missing_ok=True)
    shell.send(keys)
    shell.drain(.15)
    shell.send('\x18\x02')
    text = wait_file(shell, path).rstrip('\n')
    shell.send('\x03')
    shell.drain(.15)
    return text


def producer_contract():
    """Verify the actual zsh writer, including hostile framing and byte caps."""
    hook = Path(__file__).resolve().parent.parent / 'src/lean/assets/hook.zsh'
    with tempfile.TemporaryDirectory(prefix='aishe-state-producer-') as directory:
        root = Path(directory)
        state = root / 'state'
        env = dict(os.environ, AISHE_EXECUTION_STATE_FILE=str(state),
                   AISHE_LEANRC='', AISHE_LEANRC_POST='',
                   AISHE_EXECUTION_STATE_DENY='PRIVATE_PROVIDER_AUTH',
                   PRIVATE_PROVIDER_AUTH='private-canary', WEIRD_TOKEN='token-canary')

        def produce(code):
            return subprocess.run(['zsh', '-f', '-c',
                                   'source "$1"; ' + code + '; _aishe_capture_execution_state',
                                   '--', str(hook)], env=env, capture_output=True, timeout=5)

        result = produce('export name=human-name value=human-value TEST_UNICODE="😀é" '
                         'TEST_MULTILINE=$\'alpha\\nbeta\'; '
                         'export AISHE_EXECUTION_STATE_FILE="$HOME/incorrect-state-path"; '
                         'export AISHE_EXECUTION_STATE_DENY=""')
        assert result.returncode == 0, result.stderr.decode()
        parts = state.read_bytes().split(b'\0')
        assert parts[0] == b'AISHE_ENV_V1' and parts[-1] == b''
        values = dict(zip(parts[1:-1:2], parts[2:-1:2]))
        assert values[b'name'] == b'human-name' and values[b'value'] == b'human-value'
        assert values[b'TEST_UNICODE'] == '😀é'.encode() and values[b'TEST_MULTILINE'] == b'alpha\nbeta'
        assert b'PRIVATE_PROVIDER_AUTH' not in values and b'WEIRD_TOKEN' not in values
        assert state.stat().st_mode & 0o777 == 0o600
        state.unlink()
        result = produce('export BAD_NUL=$\'x\\0INJECTED\\0value\'')
        assert result.returncode != 0 and not state.exists(), 'NUL injection accepted'
        result = produce('export TOO_LARGE="${(l:20000::😀:)empty}"')
        assert result.returncode != 0 and not state.exists(), 'UTF8 value byte cap bypassed'
        assert not list(root.glob('state.tmp.*')), 'failed snapshot left a partial file'


def login_profiles():
    """Personal login stages run in order; clean login keeps them isolated."""
    home, env = environment('native-login-profile', mode='ask', extra={
        'AISHE_LEAN': '1', 'NO_COLOR': '1', 'AISHE_UNICODE': 'ascii',
        'AISHE_COMMAND_HINT_SHOWN': '1', 'AISHE_ZSH_PROFILE': 'personal',
    })
    root = Path(home)
    personal = root / 'personal-login'
    personal.mkdir()
    trace = root / 'login-trace'
    (root / '.zshenv').write_text('export PROFILE_ORDER=env\n'
                                 'print -r -- env >> "$HOME/login-trace"\n'
                                 'ZDOTDIR="$HOME/personal-login"\n')
    for filename, stage in (('.zprofile', 'profile'), ('.zshrc', 'rc'), ('.zlogin', 'login')):
        (personal / filename).write_text('export PROFILE_ORDER="${PROFILE_ORDER}:' + stage + '"\n'
                                          'print -r -- ' + stage + ' >> "$HOME/login-trace"\n'
                                          + ('PROMPT="LOGIN> "\n' if stage == 'rc' else ''))
    (personal / '.zlogout').write_text('print -r -- logout >> "$HOME/login-trace"\n')
    shell = None
    try:
        shell = Pty(env, argv=[binary(), '-l'])
        assert shell.ready(), shell.plain()[-4000:]
        shell.send('print -r -- LOGIN_\'\'ORDER=$PROFILE_ORDER ZDOTDIR_\'\'FINAL=$ZDOTDIR\r')
        assert shell.expect('LOGIN_ORDER=env:profile:rc:login', 4), shell.plain()[-4000:]
        assert shell.expect('ZDOTDIR_FINAL=' + str(personal), 4), '.zlogin did not restore the real directory'
        shell.send('exit\r')
        shell.proc.wait(timeout=5)
        assert trace.read_text().splitlines() == ['env', 'profile', 'rc', 'login', 'logout']
        shell.close()
        shell = None
        trace.unlink()
        env['AISHE_ZSH_PROFILE'] = 'clean'
        shell = Pty(env, argv=[binary(), '-l'])
        assert shell.ready(), shell.plain()[-4000:]
        shell.send('exit\r')
        shell.proc.wait(timeout=5)
        assert not trace.exists(), 'clean login loaded personal startup or cleanup'
    finally:
        if shell is not None:
            shell.close()
        shutil.rmtree(home, ignore_errors=True)


def run():
    producer_contract()
    login_profiles()
    home, env = environment('native-profile', zshrc=RC, mode='ask', extra={
        'AISHE_LEAN': '1', 'AISHE_LEGACY_OPENCODE': '0',
        'AISHE_ZSH_PROFILE': 'personal', 'NO_COLOR': '1',
        'AISHE_FAKE_LLM': '{"type":"answer","explanation":"NATIVE_PROFILE_ANSWER"}',
    })
    root = Path(home)
    env['XDG_CACHE_HOME'] = str(root / '.cache')
    runtime_spy = root / 'runtime-started'
    provider_spy = root / 'provider-made'
    env['AISHE_SPY_OPENCODE'] = str(runtime_spy)
    env['AISHE_SPY_PROVIDER_MAKE'] = str(provider_spy)
    personal = root / 'personal-config'
    personal.mkdir()
    (personal / '.zshrc').write_text(RC)
    (root / '.zshrc').write_text('touch "$HOME/wrong-user-rc"\n')
    (root / '.zshenv').write_text('export PROFILE_ENV=personal-environment\n'
                                 'ZDOTDIR="$HOME/personal-config"\n')
    shell = None
    try:
        shell = Pty(env)
        assert shell.ready(), shell.plain()[-4000:]
        assert 'PERSONAL> ' in shell.plain(), 'personal prompt replaced'
        assert not (root / 'wrong-user-rc').exists(), '.zshenv ZDOTDIR change was ignored'
        assert '[grant needed]' not in shell.plain(), 'AIShe prompt replaced theme'
        trace = root / 'widget-trace'
        assert 'enter:' in wait_file(shell, trace), 'custom Enter widget was bypassed'
        assert ':_profile_enter' in trace.read_text(), 'custom Enter widget context changed'
        shell.send('profile_alias\rprofile_function\r')
        assert shell.expect('ALIAS_WORKS', 4) and shell.expect('FUNCTION_WORKS', 4)
        shell.send('print -r -- ENV_\'\'PROOF=$PROFILE_ENV BACKEND_\'\'PROOF=$AISHE_BACKEND\r')
        assert shell.expect('ENV_PROOF=personal-environment BACKEND_PROOF=native', 4)
        shell.send('print -r -- HOOK_\'\'PROOF=$+functions[aishe_precmd]\r')
        assert shell.expect('HOOK_PROOF=0', 4), 'existing init line installed a second agent hook'
        shell.send('print -r -- HISTORY_\'\'PROOF=$+HISTFILE:$HISTSIZE:$SAVEHIST:$options[sharehistory]\r')
        assert shell.expect('HISTORY_PROOF=0:777:0:off', 4), 'personal history settings were replaced'
        shell.send('print -r -- ARRAY_\'\'SETUP; matrix_arr=(a b)\r')
        assert shell.expect('ARRAY_SETUP', 4)
        shell.send('print -r -- ARRAY_\'\'PROOF=$matrix_arr[2]\r')
        assert shell.expect('ARRAY_PROOF=b', 4), 'compound array assignment routed to the agent'
        assert effects(shell, 'echo \t', root) == 'echo EMACS_TAB', 'custom emacs Tab lost'
        shell.send('bindkey -v\r')
        shell.drain(.2)
        assert effects(shell, 'echo \t', root) == 'echo VI_TAB', 'custom vi Tab lost'
        shell.send('bindkey -e\r')
        shell.drain(.2)
        shell.send('\x0f')
        shell.drain(.2)
        assert 'details' in trace.read_text(), 'personal Ctrl-O was stolen'
        assert not runtime_spy.exists() and not provider_spy.exists(), 'customization started AI'
        value = effects(shell, '/stat\t', root)
        assert value.strip() == '/status', (value, shell.plain()[-4000:])
        shell.send('? demonstrate native profile\r')
        assert shell.expect('NATIVE_PROFILE_ANSWER', 4), shell.plain()[-4000:]
        assert not runtime_spy.exists(), 'personal zsh selected OpenCode'
        shell.close()
        shell = None

        # Plugins can wrap the global accept-line widget instead of binding
        # Enter to a separate widget. Preserve WIDGET-dependent dispatch there.
        (personal / '.zshrc').write_text(RC + r'''
_profile_global_enter() {
  print -r -- "global:$WIDGET" >> "$HOME/widget-trace"
  zle .accept-line
}
zle -N accept-line _profile_global_enter
bindkey -M emacs '^M' accept-line
''')
        shell = Pty(env)
        assert shell.ready()
        assert 'global:accept-line' in trace.read_text(), 'global Enter plugin context changed'
        shell.send('? demonstrate native global widget\r')
        assert shell.expect('NATIVE_PROFILE_ANSWER', 4), 'global Enter plugin displaced native routing'
        shell.close()
        shell = None
        (personal / '.zshrc').write_text(RC)

        # Optional authority indicator extends the theme and stays singular.
        env['AISHE_PERSONAL_INDICATOR'] = '1'
        shell = Pty(env)
        assert shell.ready()
        shell.send('print -r -- PROMPT_\'\'PROOF=$PROMPT; print -r -- RIGHT_\'\'PROOF=$RPROMPT; print -r -- MODE_\'\'PROOF=$AISHE_MODE_INDICATOR\r')
        assert shell.expect('PROMPT_PROOF=PERSONAL> ', 4)
        assert shell.expect('RIGHT_PROOF=personal-right', 4)
        assert shell.expect('MODE_PROOF=ask', 4)
        shell.close()
        shell = None

        # Existing early leanrc widgets are chained; late leanrc can override.
        env['AISHE_ZSH_PROFILE'] = 'clean'
        env.pop('AISHE_PERSONAL_INDICATOR', None)
        env.pop('PROFILE_ENV', None)
        completions = root / 'custom-completions'
        completions.mkdir()
        early = root / 'leanrc'
        early.write_text(RC + '\nexport EARLY_PROFILE_VALUE=early\n'
                         'fpath=("$HOME/custom-completions" $fpath)\n')
        late = root / 'leanrc.post'
        late.write_text(r'''
export LATE_PROFILE_VALUE=late
_profile_late_details() { print -r -- late-details >> "$HOME/widget-trace"; }
zle -N _profile_late_details
bindkey -M emacs '^O' _profile_late_details
''')
        env['AISHE_LEANRC'] = str(early)
        env['AISHE_LEANRC_POST'] = str(late)
        shell = Pty(env)
        assert shell.ready()
        shell.send('print -r -- PHASE_\'\'PROOF=$EARLY_PROFILE_VALUE:$LATE_PROFILE_VALUE:$PROFILE_ENV\r')
        assert shell.expect('PHASE_PROOF=early:late:', 4), shell.plain()[-4000:]
        shell.send('\x0f')
        shell.drain(.2)
        assert 'late-details' in trace.read_text(), 'late customization was overwritten'
        assert effects(shell, 'echo \t', root) == 'echo EMACS_TAB'
        shell.close()
        shell = None
        dumps = list((root / '.cache/aishe/zsh').glob('.zcompdump-*'))
        assert dumps, 'completion dump was deleted with the temporary ZDOTDIR'
        previous = {path.name: path.stat().st_mtime_ns for path in dumps}
        shell = Pty(env)
        assert shell.ready()
        shell.close()
        shell = None
        assert all(path.exists() and path.stat().st_mtime_ns == previous[path.name] for path in dumps)

        # Installing a completion into an existing fpath directory must not
        # reuse a stale dump whose only key was the directory's pathname.
        (completions / '_profile_cache_proof').write_text('#compdef profile-cache-proof\n'
                                                         '_arguments "1:proof:()"\n')
        stamp = completions.stat()
        os.utime(completions, ns=(stamp.st_atime_ns, stamp.st_mtime_ns + 2_000_000_000))
        shell = Pty(env)
        assert shell.ready()
        shell.send('print -r -- CACHE_\'\'PROOF=$_comps[profile-cache-proof]\r')
        assert shell.expect('CACHE_PROOF=_profile_cache_proof', 4), 'completion cache hid newly installed function'
        shell.close()
        shell = None

        env['AISHE_ZSH_PROFILE'] = 'personal; touch should-not-exist'
        result = subprocess.run([binary(), 'zsh'], env=env, capture_output=True, text=True, timeout=5)
        assert result.returncode != 0 and 'invalid AISHE_ZSH_PROFILE' in result.stderr
        assert not runtime_spy.exists()
        print('PASS: native personal zsh, aliases/functions/environment, prompt, Enter/Tab/keymaps, Ctrl-O, rc phases, cache, invalid selector')
    finally:
        if shell is not None:
            shell.close()
        shutil.rmtree(home, ignore_errors=True)


if __name__ == '__main__':
    run()
