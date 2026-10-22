# 선반 되살리기 — 1단계 검증 체크리스트 (VERIFY)

원칙: 바뀐 동작만 테스트한다. 무거운 빌드·실기 E2E 는 **끝에 한 번**. 반복 측정·재시도 루프 금지. 이 PC(Linux)는 메모리가 빠듯하고 Windows 전용 코드라 로컬에선 `cargo test` 가 돌지 않는다 → 빌드·테스트는 모두 win-bridge(windows-build 스킬)로, 노트북이 켜져 있을 때.
`[작업자]` = 구현 작업자가 한다. `[HQ]` = 헤드쿼터가 한다.

## 1. 단위 테스트 (Windows `cargo test --release`, 한 번)

기준: 기존 191개 전부 통과 + 아래 신규가 통과. 테스트 이름은 레포 관례(문장형 snake_case)로.

| 파일 | 테스트 | 확인하는 동작 |
|---|---|---|
| src/grid.rs | `ghosts_are_the_remembered_apps_not_alive_counted_per_exe` | 크롬 3 기억·1 생존 → 유령 2, 다른 exe 는 그대로 |
| src/grid.rs | `state_without_memory_still_loads_and_saves_memory_back` | `memory` 없는 옛 state.json → `Grid` 로드, `memory` 비어 있음, 저장하면 필드 포함 |
| src/grid.rs | `memory_of_vanished_cells_is_kept_a_week` | `sync()` 는 memory 를 안 지움, `prune_memory(now+8일)` 이 지움, `+6일` 은 유지 |
| src/grid.rs | `remember_caps_a_cell_at_twelve_windows_topmost_first` | `MEMORY_MOST` 상한과 순서 |
| src/restore.rs | `launches_go_out_one_at_a_time_four_hundred_ms_apart` | `next_launch` 간격 |
| src/restore.rs | `a_new_window_is_claimed_by_the_oldest_waiting_cell_of_its_app` | 같은 exe 두 칸 → 첫 창은 첫 칸 |
| src/restore.rs | `claims_expire_after_twenty_seconds_and_restore_ends` | `expire`·`is_done` |
| src/restore.rs | `windows_known_before_the_restore_are_never_claimed` | `known` 집합 |
| src/app.rs (또는 grid.rs 순수 함수로 뽑아서) | `a_ghosting_cell_keeps_its_memory_until_every_ghost_is_back` | 4.2 규칙 2·3·4: ghosting 중엔 덧쓰지 않음, 다 돌아오면 ghosting 해제 후 갱신, 빈 칸은 유지 |
| src/app.rs (또는 순수 함수) | `remembering_skips_pinned_explorer_and_store_hosts` | D-R5 제외 목록 |

실행 (win-bridge, 레포 루트에서; 공유 target 캐시의 app.res 절대경로 문제는 PROGRESS 10-17 메모대로 `copy /b assets\app.res +,, assets\app.res` 를 앞에 붙인다):
```
cargo test --release 2>&1 | tail -30
```
통과 개수와 실패 이름만 기록한다. 로그 전체를 붙이지 않는다.

## 2. 빌드 + E2E (한 번)

`[작업자]` windows-build 스킬로 `cargo build --release` 1회. 산출 `Flick.exe` 와 `drive.exe`(`cargo build --release --example drive`) 로 아래를 **한 번** 돈다.

### 2.1 자동 E2E: `drive navigation ghosts <out dir> <flick exe>`
절차(drive.rs 안에 구현, `navigation shelves` 를 본보기로):
1. 다른 Flick 이 떠 있으면 `RESULT another Flick is running` 출력 후 중단.
2. 데스크톱 2개 생성(`made[0]`, `made[1]`), 홈 = 현재 데스크톱. drive.exe 를 `<out>/probe-ghost.exe` 로 복사.
3. state.json 백업 후 작성:
   - rows: `[{name:"Ghost", cells:[home, made[0]]}, {name:"Other", cells:[made[1]]}]`, `guide_seen: true`, `board_hints_seen: true`
   - memory: `{ made[0]: { taken: 1, apps: [ {exe: "<out>/probe-ghost.exe", args:"", title:"probe-ghost", name:"probe-ghost"} ] } }`
4. Flick `--portable` 실행, `flick.main` 창 대기(≤10 s).
5. `drive restore 0` (WM_RESTORE_ROW, wparam 0).
6. ≤20 s 동안 `FindWindowW("flick.test.hold", "probe-ghost")` 대기 → 뜨면 `vdapi::get_desktop_by_window(hwnd).id() == made[0]` 를 확인. 결과 한 줄 `RESULT ghost window landed in its cell: true/false`.
7. 정리: probe 창 종료, Flick 종료, 데스크톱 제거(홈으로 이동 후), state.json 복구.
합격: `RESULT ... true`, 그리고 Flick `log.txt` 에 `restore` 관련 오류 줄 없음.

### 2.2 그림 한 장: 유령이 보이는 전체보기
`--render-board` 경로(`app.rs render_board`)는 `state.json` 을 읽으므로, 2.1 의 3번 state.json 상태(또는 `FLICK_GHOSTS=<exe>` 환경변수로 render 전용 주입 — 작업자 선택)에서 한 장만 렌더:
```
Flick.exe --render-board <out>\ghosts.raw
```
`[HQ]` 가 PNG 로 바꿔 눈으로 확인: 지도의 빈 칸에 흐린 아이콘, 큰 화면에 "창 없음" 대신 유령 아이콘 한 줄, 다른 칸·레일·행 칸은 0.7.0-pre.5 와 같은 모양.

## 3. 사용자 체감 확인 (실제 PC, `[HQ]` 가 사용자에게 안내)

| # | 장면 | 기대 |
|---|---|---|
| U1 | 어제 쓰던 상태로 앱 몇 개를 두 칸에 나눠 띄움 → 전체보기 열고 닫음(저장) → **재부팅** → Flick 뜬 뒤 전체보기 | 각 칸에 어제 아이콘이 흐리게. 지도에도 흐린 점. 하단 힌트 한 줄이 한 번 뜬다 |
| U2 | 선반 이름 오른쪽 클릭 | 맨 위 "되살리기 (앱 N개)", "기억 지우기", 구분선, 기존 항목. N 이 유령 수와 일치 |
| U3 | 되살리기 | 판이 닫히고 그 선반의 마지막 칸으로 감. 앱이 0.4 s 간격으로 뜨고, 다른 칸 몫은 잠깐 보였다 제 칸으로 빠짐. 전체보기 다시 열면 유령 없음 |
| U4 | 유령 칸에서 앱 하나만 직접 켬 | 그 앱 유령만 사라지고 나머지 유령은 남음 |
| U5 | 기억 지우기 | 그 선반 유령 전부 사라짐. 다른 선반은 그대로 |
| U6 | (D-R4) 선반 접기… → 예 | 창 닫히고 그 선반은 유령만. 되살리기로 복귀 |
| U7 | Flick 만 종료 후 재실행(재부팅 아님) | 유령 없음(앱이 다 살아 있으므로). 평소 화면·휙·미니맵 변화 없음 |
| U8 | `remember = false` 로 바꾸고 재실행 | 유령·메뉴 항목 없음. 다시 true 면 돌아옴 |
| U9 | 자동 실행 앱(메신저 등)이 있는 PC 에서 U1 | 자동 실행 앱이 먼저 떠도 다른 칸의 기억이 지워지지 않음 |

사용자에게 묻는 한 줄: "재부팅 뒤 전체보기에서 어제 아이콘이 흐리게 보였고, 되살리기 한 번으로 돌아왔나요? 거슬리는 깜빡임이 있었나요?"

## 4. 운영 반영 `[HQ]`

- 작업자 push 뒤 HQ 가 diff 검토(4.2 seed 규칙·WM_QUERYENDSESSION·제외 목록·`remember` 기본값 확인) → win-bridge 빌드 1회(병렬 금지) → 1·2절 결과 확인.
- 게시: D-R7 대로 0.7.0 Latest 이후 `release/0.7.1-pre.1`(프리릴리스는 묻지 않음). pin-by-place 가 움직였으면 rebase 후 빌드.
- PROGRESS.md 갱신: 결정 D-R1~7 의 채택값, 테스트 개수, 미확인 항목(U1~U9 중 사용자 미확인). 실제 경로·계정·해시 전체는 쓰지 않는다(sha 는 앞 8자리만).
- README.md / README.ko.md 한 줄 추가 확인(공개 문구이므로 HQ 가 읽는다).

## 5. 금지·주의

- 빌드·`cargo test` 는 각 1회. 실패하면 원인을 고친 뒤 **한 번만** 다시. 측정 루프·스트레스 반복 없음.
- E2E 가 만든 데스크톱·probe 프로세스·state.json 변경은 실패해도 반드시 되돌린다.
- 테스트 출력·문서에 사용자 홈 경로·계정명을 쓰지 않는다. `<out>` 처럼 자리표시로.
- 이 PC 에서 Windows 바이너리 실행·대용량 로그 적재 금지. 로그는 `tail`/`grep` 으로 필요한 줄만.
