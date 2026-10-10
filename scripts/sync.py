#!/usr/bin/env python3
import pathlib
import sys
import time
from state import clean, update

def change(d):
    d['connected'] = True; d['checkedAt'] = int(time.time())
    for line in pathlib.Path(sys.argv[1]).read_text().splitlines():
        row = line.split('\t', 2)
        if len(row) < 2:
            continue
        kind, app = row[:2]
        name = 'mulbit-build' if app == 'mulbit' else 'flick-permissions'
        step = d.setdefault('steps', {}).setdefault(name, {})
        step['at'] = int(time.time())
        if kind == 'EXIT':
            step['status'] = 'done' if row[2] == '0' else 'failed'
            step['reason'] = '' if row[2] == '0' else '종료 코드 ' + clean(row[2]) + '. 권한·앱 로그를 확인하세요.'
            step['command'] = './mac logs ' + app
            d.setdefault('remoteJobs', {}).pop(app, None)
            if app == 'flick':
                d['steps']['flick-run'] = dict(step)
        elif kind == 'RUN':
            step['status'] = 'running'
        elif kind == 'LOG':
            step['line'] = clean(row[2])
update(change)
