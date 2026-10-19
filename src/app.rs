//! Wires input, the grid model, the virtual desktops, the minimap, the board
//! and the settings window together.

use crate::{
    admin,
    board::{Action, Board, CellModel, Model, RowModel, WindowModel},
    config::{self, Config},
    follow,
    glide::{self, Glide, Place, Scene, Shot, WM_GLIDE_SETTLED, WM_GLIDE_TARGET},
    guide::{self, Guide},
    grid::{self, Dir, Grid, Pos, Row},
    input::{self, Hand, Trigger, WM_CAPTURED, WM_CLICK, WM_DRAG, WM_RELEASE, WM_ROWS, WM_SLIDE, WM_STEP},
    overlay::{CellView, Minimap, Overlay, View},
    settings::{self, Settings},
    stack::{self, Kind},
    tray::{self, Command, PinCommand, PinState, RowCommand},
    usage,
    vd::{self, Desktops},
    vdapi,
};
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    sync::{atomic::{AtomicU32, Ordering}, mpsc},
    time::{Duration, Instant, SystemTime},
};
use windows::{
    core::{w, HSTRING, PCWSTR},
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
        System::{
            Com::{CoInitializeEx, COINIT_MULTITHREADED},
            LibraryLoader::GetModuleHandleW,
            SystemInformation::GetTickCount,
            Threading::GetCurrentProcessId,
        },
        UI::{
            Accessibility::{SetWinEventHook, HWINEVENTHOOK},
            HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2},
            Input::KeyboardAndMouse::{
                GetAsyncKeyState, GetLastInputInfo, RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, LASTINPUTINFO,
                MOD_ALT, MOD_CONTROL, MOD_SHIFT, MOD_WIN, VK_CONTROL, VK_DOWN, VK_LEFT, VK_MENU, VK_RIGHT, VK_SHIFT, VK_UP,
            },
            Shell::ShellExecuteW,
            WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetForegroundWindow, GetMessageW,
                GetWindowThreadProcessId, EVENT_SYSTEM_FOREGROUND, WINEVENT_OUTOFCONTEXT,
                IsIconic, IsWindow, KillTimer, MessageBoxW, PeekMessageW, PostMessageW, PostQuitMessage, RegisterClassW,
                RegisterWindowMessageW, SendMessageTimeoutW, SetTimer, ShowWindow, TranslateMessage, SMTO_ABORTIFHUNG, SMTO_BLOCK, IDYES, MB_DEFBUTTON2, MB_ICONWARNING,
                MB_OK, MB_YESNO, MSG, PBT_APMRESUMEAUTOMATIC, PM_NOREMOVE, PM_REMOVE, SW_HIDE, SW_RESTORE, SW_SHOWNORMAL, WINDOW_EX_STYLE,
                WM_APP, WM_CLOSE, WM_CONTEXTMENU, WM_DESTROY, WM_HOTKEY, WM_LBUTTONUP, WM_POWERBROADCAST,
                WM_NULL, WM_QUIT, WM_RBUTTONUP, WM_TIMER, WNDCLASSW, WS_OVERLAPPED,
            },
        },
    },
};

/// The last UI message, retained while a Windows call holds up its handler.
static UI_MESSAGE: AtomicU32 = AtomicU32::new(0);

const WM_TRAY: u32 = WM_APP + 10;
const WM_DESKTOP_EVENT: u32 = WM_APP + 11;
/// Opens the settings window; for the test driver (examples/drive.rs).
const WM_OPEN_SETTINGS: u32 = WM_APP + 12;
/// Another window came to the front: the windows kept on top are put back
/// in their order (`App::restack`).
const WM_RESTACK: u32 = WM_APP + 13;
/// Opens the window showing how Flick is used; sent by a second start.
const WM_OPEN_GUIDE: u32 = WM_APP + 14;

/// How long the minimap stays after the trigger is released and after a
/// hotkey (which has no release to wait for).
const LINGER_RELEASE_MS: u64 = 350;
const LINGER_HOTKEY_MS: u64 = 900;
/// One-shot timer that refreshes the board after windows were asked to close.
const REFRESH_TIMER_ID: usize = 2;
/// Periodic check for rows idle long enough to be put to sleep.
const SLEEP_TIMER_ID: usize = 3;
/// Puts Windows' own "desktop name" label back after a switch.
const LABEL_TIMER_ID: usize = 4;
/// How long that label would have stayed up.
const LABEL_HIDE_MS: u32 = 1600;
/// Periodic look at the config file, to apply it when the user saves it.
const CONFIG_TIMER_ID: usize = 5;
/// One-shot timer that tells the app again that the trigger was let go of,
/// when it was too busy to hear it.
const RELEASE_TIMER_ID: usize = 6;
/// One-shot timer that gives the board the keyboard back (`Action::Dismiss`).
const RECLAIM_TIMER_ID: usize = 7;
/// One-shot timer that sorts the windows kept on top once more, after Windows
/// has finished raising the one just clicked.
const RESTACK_TIMER_ID: usize = 8;
/// Recovery timer, kept until the current desktop can be read.
const FLIGHT_TIMER_ID: usize = 9;
/// A switch asked of Windows is taken as made after this long even when
/// Windows has not told of it, so a cell waiting behind it is not kept waiting.
const FLIGHT_MS: u32 = 1000;
/// One-shot timer that goes on from a cell passed by (`App::go_on`).
const ONWARD_TIMER_ID: usize = 10;
/// How long Windows goes on showing a cell after telling it is there
/// (250 was not always enough on a 23H2 PC with a busy CPU, 400 was).
const ONWARD_MS: u32 = 400;

/// Whether a window of desktop `id` has the keyboard.
fn keyboard_held_on(id: &str) -> bool {
    let window = unsafe { GetForegroundWindow() };
    vdapi::get_desktop_by_window(window).is_ok_and(|desktop| desktop.id() == id)
}

fn onward_ms() -> u32 {
    #[cfg(feature = "debug-log")]
    if let Some(ms) = std::env::var("FLICK_T533_WAIT").ok().and_then(|v| v.parse().ok()) {
        return ms;
    }
    ONWARD_MS
}
/// For this long after the board opened, or a cell was added or removed in
/// it, Windows may still hand the keyboard to a window of its own choosing.
const BOARD_SETTLES_MS: u64 = 1500;
/// For this long after going to a cell from the board, or letting go of the
/// trigger on one, Windows may go back to the cell left.
const GOING_SETTLES_MS: u64 = 700;
const CONFIG_CHECK_MS: u32 = 2000;
/// Started with Windows, Flick can come up before Explorer has its virtual
/// desktops ready: it asks again this often, for this long, before giving up.
const DESKTOPS_POLL_MS: u64 = 500;
const DESKTOPS_WAIT_MS: u64 = 180_000;
/// Hotkey ids 0..=3 move in the direction of `input::dir_from_index`, 4..=7
/// do the same carrying the active window, and this one opens the pin menu.
const PIN_HOTKEY_ID: i32 = 8;

struct App {
    hwnd: HWND,
    config: Config,
    grid: Grid,
    /// Last desktop actually observed, separate from a queued switch or board selection.
    usage_cell: usage::Stay<String>,
    foreground: usage::Stay<isize>,
    usage_dirty: bool,
    usage_saved: Instant,
    minimap: Minimap,
    board: Board,
    /// Until when the board losing the keyboard is Windows' doing, not the
    /// user's (see `BOARD_SETTLES_MS`).
    board_settling: Instant,
    /// The window that had focus when the board opened, to give it back on cancel.
    focus_before_board: HWND,
    /// The desktop the board was last opened on. Opened again on it, the
    /// board comes back to the cell it was left on.
    board_opened_on: String,
    /// The cell gone to from the board and when, for as long as Windows may
    /// still go back on it.
    going: Option<(String, Instant)>,
    settings: Settings,
    /// The window showing how Flick is used; none in pictures drawn offscreen.
    guide: Option<Guide>,
    /// The settings window is waiting for the new trigger to be pressed.
    capturing: bool,
    /// Set to run as administrator (`admin`), and to start with Windows.
    /// Kept here because asking Windows for either takes a moment.
    admin: bool,
    autostart: bool,
    /// When the config file that is in use was written.
    config_seen: Option<SystemTime>,
    /// Steps are arriving from a held trigger; `unsettled` is the desktop gone
    /// to when one of them left keyboard focus behind on the one it came from.
    in_gesture: bool,
    unsettled: Option<String>,
    /// The desktop Windows was last asked to switch to, until it tells of
    /// having done so, and the cell to go to after it. A switch asked for
    /// while another is under way makes Windows show the one before again
    /// once it is there (a fast flick past a cell): so one at a time.
    in_flight: Option<String>,
    queued: Option<String>,
    /// The trigger was let go of with a switch under way: the keyboard goes
    /// to this cell once Windows is there.
    focus_on_arrival: Option<String>,
    /// The stretch of sideways travel going on (which hold, and which
    /// stretch of it) and the cell it began on: the hand is followed along
    /// the row from there (`slide`).
    sliding: Option<((u32, u8), String)>,
    /// The hold going up and down, and the row it began on.
    rowing: Option<(u32, usize)>,
    /// The picture of the desktops following the hand (`glide`), and the
    /// hold it follows.
    glide: Glide,
    gliding: Option<Gliding>,
    /// Windows that follow the user from cell to cell inside their row.
    /// Window handles do not outlive a session, so this is not saved.
    following: vd::WindowPins,
    /// A pin menu to show once the current handler has returned.
    pending_menu: Option<(HWND, PinState)>,
    /// Likewise the menu of a shelf in the board.
    pending_row_menu: Option<usize>,
    /// When each cell was last shown (cells not in here count from
    /// `started`), and the cells of rows put to sleep. Both only matter with
    /// `sleep_after_minutes` set.
    seen: HashMap<String, Instant>,
    asleep: HashSet<String>,
    started: Instant,
    /// (old id, new id) of each change of the current desktop, from `listen`.
    events: mpsc::Receiver<(String, String)>,
    _listener: Option<vdapi::DesktopEventThread>,
    /// The message Explorer broadcasts when the taskbar is (re)created.
    taskbar_created: u32,
}

/// A hold the screen follows (`input::followed`).
struct Gliding {
    hold: u32,
    fallback: Option<follow::Follow>,
    /// The cell the picture counts from: where the hold began, or where the
    /// app went since a way the hand is not followed.
    origin: String,
    /// The cell it settled on, until Windows is there.
    landing: Option<String>,
    /// For steps (hotkeys), not holds: the desktop stepped to, and whether
    /// the active window goes along.
    step: Option<(i32, i32)>,
    carry: bool,
}

/// Steps are numbered apart from holds (`Gliding::hold`).
static STEPS: AtomicU32 = AtomicU32::new(0x8000_0000);

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    /// The windows of the board and of the settings, readable without
    /// borrowing the app.
    static BOARD_HWND: Cell<isize> = const { Cell::new(0) };
    static SETTINGS_HWND: Cell<isize> = const { Cell::new(0) };
    static GUIDE_HWND: Cell<isize> = const { Cell::new(0) };
    /// The app's message window, for the hook told of windows coming to the front.
    static MAIN_HWND: Cell<isize> = const { Cell::new(0) };
}

/// Told by Windows of each window that comes to the front. It brings that
/// window above the others kept on top whatever kind it is, so the order is
/// put back once the app is free.
unsafe extern "system" fn foreground_changed(
    _hook: HWINEVENTHOOK,
    _event: u32,
    hwnd: HWND,
    _object: i32,
    _child: i32,
    _thread: u32,
    _time: u32,
) {
    let _ = PostMessageW(HWND(MAIN_HWND.get()), WM_RESTACK, WPARAM(hwnd.0 as usize), LPARAM(0));
}

/// Runs `f` on the app unless it is already borrowed, which happens when a
/// handler pumps messages (menus, message boxes); such re-entrant events are
/// simply dropped.
fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|app| app.try_borrow_mut().ok()?.as_mut().map(f))
}

/// While a window run as administrator has the keyboard, Windows gives
/// Flick's hook, and its raw input as well, nothing of the trigger: it does
/// nothing until another window is clicked. Unless Flick runs as
/// administrator itself, that is logged each time and told once a run.
fn admin_in_front(window: HWND) {
    thread_local! {
        static TOLD: Cell<bool> = const { Cell::new(false) };
    }
    if crate::admin::elevated() || !input::above_us(window) {
        return;
    }
    // One whose program cannot even be named is likely the system's own.
    let Some(program) = vd::exe_of_window(window) else { return };
    log(&format!("a window run as administrator ({program}) is in front: the trigger cannot be heard until another window has the keyboard"));
    if !TOLD.replace(true) {
        let (title, text) = crate::guide::words().admin_front;
        tray::notify(HWND(MAIN_HWND.get()), title, text);
    }
}

pub fn log(message: &str) {
    use std::io::Write;
    let path = config::config_path().with_file_name("log.txt");
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let _ = writeln!(file, "{secs} {message}");
    }
}

/// Uses an independent thread so an unresponsive UI can still leave evidence.
fn watch_ui(hwnd: HWND) {
    let target = hwnd.0;
    let started = std::thread::Builder::new().name("UI watchdog".into()).spawn(move || {
        let mut delayed = false;
        loop {
            std::thread::sleep(Duration::from_secs(2));
            let hwnd = HWND(target);
            if !unsafe { IsWindow(hwnd) }.as_bool() {
                return;
            }
            let answering = unsafe {
                SendMessageTimeoutW(hwnd, WM_NULL, WPARAM(0), LPARAM(0), SMTO_ABORTIFHUNG | SMTO_BLOCK, 2000, None).0 != 0
            };
            if !unsafe { IsWindow(hwnd) }.as_bool() {
                return;
            }
            if !answering && !delayed {
                log(&format!("UI did not answer within 2 s; last message {:#x}", UI_MESSAGE.load(Ordering::Relaxed)));
            } else if answering && delayed {
                log("UI is answering again");
            }
            delayed = !answering;
        }
    });
    if let Err(error) = started {
        log(&format!("starting the UI watchdog failed: {error}"));
    }
}

/// Logs `text` and shows it in a message box: for what the user must know
/// about, such as why the app will not start.
pub fn warn(text: &str) {
    log(text);
    unsafe {
        MessageBoxW(None, &HSTRING::from(text), &HSTRING::from(config::APP_NAME), MB_OK | MB_ICONWARNING);
    }
}

fn window_model(follows: bool, pin_from_app: bool, window: vd::WindowInfo) -> WindowModel {
    WindowModel {
        follows,
        pin_from_app,
        app: vd::exe_name(window.pid).unwrap_or_default(),
        icon_key: crate::board::icon_key(window.hwnd),
        app_name: vd::app_label(window.pid),
        hwnd: window.hwnd,
        rect: window.rect,
        minimized: window.minimized,
        topmost: vd::is_topmost(window.hwnd),
        title: window.title,
    }
}

impl App {
    fn new(
        hwnd: HWND,
        board: Board,
        settings: Settings,
        events: mpsc::Receiver<(String, String)>,
        listener: Option<vdapi::DesktopEventThread>,
    ) -> App {
        let (grid, error) = config::load_grid();
        if let Some(error) = error {
            log(&format!("state.json could not be read, starting from the desktops as they are: {error}"));
        }
        App {
            hwnd,
            config: Config::default(),
            grid,
            usage_cell: usage::Stay::default(),
            foreground: usage::Stay::default(),
            usage_dirty: false,
            usage_saved: Instant::now(),
            minimap: Minimap::spawn(),
            board,
            board_settling: Instant::now(),
            focus_before_board: HWND(0),
            board_opened_on: String::new(),
            going: None,
            settings,
            guide: None,
            capturing: false,
            admin: false,
            autostart: false,
            config_seen: None,
            in_gesture: false,
            unsettled: None,
            in_flight: None,
            queued: None,
            focus_on_arrival: None,
            sliding: None,
            rowing: None,
            glide: Glide::spawn(hwnd),
            gliding: None,
            following: vd::WindowPins::default(),
            pending_menu: None,
            pending_row_menu: None,
            seen: HashMap::new(),
            asleep: HashSet::new(),
            started: Instant::now(),
            events,
            _listener: listener,
            taskbar_created: unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) },
        }
    }

    // ---- the grid and the desktops ------------------------------------------

    /// Reads the real desktops and brings the grid in line with them.
    fn sync(&mut self) -> Option<Desktops> {
        let desktops = Desktops::read()?;
        // Both must run: a row made just now is still to be named.
        let synced = self.grid.sync(&desktops.ids(), &desktops.current);
        if self.grid.name_unnamed() || synced {
            self.save();
        }
        Some(desktops)
    }

    fn save(&mut self) {
        let now = usage::now();
        self.grid.prune_usage(now);
        self.grid.window_usage.retain(|hwnd, score| {
            unsafe { IsWindow(HWND(*hwnd)) }.as_bool() && score.value(now, usage::WINDOW_HALF_LIFE) > 0.0
        });
        match config::save_grid(&self.usage_grid()) {
            Ok(()) => {
                self.usage_dirty = false;
                self.usage_saved = Instant::now();
            }
            Err(e) => log(&format!("saving the grid failed: {e}")),
        }
    }

    /// Includes ongoing stays in a snapshot, leaving completed history alone
    /// so repeated saves and layouts cannot credit a stay twice.
    fn usage_grid(&self) -> Grid {
        let mut grid = self.grid.clone();
        let now = usage::now();
        if let Some(id) = &self.usage_cell.current {
            let credit = self.usage_cell.credit(usage::CELL_TAU);
            if credit > 0.0 && grid.find(id).is_some() {
                grid.usage.entry(id.clone()).or_default().visits.record(now, credit, usage::CELL_HALF_LIFE);
            }
        }
        if let Some(hwnd) = self.foreground.current {
            let credit = self.foreground.credit(usage::WINDOW_TAU);
            if credit > 0.0 && unsafe { IsWindow(HWND(hwnd)) }.as_bool() {
                grid.window_usage.entry(hwnd).or_default().record(now, credit, usage::WINDOW_HALF_LIFE);
            }
        }
        grid
    }

    /// Starts a stay only after a settled desktop was actually observed.
    fn record_arrival(&mut self, id: &str) {
        if self.grid.find(id).is_none() {
            return;
        }
        if let Some((previous, active)) = self.usage_cell.switch(Some(id.to_owned()), self.started.elapsed().as_millis() as u64) {
            let credit = usage::credit(active, usage::CELL_TAU);
            if credit > 0.0 && self.grid.find(&previous).is_some() {
                self.grid.usage.entry(previous).or_default().visits.record(usage::now(), credit, usage::CELL_HALF_LIFE);
                self.usage_dirty = true;
            }
        }
    }

    /// Foreground stays belong to individual handles. Flick, the shell and
    /// any foreground while the board is open end the previous window stay.
    fn record_foreground(&mut self, hwnd: HWND) {
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)); }
        let current = (!self.board.is_open() && pid != unsafe { GetCurrentProcessId() } && vd::is_app_window(hwnd))
            .then_some(hwnd.0);
        if let Some((previous, active)) = self.foreground.switch(current, self.started.elapsed().as_millis() as u64) {
            let credit = usage::credit(active, usage::WINDOW_TAU);
            if credit > 0.0 && unsafe { IsWindow(HWND(previous)) }.as_bool() {
                self.grid.window_usage.entry(previous).or_default().record(usage::now(), credit, usage::WINDOW_HALF_LIFE);
            }
        }
    }

    /// Samples activity on the existing two-second config timer only.
    fn usage_tick(&mut self) {
        let mut input = LASTINPUTINFO { cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32, ..Default::default() };
        let input_age = unsafe { GetLastInputInfo(&mut input) }.as_bool()
            .then(|| unsafe { GetTickCount() }.wrapping_sub(input.dwTime));
        self.record_foreground(unsafe { GetForegroundWindow() });
        let millis = self.started.elapsed().as_millis() as u64;
        self.usage_dirty |= self.usage_cell.tick(millis, input_age);
        self.foreground.tick(millis, input_age);
    }

    /// Saves the grid and reorders Windows' own desktop list to match it
    /// (row after row), so Win+Ctrl+arrows and Task View agree with the grid.
    fn commit(&mut self) {
        self.save();
        let Some(desktops) = Desktops::read() else { return };
        let wanted: Vec<&String> = self.grid.rows.iter().flat_map(|row| &row.cells).collect();
        let mut actual = desktops.ids();
        if wanted.len() != actual.len() {
            return;
        }
        for (i, id) in wanted.into_iter().enumerate() {
            if actual[i] == *id {
                continue;
            }
            let (Some(desktop), Some(from)) = (desktops.get(id), actual.iter().position(|a| a == id)) else { return };
            if let Err(e) = vdapi::move_desktop(desktop, i as u32) {
                return log(&format!("move_desktop failed: {e:?}"));
            }
            let moved = actual.remove(from);
            actual.insert(i, moved);
        }
    }

    /// Records that `id` is being shown: per-row memory, idle clock, and its
    /// row is awake again.
    fn visit(&mut self, id: &str) {
        self.grid.visit(id);
        self.seen.insert(id.to_owned(), Instant::now());
        if let Some(pos) = self.grid.find(id) {
            for cell in &self.grid.rows[pos.row].cells {
                self.asleep.remove(cell);
            }
        }
    }

    /// Switches to a desktop without Windows' own name label popping up.
    /// Returns whether that worked.
    fn switch(&self, target: vdapi::Desktop) -> bool {
        vd::set_switch_label_hidden(true);
        unsafe {
            SetTimer(self.hwnd, LABEL_TIMER_ID, LABEL_HIDE_MS, None);
        }
        vdapi::switch_desktop(target).map_err(|e| log(&format!("switch_desktop failed: {e:?}"))).is_ok()
    }

    /// Creates a desktop and returns its id. The caller places it in the
    /// grid before the next `sync`, which would file a desktop it does not
    /// know under the current row.
    fn create_desktop(&self) -> Option<String> {
        vdapi::create_desktop().map_err(|e| log(&format!("create_desktop failed: {e:?}"))).ok().map(|d| d.id())
    }

    /// What the minimap shows with the user on `current`.
    fn view(&self, current: &str) -> View {
        let cur = self.grid.find(current);
        let grid = self.usage_grid();
        let now = usage::now();
        View {
            rows: self
                .grid
                .rows
                .iter()
                .map(|row| row.cells.iter().map(|id| CellView { emphasised: self.grid.is_emphasised(id), usage: grid.cell_usage(id, now) }).collect())
                .collect(),
            anchors: self.grid.rows.iter().map(Row::anchor).collect(),
            cur,
            title: cur.map_or(String::new(), |pos| grid::row_title(&self.grid.rows[pos.row].name, pos.row)),
        }
    }

    // ---- windows that follow --------------------------------------------------

    fn follows(&self, window: &vd::WindowInfo) -> bool {
        self.following.contains(&window.hwnd.0)
            || (!self.grid.follow_apps.is_empty()
                && vd::exe_name(window.pid).is_some_and(|exe| self.grid.follow_apps.contains(&exe)))
    }

    /// Brings the windows that follow within a row over to cell `to` from the
    /// other cells of its row.
    fn bring_followers(&mut self, desktops: &Desktops, to: &str) {
        if self.following.is_empty() && self.grid.follow_apps.is_empty() {
            return;
        }
        let (Some(pos), Some(target)) = (self.grid.find(to), desktops.get(to)) else { return };
        let row = &self.grid.rows[pos.row].cells;
        if row.len() < 2 {
            return;
        }
        for window in vd::windows(&desktops.current) {
            if window.desktop != to && row.contains(&window.desktop) && self.follows(&window) {
                if let Err(error) = vdapi::move_window_to_desktop(target, window.hwnd) {
                    log(&format!("moving shelf window {:x} to {to} failed: {error:?}", window.hwnd.0));
                }
            }
        }
    }

    fn pin_state(&self, hwnd: HWND) -> PinState {
        PinState {
            row_window: self.following.contains(&hwnd.0),
            row_app: vd::exe_of_window(hwnd).is_some_and(|exe| self.grid.follow_apps.contains(&exe)),
            all_window: vdapi::is_pinned_window(hwnd).unwrap_or(false),
            all_app: vdapi::is_pinned_app(hwnd).unwrap_or(false),
        }
    }

    /// The menu and board both choose one exclusive scope; app choices stay explicit.
    fn apply_pin(&mut self, hwnd: HWND, command: PinCommand) {
        let state = self.pin_state(hwnd);
        match command {
            PinCommand::RowWindow | PinCommand::AllWindow => {
                let current = crate::pin::Pin::from_flags(state.row_window, state.all_window);
                let choice = if command == PinCommand::RowWindow { crate::pin::Pin::Row } else { crate::pin::Pin::Everywhere };
                let (row, all) = current.choose(choice).flags();
                self.set_pin(hwnd, row, all);
            }
            PinCommand::RowApp | PinCommand::AllApp => {
                let Some(exe) = vd::exe_of_window(hwnd) else { return };
                let current = crate::pin::Pin::from_flags(state.row_app, state.all_app);
                let choice = if command == PinCommand::RowApp { crate::pin::Pin::Row } else { crate::pin::Pin::Everywhere };
                let (row, all) = current.choose(choice).flags();
                let result = if all { vdapi::pin_app(hwnd) } else if state.all_app { vdapi::unpin_app(hwnd) } else { Ok(()) };
                if let Err(e) = result { log(&format!("app pin change failed: {e:?}")); return; }
                self.grid.follow_apps.retain(|e| *e != exe);
                if row { self.grid.follow_apps.push(exe); }
            }
        }
        self.save();
        self.refresh_board();
    }

    /// Window scope never changes an app-wide pin or the separate topmost setting.
    fn set_pin(&mut self, window: HWND, row: bool, all: bool) {
        let state = self.pin_state(window);
        let requested = crate::pin::Pin::from_flags(row, all);
        if state.window_change(requested).is_none() {
            tray::notify(self.hwnd, "Flick", crate::i18n::text(
                "This pin comes from the app. Change App in the pin menu (Ctrl+Alt+Win+P).",
                "앱에서 상속된 고정입니다. 고정 메뉴(Ctrl+Alt+Win+P)에서 앱 항목을 바꾸세요.",
                "此固定来自应用。请在固定菜单（Ctrl+Alt+Win+P）中更改应用选项。"));
            return;
        }
        // Tag first so failure cannot leave a shell pin removed without its replacement.
        if row && !self.following.insert(window.0) {
            log("could not identify the window for its shelf pin");
            return;
        }
        let result = if all { vdapi::pin_window(window) }
            else if state.all_window { vdapi::unpin_window(window) } else { Ok(()) };
        if let Err(e) = result {
            if row && !state.row_window { self.following.remove(&window.0); }
            log(&format!("pin change failed: {e:?}"));
            return;
        }
        if !row { self.following.remove(&window.0); }
        self.restack();
    }

    /// Puts the windows kept on top in their order: the board while it is up
    /// over all, then the ones pinned to every desktop, the ones pinned to
    /// their row, and the ones only kept on top (`stack`). Windows kept on
    /// top by other means, and pinned ones whose tile turned that off, are
    /// left where they are.
    fn restack(&self) {
        self.restack_clicked(HWND::default());
    }

    /// The same, with `clicked` counted as the last one clicked: Windows tells
    /// of a new foreground window before it has raised it.
    fn restack_clicked(&self, clicked: HWND) {
        let me = unsafe { GetCurrentProcessId() };
        let mut now: Vec<(HWND, Kind)> = vd::topmost_windows()
            .into_iter()
            .filter_map(|hwnd| {
                let mut pid = 0;
                unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
                let kind = if pid == me {
                    Kind::Flick
                } else if !vd::is_app_window(hwnd) {
                    return None;
                } else if vd::pinned_everywhere(hwnd) {
                    Kind::All
                } else if self.following.contains(&hwnd.0)
                    || (!self.grid.follow_apps.is_empty()
                        && vd::exe_name(pid).is_some_and(|exe| self.grid.follow_apps.contains(&exe)))
                {
                    Kind::Row
                } else if self.grid.kept_on_top.iter().any(|(kept, _)| *kept == hwnd.0) {
                    Kind::Top
                } else {
                    return None;
                };
                Some((hwnd, kind))
            })
            .collect();
        let seen = now.clone();
        if let Some(at) = now.iter().position(|(hwnd, _)| *hwnd == clicked) {
            let it = now.remove(at);
            now.insert(0, it);
        }
        let order = stack::stacked(&now);
        if order.iter().ne(seen.iter().map(|(hwnd, _)| hwnd)) {
            debug_log!("restack: {:x?}", order.iter().map(|h| h.0).collect::<Vec<_>>());
            vd::stack(&order);
        }
    }

    /// Puts a window above the others or takes it down again, and keeps
    /// note of the ones put up here.
    fn set_top(&mut self, window: HWND, on: bool) {
        if !vd::set_topmost(window, on) {
            log("a window could not be put above the others or taken down");
            return self.board.close_refused(window);
        }
        self.grid.kept_on_top.retain(|(hwnd, _)| *hwnd != window.0);
        if on {
            self.grid.kept_on_top.push((window.0, vd::exe_of_window(window).unwrap_or_default()));
        }
        self.restack();
    }

    /// Forgets the windows put above the others that are gone (a handle may
    /// by now be another app's window).
    fn forget_gone_windows(&mut self) {
        self.following.prune();
        self.grid.kept_on_top.retain(|(hwnd, exe)| {
            unsafe { IsWindow(HWND(*hwnd)) }.as_bool() && vd::exe_of_window(HWND(*hwnd)).unwrap_or_default() == *exe
        });
    }

    // ---- sleeping rows --------------------------------------------------------

    /// Pushes the memory of rows nobody visited for a while out of RAM.
    fn sleep_idle_rows(&mut self) {
        let minutes = self.config.sleep_after_minutes;
        if minutes == 0 {
            return;
        }
        let Some(desktops) = self.sync() else { return };
        let limit = Duration::from_secs(minutes as u64 * 60);
        let current_row = self.grid.find(&desktops.current).map(|pos| pos.row);
        let idle: Vec<Vec<String>> = self
            .grid
            .rows
            .iter()
            .enumerate()
            .filter(|(r, row)| {
                Some(*r) != current_row
                    && !row.cells.iter().all(|c| self.asleep.contains(c))
                    && row.cells.iter().all(|c| self.seen.get(c).unwrap_or(&self.started).elapsed() >= limit)
            })
            .map(|(_, row)| row.cells.clone())
            .collect();
        if idle.is_empty() {
            return;
        }
        let windows = vd::windows(&desktops.current);
        let parents = vd::process_parents();
        // An app with a window on a row that is awake must stay untouched.
        let awake: HashSet<u32> = windows
            .iter()
            .filter(|w| !self.asleep.contains(&w.desktop) && !idle.iter().any(|cells| cells.contains(&w.desktop)))
            .map(|w| w.pid)
            .chain(vd::pinned_windows().iter().map(|w| w.pid))
            .collect();
        let keep: HashSet<u32> = vd::with_descendants(&awake, &parents).into_iter().collect();
        for cells in idle {
            let roots: HashSet<u32> = windows.iter().filter(|w| cells.contains(&w.desktop)).map(|w| w.pid).collect();
            let pids: Vec<u32> =
                vd::with_descendants(&roots, &parents).into_iter().filter(|pid| !keep.contains(pid)).collect();
            vd::trim(&pids);
            self.asleep.extend(cells);
        }
    }

    // ---- moving about ---------------------------------------------------------

    fn carry_held(&self) -> bool {
        let key = match self.config.carry_modifier.trim().to_ascii_lowercase().as_str() {
            "ctrl" => VK_CONTROL,
            "alt" => VK_MENU,
            _ => VK_SHIFT,
        };
        unsafe { GetAsyncKeyState(key.0 as i32) < 0 }
    }

    /// Moves one cell in `dir`. With `carry` the foreground window comes
    /// along. Returns false when there is no cell that way.
    fn step(&mut self, dir: Dir, carry: bool) -> bool {
        if self.board.is_open() {
            // Row anchors change now; the desktop changes when confirmed.
            debug_log!("app: {dir:?} moves the selection, the board being open");
            if let Some(from) = self.grid.find(&self.board.selected()) {
                if let Some(id) = self.grid.target(from, dir).and_then(|pos| self.grid.id_at(pos).cloned()) {
                    self.select_board(id);
                }
            }
            return true;
        }
        if self.glide_step(dir, carry) {
            return true;
        }
        let Some(desktops) = self.sync() else { return true };
        let heading = self.heading(&desktops);
        let Some(from) = self.grid.find(&heading) else { return true };
        match self.grid.target(from, dir).and_then(|pos| self.grid.id_at(pos).cloned()) {
            Some(to) => {
                debug_log!("app: {dir:?} from row {} cell {}", from.row, from.col);
                self.go(&desktops, from, &to, carry);
                true
            }
            None => {
                // The edge of the grid: the minimap shows where one is and
                // nothing else happens. Cells are only ever added in the
                // board, with its + buttons.
                debug_log!("app: {dir:?} from row {} cell {}: nothing that way", from.row, from.col);
                let view = self.view(&heading);
                self.minimap.show(view, None);
                false
            }
        }
    }

    /// The held trigger is travelling sideways, and with `to` the hand has
    /// come to that cell, counted from the one this stretch of the hold
    /// (`key`) began on: the app goes there, in one go however many cells
    /// away it is, or to the end of the row where the row is shorter.
    /// Without `to` the minimap comes up, to follow the hand before any cell
    /// is come to. Returns the cell the app is on, counted likewise, and how
    /// many cells the row has to the left and to the right of the first.
    fn slide(&mut self, key: (u32, u8), to: Option<i32>, carry: bool) -> Option<(i32, (usize, usize))> {
        let began = self.sliding.take().filter(|(of, _)| *of == key).map(|(_, origin)| origin);
        let (origin, on, room) = if self.board.is_open() {
            self.board.follow_hand();
            let origin = began.unwrap_or_else(|| self.board.selected());
            let (on, room) = match to {
                Some(to) => {
                    let along = self.grid.along(&origin, to)?;
                    self.select_board(self.grid.id_at(along.0)?.clone());
                    along
                }
                None => self.grid.along(&origin, 0)?,
            };
            debug_log!("app: the selection slides to {to:?}, the board being open");
            (origin, on, room)
        } else {
            let desktops = self.sync()?;
            let heading = self.heading(&desktops);
            let from = self.grid.find(&heading)?;
            let origin = began.filter(|origin| self.grid.find(origin).is_some()).unwrap_or_else(|| heading.clone());
            let (on, room) = match to {
                Some(to) => self.grid.along(&origin, to)?,
                None => (from, self.grid.along(&origin, 0)?.1),
            };
            match self.grid.id_at(on).cloned() {
                Some(id) if on != from => {
                    debug_log!("app: slide to {to:?}: from row {} cell {} to cell {}", from.row, from.col, on.col);
                    self.go(&desktops, from, &id, carry);
                    self.glide_moved(key.0, &id);
                }
                // Nothing to go to: before the first cell, or pushing at the
                // end of the row. The minimap shows where one is. Cells are
                // only ever added in the board, with its + buttons.
                _ => {
                    debug_log!("app: slide to {to:?}: staying on row {} cell {}", from.row, from.col);
                    let view = self.view(&heading);
                    self.minimap.show(view, None);
                }
            }
            (origin, on, room)
        };
        self.sliding = Some((key, origin));
        Some((on.col as i32 - room.0 as i32, room))
    }

    /// The held trigger has come to row `to`, counted from the one the hold
    /// began on: the app goes there, to the cell a move there lands on, in
    /// one go however many rows away it is, or to the first or last row.
    /// Returns the row the app is on, counted likewise, and how many rows
    /// there are above and below the first.
    fn rows(&mut self, hold: u32, to: i32, carry: bool) -> Option<(i32, (usize, usize))> {
        let began = self.rowing.take().filter(|(of, _)| *of == hold).map(|(_, row)| row);
        let (on, room) = if self.board.is_open() {
            self.board.follow_hand();
            let origin = began.or_else(|| self.grid.find(&self.board.selected()).map(|pos| pos.row))?;
            debug_log!("app: the selection goes to row {to:+}, the board being open");
            let across = self.grid.across(origin, to)?;
            self.select_board(self.grid.id_at(across.0)?.clone());
            across
        } else {
            let desktops = self.sync()?;
            let heading = self.heading(&desktops);
            let from = self.grid.find(&heading)?;
            let origin = began.unwrap_or(from.row);
            let (on, room) = self.grid.across(origin, to)?;
            match self.grid.id_at(on).cloned() {
                Some(id) if on.row != from.row => {
                    debug_log!("app: rows to {to:+}: from row {} to row {} cell {}", from.row, on.row, on.col);
                    self.go(&desktops, from, &id, carry);
                    self.glide_moved(hold, &id);
                }
                // Above the first row or below the last: the minimap shows
                // where one is.
                _ => {
                    debug_log!("app: rows to {to:+}: staying on row {}", from.row);
                    let view = self.view(&heading);
                    self.minimap.show(view, None);
                }
            }
            (on, room)
        };
        // The row it began on, as found in the grid (`Grid::across`).
        self.rowing = Some((hold, room.0));
        Some((on.row as i32 - room.0 as i32, room))
    }

    /// Switches from the cell at `from` to cell `to` and shows the minimap.
    fn go(&mut self, desktops: &Desktops, from: Pos, to: &str, carry: bool) {
        let Some(target) = desktops.get(to) else { return };
        self.going = None;
        let window = unsafe { GetForegroundWindow() };
        let carried = carry
            && vd::is_app_window(window)
            && vdapi::is_window_on_current_desktop(window).unwrap_or(false)
            && vdapi::move_window_to_desktop(target, window).is_ok();
        self.bring_followers(desktops, to);
        if self.in_flight.is_some() {
            debug_log!("app: {} waits for the switch under way", &to[..4.min(to.len())]);
            self.queued = Some(to.to_owned());
        } else if self.switch(target) {
            self.take_off(to);
        } else {
            return;
        }
        self.visit(to);
        // While Windows is still on its way elsewhere, focusing a window of
        // this cell would be a switch of its own: that waits until it is here.
        let waiting = self.queued.as_deref() == Some(to);
        if carried && !waiting {
            vd::force_foreground(window);
        } else if self.in_gesture {
            // Mid-gesture more steps may follow at once; handing focus over
            // and saving wait until the trigger is let go (`finish_gesture`).
            self.unsettled = Some(to.to_owned());
        } else if waiting {
            self.focus_on_arrival = Some(to.to_owned());
        } else {
            vd::focus_top_window();
        }
        if !self.in_gesture {
            self.save();
        }
        let view = self.view(to);
        self.minimap.show(view, Some(from));
    }

    /// The cell the app is on or on its way to: moves are counted from there.
    fn heading(&self, desktops: &Desktops) -> String {
        self.queued.clone().or_else(|| self.in_flight.clone()).unwrap_or_else(|| desktops.current.clone())
    }

    /// Windows was just asked to switch to `to`.
    fn take_off(&mut self, to: &str) {
        self.usage_cell.suspend();
        self.in_flight = Some(to.to_owned());
        unsafe {
            SetTimer(self.hwnd, FLIGHT_TIMER_ID, FLIGHT_MS, None);
        }
    }

    /// Windows is on `new` (or is taken to be, `FLIGHT_MS` having passed):
    /// the switch under way is over, and the cell waiting behind it is
    /// switched to. True when that is under way, the cell come to only
    /// being passed by.
    fn land(&mut self, new: &str) -> bool {
        if self.in_flight.take().is_none() {
            return false;
        }
        unsafe {
            let _ = KillTimer(self.hwnd, FLIGHT_TIMER_ID);
        }
        if self.queued.as_deref().is_some_and(|next| next != new) {
            // Windows tells of a switch as it starts it. With a window of
            // this cell holding the keyboard (one of Chromium's, as Orca's
            // and the browsers' are), the next asked for before it is done
            // showing this one brings it back here after.
            self.in_flight = Some(new.to_owned());
            let wait = if keyboard_held_on(new) { onward_ms() } else { 0 };
            debug_log!("app: passing {}, on in {wait}ms", &new[..4.min(new.len())]);
            if wait == 0 {
                self.go_on();
            } else {
                unsafe {
                    SetTimer(self.hwnd, ONWARD_TIMER_ID, wait, None);
                }
            }
            return true;
        }
        self.queued = None;
        if let Some(to) = self.focus_on_arrival.take() {
            if to == new {
                vd::focus_top_window_of(&to);
            }
            // Windows may yet go back, or be elsewhere: `desktop_changed`
            // goes there again.
            self.going = Some((to, Instant::now()));
        }
        false
    }

    /// Windows is done showing the cell passed by (`land`): on to the one
    /// waiting behind it.
    fn go_on(&mut self) {
        let Some(passed) = self.in_flight.clone() else { return };
        if self.queued.as_deref().map_or(true, |next| next == passed) {
            // The hand came back meanwhile: this is where it stays.
            self.land(&passed);
            self.land_failed(&passed);
            return;
        }
        self.in_flight = None;
        let Some(next) = self.queued.take() else { return };
        if let Some(desktop) = Desktops::read().and_then(|d| d.get(&next)) {
            debug_log!("app: on to {}", &next[..4.min(next.len())]);
            if self.switch(desktop) {
                return self.take_off(&next);
            }
        }
        self.land_failed(&passed);
    }

    /// The switch on from a cell passed by was not made: the app stays there.
    fn land_failed(&mut self, passed: &str) {
        self.focus_on_arrival = None;
        if let Some(desktops) = self.sync() {
            self.record_arrival(&desktops.current);
            self.visit(passed);
            let view = self.view(&desktops.current);
            self.minimap.update(view);
        }
    }

    /// The trigger was released: do what the steps of the gesture put off.
    fn finish_gesture(&mut self) {
        if std::mem::take(&mut self.in_gesture) {
            // Windows may still be on its way there, a slow switch behind:
            // a window of the desktop it shows yet would take it back.
            if let Some(to) = self.unsettled.take() {
                if self.in_flight.is_some() {
                    // Focusing a window of a cell Windows is not on yet
                    // would be one more switch on top of the one under way.
                    self.focus_on_arrival = Some(to);
                } else {
                    vd::focus_top_window_of(&to);
                    self.going = Some((to, Instant::now()));
                }
            }
            self.save();
        }
    }

    // ---- the screen following the hand ----------------------------------------

    /// The cell `to` (sideways, up and down) from `origin`, as a picture of a
    /// hold counts them: along a row from the cell a move there lands on.
    fn glide_cell(&self, origin: &str, (col, row): (i32, i32)) -> Option<String> {
        let from = self.grid.find(origin)?;
        let line = self.grid.rows.get(usize::try_from(from.row as i32 + row).ok()?)?;
        let reference = if row == 0 { from.col } else { line.anchor() };
        line.cells.get(usize::try_from(reference as i32 + col).ok()?).cloned()
    }

    /// What hold `hold` shows going `ways` (`input::FOLLOW_X` and the like)
    /// from `origin`: the desktops it can go to, and their windows.
    fn glide_scene(&self, desktops: &Desktops, origin: &str, hold: u32, ways: u32) -> Option<Scene> {
        let from = self.grid.find(origin)?;
        let (sideways, across) = (ways & input::FOLLOW_X != 0, ways & input::FOLLOW_Y != 0);
        let last = self.grid.rows.len() as i32 - 1;
        let rows = if across { (-(from.row as i32), last - from.row as i32) } else { (0, 0) };
        let mut cols = Vec::new();
        let mut cells: HashMap<&str, (i32, i32)> = HashMap::new();
        for row in rows.0..=rows.1 {
            let line = &self.grid.rows[(from.row as i32 + row) as usize];
            let reference = if row == 0 { from.col } else { line.anchor() } as i32;
            let span = if sideways { (-reference, line.cells.len() as i32 - 1 - reference) } else { (0, 0) };
            cols.push(span);
            for col in span.0..=span.1 {
                if let Some(id) = line.cells.get((reference + col) as usize) {
                    cells.insert(id, (row, col));
                }
            }
        }
        // A window carried along stays where it is, as one on every desktop.
        let carried = self.carry_held().then(|| unsafe { GetForegroundWindow() });
        let mut shots: Vec<(i32, Shot)> = Vec::new();
        for window in vd::shown_windows(&desktops.current) {
            let place = if window.desktop.is_empty() || carried == Some(window.hwnd) {
                Place::Fixed
            } else if self.follows(&window) {
                // A shelf's last cell can differ from its anchor while the board is open.
                let Some(pos) = self.grid.find(&window.desktop) else { continue };
                let row = pos.row as i32 - from.row as i32;
                if row < rows.0 || row > rows.1 { continue; }
                Place::Row(row)
            } else {
                let Some(&(row, col)) = cells.get(window.desktop.as_str()) else { continue };
                Place::Cell(row, col)
            };
            let far = match place {
                Place::Fixed => 0,
                Place::Row(row) => 2 * row.abs(),
                Place::Cell(row, col) => 2 * row.abs() + col.abs(),
            };
            shots.push((far, Shot { hwnd: window.hwnd, rect: window.rect, place }));
        }
        // The nearest within the limit, in their order, bottom first.
        let mut nearest: Vec<usize> = (0..shots.len()).collect();
        nearest.sort_by_key(|&at| (shots[at].0, at));
        let kept: HashSet<usize> = nearest.into_iter().take(glide::MOST_THUMBNAILS).collect();
        let shots = shots.into_iter().enumerate().filter(|(at, _)| kept.contains(at)).map(|(_, (_, shot))| shot).rev().collect();
        let names = (rows.0..=rows.1)
            .map(|row| {
                let at = (from.row as i32 + row) as usize;
                crate::grid::row_title(&self.grid.rows[at].name, at)
            })
            .collect();
        Some(Scene { hold, origin: from, anchors: self.grid.rows.iter().map(|r| r.anchor()).collect(), rows, cols, shots, names })
    }

    /// Hold `hold` began to move: the picture follows it the ways it is set
    /// to. A hold before it still settling goes where it was settling, now.
    fn glide_begin(&mut self, hold: u32) {
        let Some(saved) = input::follow_hold(hold) else { return };
        let hand = saved.hand;
        if hand.hold != hold || hand.ways & (input::FOLLOW_X | input::FOLLOW_Y) == 0 || self.board.is_open() {
            return;
        }
        if self.gliding.as_ref().is_some_and(|gliding| gliding.hold == hold) {
            return;
        }
        if let Some(old) = self.gliding.take() {
            self.glide_finish(old);
        }
        let Some(desktops) = self.sync() else { return };
        let origin = self.heading(&desktops);
        let Some(scene) = self.glide_scene(&desktops, &origin, hold, hand.ways) else { return };
        let fallback = follow::Follow::new(scene.rows, scene.cols.clone());
        self.glide.begin(scene);
        self.gliding = Some(Gliding { hold, fallback: Some(fallback), origin, landing: None, step: None, carry: false });
    }

    /// A glide still settling, or a step still under way, as a new one
    /// begins: it goes where it was going, now.
    fn glide_finish(&mut self, mut old: Gliding) {
        let saved = input::follow_hold(old.hold);
        input::forget_hold(old.hold);
        if old.landing.is_some() {
            return;
        }
        if let Some(to) = old.step.or_else(|| glide::decided(old.hold)).or_else(|| Some(old.fallback.as_mut()?.finish_path(&saved?.path))) {
            self.glide_go(&old.origin, to, old.carry);
        }
    }

    /// A step `dir` (a hotkey) with the screen following the hand that way:
    /// the screen recedes, slides there and comes forward, and Windows goes
    /// there after (`glide_settled`). Steps upon a step under way go on from
    /// it. False where the screen does not follow that way, or there is no
    /// desktop that way to start from.
    fn glide_step(&mut self, dir: Dir, carry: bool) -> bool {
        let by = match dir {
            Dir::Left => (-1, 0),
            Dir::Right => (1, 0),
            Dir::Up => (0, -1),
            Dir::Down => (0, 1),
        };
        let mouse = self.config.mouse;
        if !(if by.0 != 0 { mouse.follow_x } else { mouse.follow_y }) {
            return false;
        }
        if let Some((hold, origin, at)) = self.gliding.as_ref().and_then(|g| Some((g.hold, g.origin.clone(), g.step.filter(|_| g.landing.is_none())?))) {
            let to = (at.0 + by.0, at.1 + by.1);
            // At the edge nothing more happens; the step under way goes on.
            if let Some(id) = self.glide_cell(&origin, to) {
                if let Some(gliding) = &mut self.gliding {
                    gliding.step = Some(to);
                    gliding.carry |= carry;
                }
                self.glide.retarget(hold, to);
                let view = self.view(&id);
                self.minimap.update(view);
            }
            return true;
        }
        let Some(desktops) = self.sync() else { return false };
        let origin = self.heading(&desktops);
        let (Some(from), Some(id)) = (self.grid.find(&origin), self.glide_cell(&origin, by)) else { return false };
        if let Some(old) = self.gliding.take() {
            self.glide_finish(old);
        }
        let hold = STEPS.fetch_add(1, Ordering::SeqCst) | 0x8000_0000;
        let Some(scene) = self.glide_scene(&desktops, &origin, hold, input::FOLLOW_X | input::FOLLOW_Y) else { return false };
        self.glide.step(scene, by);
        self.gliding = Some(Gliding { hold, fallback: None, origin, landing: None, step: Some(by), carry });
        let view = self.view(&id);
        self.minimap.show(view, Some(from));
        true
    }

    /// The app went to `to` in hold `hold` a way the hand is not followed:
    /// the picture goes on from there.
    fn glide_moved(&mut self, hold: u32, to: &str) {
        let Some(ways) = self.gliding.as_ref().filter(|gliding| gliding.hold == hold).map(|_| input::followed().ways) else { return };
        let Some(desktops) = self.sync() else { return };
        if let Some(scene) = self.glide_scene(&desktops, to, hold, ways) {
            if let Some(gliding) = &mut self.gliding {
                if let Some(fallback) = &mut gliding.fallback {
                    if let Some(saved) = input::follow_hold(hold) {
                        fallback.finish_path(&saved.path);
                        fallback.reframe(scene.rows, scene.cols.clone(), (saved.hand.x, saved.hand.y), self.grid.find(&gliding.origin).is_some_and(|from| from.row != scene.origin.row));
                    }
                }
            }
            self.glide.rebase(scene);
        }
        if let Some(gliding) = &mut self.gliding {
            gliding.origin = to.to_owned();
        }
    }

    /// Hold `hold` was let go of and settles on `to`: the minimap goes there
    /// along with the picture.
    fn glide_target(&mut self, hold: u32, to: (i32, i32)) {
        let Some(origin) = self.gliding.as_ref().filter(|gliding| gliding.hold == hold).map(|gliding| gliding.origin.clone()) else { return };
        if let Some(id) = self.glide_cell(&origin, to) {
            let view = self.view(&id);
            self.minimap.update(view);
        }
    }

    /// Goes to the cell `to` from `origin`, as the end of a gesture does.
    /// The cell when that is another one.
    fn glide_go(&mut self, origin: &str, to: (i32, i32), carry: bool) -> Option<String> {
        let id = self.glide_cell(origin, to)?;
        let desktops = self.sync()?;
        let heading = self.heading(&desktops);
        let from = self.grid.find(&heading)?;
        if id == heading {
            return None;
        }
        debug_log!("app: the picture settled {to:?} from {}", &origin[..4.min(origin.len())]);
        self.in_gesture = true;
        self.go(&desktops, from, &id, carry || self.carry_held());
        self.finish_gesture();
        self.minimap.hide_after(LINGER_RELEASE_MS);
        Some(id)
    }

    /// The picture of hold `hold` settled on `to`: Windows goes there, and
    /// the picture goes once it is.
    fn glide_settled(&mut self, hold: u32, to: (i32, i32)) {
        input::forget_hold(hold);
        let Some((origin, carry)) = self.gliding.as_ref().filter(|gliding| gliding.hold == hold).map(|g| (g.origin.clone(), g.carry)) else { return };
        match self.glide_go(&origin, to, carry) {
            Some(id) => {
                if let Some(gliding) = &mut self.gliding {
                    gliding.landing = Some(id);
                }
            }
            None => {
                self.glide.arrived(hold);
                self.gliding = None;
            }
        }
    }

    /// Windows is on `new`: the picture that settled there goes.
    fn glide_arrived(&mut self, new: &str) {
        if let Some(gliding) = self.gliding.take() {
            if gliding.landing.as_deref() == Some(new) {
                self.glide.arrived(gliding.hold);
            } else {
                self.gliding = Some(gliding);
            }
        }
    }

    /// The current desktop changed, by us or by Windows' own shortcuts.
    fn desktop_changed(&mut self, new: &str) {
        self.glide_arrived(new);
        self.usage_cell.suspend();
        debug_log!("app: the desktop is now {}", &new[..4.min(new.len())]);
        #[cfg(feature = "debug-log")]
        {
            let fg = unsafe { GetForegroundWindow() };
            debug_log!("app: foreground {:x} on {:?}", fg.0, vdapi::get_desktop_by_window(fg).map(|d| d.id()[..4].to_owned()).ok());
        }
        if self.land(new) {
            return;
        }
        // Windows went back on a cell just gone to: it had given the keyboard
        // to a window of the cell left, or was still on its way through the
        // cells before it. Go there again.
        if let Some((target, when)) = self.going.take() {
            if when.elapsed() < Duration::from_millis(GOING_SETTLES_MS) {
                self.going = Some((target.clone(), when));
                if new != target {
                    debug_log!("app: Windows went back, going to {} again", &target[..4.min(target.len())]);
                    if let Some(desktop) = Desktops::read().and_then(|d| d.get(&target)) {
                        if self.switch(desktop) {
                            vd::focus_top_window_of(&target);
                        }
                    }
                    return;
                }
            }
        }
        let Some(desktops) = self.sync() else { return };
        self.record_arrival(&desktops.current);
        self.visit(new);
        self.bring_followers(&desktops, new);
        self.save();
        let view = self.view(&desktops.current);
        self.minimap.update(view);
        self.refresh_board();
    }

    // ---- the board ------------------------------------------------------------

    /// Saves a board move in the real grid without visiting the desktop.
    fn select_board(&mut self, id: String) {
        if id == self.board.selected() {
            return;
        }
        self.grid.visit(&id);
        self.save();
        self.board.select(id, self.usage_grid());
    }

    fn model(&self, desktops: &Desktops) -> Model {
        let mut windows = vd::windows(&desktops.current);
        let parents = vd::process_parents();
        let grid = self.usage_grid();
        let now = usage::now();
        let rows = self
            .grid
            .rows
            .iter()
            .map(|row| {
                let mut pids = HashSet::new();
                let cells = row
                    .cells
                    .iter()
                    .map(|id| {
                        let (here, rest): (Vec<_>, Vec<_>) = windows.drain(..).partition(|w| w.desktop == *id);
                        windows = rest;
                        pids.extend(here.iter().map(|w| w.pid));
                        let windows = here.into_iter().map(|w| window_model(self.follows(&w), self.grid.follow_apps.contains(&vd::exe_name(w.pid).unwrap_or_default()), w)).collect();
                        CellModel { id: id.clone(), emphasised: self.grid.is_emphasised(id), usage: grid.cell_usage(id, now), windows }
                    })
                    .collect();
                RowModel {
                    name: row.name.clone(),
                    memory: vd::memory_of(&pids, &parents),
                    asleep: row.cells.iter().all(|c| self.asleep.contains(c)),
                    cells,
                }
            })
            .collect();
        let pinned = vd::pinned_windows().into_iter().map(|w| window_model(false, vdapi::is_pinned_app(w.hwnd).unwrap_or(false), w)).collect();
        Model { rows, pinned, current: desktops.current.clone() }
    }

    fn open_board(&mut self) {
        debug_log!("ui: open board sync");
        if let Some(old) = self.gliding.take() {
            self.glide_finish(old);
            self.glide.cancel();
        }
        let Some(desktops) = self.sync() else { return };
        self.forget_gone_windows();
        self.minimap.hide_after(0);
        self.focus_before_board = unsafe { GetForegroundWindow() };
        debug_log!("ui: open board model");
        let model = self.model(&desktops);
        // Left without going to its selection (losing the keyboard), the
        // board keeps the cell it was on, as long as Windows is still on the
        // same desktop.
        let left_on = self.board.selected();
        let resume = desktops.current == self.board_opened_on && left_on != desktops.current && self.grid.find(&left_on).is_some();
        self.board_opened_on = desktops.current.clone();
        debug_log!("ui: board open");
        self.board.open(model, self.usage_grid());
        if !self.grid.board_hints_seen {
            self.grid.board_hints_seen = true;
            self.save();
        }
        self.record_foreground(self.board.hwnd());
        if resume {
            self.board.select(left_on, self.usage_grid());
        }
        self.board_settling = Instant::now() + Duration::from_millis(BOARD_SETTLES_MS);
        debug_log!("ui: board foreground");
        vd::force_foreground(self.board.hwnd());
        debug_log!("ui: board opened");
    }

    fn refresh_board(&mut self) {
        if self.board.is_open() {
            if let Some(desktops) = self.sync() {
                let model = self.model(&desktops);
                self.board.set_model(model, self.usage_grid());
            }
        }
    }

    /// Refreshes the board once windows asked to close have had a moment to
    /// go (or to ask about saving).
    fn refresh_board_in(&self, ms: u32) {
        unsafe {
            SetTimer(self.hwnd, REFRESH_TIMER_ID, ms, None);
        }
    }

    /// A click of the trigger opens the board; the next one goes to the
    /// selected cell.
    fn toggle_board(&mut self) {
        if self.board.is_modal() {
            // The question that is up is about the board as it is: it stays
            // until answered.
            debug_log!("app: click ignored, a question being up");
        } else if self.board.is_open() {
            let selected = self.board.selected();
            self.perform(Action::Go(selected, None));
        } else {
            self.open_board();
        }
    }

    fn board_message(&mut self, message: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
        let started = Instant::now();
        debug_log!("ui: board handle {message:#x}");
        let action = self.board.handle(message, wparam, lparam)?;
        let handled = started.elapsed();
        debug_log!("ui: board perform {message:#x}");
        self.perform(action);
        debug_log!("ui: board finished {message:#x}");
        // Kept as a diagnostic: anything this slow is felt as a hitch.
        if started.elapsed() > Duration::from_millis(250) {
            log(&format!("slow board message {message:#x}: handle {handled:?}, total {:?}", started.elapsed()));
        }
        Some(LRESULT(0))
    }

    /// Carries out what the user asked for in the board.
    fn perform(&mut self, action: Action) {
        #[cfg(feature = "debug-log")]
        match &action {
            Action::None => {}
            Action::Dismiss => {
                debug_log!("app: board closed, having lost the keyboard");
            }
            Action::Cancel => {
                debug_log!("app: board closed at its selection");
            }
            Action::Go(id, _) => {
                debug_log!("app: board closed to go to cell {}", &id[..4.min(id.len())]);
            }
            Action::CloseWindow(hwnd) => {
                debug_log!("app: asked window {:x} to close", hwnd.0);
            }
            Action::SetPin { window, row, all } => {
                debug_log!("app: pin of {:x}: row {row}, everywhere {all}", window.0);
            }
            Action::SetTop { window, on } => {
                debug_log!("app: above the others {:x}: {on}", window.0);
            }
            Action::ToggleEmphasis(id) => {
                debug_log!("app: emphasis toggled on cell {}", &id[..4.min(id.len())]);
            }
            Action::AddCell { .. } | Action::AddRow { .. } => {
                debug_log!("app: a cell is added");
            }
            Action::RemoveCell(id) => {
                debug_log!("app: cell {} is removed", &id[..4.min(id.len())]);
            }
            _ => {
                debug_log!("app: the board is rearranged");
            }
        }
        match action {
            // These are done with here: no grid to commit, no board to rebuild.
            Action::None => return,
            Action::Step(dir) => {
                self.step(dir, false);
                return;
            }
            // Right after a switch of desktops, or a desktop made or removed,
            // Windows gives the keyboard to a window of that desktop: the
            // board takes it back instead of closing under the user's hand.
            Action::Dismiss if Instant::now() < self.board_settling => {
                debug_log!("app: the board lost the keyboard while settling, and takes it back");
                unsafe {
                    SetTimer(self.hwnd, RECLAIM_TIMER_ID, 40, None);
                }
                return;
            }
            Action::Dismiss => return self.board.close(),
            Action::Cancel => return self.cancel_board(),
            Action::Go(id, window) => return self.leave_board(&id, window),
            Action::CloseWindow(hwnd) => {
                if input::above_us(hwnd) || !vd::ask_to_close(hwnd) {
                    self.board.close_refused(hwnd);
                }
                return self.refresh_board_in(500);
            }
            // The menus are shown once this handler has returned
            // (`run_pending_menus`).
            Action::RowMenu(row) => {
                self.pending_row_menu = Some(row);
                return;
            }

            Action::MoveWindow { window, cell, monitor } => self.move_window(window, cell, monitor),
            Action::MoveCell { id, row, index } => self.grid.move_cell(&id, row, index),
            Action::MoveCellToNewRow { id, at } => self.grid.move_cell_to_new_row(&id, at),
            Action::MoveRow { from, at } => self.grid.move_row(from, at),
            Action::ToggleEmphasis(id) => self.grid.toggle_emphasis(&id),
            Action::AddCell { row, front } => {
                self.board_settling = Instant::now() + Duration::from_millis(BOARD_SETTLES_MS);
                self.add_cell(row, front)
            }
            Action::AddRow { top } => {
                self.board_settling = Instant::now() + Duration::from_millis(BOARD_SETTLES_MS);
                self.add_row(top)
            }
            Action::RemoveCell(id) => {
                self.board_settling = Instant::now() + Duration::from_millis(BOARD_SETTLES_MS);
                self.remove_cell(&id)
            }
            Action::SetPin { window, row, all } => self.set_pin(window, row, all),
            Action::SetTop { window, on } => self.set_top(window, on),
            Action::Rename(row, name) => {
                if let Some(row) = self.grid.rows.get_mut(row) {
                    row.name = name;
                }
            }
        }
        self.commit();
        self.refresh_board();
    }

    /// Esc or a click on nothing: closes the board at its selection, going
    /// there as Enter would, with the row anchors as left in the board. On
    /// the current desktop already, it only gives focus back.
    fn cancel_board(&mut self) {
        let selected = self.board.selected();
        let current = self.sync().map(|d| d.current);
        if let Some(id) = current.as_deref().and_then(|current| dismissal_target(&selected, current)) {
            return self.leave_board(id, None);
        }
        self.board.close();
        if unsafe { IsWindow(self.focus_before_board) }.as_bool() {
            vd::force_foreground(self.focus_before_board);
        }
    }

    /// Closes the board and goes to cell `id`, focusing `window` if given.
    /// An empty id stands for a window shown on every desktop: stay here.
    fn leave_board(&mut self, id: &str, window: Option<HWND>) {
        // The board stays up until the keyboard is with a window of the cell
        // gone to. Hidden while it has the keyboard, Windows gives that to
        // the window under it, one of the cell left, and goes back there.
        debug_log!("ui: leave board sync");
        let Some(desktops) = self.sync() else { return self.board.close() };
        if !id.is_empty() && id != desktops.current {
            let Some(target) = desktops.get(id) else { return self.board.close() };
            self.bring_followers(&desktops, id);
            if !self.switch(target) {
                return self.board.close();
            }
            self.visit(id);
            self.going = Some((id.to_owned(), Instant::now()));
        }
        debug_log!("ui: leave board focus");
        match window {
            Some(hwnd) if unsafe { IsWindow(hwnd) }.as_bool() => unsafe {
                if IsIconic(hwnd).as_bool() {
                    ShowWindow(hwnd, SW_RESTORE);
                }
                vd::force_foreground(hwnd);
            },
            _ => vd::focus_top_window(),
        }
        debug_log!("ui: leave board close");
        self.board.close();
        debug_log!("ui: leave board save");
        self.save();
        debug_log!("ui: left board");
    }

    fn move_window(&mut self, window: HWND, cell: Option<String>, monitor: Option<RECT>) {
        if let Some(id) = cell {
            if let Some(target) = Desktops::read().and_then(|d| d.get(&id)) {
                if let Err(e) = vdapi::move_window_to_desktop(target, window) {
                    log(&format!("move_window_to_desktop failed: {e:?}"));
                }
            }
        }
        if let Some(monitor) = monitor {
            vd::move_to_monitor(window, monitor);
        }
    }

    fn add_cell(&mut self, row: usize, front: bool) {
        if let Some(id) = self.create_desktop() {
            if let Some(row) = self.grid.rows.get_mut(row) {
                row.cells.insert(if front { 0 } else { row.cells.len() }, id);
            }
        }
    }

    fn add_row(&mut self, top: bool) {
        if let Some(id) = self.create_desktop() {
            let at = if top { 0 } else { self.grid.rows.len() };
            self.grid.rows.insert(at, Row::with_cell(id));
        }
    }

    /// Deletes a desktop, moving its windows to a cell that stays: the left
    /// (else the right) neighbour in its row when there is one, else the
    /// current cell, else the landing cell of the row above or below.
    fn remove_cell(&mut self, id: &str) {
        let Some(desktops) = self.sync() else { return };
        let Some(pos) = self.grid.find(id) else { return };
        let is_current = desktops.current == id;
        let neighbours: Vec<&String> = self.grid.rows[pos.row].cells.iter().filter(|c| *c != id).collect();
        let fallback = if !neighbours.is_empty() {
            neighbours[pos.col.saturating_sub(1).min(neighbours.len() - 1)].clone()
        } else if !is_current {
            desktops.current.clone()
        } else {
            let other = if pos.row > 0 { pos.row - 1 } else { pos.row + 1 };
            let Some(row) = self.grid.rows.get(other) else { return };
            row.cells[row.anchor()].clone()
        };
        let (Some(desktop), Some(target)) = (desktops.get(id), desktops.get(&fallback)) else { return };
        if is_current {
            if !self.switch(target) {
                return;
            }
            self.visit(&fallback);
        }
        match vdapi::remove_desktop(desktop, target) {
            Ok(()) => self.grid.remove(id),
            Err(e) => log(&format!("remove_desktop failed: {e:?}")),
        }
    }

    /// Asks every window of a shelf to close; the shelf stays.
    fn close_row_windows(&mut self, row: usize) {
        let Some(desktops) = self.sync() else { return };
        let Some(ids) = self.grid.rows.get(row).map(|r| r.cells.clone()) else { return };
        for window in vd::windows(&desktops.current).iter().filter(|w| ids.contains(&w.desktop)) {
            vd::ask_to_close(window.hwnd);
        }
        self.refresh_board_in(600);
    }

    /// Removes a shelf. Each of its desktops hands its windows to the
    /// cell drawn nearest to it in the other rows.
    fn remove_row(&mut self, row: usize) {
        let Some(desktops) = self.sync() else { return };
        if self.grid.rows.len() < 2 {
            return;
        }
        let Some(ids) = self.grid.rows.get(row).map(|r| r.cells.clone()) else { return };
        let targets: Vec<(String, String)> = ids
            .iter()
            .filter_map(|id| Some((id.clone(), self.grid.nearest_in_other_rows(self.grid.find(id)?)?.clone())))
            .collect();
        // Leave the shelf first if we are standing in it.
        if let Some((_, to)) = targets.iter().find(|(id, _)| *id == desktops.current) {
            let Some(target) = desktops.get(to) else { return };
            if !self.switch(target) {
                return;
            }
            self.visit(to);
        }
        for (id, to) in targets {
            if let (Some(desktop), Some(target)) = (desktops.get(&id), desktops.get(&to)) {
                match vdapi::remove_desktop(desktop, target) {
                    Ok(()) => self.grid.remove(&id),
                    Err(e) => log(&format!("remove_desktop failed: {e:?}")),
                }
            }
        }
        self.commit();
        self.refresh_board();
    }

    // ---- settings -------------------------------------------------------------

    /// What the settings window shows: the config in use.
    fn settings_view(&self) -> settings::View {
        settings::View {
            trigger: triggers(&self.config).0.iter().map(Trigger::describe).collect::<Vec<_>>().join(", "),
            capturing: self.capturing,
            autostart: self.autostart,
            admin: self.admin,
            minimap: self.config.minimap,
            mouse: self.config.mouse,
            touchpad: self.config.touchpad,
            touchpad_status: crate::touchpad::status(),
        }
    }

    /// Reads whether the app is set to run as administrator and to start
    /// with Windows. The scheduled task that does the first does the second
    /// too while there is one; otherwise it is the entry in the registry.
    fn read_startup(&mut self) {
        self.admin = admin::enabled();
        self.autostart = if self.admin { admin::at_sign_in() } else { tray::autostart_enabled() };
    }

    fn set_autostart(&mut self, on: bool) {
        if self.admin {
            admin::set_at_sign_in(on);
        } else {
            tray::set_autostart(on);
        }
        self.read_startup();
        self.refresh_settings();
    }

    fn open_settings(&mut self) {
        self.read_startup();
        let view = self.settings_view();
        self.settings.open(view);
        vd::force_foreground(self.settings.hwnd());
    }

    fn refresh_settings(&mut self) {
        if self.settings.is_open() {
            let view = self.settings_view();
            self.settings.set_view(view);
        }
    }

    fn close_settings(&mut self) {
        self.capture_trigger(false);
        self.settings.close();
    }

    /// Shows how Flick is used. `asked`: the user asked for it just now, and
    /// it takes the keyboard; shown unasked (the first run), it does not, or
    /// the Enter or Esc of what they were typing would put it away unseen.
    fn open_guide(&mut self, asked: bool) {
        let trigger = self.settings_view().trigger;
        if let Some(guide) = &mut self.guide {
            guide.set_trigger(trigger);
            guide.open(asked);
            if asked {
                vd::force_foreground(guide.hwnd());
            }
        }
    }

    /// Puts the guide away, for good unless asked for again, and points at
    /// the tray icon it told of.
    fn close_guide(&mut self) {
        let Some(guide) = &mut self.guide else { return };
        guide.close();
        let (title, text) = guide.words().balloon;
        if self.grid.complete_guide() {
            tray::notify(self.hwnd, title, text);
            self.save();
        }
    }

    fn guide_message(&mut self, message: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
        match self.guide.as_mut()?.handle(message, wparam, lparam)? {
            guide::Action::None => {}
            guide::Action::Close => self.close_guide(),
        }
        Some(LRESULT(0))
    }

    /// Starts or stops waiting for the user to press the new trigger.
    fn capture_trigger(&mut self, on: bool) {
        self.capturing = on;
        if on {
            input::begin_capture();
        } else {
            input::cancel_capture();
        }
        self.refresh_settings();
    }

    /// The user pressed what the trigger should be from now on.
    fn trigger_chosen(&mut self) {
        self.capture_trigger(false);
        if let Some(value) = input::take_captured() {
            self.set_config("trigger", &format!("\"{value}\""));
        }
    }

    /// Writes one value to the config file and applies the file.
    fn set_config(&mut self, key: &str, value: &str) {
        if let Err(e) = config::set_value(key, value) {
            log(&format!("writing {key} to config.toml failed: {e}"));
        }
        self.apply_config();
    }

    fn settings_message(&mut self, message: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
        match self.settings.handle(message, wparam, lparam)? {
            settings::Action::None => {}
            settings::Action::Try { mouse, touchpad } => {
                // Felt at once while dragging; written when the drag ends.
                let config = Config { mouse, touchpad, ..self.config.clone() };
                input::install(self.hwnd, triggers(&config).0, &config);
            }
            settings::Action::Set(key, value) => self.set_config(key, &value),
            settings::Action::Capture(on) => self.capture_trigger(on),
            settings::Action::Autostart(on) => self.set_autostart(on),
            settings::Action::Admin(on) => {
                if on {
                    let taken_over = admin::turn_on(self.autostart);
                    if admin::enabled() { tray::set_autostart(false); }
                    if taken_over { unsafe { PostQuitMessage(0); } }
                } else if admin::turn_off() {
                    tray::set_autostart(self.autostart);
                }
                self.read_startup();
                self.refresh_settings();
            }
            settings::Action::OpenFile => unsafe {
                let path = HSTRING::from(config::config_path().as_os_str());
                ShellExecuteW(None, w!("open"), w!("notepad.exe"), &path, PCWSTR::null(), SW_SHOWNORMAL);
            },
            settings::Action::TouchpadSettings => unsafe {
                ShellExecuteW(None, w!("open"), w!("ms-settings:devices-touchpad"), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
            },
            settings::Action::Close => self.close_settings(),
        }
        Some(LRESULT(0))
    }

    /// Applies the config file if it was saved since it was last read.
    fn apply_saved_config(&mut self) {
        if config::config_modified() != self.config_seen {
            self.apply_config();
        }
    }

    /// Applies the config file: hooks, hotkeys and the sleep timer.
    fn apply_config(&mut self) {
        let (config, error) = config::load_config();
        // Noted first, so that a file reported as broken is not reported again.
        self.config_seen = config::config_modified();
        if let Some(error) = error {
            warn(&format!("Could not read config.toml, so the defaults are used.\n\n{error}"));
        }
        let (triggers, unknown) = triggers(&config);
        if !unknown.is_empty() {
            warn(&format!("Unknown entries in trigger in config.toml: {}", unknown.join(", ")));
        }
        crate::touchpad::poll(config.touchpad.three_finger);
        input::install(self.hwnd, triggers, &config);
        self.register_hotkeys(config.hotkeys);
        unsafe {
            let _ = KillTimer(self.hwnd, SLEEP_TIMER_ID);
            if config.sleep_after_minutes > 0 {
                SetTimer(self.hwnd, SLEEP_TIMER_ID, 60_000, None);
            }
        }
        self.minimap.set_enabled(config.minimap);
        self.config = config;
        let trigger = self.settings_view().trigger;
        if let Some(guide) = &mut self.guide { guide.set_trigger(trigger); }
        self.refresh_settings();
        if crate::touchpad::status() == crate::touchpad::Status::Conflict && !self.settings.is_open() {
            self.settings.open(self.settings_view());
        }
    }

    /// Ctrl+Alt+Win with an arrow moves, with Shift added it carries the
    /// active window along, and with P it opens the pin menu.
    fn register_hotkeys(&self, enabled: bool) {
        unsafe {
            for id in 0..=PIN_HOTKEY_ID {
                let _ = UnregisterHotKey(self.hwnd, id);
            }
            if !enabled {
                return;
            }
            let base = MOD_CONTROL | MOD_ALT | MOD_WIN;
            let _ = RegisterHotKey(self.hwnd, PIN_HOTKEY_ID, HOT_KEY_MODIFIERS(base.0), 'P' as u32);
            for (i, key) in [VK_LEFT, VK_RIGHT, VK_UP, VK_DOWN].into_iter().enumerate() {
                for (carry, mods) in [(0, base), (4, base | MOD_SHIFT)] {
                    let id = i as i32 + carry;
                    if RegisterHotKey(self.hwnd, id, HOT_KEY_MODIFIERS(mods.0), key.0 as u32).is_err() {
                        log(&format!("hotkey {id} could not be registered"));
                    }
                }
            }
        }
    }
}

/// The triggers a config asks for (the mouse's forward button when it names
/// none that is known) and the entries of it that were not understood.
fn triggers(config: &Config) -> (Vec<Trigger>, Vec<String>) {
    let (mut triggers, unknown) = Trigger::parse_list(&config.trigger);
    if triggers.is_empty() {
        triggers.push(Trigger::XButton2);
    }
    (triggers, unknown)
}

fn on_tray_command(hwnd: HWND, command: Command) {
    match command {
        Command::Peek => {
            with_app(App::open_board);
        }
        Command::Settings => {
            with_app(App::open_settings);
        }
        Command::Guide => {
            with_app(|app| app.open_guide(true));
        }
        Command::DebugLog => unsafe {
            let select = HSTRING::from(format!("/select,\"{}\"", crate::debuglog::path().display()));
            ShellExecuteW(None, w!("open"), w!("explorer.exe"), &select, PCWSTR::null(), SW_SHOWNORMAL);
        },
        Command::Exit => unsafe {
            let _ = DestroyWindow(hwnd);
        },
    }
}

/// The window procedure of the app's hidden message window, the board and
/// the settings window.
unsafe extern "system" fn wndproc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if message != WM_NULL {
        UI_MESSAGE.store(message, Ordering::Relaxed);
    }
    if hwnd.0 == SETTINGS_HWND.get() {
        if message == WM_CLOSE {
            // Its close button puts it away; the window itself is kept.
            if with_app(App::close_settings).is_none() {
                ShowWindow(hwnd, SW_HIDE);
            }
            return LRESULT(0);
        }
        let handled = with_app(|app| app.settings_message(message, wparam, lparam)).flatten();
        return handled.unwrap_or_else(|| DefWindowProcW(hwnd, message, wparam, lparam));
    }
    if hwnd.0 == GUIDE_HWND.get() {
        if message == WM_CLOSE {
            if with_app(App::close_guide).is_none() {
                ShowWindow(hwnd, SW_HIDE);
            }
            return LRESULT(0);
        }
        let handled = with_app(|app| app.guide_message(message, wparam, lparam)).flatten();
        return handled.unwrap_or_else(|| DefWindowProcW(hwnd, message, wparam, lparam));
    }
    if hwnd.0 == BOARD_HWND.get() {
        let handled = with_app(|app| app.board_message(message, wparam, lparam)).flatten();
        run_pending_menus(hwnd);
        return handled.unwrap_or_else(|| DefWindowProcW(hwnd, message, wparam, lparam));
    }
    match message {
        WM_STEP => {
            if let Some(dir) = input::dir_from_index(wparam.0) {
                #[cfg(feature = "debug-log")]
                let started = Instant::now();
                let taken = with_app(|app| {
                    app.in_gesture = true;
                    app.step(dir, app.carry_held())
                });
                debug_log!("app: {dir:?} {} in {:?}", if taken.is_some() { "done" } else { "dropped, the app being busy" }, started.elapsed());
                let _ = taken;
            }
        }
        WM_SLIDE | WM_DRAG => {
            let hold = lparam.0 as u32;
            let (stretch, mut cell) = input::slide_of(wparam.0);
            // Only where the hand is by now matters: of the cells it came to
            // while the app was busy with one, the last is gone to, at once.
            let mut next = MSG::default();
            while message == WM_SLIDE
                && PeekMessageW(&mut next, hwnd, WM_SLIDE, WM_SLIDE, PM_NOREMOVE).as_bool()
                && (next.lParam.0 as u32, input::slide_of(next.wParam.0).0) == (hold, stretch)
                && PeekMessageW(&mut next, hwnd, WM_SLIDE, WM_SLIDE, PM_REMOVE).as_bool()
            {
                cell = input::slide_of(next.wParam.0).1;
            }
            #[cfg(feature = "debug-log")]
            let started = Instant::now();
            let on = with_app(|app| {
                app.in_gesture |= message == WM_SLIDE;
                app.slide((hold, stretch), (message == WM_SLIDE).then_some(cell), app.carry_held())
            });
            // An app too busy to hear of it (a menu is up) is not asked
            // again: it goes to where the hand is on hearing of the next cell.
            if let Some((on, room)) = on.flatten() {
                input::slide_shown(hold, stretch, on, Some(room));
            }
            if message == WM_DRAG {
                if with_app(|app| app.glide_begin(hold)).is_none() {
                    let _ = PostMessageW(hwnd, message, wparam, lparam);
                }
            }
            debug_log!("app: slide {:?} in {:?}", on.flatten().map(|on| on.0), started.elapsed());
        }
        WM_ROWS => {
            let hold = lparam.0 as u32;
            let mut row = input::row_of(wparam.0);
            // As sideways: only where the hand is by now matters, as long as
            // nothing else came in between.
            let mut next = MSG::default();
            while PeekMessageW(&mut next, hwnd, 0, 0, PM_NOREMOVE).as_bool()
                && next.message == WM_ROWS
                && next.lParam.0 as u32 == hold
                && PeekMessageW(&mut next, hwnd, WM_ROWS, WM_ROWS, PM_REMOVE).as_bool()
            {
                row = input::row_of(next.wParam.0);
            }
            #[cfg(feature = "debug-log")]
            let started = Instant::now();
            let on = with_app(|app| {
                app.in_gesture = true;
                app.rows(hold, row, app.carry_held())
            });
            if let Some((on, room)) = on.flatten() {
                input::rows_shown(hold, on, room);
            }
            debug_log!("app: rows {:?} in {:?}", on.flatten().map(|on| on.0), started.elapsed());
        }
        WM_CLICK => {
            input::forget_hold(lparam.0 as u32);
            #[cfg(feature = "debug-log")]
            let started = Instant::now();
            let taken = with_app(App::toggle_board);
            debug_log!("app: click {} in {:?}", if taken.is_some() { "taken" } else { "ignored, the app being busy (a menu or a question is up)" }, started.elapsed());
            let _ = taken;
        }
        WM_RELEASE => {
            let heard = with_app(|app| {
                if !app.gliding.as_ref().is_some_and(|g| g.hold == lparam.0 as u32) { input::forget_hold(lparam.0 as u32); }
                app.finish_gesture();
                app.minimap.hide_after(LINGER_RELEASE_MS);
            });
            // Not to be lost: the minimap would stay up for good.
            if heard.is_none() {
                SetTimer(hwnd, RELEASE_TIMER_ID, 50, None);
            }
        }
        WM_GLIDE_TARGET => {
            with_app(|app| app.glide_target(lparam.0 as u32, glide::target_of(wparam.0)));
        }
        WM_GLIDE_SETTLED => {
            let (hold, to) = (lparam.0 as u32, glide::target_of(wparam.0));
            // Not to be lost: the picture would wait for nothing.
            if with_app(|app| app.glide_settled(hold, to)).is_none() {
                let _ = PostMessageW(hwnd, message, wparam, lparam);
            }
        }
        WM_TIMER if wparam.0 == RELEASE_TIMER_ID => {
            let _ = KillTimer(hwnd, RELEASE_TIMER_ID);
            let _ = PostMessageW(hwnd, WM_RELEASE, WPARAM(0), LPARAM(0));
        }
        WM_POWERBROADCAST if wparam.0 as u32 == PBT_APMRESUMEAUTOMATIC => {
            input::woke();
            return DefWindowProcW(hwnd, message, wparam, lparam);
        }
        WM_OPEN_SETTINGS => {
            with_app(App::open_settings);
        }
        WM_OPEN_GUIDE => {
            with_app(|app| app.open_guide(true));
        }
        WM_CAPTURED => {
            with_app(|app| if wparam.0 == 1 { app.trigger_chosen() } else { app.capture_trigger(false) });
        }
        WM_HOTKEY if wparam.0 as i32 == PIN_HOTKEY_ID => {
            let window = GetForegroundWindow();
            if vd::is_app_window(window) {
                with_app(|app| app.pending_menu = Some((window, app.pin_state(window))));
                // The menu needs its owner in the foreground to close on a
                // click elsewhere.
                vd::force_foreground(hwnd);
                run_pending_menus(hwnd);
                vd::force_foreground(window);
            }
        }
        WM_HOTKEY => {
            if let Some(dir) = input::dir_from_index(wparam.0 % 4) {
                with_app(|app| {
                    app.step(dir, wparam.0 >= 4);
                    app.minimap.hide_after(LINGER_HOTKEY_MS);
                });
            }
        }
        WM_TIMER if wparam.0 == RECLAIM_TIMER_ID => {
            let _ = KillTimer(hwnd, RECLAIM_TIMER_ID);
            with_app(|app| {
                if app.board.is_open() {
                    vd::force_foreground(app.board.hwnd());
                }
            });
        }
        WM_TIMER if wparam.0 == ONWARD_TIMER_ID => {
            with_app(|app| {
                let _ = KillTimer(hwnd, ONWARD_TIMER_ID);
                app.go_on();
            });
        }
        WM_TIMER if wparam.0 == FLIGHT_TIMER_ID => {
            with_app(|app| {
                if let Some(now) = Desktops::read().map(|d| d.current) {
                    let _ = KillTimer(hwnd, FLIGHT_TIMER_ID);
                    debug_log!("app: switch recovery read the current desktop");
                    if !app.land(&now) {
                        app.desktop_changed(&now);
                    }
                }
            });
        }
        WM_TIMER if wparam.0 == LABEL_TIMER_ID => {
            let _ = KillTimer(hwnd, LABEL_TIMER_ID);
            vd::set_switch_label_hidden(false);
        }
        WM_TIMER if wparam.0 == REFRESH_TIMER_ID => {
            let _ = KillTimer(hwnd, REFRESH_TIMER_ID);
            with_app(App::refresh_board);
        }
        WM_TIMER if wparam.0 == SLEEP_TIMER_ID => {
            with_app(App::sleep_idle_rows);
        }
        WM_TIMER if wparam.0 == CONFIG_TIMER_ID => {
            with_app(|app| {
                app.apply_saved_config();
                let before = crate::touchpad::status();
                let status = crate::touchpad::poll(app.config.touchpad.three_finger);
                if status != before {
                    app.refresh_settings();
                    if status == crate::touchpad::Status::Conflict { app.open_settings(); }
                }
                app.usage_tick();
                // Piggyback on the existing timer, at most once per half minute.
                if app.usage_dirty && app.usage_saved.elapsed() >= Duration::from_secs(30) {
                    app.save();
                }
            });
            if vd::put_away_stuck_notice() {
                log("put away the shell's notice window left shown over the taskbar");
            }
        }
        WM_RESTACK => {
            with_app(|app| {
                let window = HWND(wparam.0 as _);
                app.record_foreground(window);
                app.restack_clicked(window);
            });
            admin_in_front(HWND(wparam.0 as _));
            let _ = SetTimer(hwnd, RESTACK_TIMER_ID, 120, None);
        }
        WM_TIMER if wparam.0 == RESTACK_TIMER_ID => {
            let _ = KillTimer(hwnd, RESTACK_TIMER_ID);
            with_app(|app| app.restack());
        }
        WM_DESKTOP_EVENT => {
            with_app(|app| {
                while let Ok((_, new)) = app.events.try_recv() {
                    app.desktop_changed(&new);
                }
            });
        }
        WM_TRAY => {
            let event = lparam.0 as u32 & 0xFFFF;
            if event == WM_LBUTTONUP {
                with_app(App::open_board);
            } else if event == WM_RBUTTONUP || event == WM_CONTEXTMENU {
                // The menu runs its own message loop, so it must not hold the app.
                if let Some(command) = tray::menu(hwnd) {
                    on_tray_command(hwnd, command);
                }
            }
        }
        WM_DESTROY => {
            with_app(App::save);
            vd::set_switch_label_hidden(false);
            input::uninstall();
            tray::remove(hwnd);
            PostQuitMessage(0);
        }
        _ => {
            if with_app(|app| app.taskbar_created == message && app.hwnd == hwnd) == Some(true) {
                // Explorer restarted and forgot our icon.
                tray::add(hwnd, WM_TRAY);
            }
            return DefWindowProcW(hwnd, message, wparam, lparam);
        }
    }
    LRESULT(0)
}

/// Shows the menus a handler asked for. They run after the handler returned
/// because a menu pumps messages, which must be able to reach the app.
fn run_pending_menus(owner: HWND) {
    run_row_menu(owner);
    if let Some((window, state)) = with_app(|app| app.pending_menu.take()).flatten() {
        if let Some(command) = tray::pin_menu(owner, state) {
            with_app(|app| app.apply_pin(window, command));
        }
    }
}

/// The menu of a shelf in the board, if one was asked for.
fn run_row_menu(owner: HWND) {
    let pending = with_app(|app| {
        let row = app.pending_row_menu.take()?;
        Some((row, app.board.row_summary(row)?, app.grid.rows.len() > 1))
    })
    .flatten();
    let Some((row, summary, removable)) = pending else { return };
    match tray::row_menu(owner, &summary, removable) {
        Some(RowCommand::CloseWindows) => {
            // Closing windows can lose work, so it is asked about in earnest,
            // with "No" as the default answer.
            let text = format!(
                "{summary}\n\nThis closes every window in this shelf.\nUnsaved work can be lost, and this cannot be undone.\n\nClose them all?"
            );
            with_app(|app| app.board.set_modal(true));
            let answer = unsafe {
                MessageBoxW(owner, &HSTRING::from(text), w!("Close all windows"), MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2)
            };
            with_app(|app| app.board.set_modal(false));
            if answer == IDYES {
                with_app(|app| app.close_row_windows(row));
            }
        }
        Some(RowCommand::Remove) => {
            with_app(|app| app.remove_row(row));
        }
        None => {}
    }
}

/// Forwards "current desktop changed" notifications to the UI thread as
/// (old id, new id) pairs.
fn listen(hwnd: HWND) -> (mpsc::Receiver<(String, String)>, Option<vdapi::DesktopEventThread>) {
    let (raw_tx, raw_rx) = mpsc::channel::<vdapi::DesktopEvent>();
    let (tx, rx) = mpsc::channel();
    let listener = vdapi::listen_desktop_events(raw_tx).map_err(|e| log(&format!("listener failed: {e:?}"))).ok();
    let target = hwnd.0;
    std::thread::spawn(move || {
        for event in raw_rx {
            if let vdapi::DesktopEvent::DesktopChanged { new, old } = event {
                if tx.send((old.id(), new.id())).is_err() {
                    break;
                }
                unsafe {
                    let _ = PostMessageW(HWND(target), WM_DESKTOP_EVENT, WPARAM(0), LPARAM(0));
                }
            }
        }
    });
    (rx, listener)
}

/// Sets the process up and creates the app's message window.
fn init() -> windows::core::Result<HWND> {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        // Multithreaded apartment: calls into the shell block instead of
        // pumping messages, so handlers are never re-entered mid-switch.
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

        let instance = GetModuleHandleW(None)?;
        let class = crate::install::MAIN_CLASS;
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            lpszClassName: class,
            ..Default::default()
        });
        // Never shown; it only receives messages.
        let hwnd =
            CreateWindowExW(WINDOW_EX_STYLE(0), class, w!("Flick"), WS_OVERLAPPED, 0, 0, 0, 0, None, None, instance, None);
        if hwnd.0 == 0 {
            return Err(windows::core::Error::from_win32());
        }
        // Also when run as administrator: an installer of a newer version
        // closes it, and the test driver opens its settings.
        admin::let_in(hwnd, &[WM_CLOSE, WM_OPEN_SETTINGS, WM_OPEN_GUIDE]);
        Ok(hwnd)
    }
}

pub fn run(first_run: bool) {
    std::panic::set_hook(Box::new(|info| {
        log(&format!("panic: {info}\n{}", std::backtrace::Backtrace::force_capture()));
    }));
    let hwnd = match init() {
        Ok(hwnd) => hwnd,
        Err(e) => return warn(&format!("Flick could not start: {e}")),
    };
    if first_run {
        tray::set_autostart(true);
    }
    // Elevation is opt-in in Settings; a cancelled request is never retried at startup.
    let board = match Board::new(wndproc) {
        Ok(board) => board,
        Err(e) => return warn(&format!("Flick could not create its windows: {e}")),
    };
    BOARD_HWND.set(board.hwnd().0);
    let settings = match Settings::new(wndproc, tray::app_icon()) {
        Ok(settings) => settings,
        Err(e) => return warn(&format!("Flick could not create its windows: {e}")),
    };
    SETTINGS_HWND.set(settings.hwnd().0);
    let guide = match Guide::new(wndproc, tray::app_icon()) {
        Ok(guide) => guide,
        Err(e) => return warn(&format!("Flick could not create its windows: {e}")),
    };
    GUIDE_HWND.set(guide.hwnd().0);
    match wait_for_desktops() {
        Some(true) => {}
        Some(false) => {
            return warn("Flick could not reach the virtual desktops. This build of Windows may not be supported.");
        }
        None => return,
    }

    debug_log!(
        "Flick {}, Windows build {:?}, monitors {:?}",
        env!("CARGO_PKG_VERSION"),
        vdapi::windows_build(),
        vd::monitors().iter().map(|m| (m.bounds.right - m.bounds.left, m.bounds.bottom - m.bounds.top, m.scale)).collect::<Vec<_>>()
    );
    let (events, listener) = listen(hwnd);
    MAIN_HWND.set(hwnd.0);
    unsafe {
        SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            None,
            Some(foreground_changed),
            0,
            0,
            WINEVENT_OUTOFCONTEXT,
        );
    }
    let mut app = App::new(hwnd, board, settings, events, listener);
    app.guide = Some(guide);
    if let Some(desktops) = app.sync() {
        // A board gesture may have lined this shelf up on another cell.
        let remembered = app.grid.find(&desktops.current).is_some_and(|pos| {
            let row = &app.grid.rows[pos.row];
            row.last.as_ref().is_some_and(|id| row.cells.contains(id))
        });
        if !remembered {
            app.visit(&desktops.current);
        }
        app.record_arrival(&desktops.current);
        app.commit();
    }
    app.record_foreground(unsafe { GetForegroundWindow() });
    app.apply_config();
    unsafe {
        SetTimer(hwnd, CONFIG_TIMER_ID, CONFIG_CHECK_MS, None);
    }
    tray::add(hwnd, WM_TRAY);
    // Until it has been seen once: how Flick is used, and where it is.
    if !app.grid.guide_seen {
        app.open_guide(false);
    }
    APP.with(|slot| *slot.borrow_mut() = Some(app));
    watch_ui(hwnd);

    unsafe {
        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    APP.with(|slot| slot.borrow_mut().take());
}

/// Waits until the virtual desktops can be read, handling this thread's
/// messages meanwhile: `Some(false)` when they never could, `None` when the
/// app was told to quit.
fn wait_for_desktops() -> Option<bool> {
    let start = std::time::Instant::now();
    loop {
        if Desktops::read().is_some() {
            if start.elapsed().as_millis() >= DESKTOPS_POLL_MS as u128 {
                log(&format!("the virtual desktops answered after {} ms", start.elapsed().as_millis()));
            }
            return Some(true);
        }
        if start.elapsed().as_millis() >= DESKTOPS_WAIT_MS as u128 {
            return Some(false);
        }
        let until = std::time::Instant::now() + std::time::Duration::from_millis(DESKTOPS_POLL_MS);
        while std::time::Instant::now() < until {
            unsafe {
                let mut message = MSG::default();
                while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                    if message.message == WM_QUIT {
                        return None;
                    }
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}

/// Writes a picture as a raw file: width and height as i32 LE, then the
/// BGRA pixels.
fn write_raw(path: &str, (w, h): (i32, i32), pixels: &[u8]) {
    let mut out = Vec::with_capacity(pixels.len() + 8);
    out.extend_from_slice(&w.to_le_bytes());
    out.extend_from_slice(&h.to_le_bytes());
    out.extend_from_slice(pixels);
    let _ = std::fs::write(path, out);
}

/// Render-only scores span unused, lightly used and heavily used windows/cells.
const SAMPLE_USAGE: [f64; 4] = [0.0, 1.0, 4.0, 16.0];

fn sample_usage(rows: &mut [Vec<CellView>]) {
    if std::env::var_os("FLICK_USAGE").is_some() {
        for (i, cell) in rows.iter_mut().flatten().enumerate() {
            cell.usage = usage::normalised(SAMPLE_USAGE[i % SAMPLE_USAGE.len()], 16.0).sqrt();
        }
    }
}

/// Renders a sample minimap into `path` (see `write_raw`; premultiplied
/// alpha) so the design can be reviewed without touching the desktop.
pub fn render_sample(path: &str) {
    let Ok(mut overlay) = Overlay::new() else { return };
    let mut view = View {
        rows: vec![
            vec![CellView::default(), CellView { emphasised: true, ..CellView::default() }, CellView::default(), CellView::default()],
            vec![CellView::default(); 2],
            vec![CellView::default(); 3],
        ],
        anchors: vec![2, 1, 0],
        title: "Shelf 2".into(),
        cur: Some(Pos { row: 1, col: 1 }),
    };
    sample_usage(&mut view.rows);
    if let Some((size, pixels)) = overlay.render_to_pixels(view, (0.0, 0.0), 2.0) {
        write_raw(path, size, &pixels);
    }
}

/// Plays a made-up hold through the minimap at 60 frames a second, the
/// hand's mouse counts going through the same `Plane` a real hold does
/// (sideways 260 to a cell, up and down 400 to a row): 300 right, then 450
/// down, the app getting to each cell or row 3 frames after the hand and the
/// count of the hand catching up a frame after that. Frames going sideways,
/// leaning towards the next row, and every frame of going over to it go side
/// by side into `path` (see `write_raw`), and each frame's place of the grid
/// into `path`.txt: the rows' first cells sideways and the first row down, in
/// cells from the highlight.
pub fn render_minimap_strip(path: &str) {
    let Ok(mut overlay) = Overlay::new() else { return };
    let cells = |n: usize| vec![CellView::default(); n];
    let mut rows = vec![cells(3), vec![CellView::default(), CellView { emphasised: true, ..CellView::default() }], cells(4), cells(2)];
    sample_usage(&mut rows);
    let smooth = |u: f32| u * u * (3.0 - 2.0 * u);
    let (dt, scale, gap, steps) = (1.0 / 60.0, 1.5, 6, (260, 400));
    let mut plane = crate::slide::Plane::new();
    // The hold begins on row 1, cell 0, with a row above and two below.
    plane.rows.set_room(1, 2, steps.1);
    plane.cols.set_room(0, 1, steps.0);
    let (mut counts, mut stretch) = ((0, 0), 0u64);
    let (mut came_col, mut came_row) = (None, None);
    let mut frames: Vec<((i32, i32), Vec<u8>)> = Vec::new();
    let mut record = String::new();
    for frame in 0..64 {
        let t = frame as f32;
        // The hand, in mouse counts: 300 right, a rest, then 450 down.
        let x = (300.0 * smooth((t / 14.0).min(1.0))) as i32;
        let y = (450.0 * smooth(((t - 18.0) / 30.0).clamp(0.0, 1.0))) as i32;
        let moved = plane.travel(x - counts.0, y - counts.1, steps);
        counts = (x, y);
        if moved.cell && came_col.is_none() {
            came_col = Some(frame);
        }
        if moved.row {
            came_row = Some(frame);
            stretch += 1;
        }
        let after = |came: Option<i32>, by: i32| came.is_some_and(|at| frame >= at + by);
        let (view_col, told_col) = (after(came_col, 3) as i32, after(came_col, 4) as i32);
        let (view_row, told_row) = (1 + after(came_row, 3) as usize, after(came_row, 4) as i32);
        let hand = Hand {
            along: if stretch == 1 { plane.cols.cell() as f32 } else { (plane.cols.cell() - told_col) as f32 }
                + plane.cols.rest(steps.0) as f32 / steps.0 as f32,
            across: (plane.rows.cell() - told_row) as f32 + plane.rows.lean(steps.1),
            counted_from: stretch << 8 | (told_col as u64) << 4 | told_row as u64,
        };
        let cur = if view_row == 2 { Pos { row: 2, col: 2 } } else { Pos { row: 1, col: view_col as usize } };
        let view = View {
            rows: rows.clone(),
            anchors: vec![1, view_col as usize, 2, 1],
            cur: Some(cur),
            title: format!("Shelf {}", cur.row + 1),
        };
        let Some((size, pixels, (xs, y_drawn))) = overlay.frame_to_pixels(view, hand, dt, scale) else { return };
        let xs: Vec<String> = xs.iter().map(|x| format!("{x:+.3}")).collect();
        record += &format!(
            "frame {frame:2} counts {x},{y} gathered {} cur {},{} hand {:+.3},{:+.3} rows x [{}] y {y_drawn:+.3}\n",
            plane.rows.gathered(steps.1),
            cur.row,
            cur.col,
            hand.along,
            hand.across,
            xs.join(" ")
        );
        frames.push((size, pixels));
    }
    // Sideways; leaning down; then every frame of going over to the row.
    let snap = came_row.unwrap_or(40) as usize;
    let mut picked: Vec<usize> = vec![0, 6, 12, 18];
    picked.extend((1..=4).map(|q| 18 + (snap - 18) * q / 4 - (q == 4) as usize));
    picked.extend(snap..(snap + 9).min(frames.len()));
    let strip: Vec<((i32, i32), Vec<u8>)> = picked.iter().filter_map(|&f| frames.get(f).cloned()).collect();
    record += &format!("picked frames {picked:?}\n");
    let _ = std::fs::write(format!("{path}.txt"), record);
    let Some(&((w, h), _)) = strip.first() else { return };
    let width = strip.len() as i32 * (w + gap) - gap;
    let mut out = vec![0u8; (width * h * 4) as usize];
    for (i, (_, pixels)) in strip.iter().enumerate() {
        let left = i as i32 * (w + gap);
        for y in 0..h {
            let from = (y * w * 4) as usize;
            let to = ((y * width + left) * 4) as usize;
            out[to..to + (w * 4) as usize].copy_from_slice(&pixels[from..from + (w * 4) as usize]);
        }
    }
    write_raw(path, (width, h), &out);
}

/// Measures a made-up hold on the actual monitors, at their current DPI,
/// without starting input hooks or switching any real desktops.
pub fn time_minimap(path: &str) {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let run = || -> Result<(), Box<dyn std::error::Error>> {
        Overlay::new()?.time_minimap(path)
    };
    if let Err(e) = run() {
        warn(&format!("Minimap timing failed: {e}"));
    }
}

/// Renders the board for the real desktops and windows into `path` (see
/// `write_raw`), without live thumbnails and without showing it. With
/// `sizes` ("1920x1040,400x300") the current cell holds made-up windows of
/// those sizes instead of its own, to see how any mix of them is laid out.
/// `FLICK_HOVER=row,col` also previews that cell (zero-based), using the
/// same made-up windows when sizes were supplied. `FLICK_USAGE=1` adds
/// scores 0,1,4,16 and a sample grid; it also adds varied usage to minimap renders.
/// `FLICK_HOVER_FROM=row,col` and `FLICK_HOVER_MS=60` sample a glide to `FLICK_HOVER`.
/// `FLICK_STEP_FROM=row,col` and `FLICK_STEP_MS=30` show the board that long
/// after the selection moved from that cell to the current one.
pub fn render_board(path: &str, sizes: Option<&str>) {
    // Reading the desktops needs a backend, and only a known Windows gets one.
    if vdapi::select(false).is_none() {
        return;
    }
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let (Ok(board), Ok(settings)) = (Board::new(wndproc), Settings::new(wndproc, Default::default())) else { return };
    let mut app = App::new(HWND(0), board, settings, mpsc::channel().1, None);
    let Some(desktops) = Desktops::read() else { return };
    app.grid.sync(&desktops.ids(), &desktops.current);
    // A second, shorter row so that ragged rows are visible in the picture.
    if app.grid.rows.len() == 1 && app.grid.rows[0].cells.len() > 1 {
        let moved = app.grid.rows[0].cells.last().cloned().unwrap_or_default();
        app.grid.move_cell_to_new_row(&moved, 1);
        app.grid.rows[1].name = "Example row".into();
    }
    // A cell made to stand out, when the user has none.
    if app.grid.emphasised.is_empty() {
        let other = app.grid.rows.iter().flat_map(|row| &row.cells).find(|id| **id != desktops.current).cloned();
        app.grid.emphasised.extend(other);
    }
    let hover = std::env::var("FLICK_HOVER").ok().and_then(|value| {
        let (row, col) = value.split_once(',')?;
        Some(Pos { row: row.trim().parse().ok()?, col: col.trim().parse().ok()? })
    });
    let sample = sizes.is_some() && std::env::var_os("FLICK_USAGE").is_some();
    if sample {
        app.grid.rows = vec![
            Row { name: "Importance".into(), cells: vec![desktops.current.clone(), "sample-1".into(), "sample-2".into(), "sample-3".into()], last: None },
            Row { name: "Other shelf".into(), cells: (4..8).map(|i| format!("sample-{i}")).collect(), last: None },
        ];
        app.grid.emphasised = vec!["sample-2".into()];
        app.grid.usage.clear();
        let now = usage::now();
        for (i, id) in app.grid.rows.iter().flat_map(|row| &row.cells).enumerate() {
            app.grid.usage.entry(id.clone()).or_default().visits.record(now, SAMPLE_USAGE[i % SAMPLE_USAGE.len()], usage::CELL_HALF_LIFE);
        }
    }
    let mut model = app.model(&desktops);
    if let Some(sizes) = sizes {
        let made_up = sizes.split(',').filter_map(|size| size.trim().split_once('x')).enumerate().filter_map(|(i, (w, h))| {
            let (w, h): (i32, i32) = (w.parse().ok()?, h.parse().ok()?);
            Some(WindowModel {
                follows: false,
                pin_from_app: false,
                app: format!("sample{i}"),
                icon_key: 0,
                app_name: String::new(),
                // No window has such a handle; it only tells the tiles apart.
                hwnd: HWND(-1 - i as isize),
                rect: RECT { left: 0, top: 0, right: w, bottom: h },
                minimized: false,
                topmost: false,
                title: format!("{w} × {h}"),
            })
        });
        let mut made_up: Vec<WindowModel> = made_up.collect();
        if sample {
            let now = usage::now();
            for (i, window) in made_up.iter_mut().enumerate() {
                let score = SAMPLE_USAGE[i % SAMPLE_USAGE.len()];
                app.grid.window_usage.entry(window.hwnd.0).or_default().record(now, score, usage::WINDOW_HALF_LIFE);
                window.title += &format!(" · use {score}");
            }
        }
        model.pinned.clear();
        // For pictures of how pinned windows are placed: FLICK_PINS=a,r pins
        // the first a made-up windows everywhere and the next r to their row.
        let pins = std::env::var("FLICK_PINS").ok();
        let (all, row) = pins
            .as_deref()
            .and_then(|v| v.split_once(','))
            .and_then(|(a, r)| Some((a.trim().parse::<usize>().ok()?, r.trim().parse::<usize>().ok()?)))
            .unwrap_or((0, 0));
        for window in made_up.iter_mut().take(all + row) {
            window.topmost = true;
        }
        for window in made_up.iter_mut().skip(all).take(row) {
            window.follows = true;
        }
        model.pinned.extend(made_up.drain(..all.min(made_up.len())));
        if let Some(cell) = hover.and_then(|pos| model.rows.get_mut(pos.row)?.cells.get_mut(pos.col)) {
            cell.windows = made_up.clone();
        }
        if let Some(cell) = model.rows.iter_mut().flat_map(|row| &mut row.cells).find(|cell| cell.id == desktops.current) {
            cell.windows = made_up;
        }
    }
    let glide = std::env::var("FLICK_HOVER_FROM").ok().and_then(|value| {
        let (row, col) = value.split_once(',')?;
        let pos = Pos { row: row.trim().parse().ok()?, col: col.trim().parse().ok()? };
        let ms = std::env::var("FLICK_HOVER_MS").ok().and_then(|ms| ms.parse::<u64>().ok()).unwrap_or(60).min(10_000);
        Some((pos, ms))
    });
    let step = std::env::var("FLICK_STEP_FROM").ok().and_then(|value| {
        let (row, col) = value.split_once(',')?;
        let pos = Pos { row: row.trim().parse().ok()?, col: col.trim().parse().ok()? };
        let ms = std::env::var("FLICK_STEP_MS").ok().and_then(|ms| ms.parse::<u64>().ok()).unwrap_or(30).min(10_000);
        Some((pos, ms))
    });
    // The current cell is where the move went, its row lined up on it.
    if step.is_some() {
        app.grid.visit(&desktops.current);
    }
    let size = (2560, 1440);
    if let Some(pixels) = app.board.render_to_pixels(model, app.grid.clone(), size, 1.5, hover, glide, step) {
        write_raw(path, size, &pixels);
    }
}

/// Renders the settings window showing the default settings into `path`
/// (see `write_raw`), without showing it.
/// Asks the Flick already running to show how it is used.
pub fn show_guide_of_running() {
    unsafe {
        let running = windows::Win32::UI::WindowsAndMessaging::FindWindowW(crate::install::MAIN_CLASS, None);
        if running.0 != 0 {
            let _ = PostMessageW(running, WM_OPEN_GUIDE, WPARAM(0), LPARAM(0));
        }
    }
}

/// Renders the guide `seconds` into its playing into `path` (see
/// `write_raw`), without showing it.
pub fn render_guide(path: &str, seconds: f32) {
    let Ok(mut window) = Guide::new(wndproc, Default::default()) else { return };
    if let Some((size, pixels)) = window.render_to_pixels(seconds, 1.5) {
        write_raw(path, size, &pixels);
    }
}

/// Renders the settings window with the default config into `path` (see
/// `write_raw`), on the touchpad's tab with `touchpad`.
pub fn render_settings(path: &str, touchpad: bool) {
    let Ok(mut window) = Settings::new(wndproc, Default::default()) else { return };
    let config = Config::default();
    let view = settings::View {
        trigger: triggers(&config).0.iter().map(Trigger::describe).collect::<Vec<_>>().join(", "),
        capturing: false,
        autostart: true,
        admin: false,
        minimap: config.minimap,
        mouse: config.mouse,
        touchpad: config.touchpad,
        touchpad_status: if touchpad { crate::touchpad::Status::Conflict } else { crate::touchpad::Status::Absent },
    };
    let tab = if touchpad { settings::Tab::Touchpad } else { settings::Tab::Mouse };
    if let Some((size, pixels)) = window.render_to_pixels(view, tab, 1.5) {
        write_raw(path, size, &pixels);
    }
}

/// The cell closing the board by Esc or a click on nothing goes to: the
/// selection, unless that is the current desktop or a window shown on every
/// desktop (an empty id).
fn dismissal_target<'a>(selected: &'a str, current: &str) -> Option<&'a str> {
    (!selected.is_empty() && selected != current).then_some(selected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_the_board_goes_to_a_selection_moved_off_the_current_desktop() {
        assert_eq!(dismissal_target("b", "a"), Some("b"));
        assert_eq!(dismissal_target("a", "a"), None);
        assert_eq!(dismissal_target("", "a"), None);
    }
}
