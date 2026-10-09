#!/usr/bin/env python3
"""Default-shell pickers use the inner TTY and preserve selection authority."""
import os
from pathlib import Path
import re
import shutil
import tempfile
from pty_helper import Pty,environment


def identity(shell, marker):
    shell.send(f'print -r -- {marker}_\'\'VALUE=$AISHE_CONNECTION:$AISHE_MODEL\r')
    assert shell.expect(marker+'_VALUE=',3), shell.plain()[-2000:]
    shell.drain(.2)
    found=re.findall(re.escape(marker)+r'_VALUE=([^\r\n ]+)',shell.plain())
    assert found, shell.plain()[-2000:]
    return found[-1]


def run():
    home,env=environment('lean-picker',mode='ask',extra={'AISHE_LEAN':'1','NO_COLOR':'1','AISHE_UNICODE':'ascii','AISHE_FAKE_LLM':'{"type":"answer","explanation":"fixture"}'})
    env.pop('AISHE_LEGACY_OPENCODE',None)
    config=Path(home)/'.config/aishe/config.toml'
    config.write_text('''version = 7
[aishe]
mode = "ask"
connection = "work"
connection_fallback = "work"
pty_prompt = true
[connections.work]
provider = "openai"
label = "Work"
base_url = "http://127.0.0.1:18081"
model = "work-model"
auth_required = false
[connections.work.auth]
type = "none"
[connections.personal]
provider = "openai"
label = "Personal"
base_url = "http://127.0.0.1:18082"
model = "personal-model"
auth_required = false
[connections.personal.auth]
type = "none"
[backend]
engine = "native"
''')
    baseline=config.read_bytes()
    spy=Path(home)/'runtime-started';env['AISHE_SPY_OPENCODE']=str(spy)
    first=Pty(env,cols=80,rows=24);second=Pty(env,cols=80,rows=24)
    try:
        assert first.ready() and second.ready()
        first.send('/connection\r');assert first.expect('Select a connection',3), first.plain()[-6000:]
        first.drain(.2);first.send('Personal');first.drain(.2);first.send('\r')
        assert first.expect('default connection for new shells?',3)
        first.send('\r');first.drain(.3)
        assert identity(first,'PICKED')=='personal:personal-model'
        assert identity(second,'ISOLATED')=='work:work-model'
        assert config.read_bytes()==baseline,'shell pick changed durable config'
        first.send('/model\r');assert first.expect('Select a model',3)
        first.drain(.2);first.send('\x1b[B');first.drain(.2)
        assert 'cancelled' not in first.plain(),'arrow cancelled picker'
        first.send('\x1b');first.drain(.3)
        assert identity(first,'CANCELLED')=='personal:personal-model'
        first.send('/model alternate-model\r');first.drain(.3)
        first.send('/model\r');first.drain(.3);first.send('alternate');first.drain(.2);first.send('\r')
        assert first.expect('default for new shells on this connection?',3), first.plain()[-6000:]
        first.send('n\r');first.drain(.3)
        assert identity(first,'MODEL')=='personal:alternate-model'
        first.send('/connection\r');first.drain(.3);first.send('Work');first.drain(.2);first.send('\r');first.drain(.4)
        assert identity(first,'BACK')=='work:work-model'
        assert config.read_bytes()==baseline,'cancel/pick saved config without explicit yes'
        assert not spy.exists(),'local picker started OpenCode'
        assert '\x1b[38;' not in first.transcript,'NO_COLOR picker contains colors'
        assert first.transcript.isascii(),'ASCII picker contains generated Unicode'
        print('PASS: lean searchable pickers, arrows/Esc, shell handoff, concurrent isolation, defaults, no runtime')
    finally:
        first.close();second.close();shutil.rmtree(home,ignore_errors=True)


if __name__=='__main__':run()
