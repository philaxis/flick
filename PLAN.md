# 선반 되살리기 — 설계 (PLAN)

작성 2026-10-22. 설계 전용 문서. 구현 작업자는 이 문서와 VERIFY.md 만 보고 일한다.
갈림길마다 "추천 기본값"이 있고, 사용자 결정이 없으면 추천 기본값으로 진행한다.

## 0. 한 줄 요약과 용어

**선반(셸프)은 "일 하나"다. 지금은 선반 이름과 칸(데스크톱)만 남고, 재부팅하면 칸 안의 앱은 사라진다. 칸마다 "거기 있던 앱"을 기억해 두고, 전체보기(board)의 빈 칸에 흐린 유령 아이콘으로 보여 주며, 선반 메뉴 한 번으로 그 앱들을 다시 띄워 제 칸으로 돌려놓는다.**

| 용어 | 뜻 | 코드 |
|---|---|---|
| 선반(셸프, row) | 일 하나. 이름은 선반에만 | `grid::Row` |
| 칸(셀, cell) | 선반 속 데스크톱 하나. GUID 문자열이 id | `grid::CellId` |
| 전체보기(board) | 트리거 탭으로 뜨는 판. 위 큰 화면=선택 칸의 창 모습, 아래 지도=행·칸 주소 | `src/board.rs` |
| 기억(memory) | 칸별로 저장한 "여기 있던 앱" 목록 | 신규 `Grid.memory` |
| 유령(ghost) | 기억엔 있는데 지금 그 칸에 살아 있는 창이 없는 앱. 흐린 아이콘 | 신규 `CellModel.ghosts` |
| 되살리기(restore) | 선반 단위로 유령 앱을 띄워 각자 칸으로 옮기기 | 신규 `src/restore.rs` |
| 접기(fold) | 선반의 창을 모두 닫되 기억은 남겨 유령으로 보이게 하기 | 기존 "모든 창 닫기" 자리 |

## 1. 사용자 시나리오 (전 / 후)

### 장면 A. 아침, 재부팅 직후
- **지금**: 전체보기를 열면 선반 "개발 / 딴짓 / 마케팅"은 그대로인데 칸은 전부 "창 없음". 어제 뭘 어디에 띄웠는지 기억을 더듬어 앱을 하나씩 켜고, 뜬 창을 다시 칸으로 끌어다 놓는다. 그 사이 자동 실행된 메신저는 아무 칸에 떨어져 있다.
- **후**: 전체보기를 열면 각 칸에 어제 있던 앱 아이콘이 흐리게(유령) 놓여 있다. 지도에서도 칸마다 흐린 점으로 보여 "어느 선반이 어제 일했던 선반"인지 한눈에 보인다. "개발" 선반 이름을 오른쪽 클릭 → **되살리기 (앱 4개)**. 판이 닫히고 "개발" 선반의 마지막 칸으로 가며, 그 선반의 앱들이 차례로 떠서 각자 칸으로 들어간다. 다른 칸으로 가야 할 창은 잠깐 보였다가 제 칸으로 빠진다. 2단계부터는 자리(모니터·위치)까지 되돌린다.

### 장면 B. 손으로 반만 되살리기
- 유령이 있는 칸으로 가서 사용자가 직접 VS Code 만 켠다. 그 칸의 유령 중 VS Code 는 사라지고 나머지(터미널, 브라우저)는 그대로 유령으로 남는다. 다 켜지면 유령은 없어진다. 안 쓰기로 했으면 선반 메뉴 → **기억 지우기**.

### 장면 C. (D-R4 채택 시) 일 하나 접어 두기
- 마케팅 선반을 오늘은 안 본다. 선반 메뉴 → **선반 접기…** → 확인 → 창이 모두 닫히고 그 선반 칸들은 유령만 남는다. 다음 주에 **되살리기**로 그대로 돌아온다. "선반 = 일 하나"가 켜고 끄는 단위가 된다.

### 바뀌지 않는 것
- 평상시 화면(일반보기)·미니맵·휙·전환 연출은 전혀 손대지 않는다. 유령은 전체보기에만 보인다.
- 전체보기의 글씨 없는 생김새(이름은 선반에만, 창은 아이콘)는 그대로. 유령은 같은 아이콘을 흐리게 그린 것뿐, 새 패널·버튼·글자가 늘지 않는다. 설명은 기존 하단 힌트 줄(FOOTER)에서만.

## 2. 기존 철학·결정과의 관계

근거 파일: `README.md` 13행("A shelf holds the desktops for one job"), `README.ko.md` 13행, 출발 워크트리 `PROGRESS.md`(2026-10-17 그릴링, "현재 유효한 결정" 절), 0.7.0 사양 `pin-by-place-spec.md`(현재 세션 스크래치, 원문 요지는 밤티 워크트리 PROGRESS 0.7.0 절).

| 지킨다 | 어떻게 |
|---|---|
| 선반 하나 = 일 하나 (README 13행) | 되살리기·접기·기억 지우기는 모두 **선반 단위** 메뉴. 칸 단위 버튼을 만들지 않는다. |
| 큐브 원칙 (PROGRESS "현재 유효한 결정" 1항) | 되살리기로 선반에 갈 때 `Row::anchor()` 칸으로 간다. anchor 를 바꾸지 않는다. |
| 전체보기 생김새: 글씨는 셸프 이름만, 창은 앱 아이콘 (2026-10-20 결정) | 유령 = 기존 아이콘 타일을 `dim` 낮춰 그림. 새 텍스트는 FOOTER 힌트 한 줄만(기존 힌트 체계). |
| AI 자동 분배·격자 바꿔 끼우기 폐기 (2026-10-17 그릴링) | 수동 되살리기만. 어떤 앱을 어느 칸에 둘지 추측하지 않는다. 기억에 있던 그 칸으로만 보낸다. |
| 고정 창 규칙 (모든 셸프 고정=불변, 이 셸프 고정=행 따라감; 0.7.0 pin-by-place) | MVP 는 고정 창을 기억하지 않는다(제외). 고정 체계와 섞지 않는다. 2단계 과제. |
| Esc·빈 곳 클릭 = 선택 칸 이동, 포커스 잃음 = 이동 없음 | 되살리기는 `leave_board` 와 같은 경로로 판을 닫는다. 새 닫힘 규칙 없음. |
| 입력 경로는 큰 재작성 금지 (2026-10-17 교훈) | input.rs·glide.rs·follow.rs 는 건드리지 않는다. |
| 공개 레포 | state.json 은 사용자 PC 안에만 있다. 문서·테스트 로그에 실제 경로·계정을 남기지 않는다. |

| 바꾼다 | 전 | 후 | 이유 |
|---|---|---|---|
| 빈 칸 표시 | "창 없음" 글자 (`board.rs draw_empty`) | 유령이 있으면 유령 아이콘, 없으면 기존 글자 | 빈 칸이 "비었다"가 아니라 "어제 이게 있었다"를 말해야 한다 |
| 선반 메뉴 (`tray.rs row_menu`) | 모든 창 닫기… / 셸프만 제거 | **되살리기 (앱 N개)** / **기억 지우기** 추가. D-R4 채택 시 "모든 창 닫기…" → "선반 접기…" | 선반 단위 조작의 입구는 이미 선반 메뉴 하나다 |
| 저장 시점 | 2 s 타이머·판 떠날 때 | 추가로 **WM_QUERYENDSESSION**(종료·재부팅 신호)에서 한 번 더 저장 | 재부팅 직전 상태가 가장 중요한데 지금은 종료 신호를 받지 않는다 |
| state.json | rows/usage/follow_apps/... | `memory` 필드 추가 (serde default, 옛 버전은 무시) | 하위 호환 유지 |

## 3. 사용자 결정 필요

작업자는 모두 **추천**으로 진행한다. 사용자가 바꾸면 그 항목만 고친다.

| # | 갈림길 | 선택지 | 추천 | 이유 |
|---|---|---|---|---|
| D-R1 | 앱을 무엇으로 기억·되살리나 | (a) exe 전체 경로만 (b) exe + 명령줄 인수 | **(a)** MVP. 필드 `args` 는 지금 만들어 두고 빈 문자열 | (b)는 다른 프로세스의 PEB 를 읽어야 해(NtQueryInformationProcess) 몇 시간 안에 검증까지 어렵다. 브라우저·VS Code 는 스스로 세션을 복원해 (a)로도 체감이 좋다. PWA(`--app-id`)·폴더 열기는 2단계 |
| D-R2 | 같은 앱 창이 여럿이면 | (a) exe 당 한 번 실행 (b) 창 수만큼 실행 | **(a)** | 크롬은 스스로 창을 복원하고, VS Code 는 두 번 켜도 한 창이다. (b)는 중복 창을 만든다. 기억은 창 단위로 저장해 두어(개수 표시·2단계용) 실행만 exe 당 1회 |
| D-R3 | 되살리기를 누르면 판은 | (a) 판 닫고 그 선반 anchor 칸으로 이동, 앱이 눈앞에서 뜬다 (b) 판을 연 채 뒤에서 뜬다 | **(a)** | 새 창은 포커스를 가져가 판이 어차피 Dismiss 된다. "그 선반으로 돌아간다"는 느낌이 큐브 원칙과도 맞는다. 다른 칸 몫의 창은 잠깐 보였다 빠진다(위험 7절) |
| D-R4 | "선반 접기" 도입 | (a) 기존 "모든 창 닫기…"를 "선반 접기…"로 바꿔, 닫은 뒤 기억을 유령으로 남김 (b) 메뉴는 그대로, 접기 없음 | **(a)** | 코드 추가가 거의 없고(닫기 + 유령 표시 켜기) "일 하나를 접어 두고 다시 꺼낸다"가 완성된다. 확인 대화상자는 그대로 둔다 |
| D-R5 | 기억에서 빼는 앱 | explorer.exe(폴더 창), ApplicationFrameHost.exe(스토어 앱), 고정 창 | **셋 다 제외** | explorer 재실행은 "홈" 폴더만 열어 오해를 준다. 스토어 앱은 AUMID 로 켜야 한다(2단계). 고정 창은 선반/전체 소속이라 칸 기억과 성격이 다르다 |
| D-R6 | 사라진 칸(Windows 가 데스크톱을 잃은 경우)의 기억 | (a) 7일간 보관만, UI 없음 (b) 1단계에 "유령 선반" 행까지 | **(a)** | 선반이 통째로 사라지는 경로는 드물고(원래 증상은 Flick 조기 종료였음, PROGRESS 10-16) 보드 레이아웃을 건드려 0.7.0 pin-by-place 와 충돌 위험이 크다. 보관해 두면 2단계에서 그대로 쓸 수 있다 |
| D-R7 | 출시 버전 | (a) 0.7.0 이 Latest 가 된 뒤 0.7.1-pre.1 (b) 0.7.0-pre.N 에 끼워 넣기 | **(a)** | 0.7.0 은 독립 Opus 검증 절차가 돌아가는 중. 브랜치는 지금 pin-by-place 위에서 만들고, 합치는 시점만 뒤로 |

## 4. 1단계 MVP 설계

범위: 기억 저장 → 유령 표시 → 선반 메뉴 되살리기·기억 지우기(·접기) → 실행과 "집배"(새 창을 칸으로 옮기기) → 종료 시 저장 → 테스트 심. 보드 레이아웃·입력·전환은 불변.

### 4.1 데이터 (src/grid.rs)

```rust
/// A window that was in a cell: enough to start its app again.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AppMemory {
    /// Full path of the executable (vd::exe_path).
    pub exe: String,
    /// Command line beyond the exe; empty in 0.7.x (D-R1).
    #[serde(default)]
    pub args: String,
    /// For the hint line; may be empty.
    #[serde(default)]
    pub title: String,
    /// The app's name as people know it (vd::app_label).
    #[serde(default)]
    pub name: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CellMemory {
    /// Unix seconds when the cell last had live windows (usage::now()).
    pub taken: u64,
    /// One entry per window, topmost first, at most MEMORY_MOST (12).
    pub apps: Vec<AppMemory>,
}

// Grid 에 추가 (usage 와 같은 꼴):
#[serde(default)]
pub memory: BTreeMap<CellId, CellMemory>,
```

순수 함수(Windows 의존 없음, 단위 테스트 대상):
- `grid::ghosts(memory: &CellMemory, live_exes: &[String]) -> Vec<&AppMemory>`: `apps` 에서 살아 있는 창의 exe(소문자 파일명 기준, `vd::exe_name` 과 같은 규칙)와 **개수까지** 맞춰 뺀 나머지. 크롬 창 3개 기억·1개 살아 있으면 유령 2개.
- `Grid::remember(&mut self, id, apps, now)`: 그 칸의 기억을 통째로 바꾼다(`taken = now`).
- `Grid::forget(&mut self, ids: &[CellId])`.
- `Grid::prune_memory(&mut self, now)`: 격자에 없는 칸의 기억은 `taken` 이 7일(`MEMORY_KEEP_SECS = 7*24*3600`) 지나면 버린다. **`sync()` 는 memory 를 건드리지 않는다**(D-R6: 사라진 칸 기억 보관).
- 상수: `MEMORY_MOST = 12`(칸당 저장 창 수 상한).

### 4.2 수집 (src/app.rs)

- `App` 필드 추가: `ghosting: HashSet<CellId>` (유령을 보여 줄 칸, 런타임 전용), `ghosts_seeded: bool`, `restore: restore::Restore` (4.5).
- **씨앗 뿌리기(seed)**: `run()` 의 첫 `grid.sync()` 직후(2862행 부근) 한 번, `ghosting = memory.keys()` 전부. 살아 있는 창을 보지 않고 전부 넣는 것이 핵심이다. 로그온 직후 자동 실행 앱이 먼저 떠 버려 기억이 덧쓰이는 사고를 막는다. Flick 만 재시작한 경우엔 모든 앱이 살아 있어 첫 모델 계산에서 유령이 0개가 되고 아래 규칙으로 곧바로 ghosting 에서 빠진다.
- **기억 갱신 규칙** (`save()` 안, `prune_usage` 다음; `save` 는 2 s CONFIG 타이머·판 떠날 때·종료 시 불린다):
  1. `vd::windows(current)` 를 칸별로 묶는다(모델 만들 때와 같은 분할). 제외: `pinned_everywhere`, `self.follows(w)`(행 따라가는 창), exe 파일명이 `explorer.exe`·`applicationframehost.exe`, `exe_path` 가 None, Flick 자신(이미 `app_windows` 에서 빠짐). (D-R5)
  2. 칸이 `ghosting` 에 **없고** 살아 있는 창이 1개 이상 → `remember(id, 창들, now)`.
  3. 칸이 `ghosting` 에 **있으면** 기억을 건드리지 않는다. 단 `ghosts(memory, live) `가 비면(다 돌아왔으면) `ghosting` 에서 빼고 2번을 적용한다.
  4. 살아 있는 창이 0개면 기억을 그대로 둔다(덧쓰지 않는다). 이것이 재부팅·접기 양쪽을 한 규칙으로 덮는다.
  5. 끝에 `prune_memory(now)`.
- **종료 신호**: `wndproc` 에 `WM_QUERYENDSESSION` 추가 → `with_app(|app| app.save())` 후 `LRESULT(1)` 반환. `WM_ENDSESSION` 은 아무것도 안 한다(이미 저장).
- 설정: `config.toml` 최상위 `remember = true`(기본 켬). 꺼지면 수집·유령·메뉴 항목 모두 없음, 기존 기억은 그대로 둔다. 되돌리기 스위치다(7절).

### 4.3 유령 모델 (src/app.rs `model()` → src/board.rs 모델 타입)

```rust
// board.rs
pub struct GhostModel {
    pub exe: String,
    pub name: String,
    /// Drawn from the exe's own icon, loaded once per path (board.rs icon cache).
    pub icon: Option<HICON>,
}
// CellModel 에 추가
pub ghosts: Vec<GhostModel>,   // exe 당 하나로 묶은 뒤(group 과 같은 규칙), 개수는 +N 배지로
```
- `model()` 에서 각 칸: `if ghosting.contains(id) { ghosts = grid::ghosts(memory[id], live_exes) }` 아니면 빈 벡터. `GhostModel` 은 exe 당 하나, `count` 는 유령 개수(+N 표시용) — `WindowModel` 묶음과 같은 `group_apps` 꼴의 `group_ghosts`.
- 아이콘: `board.rs` 에 `fn exe_icon(path) -> Option<HICON>` (`SHGetFileInfoW(path, 0, &mut info, size, SHGFI_ICON | SHGFI_LARGEICON)`), 경로별 `HashMap<String, Option<ID2D1Bitmap>>` 캐시. 기존 `window_icon(hwnd) → 비트맵` 변환 뒤쪽 절반(HICON → 픽셀 → D2D 비트맵)을 함수로 떼어 재사용한다. 캐시는 판이 닫힐 때 비우지 않아도 된다(경로 수가 작다).
- `Model::has_no_own_windows(id)` 는 그대로(유령은 "own window" 가 아니다).

### 4.4 그리기·힌트 (src/board.rs)

- **지도 칸**: 살아 있는 창 아이콘 배지를 그리는 자리에, 유령 배지를 `draw_badges(..., dim = GHOST_DIM)` 로 같이 그린다. 유령은 살아 있는 아이콘 **뒤에** 이어 붙여 같은 타일 규칙(두 줄까지, 넘치면 +N)을 탄다. 상수 `GHOST_DIM: f32 = 0.35`. 흑백 처리나 점선 테두리 같은 새 스타일은 만들지 않는다 — 흐림만으로 충분하고 깔끔하다.
- **위 큰 화면** (`draw_empty`): 선택 칸에 own window 가 없고 유령이 있으면 "창 없음" 글자 대신 유령 아이콘을 가운데 한 줄(최대 6개, 48 px·간격 20 px·`GHOST_DIM`)로 그린다. 글자는 넣지 않는다. 유령도 없으면 기존 "창 없음".
- **FOOTER 힌트**: 커서가 유령 위에 있을 때 `i18n::text("Yesterday's apps — right-click the shelf name to bring them back", "어제 있던 앱 — 선반 이름을 오른쪽 클릭해 되살리기", "昨天的应用 — 右键点击架子名称即可恢复")`. 또 세션 중 **처음 판을 열었을 때** 유령이 하나라도 있으면 그 힌트를 한 번 자동으로 보여 준다(기존 "처음 보여 주는 힌트" 경로, `board_hints_seen` 과 별개로 `ghost_hint_shown: bool` 런타임).
- **히트 테스트**: 유령 아이콘은 잡히지 않는다(`Hit::Nothing` 과 같은 취급, 호버만). pin-by-place 의 "지도 아이콘은 표식, 잡을 수 없다" 원칙과 같다. 유령 × 지우기는 2단계.

### 4.5 선반 메뉴와 실행 (src/tray.rs, src/app.rs, 신규 src/restore.rs)

`tray.rs`
```rust
pub enum RowCommand { CloseWindows = 1, Remove, Restore, Forget }
pub fn row_menu(owner, summary, removable, ghosts: usize) -> Option<RowCommand>
```
- `ghosts > 0` 이면 맨 위에 **"되살리기 (앱 N개)" / "Bring back N apps" / "恢复 N 个应用"**, 그 아래 **"기억 지우기" / "Forget yesterday's apps" / "忘记昨天的应用"**, 구분선, 기존 항목.
- D-R4(a): "모든 창 닫기…" 문구를 **"선반 접기…" / "Fold the shelf…" / "折叠架子…"** 로, 확인 대화상자 본문 끝에 "창은 닫히고 어제의 선반으로 남습니다." 한 줄 추가. 동작: 기존 `close_row_windows(row)` + `ghosting.extend(그 선반 칸들)`. (닫기 직전 `save()` 를 한 번 불러 기억을 최신으로 만든 뒤 닫는다.)

`app.rs run_row_menu`: `app.ghost_count(row)` 를 메뉴에 넘기고 `Restore → app.restore_row(row)`, `Forget → app.grid.forget(칸들); ghosting 에서 제거; save()`.

`app.rs restore_row(row)`:
1. 유령이 없는 선반이면 아무것도 안 한다.
2. 판이 열려 있으면 `leave_board(anchor 칸 id, None)` 과 같은 경로로 닫고 그 칸으로 간다(D-R3). 이미 그 선반에 있으면 전환 없이 닫기만.
3. `self.restore.queue(cell_order, ghosts)`: 선반의 칸 순서대로, 칸마다 유령 exe 당 하나(D-R2) `Launch { cell, exe, args }` 를 큐에 넣는다.
4. `SetTimer(RESTORE_TIMER_ID = 11, 250 ms)` 시작.

`src/restore.rs` (순수 로직 + 작은 Windows 접착면, 로직은 단위 테스트):
```rust
pub const LAUNCH_GAP_MS: u64 = 400;   // 한 번에 하나씩, 집배 순서가 섞이지 않게
pub const CLAIM_MS: u64 = 20_000;     // 이 뒤로 안 뜬 앱은 포기(log.txt 한 줄)

pub struct Launch { pub cell: CellId, pub exe: String, pub args: String }
pub struct Claim  { pub cell: CellId, pub app: String /* exe 파일명 소문자 */, pub since_ms: u64 }

pub struct Restore {
    queue: VecDeque<Launch>,
    claims: Vec<Claim>,          // 뜨기를 기다리는 앱, 큐에서 나간 순서
    known: HashSet<isize>,       // 시작 시점에 이미 있던 hwnd
    last_launch_ms: u64,
}
impl Restore {
    pub fn queue(&mut self, launches: impl IntoIterator<Item = Launch>, known: impl IntoIterator<Item = isize>);
    /// 다음 띄울 것, LAUNCH_GAP_MS 마다 하나.
    pub fn next_launch(&mut self, now_ms: u64) -> Option<Launch>;
    /// 새 창 (hwnd, exe 파일명) 을 보고 보낼 칸을 정한다: 같은 app 의 가장 오래된 claim.
    pub fn claim(&mut self, hwnd: isize, app: &str, now_ms: u64) -> Option<CellId>;
    pub fn expire(&mut self, now_ms: u64) -> Vec<Claim>;   // 포기한 것(로그용)
    pub fn is_done(&self) -> bool;
}
```
- `app.rs` 타이머 틱(250 ms): ① `expire` → log ② `next_launch` 가 있으면 `ShellExecuteExW`(동사 "open", `lpFile = exe`, `lpParameters = args`, `lpDirectory = exe 의 폴더`, `SW_SHOWNORMAL`; UAC·앱 실행 별칭을 셸이 처리) 후 `claims.push` ③ `vd::app_windows()` 중 `known` 에 없는 hwnd 마다 `vd::exe_of_window` 로 `claim` → 칸이 현재 데스크톱과 다르면 `vdapi::move_window_to_desktop`; 어쨌든 `known.insert` ④ `is_done` 이면 `KillTimer`.
- 같은 exe 가 두 칸에 기억돼 있으면 먼저 뜬 창이 먼저 큐에 든 칸으로 간다(한계, 7절). 크롬처럼 이미 떠 있는 프로세스에 창을 넘기는 앱도 **새 hwnd** 기준으로 집배하므로 pid 가 달라도 된다.
- 옮긴 창에 포커스를 주지 않는다. 사용자는 anchor 칸에 서 있다.

### 4.6 테스트 심 (examples/drive.rs)

- 메시지 `WM_RESTORE_ROW: u32 = WM_APP + 15` (`wparam = row`), `drive restore <row>` 로 보낸다. `--portable` 실행 중인 Flick 에 메뉴 없이 되살리기를 시킨다.
- `drive.rs` 가 **인수 없이** 실행되고 자기 exe 파일명이 `probe-` 로 시작하면 `hold` 모드로 창 하나(제목 = 파일명 stem, 60 s)를 띄운다. E2E 는 drive.exe 를 `probe-ghost.exe` 로 복사해 `memory.exe` 에 넣는다 → 되살리기가 인수 없는 exe 만 켜도 창이 뜬다.
- 새 하위 명령 `drive navigation ghosts <out dir> <flick exe>`: VERIFY.md 의 E2E 절차를 자동으로 돈다(기존 `navigation shelves` 가 본보기: 데스크톱 만들기 → state.json 쓰기 → Flick --portable → 메시지 → 확인 → 정리·state.json 복구).

### 4.7 구현 순서 (작업자용)

1. grid.rs 데이터·순수 함수·테스트 (옛 state.json 읽힘 포함) — 30 분
2. restore.rs 큐·집배 로직·테스트 — 45 분
3. app.rs 수집 규칙·seed·WM_QUERYENDSESSION·`remember` 설정·타이머 접착 — 60 분
4. board.rs 모델·exe 아이콘·배지·큰 화면·힌트 — 60 분
5. tray.rs 메뉴 + app.rs 메뉴 처리·restore_row(·접기) — 30 분
6. drive.rs 심·E2E 명령 — 30 분
7. win-bridge 빌드 **한 번** + cargo test + E2E 한 번 + `--render-board` 한 장 — 30 분

파일 변경 요약: `src/grid.rs`, `src/restore.rs`(신규, `main.rs` 에 `mod restore;`), `src/app.rs`, `src/board.rs`, `src/tray.rs`, `src/config.rs`(remember 키), `examples/drive.rs`, `README.md`/`README.ko.md` 한 줄씩("선반을 되살립니다…"). `input.rs`·`glide.rs`·`follow.rs`·`quick.rs`·`paint.rs`·`vendor/` 는 손대지 않는다.

## 5. 구현 기반 브랜치

- **기반: `origin/pin-by-place`** (0.7.0-pre.5, c195035). 로컬 워크트리 중 이 브랜치를 체크아웃한 곳은 없으므로 작업자는 자기 워크트리에서 `git fetch origin && git checkout -b shelf-restore origin/pin-by-place` 로 시작한다.
- 이유: 0.7.0 은 `board.rs` 를 1,400 줄 넘게 바꿨다(고정 레일·행 칸·드롭 판정). `flick-recede`(0.6.2) 위에서 만들면 board.rs 와 app.rs 가 반드시 충돌한다. 이 설계가 만지는 board.rs 부분(배지 그리기·draw_empty·FOOTER·모델 타입)은 pin-by-place 가 바꾼 영역(레일·드롭)과 겹치지 않는다.
- `main` 은 0.5.10 에 머물러 있고 이 PC 의 `main`/`플랜 워크트리`는 빈 Initial commit 이므로 기반으로 쓰지 않는다. 이 PLAN 브랜치(`philaxis/plan-big-shelfrestore`)는 문서 전용이다.
- 합치기(D-R7): pin-by-place 가 다시 움직이면 `git rebase origin/pin-by-place` 로 따라간다. 0.7.0 Latest 뒤 `release/0.7.1-pre.1` 로 게시. 프리릴리스는 묻지 않고 올린다(사용자 지시).
- push 는 `shelf-restore` 브랜치만. `pin-by-place`·`main` 에 직접 push 하지 않는다.

## 6. 2단계 이후 로드맵 (짧게)

1. **유령 선반**(D-R6): `sync()` 가 칸이 전부 사라진 선반을 지울 때 `folded_rows: Vec<{name, cells: Vec<CellMemory>}>` 로 옮기고, 지도 맨 아래 점선 행으로 보인다. 되살리기 = `create_desktop` 으로 칸을 만들고 `rows` 에 넣은 뒤 4.5 와 같다. Windows 가 데스크톱을 잃는 경우를 덮는다.
2. **명령줄 기억**(D-R1 b): PEB 에서 명령줄 읽기 → PWA(`--app-id`), VS Code 폴더, 터미널 프로필. `args` 필드는 이미 있다.
3. **자리까지**: `AppMemory` 에 `rect`·`monitor` 추가(serde default), 집배 때 `vd::move_to_monitor` + `SetWindowPlacement`.
4. **스토어 앱**: `GetApplicationUserModelId` 로 AUMID 저장, `explorer.exe shell:AppsFolder\<AUMID>` 로 실행. 폴더 창: `IShellWindows` 로 경로.
5. **고정 창 기억**: 전체 고정은 격자 전체, 행 고정은 선반 소속으로 저장. pin-by-place 가 굳은 뒤.
6. **유령 하나씩 지우기**(호버 ×), **되살리기 진행 표시**(힌트 줄에 "3/4") — 사용자 요청이 있을 때만.

## 7. 하지 말아야 할 것 · 위험 · 되돌리기

**하지 말 것**
- 앱을 어느 칸에 둘지 추측·자동 분배하지 않는다(2026-10-17 그릴링). 기억에 있던 칸 외엔 보내지 않는다.
- 일반보기·미니맵·전환 연출·입력 경로를 바꾸지 않는다. 유령은 전체보기에만.
- 보드에 새 글자·버튼·패널을 늘리지 않는다. 메뉴 항목과 FOOTER 힌트 한 줄만.
- 사용자가 켠 적 없는 프로그램을 켜지 않는다(기억은 그 칸에 떠 있던 창에서만 나온다). 설치 프로그램·업데이트 창처럼 한 번 뜨고 사라지는 exe 가 기억에 남을 수 있다 → `taken` 기준 7일 보관이라 오래 남진 않지만, 2단계에서 "창이 30 초 미만 살았던 exe 는 기억 안 함" 규칙을 검토.
- state.json 에 창 제목 외 개인 데이터를 넣지 않는다(명령줄은 2단계 결정 사항). 테스트 기록·커밋·문서에 실제 사용자 경로·계정을 쓰지 않는다.
- 병렬 작업자가 win-bridge 를 쓰지 않는다. 빌드는 끝에 한 번.

**위험과 대응**
| 위험 | 대응 |
|---|---|
| 로그온 직후 자동 실행 앱이 기억을 덧쓴다 | seed 를 첫 sync 직후 **살아 있는 창을 보지 않고** 전부 ghosting 에 넣는다(4.2). 테스트로 고정 |
| 다른 칸 몫의 창이 anchor 칸에 잠깐 보였다 빠진다 | 250 ms 집배라 한 번 깜빡임 수준. 사용자가 거슬리면 2단계에서 "칸별로 그 칸에 가서 켜기" 옵션 |
| 같은 exe 가 두 칸에 있으면 창이 바뀐 칸에 갈 수 있다 | 큐 순서=칸 순서, 400 ms 간격으로 완화. 명령줄·제목 기억(2단계)으로 해결 |
| 크롬·VS Code 처럼 기존 프로세스가 창을 여는 앱 | pid 가 아니라 **새 hwnd** 로 집배 |
| 앱이 20 초 안에 안 뜬다(UAC 대기 등) | claim 만료 → log.txt 한 줄, 유령은 그대로 남아 다시 시도 가능 |
| 되살리기 중 사용자가 다른 선반으로 간다 | 새 창은 그때의 현재 데스크톱에 뜨지만 집배는 칸 id 기준이라 제 칸으로 간다 |
| 재부팅 직전 저장을 못 한다 | WM_QUERYENDSESSION 저장 + 평소 2 s 저장. 최악에도 2 s 전 상태 |
| 옛 버전으로 내려간다 | `memory`·`remember` 는 serde default 라 옛 exe 가 무시하고 읽는다. 옛 exe 가 저장하면 `memory` 가 사라질 뿐 깨지지 않는다 |

**되돌리기**
- 즉시: `config.toml` 에 `remember = false` → 수집·유령·메뉴 항목이 모두 꺼진다.
- 코드: `shelf-restore` 브랜치 단일 머지 커밋 revert. state.json 의 `memory` 는 남아도 무해하다.
