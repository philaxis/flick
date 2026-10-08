//! Wires input, the grid model, the virtual desktops, the minimap, the board
//! and the settings window together.

use crate::{
    board::{Action, Board, CellModel, Model, RowModel, WindowModel},
    config::{self, Config},
    grid::{self, Dir, Grid, Pos, Row},
    input::{self, Trigger, WM_CAPTURED, WM_CLICK, WM_RELEASE, WM_STEP},
    overlay::{CellView, Minimap, Overlay, View},
    settings::{self, Settings},
    tray::{self, Command, PinCommand, PinState, RowCommand},
    vd::{self, Desktops},
    vdapi,
};
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    sync::mpsc,
    time::{Duration, Instant, SystemTime},
};
use windows::{
    core::{w, HSTRING, PCWSTR},
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
        System::{
            Com::{CoInitializeEx, COINIT_MULTITHREADED},
            LibraryLoader::GetModuleHandleW,
        },
        UI::{
            HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2},
            Input::KeyboardAndMouse::{
                GetAsyncKeyState, RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL,
                MOD_SHIFT, MOD_WIN, VK_CONTROL, VK_DOWN, VK_LEFT, VK_MENU, VK_RIGHT, VK_SHIFT, VK_UP,
            },
            Shell::ShellExecuteW,
            WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetForegroundWindow, GetMessageW,
                IsIconic, IsWindow, KillTimer, MessageBoxW, PostMessageW, PostQuitMessage, RegisterClassW,
                RegisterWindowMessageW, SetTimer, ShowWindow, TranslateMessage, IDYES, MB_DEFBUTTON2, MB_ICONWARNING,
                MB_OK, MB_YESNO, MSG, SW_HIDE, SW_RESTORE, SW_SHOWNORMAL, WINDOW_EX_STYLE, WM_APP, WM_CLOSE,
                WM_CONTEXTMENU, WM_DESTROY, WM_HOTKEY, WM_LBUTTONUP, WM_RBUTTONUP, WM_TIMER, WNDCLASSW,
                WS_OVERLAPPED,
            },
        },
    },
};

const WM_TRAY: u32 = WM_APP + 10;
const WM_DESKTOP_EVENT: u32 = WM_APP + 11;
/// Opens the settings window; for the test driver (examples/drive.rs).
const WM_OPEN_SETTINGS: u32 = WM_APP + 12;

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
const CONFIG_CHECK_MS: u32 = 2000;
/// Hotkey ids 0..=3 move in the direction of `input::dir_from_index`, 4..=7
/// do the same carrying the active window, and this one opens the pin menu.
const PIN_HOTKEY_ID: i32 = 8;

struct App {
    hwnd: HWND,
    config: Config,
    grid: Grid,
    minimap: Minimap,
    /// Consecutive pushes against the same edge of the grid, counted towards
    /// `edge_create_pushes`.
    edge: Option<(Dir, u32)>,
    board: Board,
    /// The window that had focus when the board opened, to give it back on cancel.
    focus_before_board: HWND,
    settings: Settings,
    /// The settings window is waiting for the new trigger to be pressed.
    capturing: bool,
    /// When the config file that is in use was written.
    config_seen: Option<SystemTime>,
    /// Steps are arriving from a held trigger; `unsettled` when one of them
    /// left keyboard focus behind on the desktop it came from.
    in_gesture: bool,
    unsettled: bool,
    /// Windows that follow the user from cell to cell inside their row.
    /// Window handles do not outlive a session, so this is not saved.
    following: HashSet<isize>,
    /// A pin menu to show once the current handler has returned.
    pending_menu: Option<(HWND, PinState)>,
    /// Likewise the menu of a workspace and that of a cell in the board.
    pending_row_menu: Option<usize>,
    pending_cell_menu: Option<String>,
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

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    /// The windows of the board and of the settings, readable without
    /// borrowing the app.
    static BOARD_HWND: Cell<isize> = const { Cell::new(0) };
    static SETTINGS_HWND: Cell<isize> = const { Cell::new(0) };
}

/// Runs `f` on the app unless it is already borrowed, which happens when a
/// handler pumps messages (menus, message boxes); such re-entrant events are
/// simply dropped.
fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|app| app.try_borrow_mut().ok()?.as_mut().map(f))
}

pub fn log(message: &str) {
    use std::io::Write;
    let path = config::config_path().with_file_name("log.txt");
    if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let _ = writeln!(file, "{secs} {message}");
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

fn window_model(follows: bool, window: vd::WindowInfo) -> WindowModel {
    WindowModel {
        follows,
        app: vd::exe_name(window.pid).unwrap_or_default(),
        app_name: vd::app_label(window.pid),
        hwnd: window.hwnd,
        rect: window.rect,
        minimized: window.minimized,
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
            minimap: Minimap::spawn(),
            edge: None,
            board,
            focus_before_board: HWND(0),
            settings,
            capturing: false,
            config_seen: None,
            in_gesture: false,
            unsettled: false,
            following: HashSet::new(),
            pending_menu: None,
            pending_row_menu: None,
            pending_cell_menu: None,
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
        if self.grid.sync(&desktops.ids(), &desktops.current) {
            self.save();
        }
        Some(desktops)
    }

    fn save(&self) {
        if let Err(e) = config::save_grid(&self.grid) {
            log(&format!("saving the grid failed: {e}"));
        }
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
    fn view(&self, current: &str, pushing: Option<Dir>) -> View {
        let cur = self.grid.find(current);
        View {
            rows: self
                .grid
                .rows
                .iter()
                .map(|row| {
                    let cell = |id| CellView { fresh: self.grid.ephemeral.contains(id), emphasised: self.grid.is_emphasised(id) };
                    row.cells.iter().map(cell).collect()
                })
                .collect(),
            anchors: self.grid.rows.iter().map(Row::anchor).collect(),
            cur,
            pushing,
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
                let _ = vdapi::move_window_to_desktop(target, window.hwnd);
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

    /// Toggles one kind of pinning, as picked from the pin menu.
    fn apply_pin(&mut self, hwnd: HWND, command: PinCommand) {
        let state = self.pin_state(hwnd);
        let result = match command {
            PinCommand::RowWindow => {
                if !self.following.remove(&hwnd.0) {
                    self.following.insert(hwnd.0);
                }
                Ok(())
            }
            PinCommand::RowApp => {
                if let Some(exe) = vd::exe_of_window(hwnd) {
                    if state.row_app {
                        self.grid.follow_apps.retain(|e| *e != exe);
                    } else {
                        self.grid.follow_apps.push(exe);
                    }
                }
                Ok(())
            }
            PinCommand::AllWindow if state.all_window => vdapi::unpin_window(hwnd),
            PinCommand::AllWindow => vdapi::pin_window(hwnd),
            PinCommand::AllApp if state.all_app => vdapi::unpin_app(hwnd),
            PinCommand::AllApp => vdapi::pin_app(hwnd),
        };
        if let Err(e) = result {
            log(&format!("pin change failed: {e:?}"));
        }
        self.save();
        self.refresh_board();
    }

    /// Sets how one window is pinned, as cycled through on its tile: within
    /// its row, on every desktop, or neither.
    fn set_pin(&mut self, window: HWND, row: bool, all: bool) {
        if row {
            self.following.insert(window.0);
        } else {
            self.following.remove(&window.0);
        }
        let result = if all {
            vdapi::pin_window(window)
        } else {
            // Being shown everywhere can also come from its app being pinned.
            if vdapi::is_pinned_app(window).unwrap_or(false) {
                let _ = vdapi::unpin_app(window);
            }
            if vdapi::is_pinned_window(window).unwrap_or(false) {
                vdapi::unpin_window(window)
            } else {
                Ok(())
            }
        };
        if let Err(e) = result {
            log(&format!("pin change failed: {e:?}"));
        }
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

    /// Moves one cell in `dir`. With `carry` the foreground window comes along.
    fn step(&mut self, dir: Dir, carry: bool) {
        if self.board.is_open() {
            // With the board open the gesture only moves the selection; the
            // desktop changes once, when the board is confirmed.
            debug_log!("app: {dir:?} moves the selection, the board being open");
            self.board.select_dir(dir);
            return;
        }
        let Some(desktops) = self.sync() else { return };
        let Some(from) = self.grid.find(&desktops.current) else { return };
        match self.grid.target(from, dir).and_then(|pos| self.grid.id_at(pos).cloned()) {
            Some(to) => {
                debug_log!("app: {dir:?} from row {} cell {}", from.row, from.col);
                self.edge = None;
                self.go(&desktops, from, &to, carry);
            }
            None => {
                debug_log!("app: {dir:?} from row {} cell {}: nothing that way", from.row, from.col);
                self.push_edge(&desktops, from, dir, carry)
            }
        }
    }

    /// A step against the edge of the grid. It only shows the minimap, unless
    /// `edge_create_pushes` is set and the edge has now been pushed that
    /// often: then a cell is created there and gone to.
    fn push_edge(&mut self, desktops: &Desktops, from: Pos, dir: Dir, carry: bool) {
        let current = desktops.current.clone();
        let pushes = match self.edge {
            Some((d, n)) if d == dir => n + 1,
            _ => 1,
        };
        // Do not chain empty cells: a fresh cell nobody put a window on yet
        // would be deleted the moment it is left.
        let on_empty_fresh = self.grid.ephemeral.contains(&current)
            && desktops.get(&current).is_some_and(|d| vd::count_on(d) == 0)
            && !carry;
        let may_create = self.config.edge_create_pushes > 0 && !on_empty_fresh;
        if !may_create || pushes < self.config.edge_create_pushes {
            self.edge = Some((dir, pushes));
            let view = self.view(&current, may_create.then_some(dir));
            self.minimap.show(view, None);
            return;
        }
        self.edge = None;
        let Some(id) = self.create_desktop() else { return };
        self.grid.insert_beside(from, dir, id.clone());
        self.grid.ephemeral.push(id.clone());
        self.commit();
        let Some(desktops) = Desktops::read() else { return };
        // Inserting to the left or above shifts the cell we came from.
        let from = self.grid.find(&current).unwrap_or(from);
        self.go(&desktops, from, &id, carry);
    }

    /// Switches from the cell at `from` to cell `to` and shows the minimap.
    fn go(&mut self, desktops: &Desktops, from: Pos, to: &str, carry: bool) {
        let Some(target) = desktops.get(to) else { return };
        let window = unsafe { GetForegroundWindow() };
        let carried = carry
            && vd::is_app_window(window)
            && vdapi::is_window_on_current_desktop(window).unwrap_or(false)
            && vdapi::move_window_to_desktop(target, window).is_ok();
        self.bring_followers(desktops, to);
        if !self.switch(target) {
            return;
        }
        self.visit(to);
        if carried {
            vd::force_foreground(window);
        } else if self.in_gesture {
            // Mid-gesture more steps may follow at once; handing focus over
            // and saving wait until the trigger is let go (`finish_gesture`).
            self.unsettled = true;
        } else {
            vd::focus_top_window();
        }
        if !self.in_gesture {
            self.save();
        }
        let view = self.view(to, None);
        self.minimap.show(view, Some(from));
    }

    /// The trigger was released: do what the steps of the gesture put off.
    fn finish_gesture(&mut self) {
        if std::mem::take(&mut self.in_gesture) {
            if std::mem::take(&mut self.unsettled) {
                vd::focus_top_window();
            }
            self.save();
        }
    }

    /// The current desktop changed, by us or by Windows' own shortcuts.
    fn desktop_changed(&mut self, old: &str, new: &str) {
        let Some(desktops) = self.sync() else { return };
        self.visit(new);
        self.bring_followers(&desktops, new);
        if self.grid.ephemeral.iter().any(|id| id == old) {
            // A cell created by an edge push is kept only if it got a window.
            match (desktops.get(old), desktops.get(new)) {
                (Some(left), Some(fallback)) if vd::count_on(left) == 0 => match vdapi::remove_desktop(left, fallback) {
                    Ok(()) => {
                        self.grid.remove(old);
                        self.commit();
                    }
                    Err(e) => log(&format!("remove_desktop failed: {e:?}")),
                },
                _ => self.grid.ephemeral.retain(|id| id != old),
            }
        }
        self.save();
        let view = self.view(&desktops.current, None);
        self.minimap.update(view);
        self.refresh_board();
    }

    // ---- the board ------------------------------------------------------------

    fn model(&self, desktops: &Desktops) -> Model {
        let mut windows = vd::windows(&desktops.current);
        let parents = vd::process_parents();
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
                        let windows = here.into_iter().map(|w| window_model(self.follows(&w), w)).collect();
                        CellModel { id: id.clone(), emphasised: self.grid.is_emphasised(id), windows }
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
        let pinned = vd::pinned_windows().into_iter().map(|w| window_model(false, w)).collect();
        Model { rows, pinned, current: desktops.current.clone() }
    }

    fn open_board(&mut self) {
        let Some(desktops) = self.sync() else { return };
        self.following.retain(|hwnd| unsafe { IsWindow(HWND(*hwnd)) }.as_bool());
        self.minimap.hide_after(0);
        self.focus_before_board = unsafe { GetForegroundWindow() };
        let model = self.model(&desktops);
        self.board.open(model, self.grid.clone());
        vd::force_foreground(self.board.hwnd());
    }

    fn refresh_board(&mut self) {
        if self.board.is_open() {
            if let Some(desktops) = self.sync() {
                let model = self.model(&desktops);
                self.board.set_model(model, self.grid.clone());
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
        if self.board.is_open() {
            let selected = self.board.selected();
            self.perform(Action::Go(selected, None));
        } else {
            self.open_board();
        }
    }

    fn board_message(&mut self, message: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
        let started = Instant::now();
        let action = self.board.handle(message, wparam, lparam)?;
        let handled = started.elapsed();
        self.perform(action);
        // Kept as a diagnostic: anything this slow is felt as a hitch.
        if started.elapsed() > Duration::from_millis(250) {
            log(&format!("slow board message {message:#x}: handle {handled:?}, total {:?}", started.elapsed()));
        }
        Some(LRESULT(0))
    }

    /// Carries out what the user asked for in the board.
    fn perform(&mut self, action: Action) {
        match action {
            // These are done with here: no grid to commit, no board to rebuild.
            Action::None => return,
            Action::Dismiss => return self.board.close(),
            Action::Cancel => return self.cancel_board(),
            Action::Go(id, window) => return self.leave_board(&id, window),
            Action::CloseWindow(hwnd) => {
                vd::ask_to_close(hwnd);
                return self.refresh_board_in(500);
            }
            // The menus are shown once this handler has returned
            // (`run_pending_menus`).
            Action::RowMenu(row) => {
                self.pending_row_menu = Some(row);
                return;
            }
            Action::CellMenu(id) => {
                self.pending_cell_menu = Some(id);
                return;
            }

            Action::MoveWindow { window, cell, monitor } => self.move_window(window, cell, monitor),
            Action::MoveCell { id, row, index } => self.grid.move_cell(&id, row, index),
            Action::MoveCellToNewRow { id, at } => self.grid.move_cell_to_new_row(&id, at),
            Action::AddCell { row, front } => self.add_cell(row, front),
            Action::AddRow { top } => self.add_row(top),
            Action::RemoveCell(id) => self.remove_cell(&id),
            Action::SetPin { window, row, all } => self.set_pin(window, row, all),
            Action::Rename(row, name) => {
                if let Some(row) = self.grid.rows.get_mut(row) {
                    row.name = name;
                }
            }
        }
        self.commit();
        self.refresh_board();
    }

    /// Closes the board and gives focus back to where it was.
    fn cancel_board(&mut self) {
        self.board.close();
        if unsafe { IsWindow(self.focus_before_board) }.as_bool() {
            vd::force_foreground(self.focus_before_board);
        }
    }

    /// Closes the board and goes to cell `id`, focusing `window` if given.
    /// An empty id stands for a window shown on every desktop: stay here.
    fn leave_board(&mut self, id: &str, window: Option<HWND>) {
        self.board.close();
        let Some(desktops) = self.sync() else { return };
        if !id.is_empty() && id != desktops.current {
            let Some(target) = desktops.get(id) else { return };
            self.bring_followers(&desktops, id);
            if !self.switch(target) {
                return;
            }
            self.visit(id);
        }
        match window {
            Some(hwnd) if unsafe { IsWindow(hwnd) }.as_bool() => unsafe {
                if IsIconic(hwnd).as_bool() {
                    ShowWindow(hwnd, SW_RESTORE);
                }
                vd::force_foreground(hwnd);
            },
            _ => vd::focus_top_window(),
        }
        self.save();
    }

    fn move_window(&mut self, window: HWND, cell: Option<String>, monitor: Option<RECT>) {
        if let Some(id) = cell {
            if let Some(target) = Desktops::read().and_then(|d| d.get(&id)) {
                if let Err(e) = vdapi::move_window_to_desktop(target, window) {
                    log(&format!("move_window_to_desktop failed: {e:?}"));
                }
                // The cell has held a window now, so it stays.
                self.grid.ephemeral.retain(|cell| *cell != id);
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

    /// Asks every window of a workspace to close; the workspace stays.
    fn close_row_windows(&mut self, row: usize) {
        let Some(desktops) = self.sync() else { return };
        let Some(ids) = self.grid.rows.get(row).map(|r| r.cells.clone()) else { return };
        for window in vd::windows(&desktops.current).iter().filter(|w| ids.contains(&w.desktop)) {
            vd::ask_to_close(window.hwnd);
        }
        self.refresh_board_in(600);
    }

    /// Removes a workspace. Each of its desktops hands its windows to the
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
        // Leave the workspace first if we are standing in it.
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
            angle: self.config.vertical_angle,
            step_x: self.config.step_x,
            step_y: self.config.step_y,
            sticky: self.config.vertical_sticky,
            autostart: tray::autostart_enabled(),
        }
    }

    fn open_settings(&mut self) {
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
            settings::Action::Try { angle, step_x, step_y } => {
                // Felt at once while dragging; written when the drag ends.
                let config = Config { vertical_angle: angle, step_x, step_y, ..self.config.clone() };
                input::install(self.hwnd, triggers(&config).0, &config);
            }
            settings::Action::Set(key, value) => self.set_config(key, &value),
            settings::Action::Capture(on) => self.capture_trigger(on),
            settings::Action::Autostart(on) => {
                tray::set_autostart(on);
                self.refresh_settings();
            }
            settings::Action::OpenFile => unsafe {
                let path = HSTRING::from(config::config_path().as_os_str());
                ShellExecuteW(None, w!("open"), w!("notepad.exe"), &path, PCWSTR::null(), SW_SHOWNORMAL);
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
            warn(&format!("config.toml을 읽지 못해 기본값을 씁니다.\n\n{error}"));
        }
        let (triggers, unknown) = triggers(&config);
        if !unknown.is_empty() {
            warn(&format!("config.toml의 trigger에서 알 수 없는 항목: {}", unknown.join(", ")));
        }
        input::install(self.hwnd, triggers, &config);
        self.register_hotkeys(config.hotkeys);
        unsafe {
            let _ = KillTimer(self.hwnd, SLEEP_TIMER_ID);
            if config.sleep_after_minutes > 0 {
                SetTimer(self.hwnd, SLEEP_TIMER_ID, 60_000, None);
            }
        }
        self.config = config;
        self.refresh_settings();
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
                    app.step(dir, app.carry_held());
                });
                debug_log!("app: {dir:?} {} in {:?}", if taken.is_some() { "done" } else { "dropped, the app being busy" }, started.elapsed());
                let _ = taken;
            }
        }
        WM_CLICK => {
            with_app(App::toggle_board);
        }
        WM_RELEASE => {
            with_app(|app| {
                app.edge = None;
                app.finish_gesture();
                app.minimap.hide_after(LINGER_RELEASE_MS);
            });
        }
        WM_OPEN_SETTINGS => {
            with_app(App::open_settings);
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
            with_app(App::apply_saved_config);
        }
        WM_DESKTOP_EVENT => {
            with_app(|app| {
                while let Ok((old, new)) = app.events.try_recv() {
                    app.desktop_changed(&old, &new);
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
    run_cell_menu(owner);
    if let Some((window, state)) = with_app(|app| app.pending_menu.take()).flatten() {
        if let Some(command) = tray::pin_menu(owner, state) {
            with_app(|app| app.apply_pin(window, command));
        }
    }
}

/// The menu of a cell in the board's map, if one was asked for.
fn run_cell_menu(owner: HWND) {
    let pending = with_app(|app| {
        let id = app.pending_cell_menu.take()?;
        Some((app.grid.is_emphasised(&id), id))
    })
    .flatten();
    let Some((emphasised, id)) = pending else { return };
    if tray::cell_menu(owner, emphasised) {
        with_app(|app| {
            app.grid.toggle_emphasis(&id);
            app.save();
            app.refresh_board();
        });
    }
}

/// The menu of a workspace in the board, if one was asked for.
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
                "{summary}\n\n이 워크스페이스의 창을 모두 닫습니다.\n저장하지 않은 작업은 잃을 수 있고, 되돌릴 수 없습니다.\n\n정말 닫을까요?"
            );
            with_app(|app| app.board.set_modal(true));
            let answer = unsafe {
                MessageBoxW(owner, &HSTRING::from(text), w!("창 모두 닫기"), MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2)
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
        Ok(hwnd)
    }
}

pub fn run(first_run: bool) {
    std::panic::set_hook(Box::new(|info| log(&format!("panic: {info}"))));
    let hwnd = match init() {
        Ok(hwnd) => hwnd,
        Err(e) => return warn(&format!("시작하지 못했습니다: {e}")),
    };
    let board = match Board::new(wndproc) {
        Ok(board) => board,
        Err(e) => return warn(&format!("화면을 만들지 못했습니다: {e}")),
    };
    BOARD_HWND.set(board.hwnd().0);
    let settings = match Settings::new(wndproc, tray::app_icon()) {
        Ok(settings) => settings,
        Err(e) => return warn(&format!("화면을 만들지 못했습니다: {e}")),
    };
    SETTINGS_HWND.set(settings.hwnd().0);
    if Desktops::read().is_none() {
        return warn("가상 데스크톱에 접근하지 못했습니다. 이 윈도우 빌드를 지원하지 않는 것일 수 있습니다.");
    }

    debug_log!(
        "Flick {}, Windows build {:?}, monitors {:?}",
        env!("CARGO_PKG_VERSION"),
        vdapi::windows_build(),
        vd::monitors().iter().map(|m| (m.bounds.right - m.bounds.left, m.bounds.bottom - m.bounds.top, m.scale)).collect::<Vec<_>>()
    );
    let (events, listener) = listen(hwnd);
    let mut app = App::new(hwnd, board, settings, events, listener);
    if let Some(desktops) = app.sync() {
        app.visit(&desktops.current);
        app.commit();
    }
    app.apply_config();
    unsafe {
        SetTimer(hwnd, CONFIG_TIMER_ID, CONFIG_CHECK_MS, None);
    }
    tray::add(hwnd, WM_TRAY);
    if first_run {
        tray::set_autostart(true);
        tray::notify(hwnd, "Flick이 켜졌습니다", "마우스 앞으로 버튼을 누른 채 밀면 옆 칸, 눌렀다 떼면 전체 보기. 버튼은 이 아이콘 우클릭 → 설정에서 바꿉니다.");
    }
    APP.with(|slot| *slot.borrow_mut() = Some(app));

    unsafe {
        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    APP.with(|slot| slot.borrow_mut().take());
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

/// Renders a sample minimap into `path` (see `write_raw`; premultiplied
/// alpha) so the design can be reviewed without touching the desktop.
pub fn render_sample(path: &str) {
    let Ok(mut overlay) = Overlay::new() else { return };
    let view = View {
        rows: vec![
            vec![CellView::default(), CellView { emphasised: true, ..CellView::default() }, CellView::default(), CellView::default()],
            vec![CellView::default(); 2],
            vec![CellView::default(), CellView::default(), CellView { fresh: true, ..CellView::default() }],
        ],
        anchors: vec![2, 1, 0],
        title: "워크스페이스 2".into(),
        cur: Some(Pos { row: 1, col: 1 }),
        pushing: Some(Dir::Right),
    };
    if let Some((size, pixels)) = overlay.render_to_pixels(view, (1.0, 1.0), 2.0) {
        write_raw(path, size, &pixels);
    }
}

/// Renders the board for the real desktops and windows into `path` (see
/// `write_raw`), without live thumbnails and without showing it. With
/// `sizes` ("1920x1040,400x300") the current cell holds made-up windows of
/// those sizes instead of its own, to see how any mix of them is laid out.
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
        app.grid.rows[1].name = "예시 행".into();
    }
    // A cell made to stand out, when the user has none.
    if app.grid.emphasised.is_empty() {
        let other = app.grid.rows.iter().flat_map(|row| &row.cells).find(|id| **id != desktops.current).cloned();
        app.grid.emphasised.extend(other);
    }
    let mut model = app.model(&desktops);
    if let Some(sizes) = sizes {
        let made_up = sizes.split(',').filter_map(|size| size.trim().split_once('x')).enumerate().filter_map(|(i, (w, h))| {
            let (w, h): (i32, i32) = (w.parse().ok()?, h.parse().ok()?);
            Some(WindowModel {
                follows: false,
                app: format!("sample{i}"),
                app_name: String::new(),
                // No window has such a handle; it only tells the tiles apart.
                hwnd: HWND(-1 - i as isize),
                rect: RECT { left: 0, top: 0, right: w, bottom: h },
                minimized: false,
                title: format!("{w} × {h}"),
            })
        });
        let made_up: Vec<WindowModel> = made_up.collect();
        model.pinned.clear();
        if let Some(cell) = model.rows.iter_mut().flat_map(|row| &mut row.cells).find(|cell| cell.id == desktops.current) {
            cell.windows = made_up;
        }
    }
    let size = (2560, 1440);
    if let Some(pixels) = app.board.render_to_pixels(model, app.grid.clone(), size, 1.5) {
        write_raw(path, size, &pixels);
    }
}

/// Renders the settings window showing the default settings into `path`
/// (see `write_raw`), without showing it.
pub fn render_settings(path: &str) {
    let Ok(mut window) = Settings::new(wndproc, Default::default()) else { return };
    let config = Config::default();
    let view = settings::View {
        trigger: triggers(&config).0.iter().map(Trigger::describe).collect::<Vec<_>>().join(", "),
        capturing: false,
        angle: config.vertical_angle,
        step_x: config.step_x,
        step_y: config.step_y,
        sticky: config.vertical_sticky,
        autostart: true,
    };
    if let Some((size, pixels)) = window.render_to_pixels(view, 1.5) {
        write_raw(path, size, &pixels);
    }
}
