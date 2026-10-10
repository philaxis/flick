#!/usr/bin/env python3
"""Tailnet-only status API and single-use invitation. No request/body logging."""
import fcntl
import gzip
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import subprocess
import threading
import time
from urllib.parse import urlparse
from state import STATE, read

ROOT = Path(__file__).resolve().parent.parent
PREFIX = '/canvas-proto/mac-borrow/api'
ORIGIN = os.environ.get('MAC_ORIGIN') or ('https://' + urlparse((STATE / 'site').read_text().strip()).netloc)


def invitation():
    try:
        d = json.loads((STATE / 'invitation.json').read_text())
        return {k: d[k] for k in ('createdAt', 'expiresAt', 'consumed')}
    except (FileNotFoundError, ValueError, KeyError):
        return None


def consume():
    with (STATE / 'newkey.lock').open('a') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        d = invitation()
        if not d or d['expiresAt'] <= time.time() or d['consumed']:
            return None
        link = (STATE / 'link').read_text()
        d['consumed'] = True
        tmp = STATE / 'invitation.json.new'; tmp.write_text(json.dumps(d)); tmp.chmod(0o600)
        os.replace(tmp, STATE / 'invitation.json')
        return link


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def reply(self, code, data):
        body = json.dumps(data, ensure_ascii=False).encode()
        compressed = 'gzip' in self.headers.get('Accept-Encoding', '')
        if compressed:
            body = gzip.compress(body)
        self.send_response(code)
        self.send_header('Content-Type', 'application/json; charset=utf-8')
        self.send_header('Cache-Control', 'no-store')
        self.send_header('Referrer-Policy', 'no-referrer')
        self.send_header('X-Content-Type-Options', 'nosniff')
        self.send_header('Vary', 'Accept-Encoding')
        if compressed:
            self.send_header('Content-Encoding', 'gzip')
        self.send_header('Content-Length', str(len(body))); self.end_headers(); self.wfile.write(body)

    def do_GET(self):
        path = urlparse(self.path).path
        # A proxy with --set-path may strip its prefix.
        if path in (PREFIX + '/status', '/status'):
            d = read(); d['invitation'] = invitation(); d['serverAt'] = int(time.time())
            self.reply(200, d)
        elif path in (PREFIX + '/health', '/health'):
            self.reply(200, {'ok': True})
        else:
            self.reply(404, {'error': '없는 주소'})

    def do_POST(self):
        if self.headers.get('Origin') != ORIGIN or self.headers.get('Content-Type') != 'application/json':
            self.reply(403, {'error': '이 화면에서만 열 수 있습니다.'}); return
        if urlparse(self.path).path not in (PREFIX + '/invite', '/invite'):
            self.reply(404, {'error': '없는 주소'}); return
        if self.headers.get('Content-Length', '0') not in ('0', '2'):
            self.reply(400, {'error': '잘못된 요청'}); return
        try:
            link = consume()
            self.reply(200 if link else 410, {'line': link} if link else {'error': '이미 열었거나 만료됐습니다. 담당이 ./mac newkey로 다시 만듭니다.'})
        except BlockingIOError:
            self.reply(409, {'error': '열쇠를 만드는 중입니다. 잠시 뒤 여세요.'})
        except (OSError, ValueError):
            self.reply(503, {'error': '초대 준비 중입니다.'})


def sync_loop():
    while True:
        if (STATE / 'host').exists() and (STATE / 'user').exists():
            try:
                subprocess.run([str(ROOT / 'mac'), 'sync'], stdout=subprocess.DEVNULL,
                               stderr=subprocess.DEVNULL, timeout=25)
            except (OSError, subprocess.TimeoutExpired):
                pass
        time.sleep(5)


if __name__ == '__main__':
    STATE.mkdir(parents=True, exist_ok=True, mode=0o700)
    threading.Thread(target=sync_loop, daemon=True).start()
    ThreadingHTTPServer(('127.0.0.1', int(os.environ.get('MAC_API_PORT', '47941'))), Handler).serve_forever()
