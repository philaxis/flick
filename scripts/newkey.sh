#!/bin/bash
# One-day, one-use invitation. Secret values only go to private files.
set -euo pipefail
umask 077
ROOT=$(cd "$(dirname "$0")/.." && pwd)
S=${MAC_STATE:-$HOME/.local/state/mac-borrow}
mkdir -p "$S"; chmod 700 "$S"
exec 9>"$S/newkey.lock"; flock -n 9 || { echo '열쇠를 이미 만드는 중입니다.'; exit 1; }
[ ! -s "$S/host" ] || { echo '연결 중인 맥이 있습니다. 열쇠를 지우지 않도록 newkey를 멈춥니다.'; exit 1; }
trap 'rm -f "$S/ts.key"' EXIT
# Aside is shared company-wide. The lock is the existing, required lock.
flock /tmp/aside.lock "$S/mkkey.sh" "borrowed-mac $(date +%m%d-%H%M)" "$S/ts.key"
"$ROOT/mac" keygen >/dev/null
"$ROOT/mac" link "$S/ts.key" >/dev/null
rm -f "$S/day.json"
python3 "$ROOT/scripts/state.py" reset
python3 - "$S" <<'PY'
import datetime, json, os, pathlib, sys, time
s = pathlib.Path(sys.argv[1]); now = int(time.time())
data = {'createdAt': now, 'expiresAt': now + 86400, 'consumed': False}
tmp = s / 'invitation.json.new'; tmp.write_text(json.dumps(data)); tmp.chmod(0o600)
os.replace(tmp, s / 'invitation.json')
fmt = lambda t: datetime.datetime.fromtimestamp(t, datetime.timezone(datetime.timedelta(hours=9))).isoformat()
print('만듦: ' + fmt(now) + ' · 만료: ' + fmt(now + 86400))
site = (s / 'site').read_text().strip() if (s / 'site').exists() else '(담당이 상태 폴더의 site에 우리 화면 주소를 넣습니다)'
print('친구에게 줄 한 줄: ' + site + '#invite')
PY
