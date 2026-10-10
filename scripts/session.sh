# Sourced by mac. Uses the existing SSH session and standard macOS tools.
valid_name() { [[ "$1" =~ ^[a-zA-Z0-9_-]+$ ]] || { echo '잘못된 이름'; return 1; }; }
q() { printf '%q' "$1"; }

ensure_probe() {
  rsh "mkdir -p $R/probes $R/log $R/out"
  # Transport our public probe only; no GitHub credentials or private sources.
  sshx "cat > $R/probes/flick-probe.swift" < "$ROOT/probes/flick-probe.swift"
  rsh "set -e; cd $R/probes
    if [ ! -x flick-probe ] || ! cmp -s flick-probe.swift built.swift; then
      swiftc -O flick-probe.swift -o flick-probe > $R/log/flick-build.log 2>&1
      codesign --force -s - flick-probe >> $R/log/flick-build.log 2>&1
      cp flick-probe.swift built.swift
    fi"
}

doctor() {
  printf '항목\t상태\t다음 명령\n'
  if ! sshx true 2>/dev/null; then
    st connection no; printf '연결\t확인 못 함\t./mac wait borrow\n'; return 1
  fi
  st connection yes
  printf '연결\t접속됨 · tcp %s\t—\n' "$PORT"
  rsh 'printf "macOS\t%s\t—\n칩\t%s\t—\n" "$(sw_vers -productVersion)" "$(uname -m)"
    if id -Gn | tr " " "\n" | grep -qx admin; then printf "계정\t관리자(멈춤)\t친구: 표준 borrow 계정으로 다시 연결\n"; exit 3; fi
    printf "계정\t표준\t—\n"
    if xcode-select -p >/dev/null 2>&1; then printf "개발 도구\t설치됨\t—\n"; else printf "개발 도구\t없음\t./mac clt\n"; fi
    if launchctl print gui/$(id -u) >/dev/null 2>&1; then printf "화면 세션\t있음\t—\n"; else printf "화면 세션\t없음\t친구: borrow 계정에 화면 로그인\n"; fi
    printf "LLDB\t%s\t./mac lldb mulbit\n" "$(xcrun --find lldb 2>/dev/null || echo 없음)"'
  if rsh 'xcrun --find swiftc >/dev/null 2>&1'; then
    ensure_probe
    rsh "$R/probes/flick-probe permissions"
  else
    printf '권한\t확인 못 함(컴파일러 없음)\t./mac clt\n'
  fi
  printf '앱별 권한\t물빛 앱·Terminal에서 별도 확인 필요\t./mac preview ; ./mac flick-check\n'
  if ! rsh 'xcode-select -p >/dev/null 2>&1'; then
    st prompt '친구: 맥 화면의 개발자 도구 설치 창에서 설치를 눌러 주세요.'; return 2
  fi
  if ! rsh 'launchctl print gui/$(id -u) >/dev/null 2>&1'; then
    st prompt '친구: borrow 계정으로 화면에 로그인해 주세요. 앱 열기·권한·화면 캡처는 아직 확인 못 했습니다.'
    [ "${MAC_HEADLESS:-0}" = 1 ] || return 2
  fi
}

build_flick() {
  st step flick-build running '공개 시험 소스 보내기 → Swift 빌드 → 결과 회수'
  ensure_probe
  mkdir -p "$S/artifacts"
  sshx "cat $R/probes/flick-probe" > "$S/artifacts/flick-probe"
  chmod 700 "$S/artifacts/flick-probe"
  rsh 'echo "Flick 앱: 미구현. 현재 결과물은 macOS 기능 시험 도구입니다."'
  st step flick-build done 'macOS 시험 도구 빌드·회수 완료. Flick 앱은 아직 미구현.'
  DEFERRED=1
}

run_flick() {
  case "$1" in permissions|move|tap|shot|all) ;; *) echo 'permissions|move|tap|shot|all'; return 1 ;; esac
  ensure_probe
  if [ "$1" = permissions ]; then
    rsh "$R/probes/flick-probe permissions"; return
  fi
  if ! rsh 'launchctl print gui/$(id -u) >/dev/null 2>&1'; then
    st step flick-run blocked '실제 맥에서 확인 못 함' '화면에 로그인한 borrow 계정이 없습니다.' './mac flick-check'
    st step flick-permissions blocked '실제 맥에서 확인 못 함' 'borrow 계정의 화면 로그인이 필요합니다.' './mac flick-check'
    DEFERRED=1; return 2
  fi
  # Terminal, rather than sshd, is the GUI privacy client. Its stable probe path is reused.
  local modes="$1"; [ "$modes" != all ] || modes='permissions request move tap shot'
  rsh "[ ! -f $R/log/flick.pid ] || ! kill -0 \$(cat $R/log/flick.pid) 2>/dev/null || { echo '이미 시험 중'; exit 1; }"
  sshx "cat > $R/flick.command" <<EOF
#!/bin/bash
umask 077
echo \$\$ > "\$HOME/borrow/log/flick.pid"
exec > >(tee "\$HOME/borrow/log/flick.log") 2>&1
result=0
echo 'Flick macOS 기능 시험. 이것은 Flick 앱이 아닙니다.'
for mode in $modes; do
  echo "시험: \$mode"
  "\$HOME/borrow/probes/flick-probe" "\$mode" || result=2
done
echo "\$result" > "\$HOME/borrow/log/flick.exit"
echo '시험 끝. 결과는 집 PC로 전달됩니다.'
EOF
  rsh "chmod 700 $R/flick.command; rm -f $R/log/flick.exit; open -a Terminal $R/flick.command"
  st job flick
  st step flick-run running '친구 맥 Terminal에서 시험 도구 실행 중'
  st step flick-permissions running '친구 맥 Terminal에서 시험 중'
  st prompt '친구: Terminal의 손쉬운 사용·입력 모니터링·화면 기록을 켜 주세요. 마우스 옆 단추를 누르고 움직여 주세요. 결과가 부족하면 ./mac flick-check로 다시 시험합니다.'
  DEFERRED=1
}

flick_check() {
  build_flick
  if [ "${MAC_HEADLESS:-0}" = 1 ]; then
    rsh "$R/probes/flick-probe permissions"
    st step flick-permissions blocked '러너: 컴파일·권한 조회까지만 확인' '사용자 화면·권한 승인·마우스가 없어 입력과 캡처는 확인 못 함.' './mac flick-check'
    return 0
  fi
  run_flick all
}

logs() {
  case "$1" in mulbit|flick) ;; *) echo 'mulbit 또는 flick'; return 1 ;; esac
  [[ "$2" =~ ^[0-9]+$ ]] || return 1
  mkdir -p "$S/logs"; chmod 700 "$S/logs"
  local file="$S/logs/$1-$(date +%Y%m%d-%H%M%S).log" duration="$2"
  local extra=''
  if [ "$1" = mulbit ]; then
    # Native app error/trigger files, rather than only the build's stdout.
    extra='mkdir -p "$HOME/Library/Application Support/mulbit";
      touch "$HOME/Library/Application Support/mulbit/last-error.log" "$HOME/Library/Application Support/mulbit/trigger.log";
      set -- "$HOME/Library/Application Support/mulbit/last-error.log" "$HOME/Library/Application Support/mulbit/trigger.log";'
  fi
  # A rejected system stream must stay visible while the working app stream continues.
  DEFERRED=1
  echo "앱·macOS 통합 로그 → $file (Ctrl+C로 멈춤)"
  # Unified logs can contain private app data. They stay in the private state directory.
  # Both remote processes are children of this SSH session and are stopped on exit.
  rsh "mkdir -p $R/log; touch $R/log/$1.log; set --; $extra
    children=''; trap 'kill \$children 2>/dev/null || true' EXIT HUP INT TERM
    tail -n 30 -F $R/log/$1.log \"\$@\" & children=\$!
    /usr/bin/log stream --style compact --level debug --predicate 'process == \"$1\" OR process == \"Flick\" OR process == \"flick-probe\" OR senderImagePath CONTAINS[c] \"$1\"' & children=\"\$children \$!\"
    if [ $duration -gt 0 ]; then sleep $duration; else wait; fi" 2>&1 | python3 "$ROOT/scripts/redact.py" "$1" | tee "$file"
  chmod 600 "$file"
  if ! grep -qE 'Must be admin|not permitted' "$file"; then
    st step debug done '앱 로그·허용된 통합 로그 회수 완료'
  fi
}

crash() {
  mkdir -p "$S/crashes"; chmod 700 "$S/crashes"
  local out="$S/crashes/crash-$(date +%Y%m%d-%H%M%S).tar.gz"
  rsh 'cd "$HOME/Library/Logs/DiagnosticReports" 2>/dev/null || exit 0
    find . -maxdepth 1 -type f \( -iname "mulbit*.ips" -o -iname "mulbit*.crash" -o -iname "flick*.ips" -o -iname "flick*.crash" \) -print0 | tar --null -T - -czf -' > "$out"
  chmod 600 "$out"
  if [ -s "$out" ]; then echo "크래시 리포트: $out"; tar -tzf "$out"; else rm "$out"; echo '앱 크래시 리포트 없음'; fi
}

lldb_app() {
  local exe
  case "$1" in
    mulbit) exe='"$HOME/borrow/src/mulbit/src-tauri/target/release/bundle/macos/mulbit.app/Contents/MacOS/mulbit"' ;;
    flick) exe='"$HOME/borrow/probes/flick-probe"' ;;
    *) exe=$(q "$1") ;;
  esac
  if ! rsh 'xcrun --find lldb >/dev/null 2>&1'; then echo '개발자 도구 없음: ./mac clt'; return 2; fi
  echo 'LLDB를 열었습니다. 시작: run · 종료: quit. 앱 권한·화면은 실제 맥에서 확인해야 합니다.'
  if [ "${2:-}" = --batch ]; then rsh "xcrun lldb --batch -o 'target list' $exe";
  else rsh "xcrun lldb $exe"; fi
}

preview() {
  if ! rsh 'launchctl print gui/$(id -u) >/dev/null 2>&1'; then
    st prompt '친구: borrow 계정에 화면 로그인해 주세요. 물빛 설치 창은 그 화면에서 열립니다.'; return 2
  fi
  rsh "set -e; [ \$(uname -m) = arm64 ] || { echo '공개 미리보기는 Apple Silicon 전용. Intel은 소스 빌드가 필요합니다.'; exit 2; }
    mkdir -p $R/dl $R/log
    curl -fL --retry 2 -o $R/dl/mulbit-preview.dmg https://github.com/philaxis/mulbit/releases/download/v0.1.2-preview-mac-linux/mulbit-mac-apple-silicon-preview.dmg
    open $R/dl/mulbit-preview.dmg"
  st prompt '친구: 열린 물빛 미리보기에서 앱을 borrow 계정의 응용 프로그램 폴더로 옮겨 실행해 주세요. 마이크·손쉬운 사용·입력 모니터링을 허용해 주세요.'
  st step mulbit-run awaiting '미리보기 dmg 열림 · 친구의 앱 실행을 기다립니다.'
  st step mulbit-permissions awaiting '친구: 물빛 앱의 마이크·손쉬운 사용·입력 모니터링을 허용해 주세요.'
  DEFERRED=1
}

build_mulbit() {
  DEFERRED=1
  if [ "$1" != --worker ]; then
    case "${PREVIOUS_PHASE:-}" in
      running|queued) st step mulbit-build "$PREVIOUS_PHASE" '이미 빌드가 진행 중입니다.'; echo '이미 빌드가 진행 중입니다. ./mac tail mulbit'; return 0 ;;
    esac
    [ -x "$HOME/.local/bin/job" ] || { echo '일 대기열 도구가 없음. 총괄에게 알리고 --worker를 일 대기열에서 실행하세요.'; return 1; }
    local phase
    # The day lock prevents duplicate queued builds in the normal day flow.
    "$HOME/.local/bin/job" add --for mac-3 --task T407 --note '친구 맥에서 물빛 소스 빌드·결과 회수' -- "$ROOT/mac" build-mulbit --worker
    st step mulbit-build queued '일 대기열에 넣음. 끝남 알림 뒤 ./mac day로 이어갑니다.'
    return
  fi
  "$ROOT/mac" start mulbit "cd mulbit && node scripts/harden-ui.mjs target/release-ui && cd src-tauri && CARGO_BUILD_JOBS=\${JOBS:-2} MACOSX_DEPLOYMENT_TARGET=12.0 npx --yes @tauri-apps/cli@2 build --config '{\"build\":{\"frontendDist\":\"../target/release-ui\"},\"bundle\":{\"targets\":[\"app\",\"dmg\"]}}' -- --locked && codesign --verify --deep --strict target/release/bundle/macos/mulbit.app && echo BUILD-OK"
  st step mulbit-build running '친구 맥에서 물빛 빌드 중'
  while true; do
    sync_state
    if rsh "test -f $R/log/mulbit.exit"; then break; fi
    sleep 10
  done
  local rc; rc=$(rsh "cat $R/log/mulbit.exit")
  [ "$rc" = 0 ] || return "$rc"
  mkdir -p "$S/artifacts"
  rsh "cd $R/src/mulbit/src-tauri/target/release/bundle; tar -czf - macos dmg" > "$S/artifacts/mulbit-bundle.tar.gz"
  chmod 600 "$S/artifacts/mulbit-bundle.tar.gz"
  st step mulbit-build done '물빛 앱·dmg 빌드와 집 PC 회수 완료'
}

sync_state() {
  local tmp; tmp=$(mktemp "$S/sync.XXXXXX")
  if ! rsh "printf 'CONNECTED\\n'
    for n in mulbit flick; do
      if [ -f $R/log/\$n.exit ]; then
        printf 'EXIT\\t%s\\t%s\\n' \"\$n\" \"\$(cat $R/log/\$n.exit)\"
      elif [ -f $R/log/\$n.pid ] && kill -0 \$(cat $R/log/\$n.pid) 2>/dev/null; then printf 'RUN\\t%s\\n' \"\$n\"; fi
      [ ! -f $R/log/\$n.log ] || printf 'LOG\\t%s\\t%s\\n' \"\$n\" \"\$(tail -n 1 $R/log/\$n.log)\"
    done" > "$tmp" 2>/dev/null; then
    st connection no; rm -f "$tmp"; return 1
  fi
  python3 "$ROOT/scripts/sync.py" "$tmp"
  rm -f "$tmp"
}

day() {
  exec 8>"$S/day.lock"; flock -n 8 || { echo 'day가 이미 실행 중입니다.'; return 1; }
  # A checkpoint is written only after its command succeeds. Reconnecting is always checked.
  if ! sshx true 2>/dev/null; then "$ROOT/mac" wait "$1" || return; fi
  st connection yes
  "$ROOT/mac" doctor || { echo '먼저 doctor의 다음 명령을 실행한 뒤 ./mac day'; return 2; }
  local src=${MULBIT_SOURCE:-$HOME/.cache/mulbit-work/xplat} stage phase
  for stage in preview toolchain source mulbit-build flick-check; do
    phase=$(python3 - "$S" "$stage" <<'PY'
import json,pathlib,sys
try: print(json.loads((pathlib.Path(sys.argv[1])/'day.json').read_text()).get(sys.argv[2],''))
except (OSError,ValueError): print('')
PY
)
    [ "$phase" != done ] || { echo "이미 마침: $stage"; continue; }
    case "$stage" in
      preview) "$ROOT/mac" preview || return ;;
      toolchain) "$ROOT/mac" toolchain || return ;;
      source) "$ROOT/mac" push "$src" HEAD mulbit || return ;;
      mulbit-build)
        sync_state || return
        phase=$(python3 - "$S" <<'PY'
import json,pathlib,sys
print(json.loads((pathlib.Path(sys.argv[1])/'status.json').read_text()).get('steps',{}).get('mulbit-build',{}).get('status',''))
PY
)
        case "$phase" in
          done) ;;
          running|queued) echo '물빛 빌드 중. 끝남 알림이 오면 ./mac day'; return 0 ;;
          *) "$ROOT/mac" build-mulbit; echo '빌드가 끝나면 ./mac day로 이어갑니다.'; return 0 ;;
        esac ;;
      flick-check) "$ROOT/mac" flick-check || return ;;
    esac
    python3 - "$S" "$stage" <<'PY'
import json,pathlib,sys,os
s=pathlib.Path(sys.argv[1]); p=s/'day.json'
try: d=json.loads(p.read_text())
except (FileNotFoundError,ValueError): d={}
d[sys.argv[2]]='done'; tmp=p.with_suffix('.new'); tmp.write_text(json.dumps(d)); tmp.chmod(0o600); os.replace(tmp,p)
PY
  done
  echo '당일 준비 차례 끝. 물빛 실행·권한을 확인하고 ./mac logs mulbit 또는 ./mac logs flick. 정리할 때 ./mac down.'
}
