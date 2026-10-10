#!/usr/bin/env python3
"""Private, atomic session state; the phone only renders this server's result."""
import fcntl
import json
import os
from pathlib import Path
import re
import sys
import time

STATE = Path(os.environ.get('MAC_STATE', Path.home() / '.local/state/mac-borrow'))


def clean(text, limit=300):
    text = re.sub(r'(?:tskey-[\w-]+|gh[pousr]_[\w]+|github_pat_[\w]+|sk-[\w-]+)', '[가림]', str(text))
    text = re.sub(r'AIza[\w-]{25,}', '[가림]', text)
    text = re.sub(r'(?i)(bearer\s+|(?:password|token|api[_-]?key|secret)\s*[:=]\s*)\S+', r'\1[가림]', text)
    text = re.sub(r'(?i)(["\'](?:password|token|api[_-]?key|secret)["\']\s*:\s*["\'])[^"\']*', r'\1[가림]', text)
    return text[-limit:] if limit else text


def read():
    try:
        return json.loads((STATE / 'status.json').read_text())
    except (FileNotFoundError, ValueError):
        return {'steps': {}, 'remoteJobs': {}, 'connected': False}


def update(fn):
    STATE.mkdir(parents=True, exist_ok=True, mode=0o700)
    with (STATE / 'status.lock').open('a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        data = read(); fn(data); data['updatedAt'] = int(time.time())
        tmp = STATE / 'status.json.new'
        tmp.write_text(json.dumps(data, ensure_ascii=False)); tmp.chmod(0o600)
        os.replace(tmp, STATE / 'status.json')


def step(name, status, line='', reason='', command=''):
    def change(d):
        old = d.setdefault('steps', {}).get(name, {})
        d['steps'][name] = {'status': status, 'line': clean(line), 'reason': clean(reason),
                            'command': clean(command or old.get('command', '')), 'at': int(time.time())}
        if status == 'running':
            d['current'] = name
    update(change)


if __name__ == '__main__':
    action, *args = sys.argv[1:]
    if action == 'step':
        step(*args)
    elif action == 'command':
        name, phase = args
        def change(d):
            d['lastCommand'] = {'name': name, 'phase': phase, 'at': int(time.time())}
        update(change)
    elif action == 'connection':
        def change(d):
            d['connected'] = args[0] == 'yes'; d['checkedAt'] = int(time.time())
        update(change)
    elif action == 'job':
        def change(d):
            d.setdefault('remoteJobs', {})[args[0]] = {'startedAt': int(time.time())}
        update(change)
    elif action == 'prompt':
        def change(d):
            d['friend'] = clean(args[0]) if args else ''
        update(change)
    elif action == 'reset':
        update(lambda d: (d.clear(), d.update(steps={}, remoteJobs={}, connected=False)))
