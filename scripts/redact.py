#!/usr/bin/env python3
import sys
from state import clean, step
for line in sys.stdin:
    text = clean(line.rstrip(), limit=None)
    if "Must be admin" in text or "not permitted" in text:
        step('debug', 'blocked', '앱 로그는 수신 중 · macOS 통합 로그는 권한 제한',
             '표준 계정에서 log stream이 거부됐습니다. 관리자 권한을 올리지 않았습니다.',
             './mac crash (크래시 회수) / ./mac logs ' + (sys.argv[1] if len(sys.argv) > 1 else 'flick'))
    print(text, flush=True)
