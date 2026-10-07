//! Wires input, the grid model, the virtual desktops and the overlay together.

use crate::{
    board::{Action, Board, CellModel, Model, RowModel, WindowModel},
    config::{self, Config},
    grid::{Dir, Grid, Pos, Row},
    input::{self, Trigger, WM_CLICK, WM_RELEASE, WM_STEP},
    overlay::{Minimap, Overlay, View},
    tray::{self, Command, PinCommand, PinState},
    vd::{self, Desktops},
};
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    sync::mpsc,
    time::{Duration, Instant},
};
use windows::{
    core::{w, HSTRING, PCWSTR},
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        System::{
            Com::{CoInitializeEx, COINIT_MULTITHREADED},
            LibraryLoader::GetModuleHandleW,
        },
        UI::{
            HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2},
            Input::KeyboardAndMouse::{
                GetAsyncKeyState, RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS, MOD_ALT, MOD_CONTROL,
                MOD_SHIFT, MOD_WIN, VK_DOWN, VK_LEFT, VK_RIGHT, VK_UP,
            },
            Shell::ShellExecuteW,
            WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetForegroundWindow, GetSystemMetrics,
                SM_CXSCREEN, SM_CYSCREEN, WINDOW_STYLE, WS_BORDER, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
                WS_VISIBLE,
                GetMessageW, IsIconic, IsWindow, KillTimer, MessageBoxW, PostMessageW, PostQuitMessage,
                RegisterClassW, RegisterWindowMessageW, SetTimer, ShowWindow, TranslateMessage, IDYES, MB_DEFBUTTON2, MB_ICONWARNING, MB_YESNO,
                MB_OK, MSG, SW_RESTORE, SW_SHOWNORMAL, WINDOW_EX_STYLE, WM_APP, WM_CLOSE, WM_CONTEXTMENU,
                WM_DESTROY, WM_HOTKEY, WM_LBUTTONUP, WM_RBUTTONUP, WM_TIMER, WNDCLASSW, WS_OVERLAPPED,
            },
        },
    },
};
use winvd::DesktopEvent;

const WM_TRAY: u32 = WM_APP + 10;
const WM_DESKTOP_EVENT: u32 = WM_APP + 11;
/// Starts choosing a new trigger; what the tray menu item sends.
const WM_CHANGE_TRIGGER: u32 = WM_APP + 12;

/// How long the minimap stays after the trigger is released, after a hotkey,
/// and after a plain click.
const LINGER_RELEASE_MS: u64 = 350;
const LINGER_HOTKEY_MS: u64 = 900;
/// One-shot timer that refreshes the board after windows were asked to close.
const REFRESH_TIMER_ID: usize = 2;
/// Puts Windows' own "desktop name" label back after a switch.
const LABEL_TIMER_ID: usize = 4;
/// How long that label would have stayed up.
const LABEL_HIDE_MS: u32 = 1600;
/// Periodic check for rows idle long enough to be put to sleep.
const SLEEP_TIMER_ID: usize = 3;
const PIN_HOTKEY_ID: i32 = 8;

struct App {
    hwnd: HWND,
    config: Config,
    grid: Grid,
    overlay: Minimap,
    /// Consecutive pushes against the same edge of the grid.
    edge: Option<(Dir, u32)>,
    board: Board,
    /// The window that had focus when the board opened, to give it back on cancel.
    focus_before_board: HWND,
    /// The "press the new trigger" prompt, while it is up.
    trigger_prompt: HWND,
    /// Steps are arriving from a held trigger; `unsettled` when one of them
    /// left keyboard focus behind on the desktop it came from.
    in_gesture: bool,
    unsettled: bool,
    /// Windows that follow the user from cell to cell inside their row.
    /// Window handles do not outlive a session, so this is not saved.
    following: HashSet<isize>,
    /// A pin menu to show once the current handler has returned.
    pending_menu: Option<(HWND, PinState)>,
    /// Likewise the menu of a workspace in the board.
    pending_row_menu: Option<usize>,
    /// When each cell was last shown, and the cells of rows put to sleep.
    seen: HashMap<String, Instant>,
    asleep: HashSet<String>,
    started: Instant,
    events: mpsc::Receiver<(String, String)>,
    _listener: Option<winvd::DesktopEventThread>,
    taskbar_created: u32,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
    /// The board's window, readable without borrowing the app.
    static BOARD_HWND: Cell<isize> = const { Cell::new(0) };
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

fn warn(text: &str) {
    log(text);
    unsafe {
        MessageBoxW(None, &HSTRING::from(text), &HSTRING::from(config::APP_NAME), MB_OK | MB_ICONWARNING);
    }
}

impl App {
    fn view(&self, current: &str, pushing: Option<Dir>) -> View {
        View {
            rows: self
                .grid
                .rows
                .iter()
                .map(|row| row.cells.iter().map(|id| self.grid.ephemeral.contains(id)).collect())
                .collect(),
            anchors: self
                .grid
                .rows
                .iter()
                .map(|row| row.last.as_ref().and_then(|id| row.cells.iter().position(|c| c == id)).unwrap_or(0))
                .collect(),
            cur: self.grid.find(current),
            pushing,
            title: self.grid.find(current).map_or(String::new(), |pos| {
                let name = &self.grid.rows[pos.row].name;
                if name.is_empty() { format!("워크스페이스 {}", pos.row + 1) } else { name.clone() }
            }),
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

    /// Saves the grid and reorders Windows' own desktop list to match it
    /// (row after row), so Win+Ctrl+arrows and Task View agree with the grid.
    fn commit(&mut self) {
        config::save_grid(&self.grid);
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
            if let Err(e) = winvd::move_desktop(desktop, i as u32) {
                return log(&format!("move_desktop failed: {e:?}"));
            }
            let moved = actual.remove(from);
            actual.insert(i, moved);
        }
    }

    fn follows(&self, window: &vd::WindowInfo) -> bool {
        self.following.contains(&window.hwnd.0)
            || (!self.grid.follow_apps.is_empty()
                && vd::exe_name(window.pid).is_some_and(|exe| self.grid.follow_apps.contains(&exe)))
    }

    /// Switches to a desktop without Windows' own name label popping up.
    fn switch(&self, target: winvd::Desktop) -> Result<(), winvd::Error> {
        vd::set_switch_label_hidden(true);
        unsafe {
            SetTimer(self.hwnd, LABEL_TIMER_ID, LABEL_HIDE_MS, None);
        }
        winvd::switch_desktop(target)
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
                let _ = winvd::move_window_to_desktop(target, &window.hwnd);
            }
        }
    }

    fn pin_state(&self, hwnd: HWND) -> PinState {
        PinState {
            row_window: self.following.contains(&hwnd.0),
            row_app: vd::exe_of_window(hwnd).is_some_and(|exe| self.grid.follow_apps.contains(&exe)),
            all_window: winvd::is_pinned_window(hwnd).unwrap_or(false),
            all_app: winvd::is_pinned_app(hwnd).unwrap_or(false),
        }
    }

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
            PinCommand::AllWindow if state.all_window => winvd::unpin_window(hwnd),
            PinCommand::AllWindow => winvd::pin_window(hwnd),
            PinCommand::AllApp if state.all_app => winvd::unpin_app(hwnd),
            PinCommand::AllApp => winvd::pin_app(hwnd),
        };
        if let Err(e) = result {
            log(&format!("pin change failed: {e:?}"));
        }
        config::save_grid(&self.grid);
        self.refresh_board();
    }

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

    /// Reads the real desktops and brings the grid in line with them.
    fn sync(&mut self) -> Option<Desktops> {
        let desktops = Desktops::read()?;
        if self.grid.sync(&desktops.ids(), &desktops.current) {
            config::save_grid(&self.grid);
        }
        Some(desktops)
    }

    fn carry_held(&self) -> bool {
        let vk = match self.config.carry_modifier.trim().to_ascii_lowercase().as_str() {
            "ctrl" => 0x11,
            "alt" => 0x12,
            _ => 0x10,
        };
        unsafe { GetAsyncKeyState(vk) < 0 }
    }

    /// Moves one cell in `dir`, creating a cell when the edge has been pushed
    /// often enough. With `carry` the foreground window comes along.
    fn step(&mut self, dir: Dir, carry: bool) {
        if self.board.is_open() {
            // With the board open the gesture only moves the selection; the
            // desktop changes once, when the board is confirmed.
            self.board.select_dir(dir);
            return;
        }
        let Some(desktops) = self.sync() else { return };
        let current = desktops.current.clone();
        let Some(from) = self.grid.find(&current) else { return };

        if let Some(to) = self.grid.target(from, dir).and_then(|pos| self.grid.id_at(pos).cloned()) {
            self.edge = None;
            self.go(&desktops, from, &to, carry);
            return;
        }

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
        if may_create && pushes >= self.config.edge_create_pushes {
            self.edge = None;
            match winvd::create_desktop().ok().and_then(|d| Some((vd::id_of(&d)?, d))) {
                Some((id, _)) => {
                    self.grid.insert_beside(from, dir, id.clone());
                    self.grid.ephemeral.push(id.clone());
                    self.commit();
                    let Some(desktops) = Desktops::read() else { return };
                    // Inserting to the left or above shifts the cell we came from.
                    let from = self.grid.find(&current).unwrap_or(from);
                    self.go(&desktops, from, &id, carry);
                }
                None => log("create_desktop failed"),
            }
        } else {
            self.edge = Some((dir, pushes));
            let view = self.view(&current, may_create.then_some(dir));
            self.overlay.show(view, None, None);
        }
    }

    fn go(&mut self, desktops: &Desktops, from: Pos, to: &str, carry: bool) {
        let Some(target) = desktops.get(to) else { return };
        let window = unsafe { GetForegroundWindow() };
        let carried = carry
            && vd::is_app_window(window)
            && winvd::is_window_on_current_desktop(window).unwrap_or(false)
            && winvd::move_window_to_desktop(target, &window).is_ok();
        self.bring_followers(desktops, to);
        if let Err(e) = self.switch(target) {
            log(&format!("switch_desktop failed: {e:?}"));
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
            config::save_grid(&self.grid);
        }
        let view = self.view(to, None);
        self.overlay.show(view, Some(from), None);
    }

    /// The trigger was released: do what the steps of the gesture put off.
    fn finish_gesture(&mut self) {
        if std::mem::take(&mut self.in_gesture) {
            if std::mem::take(&mut self.unsettled) {
                vd::focus_top_window();
            }
            config::save_grid(&self.grid);
        }
    }

    fn settle(&mut self, linger_ms: u64) {
        self.overlay.hide_after(linger_ms);
    }

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
                        let windows = here
                            .into_iter()
                            .map(|w| WindowModel { follows: self.follows(&w), app: vd::exe_name(w.pid).unwrap_or_default(), app_name: vd::app_label(w.pid), hwnd: w.hwnd, rect: w.rect, minimized: w.minimized, title: w.title })
                            .collect();
                        CellModel { id: id.clone(), windows }
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
        let pinned = vd::pinned_windows()
            .into_iter()
            .map(|w| WindowModel { follows: false, app: vd::exe_name(w.pid).unwrap_or_default(), app_name: vd::app_label(w.pid), hwnd: w.hwnd, rect: w.rect, minimized: w.minimized, title: w.title })
            .collect();
        Model { rows, pinned, current: desktops.current.clone() }
    }

    fn open_board(&mut self) {
        let Some(desktops) = self.sync() else { return };
        self.following.retain(|hwnd| unsafe { IsWindow(HWND(*hwnd)) }.as_bool());
        self.overlay.hide_after(0);
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

    fn perform(&mut self, action: Action) {
        match action {
            Action::None => return,
            Action::Dismiss => return self.board.close(),
            Action::Cancel => {
                self.board.close();
                if unsafe { IsWindow(self.focus_before_board) }.as_bool() {
                    vd::force_foreground(self.focus_before_board);
                }
                return;
            }
            Action::Go(id, window) => {
                self.board.close();
                let Some(desktops) = self.sync() else { return };
                // An empty id is a window pinned everywhere: stay on this desktop.
                if !id.is_empty() && id != desktops.current {
                    let Some(target) = desktops.get(&id) else { return };
                    self.bring_followers(&desktops, &id);
                    if let Err(e) = self.switch(target) {
                        return log(&format!("switch_desktop failed: {e:?}"));
                    }
                    self.visit(&id);
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
                config::save_grid(&self.grid);
                return;
            }
            Action::MoveWindow { window, cell, monitor } => {
                if let Some(id) = cell {
                    if let Some(target) = Desktops::read().and_then(|d| d.get(&id)) {
                        if let Err(e) = winvd::move_window_to_desktop(target, &window) {
                            log(&format!("move_window_to_desktop failed: {e:?}"));
                        }
                        self.grid.ephemeral.retain(|cell| *cell != id);
                    }
                }
                if let Some(monitor) = monitor {
                    vd::move_to_monitor(window, monitor);
                }
            }
            Action::MoveCell { id, row, index } => self.grid.move_cell(&id, row, index),
            Action::MoveCellToNewRow { id, at } => self.grid.move_cell_to_new_row(&id, at),
            Action::AddCell { row, front } => {
                // Placed in the grid before the next sync, which would otherwise
                // file the unknown desktop under the current row.
                if let Some(id) = winvd::create_desktop().ok().and_then(|d| vd::id_of(&d)) {
                    if let Some(row) = self.grid.rows.get_mut(row) {
                        row.cells.insert(if front { 0 } else { row.cells.len() }, id);
                    }
                }
            }
            Action::AddRow { top } => {
                if let Some(id) = winvd::create_desktop().ok().and_then(|d| vd::id_of(&d)) {
                    let at = if top { 0 } else { self.grid.rows.len() };
                    self.grid.rows.insert(at, Row { name: String::new(), cells: vec![id], last: None });
                }
            }
            Action::RemoveCell(id) => self.remove_cells(&[id]),
            Action::SetPin { window, row, all } => {
                if row {
                    self.following.insert(window.0);
                } else {
                    self.following.remove(&window.0);
                }
                let result = if all {
                    winvd::pin_window(window)
                } else {
                    // Being shown everywhere can also come from its app being pinned.
                    if winvd::is_pinned_app(window).unwrap_or(false) {
                        let _ = winvd::unpin_app(window);
                    }
                    if winvd::is_pinned_window(window).unwrap_or(false) { winvd::unpin_window(window) } else { Ok(()) }
                };
                if let Err(e) = result {
                    log(&format!("pin change failed: {e:?}"));
                }
            }
            Action::CloseWindow(hwnd) => {
                unsafe {
                    let _ = PostMessageW(hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
                    // The window needs a moment to go (or to ask about saving).
                    SetTimer(self.hwnd, REFRESH_TIMER_ID, 500, None);
                }
                return;
            }
            Action::RowMenu(row) => {
                // Shown once this handler has returned (`run_pending_menu`).
                self.pending_row_menu = Some(row);
                return;
            }
            Action::Rename(row, name) => {
                if let Some(row) = self.grid.rows.get_mut(row) {
                    row.name = name;
                }
            }
        }
        self.commit();
        self.refresh_board();
    }

    /// Asks every window of a workspace to close; the workspace stays.
    fn close_row_windows(&mut self, row: usize) {
        let Some(desktops) = self.sync() else { return };
        let Some(ids) = self.grid.rows.get(row).map(|r| r.cells.clone()) else { return };
        for window in vd::windows(&desktops.current).iter().filter(|w| ids.contains(&w.desktop)) {
            unsafe {
                let _ = PostMessageW(window.hwnd, WM_CLOSE, WPARAM(0), LPARAM(0));
            }
        }
        unsafe {
            SetTimer(self.hwnd, REFRESH_TIMER_ID, 600, None);
        }
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
            if self.switch(target).is_err() {
                return;
            }
            let to = to.clone();
            self.visit(&to);
        }
        for (id, to) in targets {
            if let (Some(desktop), Some(target)) = (desktops.get(&id), desktops.get(&to)) {
                match winvd::remove_desktop(desktop, target) {
                    Ok(()) => self.grid.remove(&id),
                    Err(e) => log(&format!("remove_desktop failed: {e:?}")),
                }
            }
        }
        self.commit();
        self.refresh_board();
    }

    /// Deletes desktops, moving their windows to a cell that stays: the left
    /// or right neighbour in the row when there is one, else the current cell
    /// or the nearest other row.
    fn remove_cells(&mut self, ids: &[String]) {
        let Some(desktops) = self.sync() else { return };
        let Some(first) = ids.first().and_then(|id| self.grid.find(id)) else { return };
        let survivors = |row: &Row| row.cells.iter().filter(|c| !ids.contains(c)).cloned().collect::<Vec<_>>();
        let same_row = survivors(&self.grid.rows[first.row]);
        let fallback = if !same_row.is_empty() {
            same_row[first.col.saturating_sub(1).min(same_row.len() - 1)].clone()
        } else if !ids.contains(&desktops.current) {
            desktops.current.clone()
        } else {
            let other = if first.row > 0 { first.row - 1 } else { first.row + 1 };
            let Some(row) = self.grid.rows.get(other) else { return };
            row.last.clone().filter(|id| row.cells.contains(id)).unwrap_or_else(|| row.cells[0].clone())
        };
        let Some(target) = desktops.get(&fallback) else { return };
        if ids.contains(&desktops.current) {
            if self.switch(target).is_err() {
                return;
            }
            self.visit(&fallback);
        }
        for id in ids {
            if let Some(desktop) = desktops.get(id) {
                match winvd::remove_desktop(desktop, target) {
                    Ok(()) => self.grid.remove(id),
                    Err(e) => log(&format!("remove_desktop failed: {e:?}")),
                }
            }
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

    /// The current desktop changed, by us or by Windows' own shortcuts.
    fn desktop_changed(&mut self, old: &str, new: &str) {
        let Some(desktops) = self.sync() else { return };
        self.visit(new);
        // Also covers switches made with Windows' own shortcuts.
        self.bring_followers(&desktops, new);
        if self.grid.ephemeral.iter().any(|id| id == old) {
            // A cell created by an edge push is kept only if it got a window.
            match (desktops.get(old), desktops.get(new)) {
                (Some(left), Some(fallback)) if vd::count_on(left) == 0 => {
                    if winvd::remove_desktop(left, fallback).is_ok() {
                        self.grid.remove(old);
                        self.commit();
                    }
                }
                _ => self.grid.ephemeral.retain(|id| id != old),
            }
        }
        config::save_grid(&self.grid);
        let view = self.view(&desktops.current, None);
        self.overlay.update(view);
        self.refresh_board();
    }

    /// Asks for a new trigger: a small prompt stays up until a button or key
    /// combination is pressed (or Esc).
    fn begin_trigger_change(&mut self) {
        self.end_trigger_prompt();
        unsafe {
            let (w, h) = (560, 96);
            let (sw, sh) = (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN));
            let prompt = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                w!("STATIC"),
                w!("\n새 트리거로 쓸 마우스 버튼이나 키를 누르세요.\n키는 여러 개를 함께 눌러도 됩니다.   Esc: 취소"),
                WS_POPUP | WS_VISIBLE | WS_BORDER | WINDOW_STYLE(1), // SS_CENTER
                (sw - w) / 2,
                (sh - h) / 3,
                w,
                h,
                None,
                None,
                None,
                None,
            );
            self.trigger_prompt = prompt;
        }
        input::begin_capture();
    }

    fn end_trigger_prompt(&mut self) {
        if self.trigger_prompt.0 != 0 {
            unsafe {
                let _ = DestroyWindow(self.trigger_prompt);
            }
            self.trigger_prompt = HWND(0);
        }
        input::cancel_capture();
    }

    /// The user pressed what the trigger should be from now on.
    fn trigger_chosen(&mut self) {
        self.end_trigger_prompt();
        let Some(value) = input::take_captured() else { return };
        config::set_trigger(&value);
        self.apply_config();
        let (triggers, _) = Trigger::parse_list(&value);
        let name = triggers.first().map(Trigger::describe).unwrap_or(value);
        tray::notify(self.hwnd, "트리거를 바꿨습니다", &format!("이제 {name}: 누른 채 밀면 칸 이동, 눌렀다 떼면 전체 보기."));
    }

    /// Applies the config file: hooks and hotkeys.
    fn apply_config(&mut self) {
        let (config, error) = config::load_config();
        if let Some(error) = error {
            warn(&format!("config.toml을 읽지 못해 기본값을 씁니다.\n\n{error}"));
        }
        let (mut triggers, unknown) = Trigger::parse_list(&config.trigger);
        if !unknown.is_empty() {
            warn(&format!("config.toml의 trigger에서 알 수 없는 항목: {}", unknown.join(", ")));
        }
        if triggers.is_empty() {
            triggers.push(Trigger::XButton2);
        }
        input::install(self.hwnd, triggers, config.step_x, config.step_y);

        unsafe {
            for id in 0..=PIN_HOTKEY_ID {
                let _ = UnregisterHotKey(self.hwnd, id);
            }
            let _ = KillTimer(self.hwnd, SLEEP_TIMER_ID);
            if config.sleep_after_minutes > 0 {
                SetTimer(self.hwnd, SLEEP_TIMER_ID, 60_000, None);
            }
            if config.hotkeys {
                let base = MOD_CONTROL | MOD_ALT | MOD_WIN;
                let _ = RegisterHotKey(self.hwnd, PIN_HOTKEY_ID, HOT_KEY_MODIFIERS(base.0), 'P' as u32);
                for (i, key) in [VK_LEFT, VK_RIGHT, VK_UP, VK_DOWN].into_iter().enumerate() {
                    for (carry, mods) in [(0, base), (4, base | MOD_SHIFT)] {
                        if RegisterHotKey(self.hwnd, i as i32 + carry, HOT_KEY_MODIFIERS(mods.0), key.0 as u32).is_err() {
                            log(&format!("hotkey {} could not be registered", i as i32 + carry));
                        }
                    }
                }
            }
        }
        self.config = config;
    }
}

fn on_tray_command(hwnd: HWND, command: Command) {
    match command {
        Command::Peek => {
            with_app(App::open_board);
        }
        Command::ChangeTrigger => unsafe {
            let _ = PostMessageW(hwnd, WM_CHANGE_TRIGGER, WPARAM(0), LPARAM(0));
        },
        Command::OpenConfig => unsafe {
            let path = HSTRING::from(config::config_path().as_os_str());
            ShellExecuteW(None, w!("open"), w!("notepad.exe"), &path, PCWSTR::null(), SW_SHOWNORMAL);
        },
        Command::ReloadConfig => {
            with_app(App::apply_config);
        }
        Command::ToggleAutostart => tray::set_autostart(!tray::autostart_enabled()),
        Command::Exit => unsafe {
            let _ = DestroyWindow(hwnd);
        },
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if hwnd.0 == BOARD_HWND.get() {
        let handled = with_app(|app| app.board_message(message, wparam, lparam)).flatten();
        run_pending_menu(hwnd);
        return handled.unwrap_or_else(|| DefWindowProcW(hwnd, message, wparam, lparam));
    }
    match message {
        WM_STEP => {
            if let Some(dir) = input::dir_from_index(wparam.0) {
                with_app(|app| {
                    app.in_gesture = true;
                    app.step(dir, app.carry_held());

                });
            }
        }
        WM_CLICK => {
            with_app(App::toggle_board);
        }
        WM_CHANGE_TRIGGER => {
            with_app(App::begin_trigger_change);
        }
        input::WM_CAPTURED => {
            with_app(|app| if wparam.0 == 1 { app.trigger_chosen() } else { app.end_trigger_prompt() });
        }
        WM_RELEASE => {
            with_app(|app| {
                app.edge = None;
                app.finish_gesture();
                app.settle(LINGER_RELEASE_MS);
            });
        }
        WM_HOTKEY if wparam.0 as i32 == PIN_HOTKEY_ID => {
            let window = GetForegroundWindow();
            if vd::is_app_window(window) {
                with_app(|app| app.pending_menu = Some((window, app.pin_state(window))));
                vd::force_foreground(hwnd);
                run_pending_menu(hwnd);
                vd::force_foreground(window);
            }
        }
        WM_HOTKEY => {
            if let Some(dir) = input::dir_from_index(wparam.0 % 4) {
                with_app(|app| {
                    app.step(dir, wparam.0 >= 4);
                    app.settle(LINGER_HOTKEY_MS);
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

/// Shows the pin menu a handler asked for. It runs after the handler returned
/// because a menu pumps messages, which must be able to reach the app.
fn run_pending_menu(owner: HWND) {
    let row = with_app(|app| {
        let row = app.pending_row_menu.take()?;
        let (name, summary) = app.board.row_summary(row)?;
        Some((row, format!("{name}: {summary}"), app.grid.rows.len() > 1))
    })
    .flatten();
    if let Some((row, summary, removable)) = row {
        match tray::row_menu(owner, &summary, removable) {
            Some(tray::RowCommand::CloseWindows) => {
                // Closing windows can lose work, so it is asked about in earnest,
                // with "No" as the default answer.
                let text = format!(
                    "{summary}\n\n이 워크스페이스의 창을 전부 닫습니다.\n저장하지 않은 작업은 잃을 수 있고, 되돌릴 수 없습니다.\n\n정말 닫을까요?"
                );
                with_app(|app| app.board.set_modal(true));
                let answer = unsafe {
                    MessageBoxW(owner, &HSTRING::from(text), w!("창을 모두 닫습니다"), MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2)
                };
                with_app(|app| app.board.set_modal(false));
                if answer == IDYES {
                    with_app(|app| app.close_row_windows(row));
                }
            }
            Some(tray::RowCommand::Remove) => {
                with_app(|app| app.remove_row(row));
            }
            None => {}
        }
    }
    if let Some((window, state)) = with_app(|app| app.pending_menu.take()).flatten() {
        if let Some(command) = tray::pin_menu(owner, state) {
            with_app(|app| app.apply_pin(window, command));
        }
    }
}

/// Forwards "current desktop changed" notifications to the UI thread as
/// (old id, new id) pairs.
fn listen(hwnd: HWND) -> (mpsc::Receiver<(String, String)>, Option<winvd::DesktopEventThread>) {
    let (raw_tx, raw_rx) = mpsc::channel::<DesktopEvent>();
    let (tx, rx) = mpsc::channel();
    let listener = winvd::listen_desktop_events(raw_tx).map_err(|e| log(&format!("listener failed: {e:?}"))).ok();
    let target = hwnd.0;
    std::thread::spawn(move || {
        for event in raw_rx {
            if let DesktopEvent::DesktopChanged { new, old } = event {
                if let (Some(old), Some(new)) = (vd::id_of(&old), vd::id_of(&new)) {
                    if tx.send((old, new)).is_err() {
                        break;
                    }
                    unsafe {
                        let _ = PostMessageW(HWND(target), WM_DESKTOP_EVENT, WPARAM(0), LPARAM(0));
                    }
                }
            }
        }
    });
    (rx, listener)
}

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
    let overlay = Minimap::spawn();
    BOARD_HWND.set(board.hwnd().0);
    if Desktops::read().is_none() {
        return warn("가상 데스크톱에 접근하지 못했습니다. 이 윈도우 빌드를 지원하지 않는 것일 수 있습니다.");
    }

    let (events, listener) = listen(hwnd);
    let mut app = App {
        hwnd,
        config: Config::default(),
        grid: config::load_grid(),
        overlay,
        edge: None,
        board,
        focus_before_board: HWND(0),
        trigger_prompt: HWND(0),
        in_gesture: false,
        unsettled: false,
        following: HashSet::new(),
        pending_menu: None,
        pending_row_menu: None,
        seen: HashMap::new(),
        asleep: HashSet::new(),
        started: Instant::now(),
        events,
        _listener: listener,
        taskbar_created: unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) },
    };
    if let Some(desktops) = app.sync() {
        app.visit(&desktops.current);
        app.commit();
    }
    app.apply_config();
    tray::add(hwnd, WM_TRAY);
    if first_run {
        tray::set_autostart(true);
        tray::notify(hwnd, "설치되어 실행 중입니다", "트리거(처음에는 마우스 앞으로 버튼)를 누른 채 밀면 칸 이동, 눌렀다 떼면 전체 보기. 트리거는 이 아이콘을 우클릭해 바꿉니다.");
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

/// Renders sample minimaps into `path` (raw: width, height as i32 LE, then
/// premultiplied BGRA) so the design can be reviewed without touching the desktop.
pub fn render_sample(path: &str) {
    let Ok(mut overlay) = Overlay::new() else { return };
    let view = View {
        rows: vec![vec![false; 4], vec![false, false], vec![false, false, true]],
        anchors: vec![2, 1, 0],
        title: "워크스페이스 2".into(),
        cur: Some(Pos { row: 1, col: 1 }),
        pushing: Some(Dir::Right),
    };
    if let Some((w, h, pixels)) = overlay.render_to_pixels(view, (1.0, 1.0), 2.0) {
        let mut out = Vec::with_capacity(pixels.len() + 8);
        out.extend_from_slice(&w.to_le_bytes());
        out.extend_from_slice(&h.to_le_bytes());
        out.extend_from_slice(&pixels);
        let _ = std::fs::write(path, out);
    }
}

/// Renders the board for the real desktops and windows into `path` (same raw
/// format as `render_sample`), without live thumbnails and without showing it.
pub fn render_board(path: &str) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let Ok(board) = Board::new(wndproc) else { return };
    let overlay = Minimap::spawn();
    let (_tx, events) = mpsc::channel();
    let mut app = App {
        hwnd: HWND(0),
        config: Config::default(),
        grid: config::load_grid(),
        overlay,
        edge: None,
        board,
        focus_before_board: HWND(0),
        trigger_prompt: HWND(0),
        in_gesture: false,
        unsettled: false,
        following: HashSet::new(),
        pending_menu: None,
        pending_row_menu: None,
        seen: HashMap::new(),
        asleep: HashSet::new(),
        started: Instant::now(),
        events,
        _listener: None,
        taskbar_created: 0,
    };
    let Some(desktops) = Desktops::read() else { return };
    app.grid.sync(&desktops.ids(), &desktops.current);
    // A second, shorter row so that ragged rows are visible in the picture.
    if app.grid.rows.len() == 1 && app.grid.rows[0].cells.len() > 1 {
        let moved = app.grid.rows[0].cells.last().cloned().unwrap_or_default();
        app.grid.move_cell_to_new_row(&moved, 1);
        app.grid.rows[1].name = "예시 행".into();
    }
    let model = app.model(&desktops);
    let (w, h) = (2560, 1440);
    if let Some(pixels) = app.board.render_to_pixels(model, app.grid.clone(), (w, h), 1.5) {
        let mut out = Vec::with_capacity(pixels.len() + 8);
        out.extend_from_slice(&w.to_le_bytes());
        out.extend_from_slice(&h.to_le_bytes());
        out.extend_from_slice(&pixels);
        let _ = std::fs::write(path, out);
    }
}
