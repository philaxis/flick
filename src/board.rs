//! The full-screen view: the selected cell's windows as live previews, each
//! with area set by recent active-stay credits and each window's aspect
//! ratio preserved, and a small map of all rows and cells on every monitor.
//! Windows can be dragged onto cells and monitors, cells around the map.
//!
//! The board owns layout, hit-testing, drawing and the DWM thumbnails. It does
//! not touch desktops itself; input is turned into an `Action` for the app.

use crate::{
    grid::{self, CellId, Dir, Grid, Row},
    paint::{self, accent, rect, rgba, warm, white, Canvas, Dib, Painter, Rect},
    tiles::{self, Space},
    usage,
    vd,
};
use std::{
    cell::RefCell,
    collections::HashMap,
    hash::{DefaultHasher, Hash, Hasher},
    time::{Duration, Instant},
};
use windows::{
    core::{w, Error, Result},
    Foundation::Numerics::Matrix3x2,
    Win32::{
        Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
        Graphics::{
            Direct2D::{
                Common::{
                    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_SIZE_U,
                },
                ID2D1Bitmap, ID2D1Factory, ID2D1RenderTarget, D2D1_BITMAP_PROPERTIES,
            },
            DirectWrite::{
                IDWriteTextFormat, DWRITE_TEXT_ALIGNMENT, DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING,
                DWRITE_TRIMMING, DWRITE_TRIMMING_GRANULARITY_CHARACTER,
            },
            Dwm::{
                DwmQueryThumbnailSourceSize, DwmRegisterThumbnail, DwmUnregisterThumbnail,
                DwmUpdateThumbnailProperties, DWM_THUMBNAIL_PROPERTIES, DWM_TNP_OPACITY, DWM_TNP_RECTDESTINATION,
                DWM_TNP_SOURCECLIENTAREAONLY, DWM_TNP_VISIBLE,
            },
            Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
            Gdi::{BitBlt, CreateRoundRectRgn, DeleteObject, GdiFlush, GetDC, InvalidateRect, ReleaseDC, SetWindowRgn, ValidateRect, SRCCOPY},
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            Input::{
                Ime::{
                    ImmGetCompositionStringW, ImmGetContext, ImmNotifyIME, ImmReleaseContext, CPS_CANCEL, GCS_COMPSTR,
                    GCS_RESULTSTR, IME_COMPOSITION_STRING, NI_COMPOSITIONSTR,
                },
                KeyboardAndMouse::{
                    ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VK_BACK, VK_DOWN, VK_ESCAPE, VK_F2, VK_LEFT, VK_RETURN, VK_RIGHT,
                    VK_UP,
                },
            },
            WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DrawIconEx, GetClassLongPtrW, GetWindow, LoadCursorW, RegisterClassW, SendMessageTimeoutW,
                KillTimer, SetTimer, SetWindowPos, SetLayeredWindowAttributes, ShowWindow, LWA_ALPHA, CS_DBLCLKS, DI_NORMAL, GCLP_HICON, HICON, HWND_TOPMOST,
                GetCursorPos, GetAncestor, GetClassNameW, GetWindowThreadProcessId, WindowFromPoint, GA_ROOT, SW_SHOWMINNOACTIVE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOACTIVATE, SWP_NOREDRAW, ICON_BIG, IDC_ARROW, SMTO_ABORTIFHUNG, SWP_SHOWWINDOW, SW_HIDE, WM_ACTIVATE, WM_CHAR, WM_ERASEBKGND,
                WM_GETICON, WM_IME_COMPOSITION, WM_IME_STARTCOMPOSITION, WM_KEYDOWN, WM_LBUTTONDBLCLK,
                WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_PAINT, WM_RBUTTONUP, WM_TIMER, WNDCLASSW,
                GW_OWNER, HTTRANSPARENT, WM_NCHITTEST, WS_EX_NOACTIVATE, WS_EX_TRANSPARENT,
                WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_LAYERED, WS_POPUP,
            },
        },
    },
};

#[derive(Clone)]
pub struct WindowModel {
    pub hwnd: HWND,
    /// Screen rectangle; for a minimized window, where it will be restored to.
    pub rect: RECT,
    pub minimized: bool,
    pub title: String,
    /// Executable name; used for grouping when the window has no icon.
    pub app: String,
    /// A hash of the pixels of the window's own icon; 0 when it has none.
    pub icon_key: u64,
    /// The app's name as people know it; may be empty.
    pub app_name: String,
    /// Follows the user from cell to cell inside its row.
    pub follows: bool,
    /// Kept above the other windows.
    pub topmost: bool,
}

pub struct CellModel {
    pub id: CellId,
    /// Marked by the user to stand out.
    pub emphasised: bool,
    /// Square root of this cell's share of the highest active-stay score.
    pub usage: f32,
    /// Topmost first.
    pub windows: Vec<WindowModel>,
}

pub struct RowModel {
    pub name: String,
    pub memory: u64,
    /// Its apps' memory was pushed out of RAM after being idle.
    pub asleep: bool,
    pub cells: Vec<CellModel>,
}

/// Everything the board shows, built by the app from the grid and the windows.
pub struct Model {
    pub rows: Vec<RowModel>,
    /// Windows shown on every desktop.
    pub pinned: Vec<WindowModel>,
    pub current: CellId,
}

impl Model {
    fn cell_count(&self) -> usize {
        self.rows.iter().map(|row| row.cells.len()).sum()
    }

    fn cell(&self, id: &str) -> Option<&CellModel> {
        self.rows.iter().flat_map(|row| &row.cells).find(|cell| cell.id == id)
    }

    /// Popup membership keeps row followers but leaves out All rows windows.
    fn popup_windows(&self, id: &str) -> Vec<(&WindowModel, bool)> {
        self.tile_windows(id).into_iter().filter(|(_, all)| !all).collect()
    }

    /// Normalize across the whole cell, before splitting monitors or pinned
    /// sections, so a window has the same weight in the board and its popup.
    fn window_weights(&self, id: &str, grid: &Grid) -> HashMap<isize, f32> {
        let now = usage::now();
        let scores: Vec<_> = self.tile_windows(id).into_iter().map(|(w, _)| {
            let score = grid.window_usage.get(&w.hwnd.0)
                .map_or(0.0, |score| score.value(now, usage::WINDOW_HALF_LIFE));
            (w.hwnd.0, score)
        }).collect();
        let maximum = scores.iter().map(|(_, score)| *score).fold(0.0, f64::max);
        scores.into_iter().map(|(hwnd, score)| (hwnd, usage::preview_factor(score, maximum))).collect()
    }

    /// Shared membership and order, including both kinds of pinned windows.
    fn tile_windows(&self, id: &str) -> Vec<(&WindowModel, bool)> {
        let cell = self.cell(id).into_iter().flat_map(|cell| &cell.windows).map(|w| (w, false));
        // Windows pinned to their row are shown in every cell of it: going
        // there brings them along. So their part stays put within the row.
        let row = self.rows.iter().find(|row| row.cells.iter().any(|cell| cell.id == id));
        let followers = row
            .into_iter()
            .flat_map(|row| &row.cells)
            .filter(|cell| cell.id != id)
            .flat_map(|cell| &cell.windows)
            .filter(|w| w.follows)
            .map(|w| (w, false));
        let mut windows: Vec<(&WindowModel, bool)> =
            cell.chain(followers).chain(self.pinned.iter().map(|w| (w, true))).collect();
        windows.sort_by_cached_key(|(w, _)| (w.app_name.to_lowercase(), w.app.clone(), w.hwnd.0));
        windows
    }
}

/// What the user asked for; carried out by the app.
pub enum Action {
    None,
    /// Move the board selection and save its row anchor without switching desktops.
    Step(Dir),
    /// Close without going anywhere and give focus back.
    Cancel,
    /// The board lost focus; close without touching focus.
    Dismiss,
    /// Switch to a cell and close; optionally focus one of its windows. An
    /// empty id (a window shown on every desktop) stays on the current cell.
    Go(CellId, Option<HWND>),
    /// Move a window to another cell, another monitor (given in screen
    /// coordinates), or both.
    MoveWindow { window: HWND, cell: Option<CellId>, monitor: Option<RECT> },
    MoveCell { id: CellId, row: usize, index: usize },
    MoveCellToNewRow { id: CellId, at: usize },
    /// Add a desktop to a row, at its front or its end.
    AddCell { row: usize, front: bool },
    /// Add a row with one desktop, above the first or below the last.
    AddRow { top: bool },
    RemoveCell(CellId),
    /// Ask a window to close.
    CloseWindow(HWND),
    /// Set how one window is pinned: following within its row, shown on
    /// every desktop, or neither (never both).
    SetPin { window: HWND, row: bool, all: bool },
    /// Keep a window above the others, or stop doing so.
    SetTop { window: HWND, on: bool },
    /// Show the menu of a shelf (close its windows, remove it).
    RowMenu(usize),
    /// Make a cell stand out, or stop it doing so.
    ToggleEmphasis(CellId),
    /// Move a shelf to before row `at`, counted without the moved one.
    MoveRow { from: usize, at: usize },
    Rename(usize, String),
}

/// Height of the line at the bottom that says what the thing under the
/// cursor is or does, in unscaled units.
const FOOTER: f32 = 44.0;
/// While the board is up, how often it checks that nothing came over it.
const ON_TOP_TIMER_ID: usize = 2;
const ON_TOP_MS: u32 = 250;
const POPUP_TIMER_ID: usize = 3;
const POPUP_GLIDE_MS: u64 = 120;
const POPUP_GRACE_MS: u64 = 200;
const POPUP_FADE_IN_MS: u64 = 80;
const POPUP_FADE_OUT_MS: u64 = 120;
/// The longest name a shelf can be given, in characters.
const NAME_LENGTH: usize = 24;
/// Sent after the cursor leaves a window that called `TrackMouseEvent`.
const WM_MOUSELEAVE: u32 = 0x02A3;

fn contains(r: &Rect, x: f32, y: f32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

fn grow(r: &Rect, by: f32) -> Rect {
    Rect { left: r.left - by, top: r.top - by, right: r.right + by, bottom: r.bottom + by }
}

fn size(r: &Rect) -> (f32, f32) {
    (r.right - r.left, r.bottom - r.top)
}

fn centre(r: &Rect) -> (f32, f32) {
    ((r.left + r.right) / 2.0, (r.top + r.bottom) / 2.0)
}

/// The colour of what is pinned to all rows, on its pin and its part.
fn everywhere(a: f32) -> D2D1_COLOR_F {
    rgba(0.62, 0.45, 1.0, a)
}

fn danger(a: f32) -> D2D1_COLOR_F {
    rgba(0.93, 0.33, 0.33, a)
}

fn format_memory(bytes: u64) -> String {
    let mb = bytes as f64 / (1024.0 * 1024.0);
    if mb >= 1024.0 {
        format!("≈ {:.1} GB", mb / 1024.0)
    } else {
        format!("≈ {mb:.0} MB")
    }
}

/// How a window is pinned, as the one pin button of its tile shows it.
/// Clicking the button goes on to the next state.
#[derive(Clone, Copy, PartialEq)]
enum Pin {
    Off,
    /// Follows the user within its shelf.
    Row,
    /// Shown on every desktop.
    Everywhere,
}

impl Pin {
    fn of(window: &WindowModel, everywhere: bool) -> Pin {
        match (window.follows, everywhere) {
            (_, true) => Pin::Everywhere,
            (true, _) => Pin::Row,
            _ => Pin::Off,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Pin::Off => "Pin",
            Pin::Row => "This row",
            Pin::Everywhere => "All rows",
        }
    }

    /// Width of the button, unscaled: the same in every state, so that a
    /// click changes what it says and nothing else.
    const WIDTH: f32 = 74.0;
    /// Width of the "above the others" button that goes with it.
    const TOP_WIDTH: f32 = 46.0;
}

/// One app in a map cell: its icon and how many windows it has there.
struct AppBadge {
    /// A window of the app, whose icon is shown.
    hwnd: HWND,
    icon: Rect,
    count: usize,
    /// Where the count is written (only when above one).
    label: Rect,
}

struct CellLayout {
    id: CellId,
    emphasised: bool,
    usage: f32,
    body: Rect,
    close: Rect,
    apps: Vec<AppBadge>,
    /// How many apps did not fit, and where that is written: in the bottom
    /// right corner, clear of the icons.
    more: usize,
    more_label: Rect,
}

struct RowLayout {
    /// The whole label area left of the row; a right-click in it opens the
    /// row's menu.
    header: Rect,
    name: Rect,
    top: f32,
    bottom: f32,
    cells: Vec<CellLayout>,
    /// The "add a cell" buttons after and before the row.
    plus: Rect,
    plus_left: Rect,
    /// The apps of the windows pinned to the row, at the right of its label:
    /// they are in whichever of its cells the user is, so no cell shows them.
    pins: Vec<AppBadge>,
    pins_more: usize,
    pins_more_label: Rect,
}

/// One window of the selected cell, shown large with its title.
struct TileLayout {
    hwnd: HWND,
    frame: Rect,
    icon: Rect,
    /// The title bar from the icon to the close button. The pin button
    /// takes its place at the right end of this while it is shown.
    title: Rect,
    thumb: Rect,
    /// The "above the others" button, the pin button and the close button
    /// at the right of the title bar.
    top: Rect,
    pin: Rect,
    close: Rect,
}

/// A cell's small previews, above the map on the monitor being pointed at.
struct PopupLayout {
    id: CellId,
    monitor: usize,
    frame: Rect,
    cell: Rect,
    label: Rect,
    body: Rect,
    tiles: Vec<TileLayout>,
}

impl PopupLayout {
    fn move_to(&mut self, frame: Rect, cell: Rect) {
        let old = self.frame;
        let place = |r: Rect| {
            let x = |x| frame.left + (x - old.left) * size(&frame).0 / size(&old).0.max(1.0);
            let y = |y| frame.top + (y - old.top) * size(&frame).1 / size(&old).1.max(1.0);
            Rect { left: x(r.left), right: x(r.right), top: y(r.top), bottom: y(r.bottom) }
        };
        self.label = place(self.label);
        self.body = place(self.body);
        for tile in &mut self.tiles {
            tile.frame = place(tile.frame);
            tile.thumb = place(tile.thumb);
        }
        self.frame = frame;
        self.cell = cell;
    }
}

/// Geometry and opacity have separate clocks so crossing a gap never restarts a glide.
struct PopupAnimation {
    from: (Rect, Rect),
    target: (Rect, Rect),
    moved: Instant,
    fade_from: f32,
    faded: Instant,
    leave_at: Option<Instant>,
}

impl PopupAnimation {
    fn new(frame: Rect, cell: Rect, now: Instant) -> Self {
        Self { from: (frame, cell), target: (frame, cell), moved: now, fade_from: 0.0, faded: now, leave_at: None }
    }

    fn sample(&self, now: Instant) -> (Rect, Rect, f32) {
        let progress = |since: Instant, ms| (now.saturating_duration_since(since).as_secs_f32() * 1000.0 / ms as f32).min(1.0);
        let ease = |t: f32| 1.0 - (1.0 - t).powi(3);
        let t = ease(progress(self.moved, POPUP_GLIDE_MS));
        let blend = |a: Rect, b: Rect| if t == 1.0 { b } else { Rect {
            left: a.left + (b.left - a.left) * t, top: a.top + (b.top - a.top) * t,
            right: a.right + (b.right - a.right) * t, bottom: a.bottom + (b.bottom - a.bottom) * t,
        } };
        let fade_in = |at: Instant| {
            let t = (at.saturating_duration_since(self.faded).as_secs_f32() * 1000.0 / POPUP_FADE_IN_MS as f32).min(1.0);
            self.fade_from + (1.0 - self.fade_from) * ease(t)
        };
        let alpha = match self.leave_at {
            Some(at) if now >= at => fade_in(at) * (1.0 - ease(progress(at, POPUP_FADE_OUT_MS))),
            _ => fade_in(now),
        };
        (blend(self.from.0, self.target.0), blend(self.from.1, self.target.1), alpha)
    }

    fn show(&mut self, now: Instant) {
        if self.leave_at.is_some() {
            self.fade_from = self.sample(now).2;
            self.faded = now;
            self.leave_at = None;
        }
    }

    fn glide_to(&mut self, frame: Rect, cell: Rect, now: Instant) {
        let (from, from_cell, _) = self.sample(now);
        self.from = (from, from_cell);
        self.target = (frame, cell);
        self.moved = now;
        self.show(now);
    }

    fn leave(&mut self, now: Instant) {
        self.leave_at.get_or_insert(now + Duration::from_millis(POPUP_GRACE_MS));
    }

    /// One wake-up for a settled gap; frame ticks only while something changes.
    fn next_tick(&self, now: Instant) -> Option<u32> {
        let (frame, cell, alpha) = self.sample(now);
        let goal = if self.leave_at.is_some_and(|at| now >= at) { 0.0 } else { 1.0 };
        if frame != self.target.0 || cell != self.target.1 || alpha != goal {
            Some(16)
        } else {
            self.leave_at.filter(|at| *at > now).map(|at| at.duration_since(now).as_millis().max(1) as u32)
        }
    }
}

/// The map of all rows and cells, in the coordinates of the monitor it is on.
#[derive(Default)]
struct MapLayout {
    top: f32,
    /// The column on which every row's landing cell is lined up.
    spine: Rect,
    rows: Vec<RowLayout>,
    /// The "add a row" buttons below and above the rows.
    add_row: Rect,
    add_row_top: Rect,
}

/// The map as laid out for one monitor.
#[derive(Default)]
struct Map {
    /// Index of the monitor in `Board::monitors`, and its rectangle there.
    monitor: usize,
    area: Rect,
    layout: MapLayout,
}

#[derive(Clone, PartialEq, Default, Debug)]
enum Hit {
    #[default]
    Nothing,
    /// A tile. `cell` is empty for a window pinned to every desktop.
    Window { cell: CellId, hwnd: isize },
    /// A cell of the map; also the handle for dragging it.
    Cell(CellId),
    /// A tile's pin button and its close button.
    Pin(isize),
    Top(isize),
    TileClose(isize),
    CellClose(CellId),
    Plus(usize),
    PlusLeft(usize),
    AddRow,
    AddRowTop,
    RowName(usize),
}

/// Where a dragged cell would be dropped.
#[derive(Clone, Copy, PartialEq)]
enum Slot {
    /// Into `row` at `index` (counted without the dragged cell).
    Row { row: usize, index: usize, x: f32 },
    NewRow { at: usize, y: f32 },
}

/// For this long after a click added or took away something, another click
/// on the same spot does nothing.
const SECOND_CLICK_MS: u128 = 500;

/// How long a tile says that its window could not be asked to close.
const REFUSAL_MS: u128 = 6000;

struct Press {
    hit: Hit,
    x: f32,
    y: f32,
    dragging: bool,
}

struct Fonts {
    /// A tile's window title and, above it, the smaller app name.
    title: IDWriteTextFormat,
    sub: IDWriteTextFormat,
    label: IDWriteTextFormat,
    small: IDWriteTextFormat,
    centered: IDWriteTextFormat,
}

pub struct Board {
    hwnd: HWND,
    /// An owned window keeps the panel above the main DWM previews without
    /// hiding or moving them, and lets pointer input pass through.
    popup_hwnd: HWND,
    popup: Option<PopupLayout>,
    popup_thumbs: Vec<(isize, isize)>,
    popup_animation: Option<PopupAnimation>,
    popup_alpha: f32,
    popup_timer: Option<Instant>,
    factory: ID2D1Factory,
    canvas: Option<Canvas>,
    fonts: Option<Fonts>,
    open: bool,
    model: Model,
    /// The app's grid as last supplied, for laying out and labelling cells.
    grid: Grid,
    selected: CellId,
    /// The selected cell's windows, the main content of the board, in the
    /// board window's coordinates.
    tiles: Vec<TileLayout>,
    /// Where the home monitor's tiles go; says so when there are none.
    tiles_area: Rect,
    /// Where the selected cell is named, in the home monitor's coordinates.
    label: Rect,
    /// The map on every monitor, the home monitor's first. Never empty.
    maps: Vec<Map>,
    size: (i32, i32),
    scale: f32,
    /// (source window, thumbnail handle) of every tile, in stacking order.
    thumbs: Vec<(isize, isize)>,
    icons: HashMap<isize, ID2D1Bitmap>,
    hover: Hit,
    press: Option<Press>,
    cursor: (f32, f32),
    /// A confirmation box owned by the board is up.
    modal: bool,
    /// Every monitor, in the board window's coordinates. The board spans
    /// them all, each with a map of its own; `home` (the primary one) also
    /// carries the cell label and the hint line, and sets the scale.
    monitors: Vec<Rect>,
    home: Rect,
    /// Screen position of the board window's top-left corner.
    origin: (i32, i32),
    /// The row being renamed and the text typed so far.
    editing: Option<(usize, String)>,
    /// What the input method is still putting together (a Korean syllable
    /// in the making), shown after the typed text.
    composing: String,
    /// A window that could not be asked to close, and when that was found.
    refused: Option<(isize, Instant)>,
    /// When and where a click last added or took away something. What it
    /// did moves other things under the cursor, and a second click made in
    /// the same breath must not land on those.
    changed_at: Option<(Instant, f32, f32)>,
    /// The headings of the parts the tiles are placed in, when pinned
    /// windows have a part of their own.
    /// Each part's heading, the part itself, and its name.
    sections: Vec<(Rect, Rect, &'static str)>,
    /// Shell windows put out of the way while the board is open
    /// (`keep_on_top`), to be given back when it closes.
    pushed_aside: Vec<HWND>,
}

/// The parts of `area` the tiles go into: all in one, or when some windows
/// are pinned, those in parts of their own at the left, where they stay
/// whichever cell is looked at (`tiles::pin_parts`). Each part has a
/// heading, or none.
fn split_area<'a>(
    here: &[(&'a WindowModel, bool)],
    area: &Rect,
    s: f32,
) -> Vec<(Rect, Option<&'static str>, Vec<(&'a WindowModel, bool)>)> {
    let pick = |f: fn(&WindowModel, bool) -> bool| -> Vec<(&'a WindowModel, bool)> { here.iter().filter(|(w, e)| f(w, *e)).copied().collect() };
    let all = pick(|_, everywhere| everywhere);
    let row = pick(|w, everywhere| !everywhere && w.follows);
    let rest = pick(|w, everywhere| !everywhere && !w.follows);
    if all.is_empty() && row.is_empty() {
        return vec![(*area, None, here.to_vec())];
    }
    let (head, gap) = (24.0 * s, 20.0 * s);
    let parts = tiles::pin_parts(all.len(), row.len(), size(area), gap, head, tile_least(s));
    let place = |(x, y, w, h): tiles::Part| Rect { top: area.top + y + head, ..rect(area.left + x, area.top + y, w, h) };
    let mut out = Vec::new();
    for (part, name, group) in [(parts.all, "All rows", all), (parts.row, "This row", row)] {
        if let Some(part) = part {
            out.push((place(part), Some(name), group));
        }
    }
    out.push((place(parts.rest), Some("This cell"), rest));
    out
}

impl Board {
    pub fn new(wndproc: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT) -> Result<Board> {
        unsafe {
            let instance = GetModuleHandleW(None)?;
            let class = w!("flick.board");
            RegisterClassW(&WNDCLASSW {
                style: CS_DBLCLKS,
                // Without a class cursor Windows keeps showing the "busy" one.
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                lpfnWndProc: Some(wndproc),
                hInstance: instance.into(),
                lpszClassName: class,
                ..Default::default()
            });
            // A tool window stays out of the taskbar and Alt+Tab and is not
            // counted as an app window on any desktop.
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                class,
                w!("Flick"),
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                instance,
                None,
            );
            if hwnd.0 == 0 {
                return Err(Error::from_win32());
            }
            let popup_class = w!("flick.board.preview");
            RegisterClassW(&WNDCLASSW {
                lpfnWndProc: Some(popup_wndproc),
                hInstance: instance.into(),
                lpszClassName: popup_class,
                ..Default::default()
            });
            let popup_hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT | WS_EX_LAYERED,
                popup_class, w!("Flick preview"), WS_POPUP,
                0, 0, 0, 0, hwnd, None, instance, None,
            );
            if popup_hwnd.0 == 0 {
                return Err(Error::from_win32());
            }
            SetLayeredWindowAttributes(popup_hwnd, COLORREF(0), 0, LWA_ALPHA)?;
            Ok(Board {
                hwnd,
                popup_hwnd,
                popup: None,
                popup_thumbs: Vec::new(),
                popup_animation: None,
                popup_alpha: 0.0,
                popup_timer: None,
                factory: paint::d2d_factory()?,
                canvas: None,
                fonts: None,
                open: false,
                model: Model { rows: Vec::new(), pinned: Vec::new(), current: String::new() },
                grid: Grid::default(),
                selected: String::new(),
                tiles: Vec::new(),
                tiles_area: Rect::default(),
                label: Rect::default(),
                maps: vec![Map::default()],
                size: (0, 0),
                scale: 1.0,
                thumbs: Vec::new(),
                icons: HashMap::new(),
                hover: Hit::Nothing,
                press: None,
                cursor: (0.0, 0.0),
                modal: false,
                monitors: Vec::new(),
                home: Rect::default(),
                origin: (0, 0),
                editing: None,
                composing: String::new(),
                refused: None,
                changed_at: None,
                sections: Vec::new(),
                pushed_aside: Vec::new(),
            })
        }
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// What a shelf's menu is headed with: its name, how many windows
    /// it has and an estimate of the memory they take.
    pub fn row_summary(&self, row: usize) -> Option<String> {
        let model = self.model.rows.get(row)?;
        let windows: usize = model.cells.iter().map(|c| c.windows.len()).sum();
        let asleep = if model.asleep { "  ·  asleep" } else { "" };
        let name = grid::row_title(&model.name, row);
        let count = if windows == 1 { "1 window".to_owned() } else { format!("{windows} windows") };
        Some(format!("{name}  ·  {count}  ·  {}{asleep}", format_memory(model.memory)))
    }

    /// A question about the board is up and waits for its answer.
    pub fn is_modal(&self) -> bool {
        self.modal
    }

    pub fn set_modal(&mut self, modal: bool) {
        self.modal = modal;
    }

    pub fn selected(&self) -> CellId {
        self.selected.clone()
    }

    /// Covers every monitor and shows `model`.
    pub fn open(&mut self, model: Model, grid: Grid) {
        let monitors = vd::monitors();
        // Home is the primary monitor rather than the one under the cursor,
        // so the board looks the same wherever the cursor is.
        let Some(home) = monitors.iter().find(|m| m.primary) else { return };
        // One window over every monitor, so no screen is left showing the
        // desktop while the board is up.
        let bounds = || monitors.iter().map(|m| m.bounds);
        let (left, top) = (bounds().map(|r| r.left).min().unwrap_or(0), bounds().map(|r| r.top).min().unwrap_or(0));
        let (right, bottom) = (bounds().map(|r| r.right).max().unwrap_or(0), bounds().map(|r| r.bottom).max().unwrap_or(0));
        let local = |r: &RECT| Rect {
            left: (r.left - left) as f32,
            top: (r.top - top) as f32,
            right: (r.right - left) as f32,
            bottom: (r.bottom - top) as f32,
        };
        self.monitors = monitors.iter().map(|m| local(&m.bounds)).collect();
        self.home = local(&home.bounds);
        if self.scale != home.scale {
            self.fonts = None;
        }
        self.scale = home.scale;
        self.size = (right - left, bottom - top);
        self.origin = (left, top);
        self.selected = model.current.clone();
        self.hover = Hit::Nothing;
        self.press = None;
        self.hide_popup();
        self.refused = None;
        self.stop_editing();
        self.open = true;
        crate::input::set_board_open(true);
        unsafe {
            let _ = SetWindowPos(self.hwnd, HWND_TOPMOST, left, top, self.size.0, self.size.1, SWP_SHOWWINDOW);
            SetTimer(self.hwnd, ON_TOP_TIMER_ID, ON_TOP_MS, None);
        }
        self.set_model(model, grid);
    }

    /// Replaces the content, keeping the selection when its cell still exists.
    pub fn set_model(&mut self, model: Model, grid: Grid) {
        if !self.open {
            return;
        }
        self.model = model;
        self.grid = grid;
        if self.grid.find(&self.selected).is_none() {
            self.selected = self.model.current.clone();
        }
        self.press = None;
        self.hide_popup();
        self.relayout();
        self.hover_under_cursor();
        self.register_thumbnails();
        self.update_popup(Instant::now());
        self.invalidate();
    }

    /// Takes what is under the cursor now as hovered. The board opening or
    /// its content changing puts things under a cursor that has not moved.
    fn hover_under_cursor(&mut self) {
        let mut at = POINT::default();
        unsafe {
            if GetCursorPos(&mut at).is_err() {
                return;
            }
        }
        let (x, y) = ((at.x - self.origin.0) as f32, (at.y - self.origin.1) as f32);
        self.cursor = (x, y);
        self.hover = self.hit_test(x, y);
    }

    /// A window could not be asked to close: its tile says so for a while.
    pub fn close_refused(&mut self, hwnd: HWND) {
        self.refused = Some((hwnd.0, Instant::now()));
        self.invalidate();
    }

    pub fn close(&mut self) {
        if !self.open {
            return;
        }
        self.open = false;
        crate::input::set_board_open(false);
        self.hide_popup();
        self.clear_thumbnails();
        self.icons.clear();
        // A picture the size of every screen together, of no use until the
        // board is opened again.
        self.canvas = None;
        self.press = None;
        unsafe {
            let _ = ReleaseCapture();
            let _ = KillTimer(self.hwnd, ON_TOP_TIMER_ID);
            ShowWindow(self.hwnd, SW_HIDE);
            for window in self.pushed_aside.drain(..) {
                crate::vd::put_notice_away(window);
            }
        }
    }

    /// Shows a selection whose row anchors the app has already saved.
    pub fn select(&mut self, id: CellId, grid: Grid) {
        self.grid = grid;
        self.selected = id;
        self.hide_popup();
        self.relayout();
        self.hover_under_cursor();
        self.register_thumbnails();
        self.update_popup(Instant::now());
        self.invalidate();
    }

    /// Puts the board back above a window that came up over it while it is
    /// open. A few seconds after a desktop is made, the shell's window for
    /// its little notices above the taskbar (a `XamlExplorerHostIslandWindow`,
    /// the one of the desktop's name and the volume) comes over everything,
    /// unseen, and takes the clicks and the hovering there: the lower rows of
    /// the map and their + buttons stopped answering, and a click there
    /// closed the board. Being the shell's, it stays above whatever is
    /// raised; minimized it is out of the way (`vd::set_switch_label_hidden`).
    fn keep_on_top(&mut self) {
        unsafe {
            let mut at = POINT::default();
            if GetCursorPos(&mut at).is_err() {
                return;
            }
            let under = WindowFromPoint(at);
            if under == self.hwnd || GetAncestor(under, GA_ROOT) == self.hwnd {
                return;
            }
            // Our own menus, questions and windows are over it on purpose.
            let mut pid = 0;
            GetWindowThreadProcessId(under, Some(&mut pid));
            if self.modal || pid == windows::Win32::System::Threading::GetCurrentProcessId() {
                return;
            }
            // Only what is over the board: a point off every screen has nothing of it.
            let (x, y) = ((at.x - self.origin.0) as f32, (at.y - self.origin.1) as f32);
            if !self.monitors.iter().any(|m| contains(m, x, y)) {
                return;
            }
            let mut class = [0u16; 64];
            let n = GetClassNameW(under, &mut class).max(0) as usize;
            let class = String::from_utf16_lossy(&class[..n]);
            debug_log!("board: {:x} {class} came over the board at {},{}", under.0, at.x, at.y);
            if class == "XamlExplorerHostIslandWindow" {
                // Without taking the keyboard from the board, nor giving it back.
                ShowWindow(under, SW_SHOWMINNOACTIVE);
                if !self.pushed_aside.contains(&under) {
                    self.pushed_aside.push(under);
                }
            } else {
                let _ = SetWindowPos(self.hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            }
        }
    }

    fn invalidate(&self) {
        unsafe {
            InvalidateRect(self.hwnd, None, false);
        }
    }

    // ---- layout -----------------------------------------------------------

    /// The windows shown as tiles: those of the selected cell and the ones
    /// pinned to every desktop (flagged), which are visible there as well.
    /// Their order depends only on what the windows are (app, then the
    /// window's handle), never on which was used last, so the same windows
    /// always appear in the same places.
    fn tile_windows(&self) -> Vec<(&WindowModel, bool)> {
        self.model.tile_windows(&self.selected)
    }


    fn home_index(&self) -> usize {
        self.monitors.iter().position(|m| *m == self.home).unwrap_or(0)
    }

    /// Index of the monitor a window is on (by its centre), the home monitor
    /// when it is off every screen.
    fn monitor_of(&self, window: &WindowModel) -> usize {
        let r = window.rect;
        let x = ((r.left + r.right) / 2 - self.origin.0) as f32;
        let y = ((r.top + r.bottom) / 2 - self.origin.1) as f32;
        self.monitors.iter().position(|m| contains(m, x, y)).unwrap_or_else(|| self.home_index())
    }

    /// A monitor's rectangle in screen coordinates.
    fn monitor_on_screen(&self, monitor: usize) -> Option<RECT> {
        let m = self.monitors.get(monitor)?;
        Some(RECT {
            left: m.left as i32 + self.origin.0,
            top: m.top as i32 + self.origin.1,
            right: m.right as i32 + self.origin.0,
            bottom: m.bottom as i32 + self.origin.1,
        })
    }

    /// Lays out every monitor: the map at its bottom, so that a window can be
    /// dropped on a cell on the very monitor it should appear on, and above
    /// it the tiles of the windows that are on that monitor.
    fn relayout(&mut self) {
        if self.monitors.is_empty() {
            return;
        }
        let s = self.scale;
        let margin = 40.0 * s;
        // Each row is shifted so that the cell a vertical move would land on
        // sits in the same column for all rows.
        let anchors: Vec<usize> = self.grid.rows.iter().map(Row::anchor).collect();
        let windows = self.tile_windows();
        let weights = self.model.window_weights(&self.selected, &self.grid);
        let mut tiles = Vec::new();
        let mut tiles_area = Rect::default();
        let mut sections = Vec::new();
        let mut maps = Vec::new();
        for (m, monitor) in self.monitors.iter().enumerate() {
            let is_home = *monitor == self.home;
            let (w, h) = size(monitor);
            let layout = layout_map(&self.model, &anchors, (w, h - FOOTER * s), s);
            // The home monitor has the cell's label above its tiles.
            let area = Rect {
                left: monitor.left + margin,
                top: monitor.top + if is_home { 64.0 * s } else { margin },
                right: monitor.right - margin,
                bottom: monitor.top + layout.top - 26.0 * s,
            };
            let here: Vec<(&WindowModel, bool)> = windows.iter().filter(|(w, _)| self.monitor_of(w) == m).copied().collect();
            for (part, name, group) in split_area(&here, &area, s) {
                if let Some(name) = name {
                    sections.push((Rect { top: part.top - 24.0 * s, bottom: part.top, ..part }, part, name));
                }
                // The part of the cell's own windows comes last.
                if is_home {
                    tiles_area = part;
                }
                tiles.extend(layout_tiles(&group, &part, s, &weights));
            }
            maps.push(Map { monitor: m, area: *monitor, layout });
        }
        maps.sort_by_key(|map| map.area != self.home);
        self.label = rect(margin, 22.0 * s, size(&self.home).0 - 2.0 * margin, 30.0 * s);
        #[cfg(feature = "debug-log")]
        if self.open {
            // Where things are, in screen coordinates, for telling a test where to click.
            let at = |r: &Rect, map: &Map| format!("{:.0},{:.0}", centre(r).0 + map.area.left + self.origin.0 as f32, centre(r).1 + map.area.top + self.origin.1 as f32);
            let flat = Map::default();
            let home = &maps[maps.iter().position(|map| map.area == self.home).unwrap_or(0)];
            let rows: Vec<String> = home.layout.rows.iter().map(|row| {
                format!("name {} plusleft {} plus {} cells [{}]", at(&row.name, home), at(&row.plus_left, home), at(&row.plus, home),
                    row.cells.iter().map(|c| format!("{}@{}x{}", &c.id[..4.min(c.id.len())], at(&c.body, home), at(&c.close, home))).collect::<Vec<_>>().join(" "))
            }).collect();
            debug_log!("board: layout addrowtop {} addrow {} rows {}", at(&home.layout.add_row_top, home), at(&home.layout.add_row, home), rows.join(" | "));
            for tile in &tiles {
                debug_log!("board: tile {:x} top {} pin {} close {}", tile.hwnd.0, at(&tile.top, &flat), at(&tile.pin, &flat), at(&tile.close, &flat));
            }
        }
        self.tiles = tiles;
        self.sections = sections;
        self.tiles_area = tiles_area;
        self.maps = maps;
    }

    /// The map of the monitor the point is on (the home one otherwise), with
    /// the point in that monitor's coordinates.
    fn map_at(&self, x: f32, y: f32) -> (&Map, f32, f32) {
        let map = self.maps.iter().find(|map| contains(&map.area, x, y)).unwrap_or(&self.maps[0]);
        (map, x - map.area.left, y - map.area.top)
    }

    /// A tile acts like its window in the map; one pinned everywhere belongs
    /// to no cell.
    fn tile_hit(&self, hwnd: HWND) -> Hit {
        let pinned = self.model.pinned.iter().any(|w| w.hwnd == hwnd);
        Hit::Window { cell: if pinned { String::new() } else { self.selected.clone() }, hwnd: hwnd.0 }
    }

    fn hit_test(&self, x: f32, y: f32) -> Hit {
        if let Some(tile) = self.tiles.iter().find(|tile| contains(&tile.frame, x, y)) {
            if contains(&tile.pin, x, y) {
                return Hit::Pin(tile.hwnd.0);
            }
            if contains(&tile.top, x, y) {
                return Hit::Top(tile.hwnd.0);
            }
            if contains(&tile.close, x, y) {
                return Hit::TileClose(tile.hwnd.0);
            }
            return self.tile_hit(tile.hwnd);
        }
        let (map, x, y) = self.map_at(x, y);
        if contains(&map.layout.add_row, x, y) {
            return Hit::AddRow;
        }
        if contains(&map.layout.add_row_top, x, y) {
            return Hit::AddRowTop;
        }
        // The last cell cannot be removed, so it has no close button.
        let removable = self.model.cell_count() > 1;
        for (r, row) in map.layout.rows.iter().enumerate() {
            if contains(&row.plus_left, x, y) {
                return Hit::PlusLeft(r);
            }
            if contains(&row.name, x, y) {
                return Hit::RowName(r);
            }
            if contains(&row.plus, x, y) {
                return Hit::Plus(r);
            }
            for cell in &row.cells {
                if contains(&cell.close, x, y) && removable {
                    return Hit::CellClose(cell.id.clone());
                }
                if contains(&cell.body, x, y) {
                    return Hit::Cell(cell.id.clone());
                }
            }
        }
        Hit::Nothing
    }

    /// The row whose label area the point is in.
    fn row_header_at(&self, x: f32, y: f32) -> Option<usize> {
        let (map, x, y) = self.map_at(x, y);
        map.layout.rows.iter().position(|row| contains(&row.header, x, y))
    }

    /// The map cell under a point, and the monitor whose map it is on.
    fn cell_at(&self, x: f32, y: f32) -> Option<(&CellLayout, usize)> {
        // A little slack around each square makes it an easier drop target.
        let slack = 5.0 * self.scale;
        let (map, x, y) = self.map_at(x, y);
        let cell = map.layout.rows.iter().flat_map(|row| &row.cells).find(|cell| contains(&grow(&cell.body, slack), x, y))?;
        Some((cell, map.monitor))
    }

    /// Where the dragged cell would land if dropped at a point. The `x` or
    /// `y` of the slot is where to draw its marker, in the map's coordinates.
    fn slot_at(&self, dragged: &str, x: f32, y: f32) -> Option<Slot> {
        let (map, x, y) = self.map_at(x, y);
        let rows = &map.layout.rows;
        let (first, last) = (rows.first()?, rows.last()?);
        if y < first.top {
            return Some(Slot::NewRow { at: 0, y: first.top - 8.0 * self.scale });
        }
        if y >= last.bottom {
            return Some(Slot::NewRow { at: rows.len(), y: last.bottom + 8.0 * self.scale });
        }
        for (r, row) in rows.iter().enumerate() {
            if y >= row.top && y < row.bottom {
                let others: Vec<&CellLayout> = row.cells.iter().filter(|c| c.id != dragged).collect();
                let index = others.iter().filter(|c| centre(&c.body).0 < x).count();
                let gap = 10.0 * self.scale;
                let x = match others.get(index) {
                    Some(next) => next.body.left - gap,
                    None => others.last().map_or(row.plus.left, |c| c.body.right + gap),
                };
                return Some(Slot::Row { row: r, index, x });
            }
            if let Some(next) = rows.get(r + 1) {
                if y >= row.bottom && y < next.top {
                    return Some(Slot::NewRow { at: r + 1, y: (row.bottom + next.top) / 2.0 });
                }
            }
        }
        None
    }

    // ---- thumbnails -------------------------------------------------------

    fn hide_popup(&mut self) {
        unregister_thumbnails(&mut self.popup_thumbs);
        self.popup = None;
        self.popup_animation = None;
        self.popup_timer = None;
        self.popup_alpha = 0.0;
        unsafe {
            let _ = KillTimer(self.hwnd, POPUP_TIMER_ID);
            ShowWindow(self.popup_hwnd, SW_HIDE);
            let _ = SetLayeredWindowAttributes(self.popup_hwnd, COLORREF(0), 0, LWA_ALPHA);
        }
    }

    fn schedule_popup(&mut self, now: Instant) {
        if !self.open { return; }
        let delay = self.popup_animation.as_ref().and_then(|animation| animation.next_tick(now));
        let due = delay.map(|ms| now + Duration::from_millis(ms as u64));
        if self.popup_timer.is_some() && due.is_some() && self.popup_timer <= due { return; }
        unsafe {
            let _ = KillTimer(self.hwnd, POPUP_TIMER_ID);
            if let Some(ms) = delay { SetTimer(self.hwnd, POPUP_TIMER_ID, ms, None); }
        }
        self.popup_timer = due;
    }

    fn tick_popup(&mut self, now: Instant) {
        let Some(animation) = &self.popup_animation else { return };
        let (frame, cell, alpha) = animation.sample(now);
        if alpha == 0.0 && animation.leave_at.is_some() {
            self.hide_popup();
            self.invalidate();
            return;
        }
        if let Some(popup) = &mut self.popup { popup.move_to(frame, cell); }
        self.popup_alpha = alpha;
        if self.open {
            let (w, h) = (size(&frame).0.ceil() as i32, size(&frame).1.ceil() as i32);
            unsafe {
                let region = CreateRoundRectRgn(0, 0, w + 1, h + 1, (24.0 * self.scale) as i32, (24.0 * self.scale) as i32);
                if SetWindowRgn(self.popup_hwnd, region, false) == 0 { let _ = DeleteObject(region); }
                // Keep the last composed image until the resized frame has been painted.
                let _ = SetWindowPos(self.popup_hwnd, HWND_TOPMOST,
                    self.origin.0 + frame.left.round() as i32, self.origin.1 + frame.top.round() as i32,
                    w, h, SWP_NOACTIVATE | SWP_NOREDRAW | SWP_SHOWWINDOW);
            }
            self.update_thumbnails();
            self.paint();
            unsafe { let _ = SetLayeredWindowAttributes(self.popup_hwnd, COLORREF(0), (alpha * 255.0).round() as u8, LWA_ALPHA); }
        }
        self.schedule_popup(now);
    }

    /// Follows the cell under the cursor without changing the selection.
    fn update_popup(&mut self, now: Instant) {
        let wanted = match &self.hover {
            Hit::Cell(id) | Hit::CellClose(id) if *id != self.selected && self.press.is_none() && self.editing.is_none() => {
                Some((id.clone(), self.map_at(self.cursor.0, self.cursor.1).0.monitor))
            }
            _ => None,
        };
        let Some((id, monitor)) = wanted else {
            if let Some(animation) = &mut self.popup_animation { animation.leave(now); }
            self.schedule_popup(now);
            return;
        };
        if self.popup.as_ref().is_some_and(|popup| popup.id == id && popup.monitor == monitor) {
            if let Some(animation) = &mut self.popup_animation { animation.show(now); }
            self.schedule_popup(now);
            return;
        }
        let Some(popup) = self.layout_popup(id, monitor) else { return };
        match &mut self.popup_animation {
            Some(animation) => animation.glide_to(popup.frame, popup.cell, now),
            None => self.popup_animation = Some(PopupAnimation::new(popup.frame, popup.cell, now)),
        }
        self.popup = Some(popup);
        let mut old = std::mem::take(&mut self.popup_thumbs);
        if self.open {
            for (window, _) in self.model.popup_windows(&self.popup.as_ref().unwrap().id) {
                if let Some(i) = old.iter().position(|(hwnd, _)| *hwnd == window.hwnd.0) {
                    self.popup_thumbs.push(old.remove(i));
                } else if let Ok(thumb) = unsafe { DwmRegisterThumbnail(self.popup_hwnd, window.hwnd) } {
                    self.popup_thumbs.push((window.hwnd.0, thumb));
                }
            }
        }
        self.tick_popup(now);
        // Replacements are visible and painted before the old thumbnails go away.
        unregister_thumbnails(&mut old);
        self.invalidate();
    }

    fn layout_popup(&self, id: CellId, monitor: usize) -> Option<PopupLayout> {
        let map = self.maps.iter().find(|map| map.monitor == monitor)?;
        let cell = map.layout.rows.iter().flat_map(|row| &row.cells).find(|cell| cell.id == id)?.body;
        let cell = Rect { left: cell.left + map.area.left, right: cell.right + map.area.left,
            top: cell.top + map.area.top, bottom: cell.bottom + map.area.top };
        let windows = self.model.popup_windows(&id);
        let s = self.scale;
        let frame = popup_frame(&cell, &map.area, map.area.top + map.layout.top, windows.is_empty(), s);
        let label = Rect { bottom: frame.top + 42.0 * s, ..grow(&frame, -14.0 * s) };
        let body = Rect { top: label.bottom + 4.0 * s, ..grow(&frame, -14.0 * s) };
        let weights = self.model.window_weights(&id, &self.grid);
        let tiles = layout_tiles(&windows, &body, 0.55 * s, &weights);
        Some(PopupLayout { id, monitor, frame, cell, label, body, tiles })
    }

    fn clear_thumbnails(&mut self) {
        unregister_thumbnails(&mut self.thumbs);
    }

    /// Registers a live thumbnail for every tile.
    fn register_thumbnails(&mut self) {
        self.clear_thumbnails();
        let wanted: Vec<HWND> = self.tile_windows().into_iter().map(|(w, _)| w.hwnd).collect();
        for hwnd in wanted {
            if let Ok(thumb) = unsafe { DwmRegisterThumbnail(self.hwnd, hwnd) } {
                self.thumbs.push((hwnd.0, thumb));
            }
        }
        self.update_thumbnails();
    }

    fn dragged_window(&self) -> Option<isize> {
        match &self.press {
            Some(Press { hit: Hit::Window { hwnd, .. }, dragging: true, .. }) => Some(*hwnd),
            _ => None,
        }
    }

    fn dragged_row(&self) -> Option<usize> {
        match &self.press {
            Some(Press { hit: Hit::RowName(row), dragging: true, .. }) => Some(*row),
            _ => None,
        }
    }

    /// Where the dragged shelf `from` would land if dropped at height
    /// `y`: its place among the others, and where to draw the marker, in the
    /// coordinates of the map under the cursor.
    fn row_slot_at(&self, from: usize, y: f32) -> Option<(usize, f32)> {
        let (map, _, y) = self.map_at(self.cursor.0, y);
        let rows = &map.layout.rows;
        let others: Vec<&RowLayout> = rows.iter().enumerate().filter(|(r, _)| *r != from).map(|(_, row)| row).collect();
        let at = others.iter().filter(|row| (row.top + row.bottom) / 2.0 < y).count();
        let line = match others.get(at) {
            Some(below) => below.top,
            None => others.last().copied().or(rows.last())?.bottom,
        };
        Some((at, line))
    }

    fn dragged_cell(&self) -> Option<&str> {
        match &self.press {
            Some(Press { hit: Hit::Cell(id), dragging: true, .. }) => Some(id.as_str()),
            _ => None,
        }
    }

    /// Puts every thumbnail where its tile is, or under the cursor while its
    /// window is being dragged.
    fn update_thumbnails(&self) {
        self.place_thumbnails(&self.thumbs, &self.tiles, &self.tile_windows(), (0.0, 0.0), self.dragged_window(), 1.0);
        if let Some(popup) = &self.popup {
            self.place_thumbnails(&self.popup_thumbs, &popup.tiles, &self.model.popup_windows(&popup.id),
                (popup.frame.left.round(), popup.frame.top.round()), None, self.popup_alpha);
        }
    }

    fn place_thumbnails(&self, thumbs: &[(isize, isize)], tiles: &[TileLayout], windows: &[(&WindowModel, bool)], offset: (f32, f32), dragged: Option<isize>, opacity: f32) {
        let to_rect = |r: &Rect| RECT {
            left: (r.left - offset.0).round() as i32,
            top: (r.top - offset.1).round() as i32,
            right: (r.right - offset.0).round() as i32,
            bottom: (r.bottom - offset.1).round() as i32,
        };
        for &(hwnd, thumb) in thumbs {
            let mut props = DWM_THUMBNAIL_PROPERTIES {
                dwFlags: DWM_TNP_RECTDESTINATION | DWM_TNP_VISIBLE | DWM_TNP_OPACITY | DWM_TNP_SOURCECLIENTAREAONLY,
                opacity: 255,
                fVisible: true.into(),
                fSourceClientAreaOnly: false.into(),
                ..Default::default()
            };
            let source = unsafe { DwmQueryThumbnailSourceSize(thumb) }.ok();
            // The picture as large as fits into `w` by `h` without being
            // distorted or made larger than the window is.
            let fitted = |w: f32, h: f32| match source {
                Some(source) => {
                    let (sw, sh) = (source.cx.max(1) as f32, source.cy.max(1) as f32);
                    let scale = (w / sw).min(h / sh).min(1.0);
                    (sw * scale, sh * scale)
                }
                None => (w, h),
            };
            match tiles.iter().find(|tile| tile.hwnd.0 == hwnd) {
                Some(tile) if dragged == Some(hwnd) => {
                    // Follows the cursor, small enough to see the map under it.
                    let (tw, th) = size(&tile.thumb);
                    let (cx, cy) = self.cursor;
                    let (w, h) = fitted(tw.min(200.0 * self.scale), th);
                    props.rcDestination = to_rect(&rect(cx - w / 2.0, cy - h / 2.0, w, h));
                    props.opacity = 215;
                }
                Some(tile) => {
                    // A tile has the shape of its window and the picture
                    // fills it; in one that had to be made wider or higher
                    // for its buttons the picture sits in the middle.
                    let (tw, th) = size(&tile.thumb);
                    let (w, h) = fitted(tw, th);
                    let (cx, cy) = centre(&tile.thumb);
                    props.rcDestination = to_rect(&rect(cx - w / 2.0, cy - h / 2.0, w, h));
                    // A minimized window shows what it last looked like, faded.
                    // (Without a kept picture the source is only a sliver.)
                    if windows.iter().any(|(w, _)| w.hwnd.0 == hwnd && w.minimized) {
                        props.opacity = 150;
                        props.fVisible = source.is_some_and(|source| source.cy >= 80).into();
                    }
                }
                None => props.fVisible = false.into(),
            }
            unsafe {
                props.opacity = (props.opacity as f32 * opacity).round() as u8;
                let _ = DwmUpdateThumbnailProperties(thumb, &props);
            }
        }
    }

    /// Thumbnails stack in registration order; re-register the one that will
    /// follow the cursor to lift it above the rest.
    fn raise_thumbnail(&mut self, hwnd: isize) {
        if let Some(i) = self.thumbs.iter().position(|&(h, _)| h == hwnd) {
            let (_, old) = self.thumbs.remove(i);
            unsafe {
                let _ = DwmUnregisterThumbnail(old);
                if let Ok(thumb) = DwmRegisterThumbnail(self.hwnd, HWND(hwnd)) {
                    self.thumbs.push((hwnd, thumb));
                }
            }
        }
    }

    // ---- input ------------------------------------------------------------

    /// Handles a window message. `None` means "not ours, use the default".
    pub fn handle(&mut self, message: u32, wparam: WPARAM, lparam: LPARAM) -> Option<Action> {
        if !self.open {
            return None;
        }
        // Mouse messages carry the position as two signed 16-bit numbers.
        let (x, y) = ((lparam.0 & 0xFFFF) as i16 as f32, ((lparam.0 >> 16) & 0xFFFF) as i16 as f32);
        Some(match message {
            WM_PAINT => {
                self.paint();
                unsafe {
                    ValidateRect(self.hwnd, None);
                }
                Action::None
            }
            WM_ERASEBKGND => return None,
            // Losing focus closes the board, except to its own confirmation box.
            WM_ACTIVATE if wparam.0 & 0xFFFF == 0 && !self.modal => {
                #[cfg(feature = "debug-log")]
                {
                    // Who took the keyboard: its class and program, never its title.
                    let to = HWND(lparam.0);
                    let mut class = [0u16; 64];
                    let n = unsafe { windows::Win32::UI::WindowsAndMessaging::GetClassNameW(to, &mut class) }.max(0) as usize;
                    debug_log!("board: keyboard taken by {:x} {} {:?}", to.0, String::from_utf16_lossy(&class[..n]), vd::exe_of_window(to));
                }
                Action::Dismiss
            }
            WM_LBUTTONDOWN => self.mouse_down(x, y),
            WM_LBUTTONDBLCLK => {
                if let Hit::RowName(r) = self.hit_test(x, y) {
                    self.start_rename(r);
                }
                Action::None
            }
            WM_MOUSEMOVE => {
                let mut track = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE, hwndTrack: self.hwnd, ..Default::default() };
                unsafe { let _ = TrackMouseEvent(&mut track); }
                self.mouse_move(x, y);
                Action::None
            }
            WM_MOUSELEAVE => {
                self.hover = Hit::Nothing;
                self.update_popup(Instant::now());
                self.invalidate();
                Action::None
            }
            WM_LBUTTONUP => self.mouse_up(x, y),
            WM_RBUTTONUP => match self.hit_test(x, y) {
                Hit::Cell(id) | Hit::CellClose(id) => Action::ToggleEmphasis(id),
                _ => self.row_header_at(x, y).map_or(Action::None, Action::RowMenu),
            },
            WM_KEYDOWN => self.key_down(wparam.0 as u16),
            WM_TIMER if wparam.0 == POPUP_TIMER_ID => {
                unsafe { let _ = KillTimer(self.hwnd, POPUP_TIMER_ID); }
                self.popup_timer = None;
                self.tick_popup(Instant::now());
                Action::None
            }
            WM_TIMER if wparam.0 == ON_TOP_TIMER_ID => {
                self.keep_on_top();
                Action::None
            }
            WM_CHAR => {
                if let (Some((_, text)), Some(c)) = (&mut self.editing, char::from_u32(wparam.0 as u32)) {
                    if !c.is_control() && text.chars().count() < NAME_LENGTH {
                        text.push(c);
                        self.invalidate();
                    }
                }
                Action::None
            }
            // While a name is typed the board shows the text being put
            // together itself, so the input method's own little window is
            // not opened and its results are taken here.
            WM_IME_STARTCOMPOSITION if self.editing.is_some() => Action::None,
            WM_IME_COMPOSITION if self.editing.is_some() => {
                self.composition_changed(lparam.0 as u32);
                Action::None
            }
            _ => return None,
        })
    }

    fn start_rename(&mut self, row: usize) {
        if let Some(model) = self.model.rows.get(row) {
            self.editing = Some((row, model.name.clone()));
            self.composing.clear();
            self.hide_popup();
            self.invalidate();
        }
    }

    /// The input method has new text: finished text (`GCS_RESULTSTR` in
    /// `flags`) joins the name, text still being put together is shown.
    fn composition_changed(&mut self, flags: u32) {
        let read = |kind: IME_COMPOSITION_STRING| unsafe {
            let context = ImmGetContext(self.hwnd);
            let mut text = vec![0u16; 64];
            let bytes = ImmGetCompositionStringW(context, kind, Some(text.as_mut_ptr().cast()), (text.len() * 2) as u32);
            let _ = ImmReleaseContext(self.hwnd, context);
            String::from_utf16_lossy(&text[..(bytes.max(0) as usize / 2).min(text.len())])
        };
        let finished = if flags & GCS_RESULTSTR.0 != 0 { read(GCS_RESULTSTR) } else { String::new() };
        self.composing = if flags & GCS_COMPSTR.0 != 0 { read(GCS_COMPSTR) } else { String::new() };
        if let Some((_, text)) = &mut self.editing {
            let room = NAME_LENGTH.saturating_sub(text.chars().count());
            text.extend(finished.chars().filter(|c| !c.is_control()).take(room));
        }
        self.invalidate();
    }

    /// Ends renaming and returns the row and its name as typed, including
    /// what the input method had not finished.
    fn stop_editing(&mut self) -> Option<(usize, String)> {
        let (row, text) = self.editing.take()?;
        let name = format!("{text}{}", std::mem::take(&mut self.composing));
        unsafe {
            // Or the unfinished text would turn up in the next thing typed.
            let context = ImmGetContext(self.hwnd);
            let _ = ImmNotifyIME(context, NI_COMPOSITIONSTR, CPS_CANCEL, 0);
            let _ = ImmReleaseContext(self.hwnd, context);
        }
        self.invalidate();
        // A shelf always has a name: one left empty keeps what it had.
        let name = name.trim();
        (!name.is_empty()).then(|| (row, name.to_owned()))
    }

    fn key_down(&mut self, key: u16) -> Action {
        if let Some((_, text)) = &mut self.editing {
            if key == VK_RETURN.0 {
                return self.stop_editing().map_or(Action::None, |(row, name)| Action::Rename(row, name));
            }
            if key == VK_ESCAPE.0 {
                self.stop_editing();
            } else if key == VK_BACK.0 {
                text.pop();
            }
            self.invalidate();
            return Action::None;
        }
        match key {
            k if k == VK_LEFT.0 => return Action::Step(Dir::Left),
            k if k == VK_RIGHT.0 => return Action::Step(Dir::Right),
            k if k == VK_UP.0 => return Action::Step(Dir::Up),
            k if k == VK_DOWN.0 => return Action::Step(Dir::Down),
            k if k == VK_RETURN.0 => return Action::Go(self.selected.clone(), None),
            k if k == VK_ESCAPE.0 => return Action::Cancel,
            k if k == VK_F2.0 => {
                if let Some(pos) = self.grid.find(&self.selected) {
                    self.start_rename(pos.row);
                }
            }
            _ => {}
        }
        Action::None
    }

    fn mouse_down(&mut self, x: f32, y: f32) -> Action {
        let hit = self.hit_test(x, y);
        debug_log!("board: down on {:?} at {x:.0},{y:.0}", hit);
        self.cursor = (x, y);
        self.hide_popup();
        self.press = Some(Press { hit, x, y, dragging: false });
        unsafe {
            SetCapture(self.hwnd);
        }
        self.invalidate();
        // A click anywhere finishes a rename in progress.
        self.stop_editing().map_or(Action::None, |(row, name)| Action::Rename(row, name))
    }

    fn mouse_move(&mut self, x: f32, y: f32) {
        self.cursor = (x, y);
        let threshold = 6.0 * self.scale;
        if let Some(press) = &mut self.press {
            // A window shown on every desktop has no cell to be dragged out of.
            let draggable = match &press.hit {
                Hit::Window { cell, .. } => !cell.is_empty(),
                Hit::Cell(_) | Hit::RowName(_) => true,
                _ => false,
            };
            let mut raise = None;
            if !press.dragging && draggable && (x - press.x).hypot(y - press.y) > threshold {
                press.dragging = true;
                if let Hit::Window { hwnd, .. } = press.hit {
                    raise = Some(hwnd);
                }
            }
            if press.dragging {
                if let Some(hwnd) = raise {
                    self.raise_thumbnail(hwnd);
                }
                self.update_thumbnails();
                self.invalidate();
            }
            return;
        }
        let hover = self.hit_test(x, y);
        #[cfg(feature = "debug-log")]
        if hover != self.hover {
            debug_log!("board: hover {:?} at {x:.0},{y:.0}", hover);
        }
        if hover != self.hover {
            self.hover = hover;
            self.invalidate();
        }
        self.update_popup(Instant::now());
    }

    fn mouse_up(&mut self, x: f32, y: f32) -> Action {
        unsafe {
            let _ = ReleaseCapture();
        }
        let Some(press) = self.press.take() else { return Action::None };
        self.hover = self.hit_test(x, y);
        self.update_popup(Instant::now());
        self.invalidate();
        if press.dragging {
            let action = match &press.hit {
                Hit::Window { cell, hwnd } => self.drop_window(cell, *hwnd, x, y),
                Hit::Cell(id) => match self.slot_at(id, x, y) {
                    Some(Slot::Row { row, index, .. }) => Action::MoveCell { id: id.clone(), row, index },
                    Some(Slot::NewRow { at, .. }) => Action::MoveCellToNewRow { id: id.clone(), at },
                    None => Action::None,
                },
                Hit::RowName(from) => match self.row_slot_at(*from, y) {
                    Some((at, _)) if at != *from => Action::MoveRow { from: *from, at },
                    _ => Action::None,
                },
                _ => Action::None,
            };
            // Put the thumbnail back; a successful move replaces the model anyway.
            self.update_thumbnails();
            return action;
        }
        if self.hit_test(x, y) != press.hit {
            debug_log!("board: click dropped, released on {:?} after pressing {:?}", self.hit_test(x, y), press.hit);
            return Action::None;
        }
        let near = |(at, cx, cy): (Instant, f32, f32)| at.elapsed().as_millis() < SECOND_CLICK_MS && (x - cx).hypot(y - cy) < 12.0 * self.scale;
        if self.changed_at.is_some_and(near) {
            debug_log!("board: click ignored, right upon one that added or took away something here");
            return Action::None;
        }
        if matches!(press.hit, Hit::Plus(_) | Hit::PlusLeft(_) | Hit::AddRow | Hit::AddRowTop | Hit::CellClose(_) | Hit::TileClose(_)) {
            self.changed_at = Some((Instant::now(), x, y));
        }
        self.click(press.hit)
    }

    /// A window of cell `from` was dragged to a point. Dropped on a map cell
    /// it goes to that cell, on the monitor whose map that was; dropped
    /// elsewhere it only changes monitor.
    fn drop_window(&self, from: &str, hwnd: isize, x: f32, y: f32) -> Action {
        let from_monitor = self.tile_windows().iter().find(|(w, _)| w.hwnd.0 == hwnd).map(|(w, _)| self.monitor_of(w));
        let (to_cell, to_monitor) = match self.cell_at(x, y) {
            Some((target, m)) => (Some(target.id.clone()), m),
            None => (None, self.map_at(x, y).0.monitor),
        };
        let cell = to_cell.filter(|id| id != from);
        let monitor = if Some(to_monitor) != from_monitor { self.monitor_on_screen(to_monitor) } else { None };
        if cell.is_none() && monitor.is_none() {
            Action::None
        } else {
            Action::MoveWindow { window: HWND(hwnd), cell, monitor }
        }
    }

    /// The button was pressed and released on the same thing without dragging.
    fn click(&self, hit: Hit) -> Action {
        match hit {
            Hit::Window { cell, hwnd } => Action::Go(cell, Some(HWND(hwnd))),
            Hit::Cell(id) => Action::Go(id, None),
            Hit::Pin(hwnd) => {
                let windows = self.tile_windows();
                let Some((window, everywhere)) = windows.iter().find(|(w, _)| w.hwnd.0 == hwnd) else { return Action::None };
                let (row, all) = match Pin::of(window, *everywhere) {
                    Pin::Off => (true, false),
                    Pin::Row => (false, true),
                    Pin::Everywhere => (false, false),
                };
                Action::SetPin { window: HWND(hwnd), row, all }
            }
            Hit::Top(hwnd) => {
                let windows = self.tile_windows();
                let Some((window, _)) = windows.iter().find(|(w, _)| w.hwnd.0 == hwnd) else { return Action::None };
                Action::SetTop { window: HWND(hwnd), on: !window.topmost }
            }
            Hit::TileClose(hwnd) => Action::CloseWindow(HWND(hwnd)),
            Hit::CellClose(id) => Action::RemoveCell(id),
            Hit::Plus(row) => Action::AddCell { row, front: false },
            Hit::PlusLeft(row) => Action::AddCell { row, front: true },
            Hit::AddRow => Action::AddRow { top: false },
            Hit::AddRowTop => Action::AddRow { top: true },
            // Renaming takes a double click.
            Hit::RowName(_) => Action::None,
            // A click on nothing dismisses the board, like Task View.
            Hit::Nothing => Action::Cancel,
        }
    }

    // ---- drawing ----------------------------------------------------------

    /// The off-screen bitmap the board is drawn into before being copied to
    /// the window. (Drawing straight to the window with a Direct2D window
    /// target left the window blank on the machine this was developed on.)
    fn ensure_canvas(&mut self) -> Option<ID2D1RenderTarget> {
        if !self.canvas.as_ref().is_some_and(|c| c.dib.size == self.size) {
            // Icons are bitmaps of the target and go with it.
            self.icons.clear();
            self.canvas = Canvas::new(&self.factory, self.size)
                .map_err(|e| crate::app::log(&format!("board canvas failed: {e}")))
                .ok();
        }
        self.canvas.as_ref().map(|c| c.target.target.clone())
    }

    /// Draws the board into the canvas; `None` when there is no canvas.
    fn draw_to_canvas(&mut self) -> Option<Result<()>> {
        let target = self.ensure_canvas()?;
        if self.fonts.is_none() {
            self.fonts = make_fonts(self.scale).map_err(|e| crate::app::log(&format!("board fonts failed: {e}"))).ok();
        }
        self.load_icons(&target);
        unsafe {
            target.BeginDraw();
            let drawn = self.draw(&target);
            Some(target.EndDraw(None, None).and(drawn))
        }
    }

    fn paint(&mut self) {
        match self.draw_to_canvas() {
            Some(Ok(())) => {}
            Some(Err(e)) => {
                crate::app::log(&format!("board draw failed: {e}"));
                // Rebuild everything on the next paint.
                self.canvas = None;
                return;
            }
            None => return,
        }
        if let Some(canvas) = &self.canvas {
            let (w, h) = canvas.dib.size;
            unsafe {
                let window = GetDC(self.hwnd);
                let _ = BitBlt(window, 0, 0, w, h, canvas.dib.dc, 0, 0, SRCCOPY);
                ReleaseDC(self.hwnd, window);
                if let Some(popup) = &self.popup {
                    // The board underneath stays bare, so the layered panel can fade away.
                    let target = &canvas.target.target;
                    target.BeginDraw();
                    if let (Ok(p), Some(fonts)) = (Painter::new(target), &self.fonts) {
                        self.draw_popup_panel(&p, fonts);
                    }
                    if let Err(error) = target.EndDraw(None, None) {
                        crate::app::log(&format!("popup draw failed: {error}"));
                    }
                    let dc = GetDC(self.popup_hwnd);
                    let _ = BitBlt(dc, 0, 0, size(&popup.frame).0.ceil() as i32, size(&popup.frame).1.ceil() as i32,
                        canvas.dib.dc, popup.frame.left.round() as i32, popup.frame.top.round() as i32, SRCCOPY);
                    ReleaseDC(self.popup_hwnd, dc);
                }
            }
        }
    }

    fn load_icons(&mut self, target: &ID2D1RenderTarget) {
        let size = (20.0 * self.scale).round() as i32;
        let in_cells = self.model.rows.iter().flat_map(|r| &r.cells).flat_map(|c| &c.windows);
        for window in in_cells.chain(&self.model.pinned) {
            if !self.icons.contains_key(&window.hwnd.0) {
                if let Some(bitmap) = icon_bitmap(target, window.hwnd, size) {
                    self.icons.insert(window.hwnd.0, bitmap);
                }
            }
        }
    }

    fn draw(&self, target: &ID2D1RenderTarget) -> Result<()> {
        let Some(fonts) = &self.fonts else { return Ok(()) };
        unsafe {
            target.Clear(Some(&paint::backdrop()));
        }
        let p = Painter::new(target)?;
        // The tiles are placed in the window's coordinates; each map, and the
        // label and the hint of the home monitor, in its monitor's own.
        self.draw_tiles(&p, fonts);
        for map in &self.maps {
            unsafe {
                target.SetTransform(&Matrix3x2::translation(map.area.left, map.area.top));
            }
            self.draw_map(&p, fonts, map);
        }
        unsafe {
            target.SetTransform(&Matrix3x2::translation(self.home.left, self.home.top));
        }
        self.draw_captions(&p, fonts);
        unsafe {
            target.SetTransform(&Matrix3x2::identity());
        }
        self.draw_popup(&p, fonts);
        Ok(())
    }

    /// What the line at the bottom says: the full title of the tile under
    /// the cursor (it may be shortened there), or what can be done with the
    /// thing under the cursor. Nothing when the cursor is on nothing.
    fn footer(&self) -> Option<(String, f32)> {
        let hint = |text: &str| Some((text.to_owned(), 0.50));
        if self.editing.is_some() {
            return hint("Enter: save   ·   Esc: cancel");
        }
        if self.dragged_window().is_some() {
            return hint(if self.monitors.len() > 1 {
                "Drop on a cell to move it there, or on another monitor to move it to that monitor"
            } else {
                "Drop on a cell to move it there"
            });
        }
        if self.press.is_some() {
            return None;
        }
        match &self.hover {
            Hit::Top(hwnd) => {
                let on = self.tile_windows().iter().any(|(w, _)| w.hwnd.0 == *hwnd && w.topmost);
                hint(if on { "Always on top: on   ·   Click: turn off" } else { "Click: keep above other windows" })
            }
            Hit::Window { hwnd, .. } | Hit::Pin(hwnd) | Hit::TileClose(hwnd) => {
                let windows = self.tile_windows();
                let (window, _) = windows.iter().find(|(w, _)| w.hwnd.0 == *hwnd)?;
                let title = match window.app_name.is_empty() {
                    true => window.title.clone(),
                    false => format!("{}  —  {}", window.app_name, window.title),
                };
                Some((title, 0.85))
            }
            Hit::Cell(_) | Hit::CellClose(_) => {
                hint("Hover: show windows   ·   Click: go   ·   Drag: move   ·   Right-click: highlight")
            }
            Hit::RowName(_) => hint("Drag: reorder   ·   Double-click: rename   ·   Right-click: close windows, remove"),
            Hit::Plus(_) | Hit::PlusLeft(_) => hint("Add a cell"),
            Hit::AddRow | Hit::AddRowTop => hint("Add a shelf"),
            Hit::Nothing => None,
        }
    }

    /// The cell label at the top and the line at the bottom of the home monitor.
    fn draw_captions(&self, p: &Painter, fonts: &Fonts) {
        let s = self.scale;
        let (w, h) = size(&self.home);
        if let Some(pos) = self.grid.find(&self.selected) {
            let row = grid::row_title(self.model.rows.get(pos.row).map_or("", |row| &row.name), pos.row);
            let here = if self.selected == self.model.current { "   ● current" } else { "" };
            p.text(&format!("{row}  ·  cell {}{here}", pos.col + 1), &fonts.label, &self.label, white(0.80));
        }
        if let Some((line, alpha)) = self.footer() {
            p.text(&line, &fonts.centered, &rect(0.0, h - (FOOTER - 8.0) * s, w, 26.0 * s), white(alpha));
        }
    }

    fn draw_popup(&self, p: &Painter, fonts: &Fonts) {
        let Some(popup) = &self.popup else { return };
        let s = self.scale;
        let x = centre(&popup.cell).0;
        let end = (x.clamp(popup.frame.left + 16.0 * s, popup.frame.right - 16.0 * s), popup.frame.bottom);
        p.line((x, popup.cell.top - 2.0 * s), (x, end.1 + 8.0 * s), 2.0 * s, accent(0.65 * self.popup_alpha));
        p.line((x, end.1 + 8.0 * s), end, 2.0 * s, accent(0.65 * self.popup_alpha));
        p.disc((x, popup.cell.top - 2.0 * s), 3.0 * s, accent(0.9 * self.popup_alpha));
        if !self.open { self.draw_popup_panel(p, fonts); }
    }

    fn draw_popup_panel(&self, p: &Painter, fonts: &Fonts) {
        let Some(popup) = &self.popup else { return };
        let s = self.scale;
        p.fill(&popup.frame, 12.0 * s, rgba(0.09, 0.10, 0.13, 1.0));
        p.stroke(&grow(&popup.frame, -1.0 * s), 11.0 * s, 1.0 * s, accent(0.65));
        if let Some(pos) = self.grid.find(&popup.id) {
            let row = grid::row_title(&self.model.rows[pos.row].name, pos.row);
            p.text(&format!("{row}  ·  cell {}", pos.col + 1), &fonts.small, &popup.label, white(0.9));
        }
        if self.model.popup_windows(&popup.id).is_empty() {
            p.text("Empty", &fonts.centered, &popup.body, white(0.45));
        }
        let windows = self.model.popup_windows(&popup.id);
        for tile in &popup.tiles {
            p.fill(&tile.frame, 5.0 * s, rgba(0.15, 0.16, 0.19, 1.0));
            p.fill_square(&tile.thumb, rgba(0.055, 0.06, 0.075, 1.0));
            if let Some((window, _)) = windows.iter().find(|(window, _)| window.hwnd == tile.hwnd) {
                let title = Rect { left: tile.frame.left + 6.0 * s, right: tile.frame.right - 6.0 * s,
                    bottom: tile.thumb.top, ..tile.frame };
                p.text(&window.title, &fonts.small, &title, white(0.8));
                if let Some(icon) = self.icons.get(&window.hwnd.0) {
                    let (cx, cy) = centre(&tile.thumb);
                    p.bitmap(icon, &rect(cx - 12.0 * s, cy - 12.0 * s, 24.0 * s, 24.0 * s), 0.75);
                }
            }
        }
    }

    /// The selected cell's windows as large titled previews.
    fn draw_tiles(&self, p: &Painter, fonts: &Fonts) {
        let windows = self.tile_windows();
        let in_parts = !self.sections.is_empty();
        if if in_parts { !windows.iter().any(|(w, everywhere)| !everywhere && !w.follows) } else { self.tiles.is_empty() } {
            p.text("No windows in this cell", &fonts.centered, &self.tiles_area, white(0.28));
        }
        let s = self.scale;
        for (head, part, name) in &self.sections {
            let panel = Rect { left: part.left - 10.0 * s, top: head.top - 6.0 * s, right: part.right + 10.0 * s, bottom: part.bottom + 8.0 * s };
            match *name {
                // What stays on every desktop is on screen wherever the user
                // goes: its part stands out from the rest, in its pin's colour.
                "All rows" => {
                    p.fill(&panel, 12.0 * s, everywhere(0.08));
                    p.stroke(&panel, 12.0 * s, 1.2 * s, everywhere(0.55));
                    let (_, cy) = centre(&Rect { bottom: head.bottom - 4.0 * s, ..*head });
                    p.disc((head.left + 4.0 * s, cy), 4.0 * s, everywhere(1.0));
                    let text = Rect { left: head.left + 14.0 * s, ..*head };
                    p.text("All rows · on every desktop", &fonts.title, &text, everywhere(1.0));
                }
                "This row" => {
                    p.fill(&panel, 12.0 * s, accent(0.04));
                    p.stroke(&panel, 12.0 * s, 1.0 * s, accent(0.30));
                    p.text("This row · on every cell of it", &fonts.small, head, accent(0.85));
                }
                _ => {
                    p.text(name, &fonts.small, head, white(0.45));
                    p.fill_square(&Rect { top: head.bottom - 5.0 * s, bottom: head.bottom - 4.0 * s, ..*head }, white(0.08));
                }
            }
        }
        for tile in &self.tiles {
            if let Some((window, everywhere)) = windows.iter().find(|(w, _)| w.hwnd == tile.hwnd) {
                self.draw_tile(p, fonts, tile, window, Pin::of(window, *everywhere));
            }
        }
    }

    fn draw_tile(&self, p: &Painter, fonts: &Fonts, tile: &TileLayout, window: &WindowModel, pin: Pin) {
        let s = self.scale;
        let hot = self.hover == self.tile_hit(tile.hwnd);
        // The cursor is on the tile or on one of its buttons.
        let pointed = hot || matches!(&self.hover, Hit::Pin(hwnd) | Hit::Top(hwnd) | Hit::TileClose(hwnd) if *hwnd == tile.hwnd.0);
        // A window that is not pinned shows the button for it only then, so
        // that a narrow tile has room for its title. Likewise the button
        // that keeps it above the others, which sits right beside.
        let show_pin = pin != Pin::Off || pointed;
        let show_top = window.topmost || pointed;
        let buttons = if show_top { tile.top.left } else { tile.pin.left };
        let title_bar = Rect { right: if show_pin || show_top { buttons - 8.0 * s } else { tile.title.right }, ..tile.title };
        let dim = if self.dragged_window() == Some(tile.hwnd.0) { 0.35 } else { 1.0 };
        p.fill(&tile.frame, 9.0 * s, rgba(0.125, 0.135, 0.165, dim));
        // Stand-in under the live thumbnail (and all there is for a minimized window).
        p.fill_square(&tile.thumb, rgba(0.075, 0.08, 0.095, dim));
        if let Some(bitmap) = self.icons.get(&tile.hwnd.0) {
            p.bitmap(bitmap, &tile.icon, dim);
            let (cx, cy) = centre(&tile.thumb);
            p.bitmap(bitmap, &rect(cx - 20.0 * s, cy - 20.0 * s, 40.0 * s, 40.0 * s), 0.8 * dim);
        }
        if window.minimized {
            let note = Rect { top: tile.thumb.bottom - 26.0 * s, ..tile.thumb };
            p.text("Minimized", &fonts.centered, &note, white(0.40 * dim));
        }

        // Two lines: the app, then what this window of it is about.
        let title = display_title(&window.title, &window.app_name, (size(&title_bar).0 / (7.4 * s)) as usize);
        let bright = white(if hot { 1.0 } else { 0.88 } * dim);
        if window.app_name.is_empty() || title.is_empty() || title.eq_ignore_ascii_case(&window.app_name) {
            let only = if title.is_empty() { &window.app_name } else { &title };
            p.text(only, &fonts.title, &title_bar, bright);
        } else {
            let middle = centre(&title_bar).1;
            let upper = Rect { top: title_bar.top + 5.0 * s, bottom: middle, ..title_bar };
            let lower = Rect { top: middle - 2.0 * s, bottom: title_bar.bottom - 4.0 * s, ..title_bar };
            p.text(&window.app_name, &fonts.sub, &upper, white(0.50 * dim));
            p.text(&title, &fonts.title, &lower, bright);
        }

        let over = self.hover == Hit::Pin(tile.hwnd.0);
        let colour = match pin {
            Pin::Everywhere => everywhere(dim),
            Pin::Row => accent(dim),
            Pin::Off => white(if over { 0.20 } else { 0.07 } * dim),
        };
        if show_pin {
            p.fill(&tile.pin, 5.0 * s, colour);
            p.text(pin.label(), &fonts.centered, &tile.pin, white(if pin != Pin::Off || over { 1.0 } else { 0.50 } * dim));
        }
        if show_top {
            // One of a pair with the pin button: same height, same corners.
            let over = self.hover == Hit::Top(tile.hwnd.0);
            let fill = if window.topmost { warm(0.92 * dim) } else { white(if over { 0.20 } else { 0.07 } * dim) };
            p.fill(&tile.top, 5.0 * s, fill);
            let ink = if window.topmost { rgba(0.10, 0.09, 0.07, dim) } else { white(if over { 1.0 } else { 0.50 } * dim) };
            p.text("On top", &fonts.centered, &tile.top, ink);
        }

        if self.refused.is_some_and(|(hwnd, at)| hwnd == tile.hwnd.0 && at.elapsed().as_millis() < REFUSAL_MS) {
            // Over the title: the live picture of the window covers the rest.
            let note = Rect { bottom: tile.thumb.top, ..tile.frame };
            p.fill(&note, 9.0 * s, danger(0.95));
            p.text("Runs as administrator: cannot be changed", &fonts.centered, &note, white(1.0));
        }
        let over = self.hover == Hit::TileClose(tile.hwnd.0);
        if over {
            p.fill(&tile.close, 5.0 * s, danger(0.90));
        }
        p.text("✕", &fonts.centered, &tile.close, white(if over { 1.0 } else { 0.55 } * dim));

        let (width, colour) = if hot { (2.5 * s, accent(1.0)) } else { (1.0, white(0.10 * dim)) };
        p.stroke(&grow(&tile.frame, 1.0 * s), 10.0 * s, width, colour);
    }

    /// The map of all rows and cells: plain squares holding each cell's apps
    /// (icon and count), with the controls appearing on hover.
    fn draw_map(&self, p: &Painter, fonts: &Fonts, map: &Map) {
        let s = self.scale;
        let layout = &map.layout;
        let selected_row = self.grid.find(&self.selected).map(|pos| pos.row);
        // The cell a dragged window would be dropped on.
        let drop_cell = self.dragged_window().and_then(|_| self.cell_at(self.cursor.0, self.cursor.1)).map(|(c, _)| c.id.as_str());

        // The column vertical moves travel along.
        p.fill(&layout.spine, 10.0 * s, white(0.035));
        for (r, (row, model)) in layout.rows.iter().zip(&self.model.rows).enumerate() {
            let in_selected_row = selected_row == Some(r);
            self.draw_row_name(p, fonts, r, row, &model.name, in_selected_row);
            self.draw_badges(p, fonts, &row.pins, 1.0);
            if row.pins_more > 0 {
                p.text(&format!("+{}", row.pins_more), &fonts.small, &row.pins_more_label, white(0.50));
            }
            for cell in &row.cells {
                self.draw_cell(p, fonts, cell, in_selected_row, drop_cell == Some(cell.id.as_str()));
            }
            self.draw_plus(p, &row.plus, Hit::Plus(r));
            self.draw_plus(p, &row.plus_left, Hit::PlusLeft(r));
        }
        self.draw_plus(p, &layout.add_row, Hit::AddRow);
        self.draw_plus(p, &layout.add_row_top, Hit::AddRowTop);

        // What is being dragged is drawn only on the monitor the cursor is on.
        if contains(&map.area, self.cursor.0, self.cursor.1) {
            self.draw_drag(p, map);
        }
    }

    /// A row's name, in a box as wide as its text while hovered or edited.
    fn draw_row_name(&self, p: &Painter, fonts: &Fonts, r: usize, row: &RowLayout, name: &str, in_selected_row: bool) {
        let s = self.scale;
        let editing = self.editing.as_ref().filter(|(row, _)| *row == r);
        let name = match editing {
            Some((_, typed)) => format!("{typed}{}▏", self.composing),
            None => grid::row_title(name, r),
        };
        let dragged = self.dragged_row() == Some(r);
        let hot = (self.hover == Hit::RowName(r) && self.press.is_none()) || dragged;
        if editing.is_some() || hot {
            // While typing the box follows the text being typed.
            let width = text_width(&name, 12.0 * s) + 4.0 * s;
            let area = Rect { right: row.name.left + width.max(size(&row.name).0), ..row.name };
            p.fill(&grow(&area, 4.0 * s), 5.0 * s, white(if editing.is_some() { 0.12 } else { 0.06 }));
        }
        let wide = Rect { right: row.header.right, ..row.name };
        p.text(&name, &fonts.small, &wide, white(if in_selected_row || hot { 0.90 } else { 0.45 }));
    }

    fn draw_cell(&self, p: &Painter, fonts: &Fonts, cell: &CellLayout, in_selected_row: bool, drop_target: bool) {
        let s = self.scale;
        let radius = 8.0 * s;
        let dim = if self.dragged_cell() == Some(cell.id.as_str()) { 0.35 } else { 1.0 };
        let is_selected = cell.id == self.selected;
        let hovering = matches!(&self.hover, Hit::Cell(id) | Hit::CellClose(id) if *id == cell.id);
        if is_selected {
            for (by, a) in [(5.0, 0.08), (3.0, 0.14)] {
                p.fill(&grow(&cell.body, by * s), radius + by * s, accent(a * dim));
            }
        }
        let fill = if is_selected {
            accent(0.35 * dim)
        } else if cell.emphasised {
            warm(0.40 * dim)
        } else {
            white((if in_selected_row { 0.08 + 0.32 * cell.usage } else { 0.04 + 0.28 * cell.usage }) * dim)
        };
        p.fill(&cell.body, radius, fill);
        if drop_target {
            p.stroke(&cell.body, radius, 2.5 * s, accent(1.0));
        } else if is_selected {
            p.stroke(&cell.body, radius, 2.0 * s, accent(dim));
        } else if cell.emphasised {
            p.stroke(&cell.body, radius, 1.5 * s, warm(0.95 * dim));
        } else if hovering {
            p.stroke(&cell.body, radius, 1.0, white(0.35 * dim));
        }
        if cell.emphasised && (is_selected || drop_target) {
            // The selection's frame took the place of the mark's: a second
            // one inside it, or marking the selected cell would show nothing.
            p.stroke(&grow(&cell.body, -4.0 * s), (radius - 4.0 * s).max(1.0), 2.0 * s, warm(0.95 * dim));
        }
        if cell.id == self.model.current {
            // Where the user actually is, as opposed to what is selected.
            let dot = rect(cell.body.left + 7.0 * s, cell.body.top + 7.0 * s, 6.0 * s, 6.0 * s);
            p.fill(&dot, 3.0 * s, accent(dim));
        }
        self.draw_badges(p, fonts, &cell.apps, dim);
        if cell.more > 0 {
            p.text(&format!("+{}", cell.more), &fonts.centered, &cell.more_label, white(0.50 * dim));
        }
        if hovering && self.model.cell_count() > 1 && self.press.is_none() {
            let hot = self.hover == Hit::CellClose(cell.id.clone());
            p.fill(&cell.close, 4.0 * s, if hot { danger(0.90) } else { rgba(0.2, 0.21, 0.25, 1.0) });
            p.text("✕", &fonts.centered, &cell.close, white(if hot { 1.0 } else { 0.75 }));
        }
    }

    /// App icons, each with how many windows it has when more than one.
    fn draw_badges(&self, p: &Painter, fonts: &Fonts, badges: &[AppBadge], dim: f32) {
        for badge in badges {
            match self.icons.get(&badge.hwnd.0) {
                Some(bitmap) => p.bitmap(bitmap, &badge.icon, dim),
                None => p.fill(&badge.icon, 4.0 * self.scale, white(0.25 * dim)),
            }
            if badge.count > 1 {
                p.text(&badge.count.to_string(), &fonts.small, &badge.label, white(0.75 * dim));
            }
        }
    }

    /// One of the "+" buttons, which `hit` is the hit-test result of.
    fn draw_plus(&self, p: &Painter, area: &Rect, hit: Hit) {
        let hot = self.hover == hit;
        let s = self.scale;
        let (cx, cy) = centre(area);
        let half = size(area).0.min(size(area).1) / 2.0;
        // A faint sign, out of the way of the cells, that comes up as a
        // round button under the cursor: it is there to be found, not to
        // draw the eye from the map. Not to be taken for a cell either, as a
        // box of a cell's size was.
        if hot {
            let arm = half * 0.30;
            p.disc((cx, cy), half * 0.72, white(0.22));
            p.line((cx - arm, cy), (cx + arm, cy), 1.8 * s, white(1.0));
            p.line((cx, cy - arm), (cx, cy + arm), 1.8 * s, white(1.0));
        } else {
            let arm = half * 0.20;
            p.line((cx - arm, cy), (cx + arm, cy), 1.3 * s, white(0.26));
            p.line((cx, cy - arm), (cx, cy + arm), 1.3 * s, white(0.26));
        }
    }

    /// Drag feedback on the map under the cursor: where a dragged cell would
    /// land, and something at the cursor to stand for what is dragged.
    fn draw_drag(&self, p: &Painter, map: &Map) {
        let s = self.scale;
        let (cx, cy) = (self.cursor.0 - map.area.left, self.cursor.1 - map.area.top);
        if let Some(from) = self.dragged_row() {
            if let (Some((_, line)), Some(row)) = (self.row_slot_at(from, self.cursor.1), map.layout.rows.get(from)) {
                let right = map.layout.spine.right;
                p.fill(&rect(row.header.left, line - 2.0 * s, right - row.header.left, 4.0 * s), 2.0 * s, accent(1.0));
            }
        }
        if let Some(id) = self.dragged_cell() {
            match self.slot_at(id, self.cursor.0, self.cursor.1) {
                Some(Slot::Row { row, x, .. }) => {
                    let band = &map.layout.rows[row];
                    let body = band.cells.first().map_or(band.plus, |c| c.body);
                    p.fill(&rect(x - 2.0 * s, body.top, 4.0 * s, size(&body).1), 2.0 * s, accent(1.0));
                }
                Some(Slot::NewRow { y, .. }) => {
                    let spine = &map.layout.spine;
                    p.fill(&rect(spine.left, y - 2.0 * s, size(spine).0, 4.0 * s), 2.0 * s, accent(1.0));
                }
                None => {}
            }
            p.stroke(&rect(cx - 40.0 * s, cy - 25.0 * s, 80.0 * s, 50.0 * s), 8.0 * s, 2.0 * s, accent(0.9));
        }
        if let Some(hwnd) = self.dragged_window() {
            // With no thumbnail to follow the cursor (it could not be
            // registered), the window's icon does.
            if !self.thumbs.iter().any(|&(h, _)| h == hwnd) {
                if let Some(bitmap) = self.icons.get(&hwnd) {
                    p.bitmap(bitmap, &rect(cx - 16.0 * s, cy - 16.0 * s, 32.0 * s, 32.0 * s), 1.0);
                }
            }
        }
    }

    /// Draws the board (without live thumbnails) into a bitmap and returns the
    /// BGRA pixels, for reviewing the layout without showing a window.
    pub fn render_to_pixels(&mut self, model: Model, grid: Grid, size: (i32, i32), scale: f32, hover: Option<grid::Pos>, glide: Option<(grid::Pos, u64)>) -> Option<Vec<u8>> {
        self.size = size;
        self.scale = scale;
        self.home = rect(0.0, 0.0, size.0 as f32, size.1 as f32);
        self.monitors = vec![self.home];
        self.selected = model.current.clone();
        self.model = model;
        self.grid = grid;
        self.relayout();
        let now = Instant::now();
        let point_at = |board: &mut Self, pos: Option<grid::Pos>| {
            board.hover = Hit::Nothing;
            if let Some(cell) = pos.and_then(|pos| board.maps[0].layout.rows.get(pos.row)?.cells.get(pos.col)) {
                board.cursor = centre(&cell.body);
                board.hover = Hit::Cell(cell.id.clone());
            }
        };
        if let Some((from, _)) = glide {
            point_at(self, Some(from));
            self.update_popup(now);
            self.tick_popup(now + Duration::from_millis(POPUP_FADE_IN_MS));
        }
        let now = now + Duration::from_millis(POPUP_FADE_IN_MS);
        point_at(self, hover);
        self.update_popup(now);
        self.tick_popup(now + Duration::from_millis(glide.map_or(POPUP_FADE_IN_MS, |(_, ms)| ms)));
        self.fonts = None;
        let _ = self.draw_to_canvas()?;
        Some(self.canvas.as_ref()?.dib.pixels().to_vec())
    }
}

/// The panel is painted with the board, including when Windows exposes it
/// again. It never takes mouse input or the keyboard from its owner.
unsafe extern "system" fn popup_wndproc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match message {
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            InvalidateRect(GetWindow(hwnd, GW_OWNER), None, false);
            ValidateRect(hwnd, None);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

fn unregister_thumbnails(thumbs: &mut Vec<(isize, isize)>) {
    for (_, thumb) in thumbs.drain(..) {
        unsafe { let _ = DwmUnregisterThumbnail(thumb); }
    }
}

/// Keeps the panel above the whole map and within the hovered monitor,
/// including monitors whose origin is not the board's top-left corner.
fn popup_frame(cell: &Rect, monitor: &Rect, map_top: f32, empty: bool, s: f32) -> Rect {
    let bounds = grow(monitor, -12.0 * s);
    let w = (size(monitor).0 * 0.4).min(640.0 * s).min(size(&bounds).0).max(0.0);
    let bottom = (map_top - 16.0 * s).clamp(bounds.top, bounds.bottom);
    let h = if empty { 100.0 * s } else { (size(monitor).1 * 0.32).min(380.0 * s) };
    let h = h.min(bottom - bounds.top).max(0.0);
    let left = (centre(cell).0 - w / 2.0).clamp(bounds.left, bounds.right - w);
    rect(left, bottom - h, w, h)
}

fn make_fonts(scale: f32) -> Result<Fonts> {
    let factory = paint::dwrite_factory()?;
    let make = |size: f32, weight: i32, alignment: DWRITE_TEXT_ALIGNMENT| -> Result<IDWriteTextFormat> {
        let format = paint::text_format(&factory, size * scale, weight, alignment)?;
        // Text that does not fit ends in an ellipsis instead of being cut.
        let trimming = DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, delimiter: 0, delimiterCount: 0 };
        unsafe {
            let sign = factory.CreateEllipsisTrimmingSign(&format)?;
            format.SetTrimming(&trimming, &sign)?;
        }
        Ok(format)
    };
    Ok(Fonts {
        title: make(13.5, 600, DWRITE_TEXT_ALIGNMENT_LEADING)?,
        sub: make(11.0, 400, DWRITE_TEXT_ALIGNMENT_LEADING)?,
        label: make(15.0, 600, DWRITE_TEXT_ALIGNMENT_LEADING)?,
        small: make(12.0, 400, DWRITE_TEXT_ALIGNMENT_LEADING)?,
        centered: make(12.0, 400, DWRITE_TEXT_ALIGNMENT_CENTER)?,
    })
}

/// The window's own icon, falling back to its class icon.
fn window_icon(hwnd: HWND) -> Option<HICON> {
    unsafe {
        let mut icon = 0usize;
        SendMessageTimeoutW(hwnd, WM_GETICON, WPARAM(ICON_BIG as usize), LPARAM(0), SMTO_ABORTIFHUNG, 40, Some(&mut icon));
        if icon == 0 {
            icon = GetClassLongPtrW(hwnd, GCLP_HICON);
        }
        (icon != 0).then_some(HICON(icon as isize))
    }
}

/// A hash of the window's icon pixels, or 0 if it cannot be drawn.
pub fn icon_key(hwnd: HWND) -> u64 {
    thread_local! {
        // Keyed by icon handle: an app that redraws an icon in place keeps
        // its old key until Flick restarts.
        static KEYS: RefCell<HashMap<isize, u64>> = RefCell::new(HashMap::new());
    }
    let Some(icon) = window_icon(hwnd) else { return 0 };
    KEYS.with(|keys| {
        let mut keys = keys.borrow_mut();
        if let Some(&key) = keys.get(&icon.0) {
            return key;
        }
        let Ok(mut dib) = Dib::new((16, 16)) else { return 0 };
        dib.pixels_mut().fill(0);
        if unsafe { DrawIconEx(dib.dc, 0, 0, icon, 16, 16, 0, None, DI_NORMAL) }.is_err() {
            return 0;
        }
        // Finish GDI drawing before reading the bitmap's memory.
        if !unsafe { GdiFlush() }.as_bool() {
            return 0;
        }
        let mut hasher = DefaultHasher::new();
        dib.pixels().hash(&mut hasher);
        let key = hasher.finish().max(1);
        keys.insert(icon.0, key);
        key
    })
}

/// The window's icon as a Direct2D bitmap of `size` pixels.
pub fn icon_bitmap(target: &ID2D1RenderTarget, hwnd: HWND, size: i32) -> Option<ID2D1Bitmap> {
    let icon = window_icon(hwnd)?;
    unsafe {
        let mut dib = Dib::new((size, size)).ok()?;
        DrawIconEx(dib.dc, 0, 0, icon, size, size, 0, None, DI_NORMAL).ok()?;
        // Icons without an alpha channel leave alpha at zero; make what they drew opaque.
        let pixels = dib.pixels_mut();
        if pixels.chunks_exact(4).all(|p| p[3] == 0) {
            for p in pixels.chunks_exact_mut(4).filter(|p| p[0] | p[1] | p[2] != 0) {
                p[3] = 255;
            }
        }
        let (bits, pitch) = dib.raw();
        let properties = D2D1_BITMAP_PROPERTIES {
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
            dpiX: 96.0,
            dpiY: 96.0,
        };
        target.CreateBitmap(D2D_SIZE_U { width: size as u32, height: size as u32 }, Some(bits), pitch, &properties).ok()
    }
}

/// Rough width of a line of text at `size` pixels: enough to size a box
/// around a short label without laying the text out.
fn text_width(text: &str, size: f32) -> f32 {
    text.chars().map(|c| if c.is_ascii() { 0.56 } else { 1.0 }).sum::<f32>() * size
}

/// What to show as a window's title next to its app's name: the part that
/// tells this window from the app's others, shortened where it is long.
fn display_title(title: &str, app: &str, max_chars: usize) -> String {
    let mut title = title.trim();
    // "Document - App" repeats the app line; keep the document.
    if let Some((head, tail)) = title.rsplit_once(" - ") {
        let (tail, app) = (tail.trim().to_lowercase(), app.to_lowercase());
        if !head.trim().is_empty() && !app.is_empty() && (app.contains(&tail) || tail.contains(&app)) {
            title = head.trim();
        }
    }
    // The telling part of a path is its end, so cut paths at the front.
    let count = title.chars().count();
    let is_path = title.contains(":\\") || title.starts_with('/') || title.starts_with('~');
    if is_path && max_chars > 4 && count > max_chars {
        let tail: String = title.chars().skip(count - (max_chars - 1)).collect();
        return format!("…{tail}");
    }
    title.to_owned()
}

/// The largest share of the width of its area one preview may take, and the
/// highest it may be, unscaled.
const TILE_MAX: (f32, f32) = (0.6, 430.0);
/// The lowest a preview may be, unscaled.
const TILE_MIN_HEIGHT: f32 = 60.0;

/// Not narrower than the title bar of a tile needs for its buttons and a few letters.
fn tile_least(s: f32) -> f32 {
    (124.0 + Pin::WIDTH + Pin::TOP_WIDTH) * s
}

/// Importance sets area; real window dimensions supply only the aspect ratio.
/// All previews share a fit scale and keep the title-bar minimums (see `tiles`).
fn layout_tiles(windows: &[(&WindowModel, bool)], area: &Rect, s: f32, weights: &HashMap<isize, f32>) -> Vec<TileLayout> {
    let (aw, ah) = size(area);
    let (title_h, gap) = (46.0 * s, 26.0 * s);
    let sizes: Vec<(f32, f32)> = windows.iter().map(|(w, _)| {
        let aspect = (w.rect.right - w.rect.left).max(1) as f32 / (w.rect.bottom - w.rect.top).max(1) as f32;
        let area = (aw * ah).max(1.0) * weights.get(&w.hwnd.0).copied().unwrap_or(1.0);
        ((area * aspect).sqrt(), (area / aspect).sqrt())
    }).collect();
    let min_widths: Vec<f32> = vec![tile_least(s); windows.len()];
    let space = Space {
        area: (aw, ah),
        title: title_h,
        gap,
        max: (aw * TILE_MAX.0, (TILE_MAX.1 * s).min(ah - title_h).max(1.0)),
        min_height: TILE_MIN_HEIGHT * s,
    };
    let icon = 24.0 * s;
    let placed = tiles::arrange(&sizes, &min_widths, &space);
    placed
        .into_iter()
        .zip(windows)
        .map(|(at, &(window, _))| {
            let (x, y, w) = (area.left + at.x, area.top + at.y, at.w);
            let pin_w = Pin::WIDTH * s;
            let (close_w, pin_h) = (28.0 * s, 24.0 * s);
            let pin_y = y + (title_h - pin_h) / 2.0;
            let close = rect(x + w - 6.0 * s - close_w, pin_y, close_w, pin_h);
            let pin = rect(close.left - 4.0 * s - pin_w, pin_y, pin_w, pin_h);
            let top = rect(pin.left - 3.0 * s - Pin::TOP_WIDTH * s, pin_y, Pin::TOP_WIDTH * s, pin_h);
            TileLayout {
                hwnd: window.hwnd,
                frame: rect(x, y, w, title_h + at.h),
                icon: rect(x + 10.0 * s, y + (title_h - icon) / 2.0, icon, icon),
                title: rect(x + 18.0 * s + icon, y, (close.left - 6.0 * s - (x + 18.0 * s + icon)).max(0.0), title_h),
                thumb: rect(x, y + title_h, w, at.h),
                top,
                pin,
                close,
            }
        })
        .collect()
}

/// The badges of one map cell (`body`, of size `cell_w` by `cell_h`), one per
/// app in the order its windows are stacked, centred in the cell: as many as
/// fit, and how many did not.
fn layout_badges(cell: &CellModel, body: &Rect, (cell_w, cell_h): (f32, f32), icon: f32, s: f32) -> (Vec<AppBadge>, usize) {
    // Windows pinned to the row are shown by the row (`layout_pins`).
    let apps = group_apps(cell.windows.iter().filter(|w| !w.follows));
    let room = cell_w - 16.0 * s;
    let line_gap = 3.0 * s;
    let count_w = |size: f32| (size * 0.62).max(8.0 * s);
    let width = |size: f32, count: usize| size + if count > 1 { count_w(size) } else { 0.0 };
    let between = |size: f32| (5.0 * s * size / icon).max(3.0 * s);
    // The apps on each of `lines` lines of `size` icons, as many as fit; the
    // last line keeps clear of the "+N" in the corner when not all do.
    let pack = |size: f32, lines: usize| -> Vec<(usize, f32)> {
        let mut out: Vec<(usize, f32)> = Vec::new();
        let mut i = 0;
        for line in 0..lines {
            let last = line + 1 == lines;
            let (mut n, mut used) = (0, 0.0);
            while i + n < apps.len() {
                let next = used + width(size, apps[i + n].1) + if n > 0 { between(size) } else { 0.0 };
                let room = if last && i + n + 1 < apps.len() { room - 26.0 * s } else { room };
                if next > room {
                    break;
                }
                (n, used) = (n + 1, next);
            }
            if n == 0 {
                break;
            }
            out.push((n, used));
            i += n;
        }
        out
    };
    // More than one line keeps clear of the dot of the current cell.
    let lines_fit = |size: f32, lines: usize| {
        lines as f32 * size + (lines - 1) as f32 * line_gap <= cell_h - if lines > 1 { 30.0 } else { 12.0 } * s
    };
    // Rather than leave apps out, smaller icons, then a second line: what is
    // in a cell is told by its apps, so as many of them as can be seen are.
    let least = (icon * 0.55).max(11.0 * s).min(icon);
    let tries = [(1.0, 1), (0.8, 1), (0.65, 1), (0.8, 2), (0.65, 2), (0.0, 2)];
    let mut chosen = None;
    for (share, lines) in tries {
        let size = if share == 0.0 { least } else { (icon * share).max(least) };
        if !lines_fit(size, lines) {
            continue;
        }
        let packed = pack(size, lines);
        let shown: usize = packed.iter().map(|(n, _)| n).sum();
        chosen = Some((size, packed));
        if shown == apps.len() {
            break;
        }
    }
    let (size, packed) = chosen.unwrap_or_else(|| (icon, pack(icon, 1)));
    let block_h = packed.len() as f32 * size + packed.len().saturating_sub(1) as f32 * line_gap;
    let mut y = body.top + (cell_h - block_h) / 2.0;
    let mut badges = Vec::new();
    let mut i = 0;
    for (n, used) in &packed {
        let mut x = body.left + (cell_w - used) / 2.0;
        for &(window, count) in &apps[i..i + n] {
            badges.push(AppBadge { hwnd: window.hwnd, icon: rect(x, y, size, size), count, label: rect(x + size + 1.0 * s, y, count_w(size), size) });
            x += width(size, count) + between(size);
        }
        i += n;
        y += size + line_gap;
    }
    (badges, apps.len() - i)
}

/// Groups by icon, or executable when there is none, in the order first met.
fn group_apps<'a>(windows: impl Iterator<Item = &'a WindowModel>) -> Vec<(&'a WindowModel, usize)> {
    let mut apps: Vec<(&WindowModel, usize)> = Vec::new();
    for window in windows {
        match apps.iter_mut().find(|(first, _)| {
            if window.icon_key != 0 {
                first.icon_key == window.icon_key
            } else {
                first.icon_key == 0 && !window.app.is_empty() && first.app == window.app
            }
        }) {
            Some(entry) => entry.1 += 1,
            None => apps.push((window, 1)),
        }
    }
    apps
}

/// The apps of the windows pinned to a row, lined up rightwards from `left`
/// in its label, in at most `room`: the badges, how many did not fit, where
/// that number goes, and where they end.
fn layout_pins(row: &RowModel, header: &Rect, left: f32, room: f32, icon: f32, s: f32) -> (Vec<AppBadge>, usize, Rect, f32) {
    let apps = group_apps(row.cells.iter().flat_map(|c| &c.windows).filter(|w| w.follows));
    let count_w = icon * 0.62;
    let between = 5.0 * s;
    let more_w = 22.0 * s;
    let width = |count: usize| icon + if count > 1 { count_w } else { 0.0 };
    let y = header.top + (size(header).1 - icon) / 2.0;
    let mut used = 0.0;
    let mut shown = 0;
    for (i, (_, count)) in apps.iter().enumerate() {
        let reserve = if i + 1 < apps.len() { more_w } else { 0.0 };
        let next = used + width(*count) + if shown > 0 { between } else { 0.0 };
        if next + reserve > room {
            break;
        }
        used = next;
        shown += 1;
    }
    let more = apps.len() - shown;
    let mut x = left;
    let badges = apps[..shown]
        .iter()
        .map(|&(window, count)| {
            let badge = AppBadge { hwnd: window.hwnd, icon: rect(x, y, icon, icon), count, label: rect(x + icon + 1.0 * s, y, count_w, icon) };
            x += width(count) + between;
            badge
        })
        .collect();
    let more_label = rect(left + used + 2.0 * s, y, more_w, icon);
    (badges, more, more_label, left + used + if more > 0 { 2.0 * s + more_w } else { 0.0 })
}

/// Lays out the map at the bottom of a monitor: rows top to bottom, each
/// shifted sideways so that its landing cell (`anchors[row]`) sits on a common
/// column, the spine. `size` is the space above the footer; `s` the scale.
fn layout_map(model: &Model, anchors: &[usize], size: (f32, f32), s: f32) -> MapLayout {
    let (w, h) = size;
    let (margin, header_w, gap) = (60.0 * s, 170.0 * s, 10.0 * s);
    /// Share of the monitor's height the map may take.
    const MAX_SHARE: f32 = 0.34;

    let anchor = |r: usize| anchors.get(r).copied().unwrap_or(0);
    let rows = model.rows.len().max(1) as f32;
    let lead = (0..model.rows.len()).map(anchor).max().unwrap_or(0);
    let tail = model.rows.iter().enumerate().map(|(r, row)| row.cells.len().saturating_sub(anchor(r))).max().unwrap_or(1).max(1);
    let columns = (lead + tail) as f32;

    // Squares of a fixed look, shrunk only when the grid would not fit.
    let fit_w = (w - 2.0 * margin - header_w) / (columns + 1.2) - gap;
    let fit_h = (h * MAX_SHARE) / (rows + 1.2) - gap;
    let cell_h = (62.0 * s).min(fit_h).min(fit_w * 0.62).max(22.0 * s);
    let cell_w = cell_h / 0.62;
    let pitch = cell_w + gap;
    let row_pitch = cell_h + gap;
    let plus = cell_h * 0.6;

    let total_w = header_w + plus + gap + columns * pitch + plus;
    let total_h = plus + gap + rows * row_pitch + plus;
    let x0 = ((w - total_w) / 2.0).max(margin);
    let mut y = h - total_h - 10.0 * s;
    let cells_x = x0 + header_w + plus + gap;
    let spine_x = cells_x + lead as f32 * pitch;
    let icon = (cell_h * 0.36).min(22.0 * s);

    // The apps pinned to a row stand before its name, in a column as wide
    // as the most of them (at most half the label area), each row's against
    // the names, which stay in line.
    let pins_w = |row: &RowModel| layout_pins(row, &rect(0.0, 0.0, header_w, cell_h), 0.0, header_w / 2.0, icon, s).3;
    let pins_column = model.rows.iter().map(pins_w).fold(0.0, f32::max);
    let name_x = x0 + if pins_column > 0.0 { pins_column + 10.0 * s } else { 0.0 };

    let mut layout = MapLayout { top: y, ..MapLayout::default() };
    layout.add_row_top = rect(spine_x + (cell_w - plus) / 2.0, y, plus, plus);
    y += plus + gap;
    let rows_top = y;
    for (r, row) in model.rows.iter().enumerate() {
        let first_x = spine_x - anchor(r) as f32 * pitch;
        let mut x = first_x;
        let mut cells = Vec::new();
        for cell in &row.cells {
            let body = rect(x, y, cell_w, cell_h);
            let (apps, more) = layout_badges(cell, &body, (cell_w, cell_h), icon, s);
            cells.push(CellLayout {
                id: cell.id.clone(),
                emphasised: cell.emphasised,
                usage: cell.usage,
                body,
                close: rect(body.right - 15.0 * s, body.top - 5.0 * s, 20.0 * s, 20.0 * s),
                apps,
                more,
                more_label: rect(body.right - 28.0 * s, body.bottom - 18.0 * s, 24.0 * s, 16.0 * s),
            });
            x += pitch;
        }
        let header = rect(x0, y, header_w, cell_h);
        let (pins, pins_more, pins_more_label, _) = layout_pins(row, &header, x0 + pins_column - pins_w(row), header_w / 2.0, icon, s);
        let name_room = header.right - name_x - 12.0 * s;
        // As wide as the name itself, so the clickable box matches what is read.
        let name_w = (text_width(&grid::row_title(&row.name, r), 12.0 * s) + 4.0 * s).min(name_room);
        layout.rows.push(RowLayout {
            header,
            name: rect(name_x, y + (cell_h - 20.0 * s) / 2.0, name_w, 20.0 * s),
            top: y - gap / 2.0,
            bottom: y + cell_h + gap / 2.0,
            cells,
            plus: rect(x, y + (cell_h - plus) / 2.0, plus, plus),
            plus_left: rect(first_x - gap - plus, y + (cell_h - plus) / 2.0, plus, plus),
            pins,
            pins_more,
            pins_more_label,
        });
        y += row_pitch;
    }
    layout.spine = rect(spine_x - gap / 2.0, rows_top - gap / 2.0, pitch, (y - rows_top).max(0.0));
    layout.add_row = rect(spine_x + (cell_w - plus) / 2.0, y - gap / 2.0 + 2.0 * s, plus, plus);
    layout
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn popup_glides_continue_from_the_current_frame_and_connector() {
        let now = Instant::now();
        let a = rect(10.0, 20.0, 500.0, 100.0);
        let b = rect(200.0, 50.0, 640.0, 350.0);
        let c = rect(-100.0, 80.0, 400.0, 200.0);
        let cell = |x| rect(x, 800.0, 70.0, 50.0);
        let mut animation = PopupAnimation::new(a, cell(10.0), now);
        animation.glide_to(b, cell(200.0), now);
        let halfway = now + Duration::from_millis(60);
        let before = animation.sample(halfway);
        assert!(before.0.left > 105.0 && before.0.left < b.left);
        assert!(before.0.bottom > a.bottom && before.0.bottom < b.bottom);
        animation.glide_to(c, cell(-100.0), halfway);
        assert_eq!(animation.sample(halfway), before);
        let end = halfway + Duration::from_millis(POPUP_GLIDE_MS);
        assert_eq!(animation.sample(end), (c, cell(-100.0), 1.0));
        assert_eq!(animation.next_tick(end), None);
    }

    #[test]
    fn popup_gaps_wait_once_then_fade_without_extending_the_grace() {
        let now = Instant::now();
        let frame = rect(0.0, 0.0, 640.0, 350.0);
        let mut animation = PopupAnimation::new(frame, frame, now);
        let left = now + Duration::from_millis(80);
        assert_eq!(animation.sample(left).2, 1.0);
        assert_eq!(animation.next_tick(left), None);
        animation.leave(left);
        assert_eq!(animation.next_tick(left), Some(200));
        animation.leave(left + Duration::from_millis(100));
        assert_eq!(animation.sample(left + Duration::from_millis(199)).2, 1.0);
        let fading = left + Duration::from_millis(260);
        assert!((animation.sample(fading).2 - 0.125).abs() < 0.001);
        assert_eq!(animation.next_tick(fading), Some(16));
        let end = left + Duration::from_millis(320);
        assert_eq!(animation.sample(end).2, 0.0);
        assert_eq!(animation.next_tick(end), None);
        assert_eq!(animation.sample(end + Duration::from_secs(1)).2, 0.0);
    }

    #[test]
    fn returning_during_a_gap_or_fade_keeps_the_current_opacity() {
        let now = Instant::now();
        let frame = rect(0.0, 0.0, 640.0, 350.0);
        let mut animation = PopupAnimation::new(frame, frame, now);
        assert_eq!(animation.sample(now).2, 0.0);
        assert_eq!(animation.next_tick(now), Some(16));
        let visible = now + Duration::from_millis(80);
        animation.leave(visible);
        animation.show(visible + Duration::from_millis(100));
        assert_eq!(animation.sample(visible + Duration::from_millis(200)).2, 1.0);
        animation.leave(visible + Duration::from_millis(200));
        let returned = visible + Duration::from_millis(460);
        let alpha = animation.sample(returned).2;
        assert!(alpha > 0.0 && alpha < 1.0);
        animation.show(returned);
        assert_eq!(animation.sample(returned).2, alpha);
        assert!(animation.sample(returned + Duration::from_millis(40)).2 > alpha);
        let settled = returned + Duration::from_millis(80);
        assert_eq!(animation.sample(settled).2, 1.0);
        assert_eq!(animation.next_tick(settled), None);
    }

    #[test]
    fn popup_stays_above_the_map_on_its_monitor() {
        for (monitor, s) in [(rect(0.0, 0.0, 1920.0, 1080.0), 1.0),
            (rect(2560.0, 400.0, 1280.0, 720.0), 1.5),
            (rect(-1600.0, -900.0, 1600.0, 900.0), 1.0)] {
            let map_top = monitor.top + size(&monitor).1 * 0.65;
            for x in [monitor.left + 12.0 * s, centre(&monitor).0, monitor.right - 90.0 * s] {
                let cell = rect(x, map_top + 20.0 * s, 70.0 * s, 50.0 * s);
                for empty in [false, true] {
                    let frame = popup_frame(&cell, &monitor, map_top, empty, s);
                    assert!(frame.left >= monitor.left && frame.right <= monitor.right);
                    assert!(frame.top >= monitor.top && frame.bottom < map_top);
                    assert!(size(&frame).0 > 0.0 && size(&frame).0 <= size(&monitor).0 * 0.4 + 0.01);
                    assert!(size(&frame).1 > 0.0);
                    assert!(!overlaps(&frame, &cell));
                }
            }
        }
    }

    fn window(hwnd: isize, follows: bool) -> WindowModel {
        WindowModel { hwnd: HWND(hwnd), rect: RECT { right: 1280, bottom: 720, ..Default::default() },
            follows, minimized: false, title: format!("Window {hwnd}"), app: String::new(),
            icon_key: 0, app_name: String::new(), topmost: false }
    }

    #[test]
    fn popup_keeps_row_followers_and_leaves_all_rows_on_the_board() {
        let mut rows = vec![row(&["a", "b"]), row(&["c"])];
        rows[0].cells[0].windows = vec![window(1, false), window(2, true)];
        rows[0].cells[1].windows = vec![window(3, false), window(4, true)];
        rows[1].cells[0].windows = vec![window(5, true)];
        let model = Model { rows, pinned: vec![window(6, false)], current: "a".into() };
        let handles = |id| model.tile_windows(id).iter().map(|(window, all)| (window.hwnd.0, *all)).collect::<Vec<_>>();
        assert_eq!(handles("a"), [(1, false), (2, false), (4, false), (6, true)]);
        assert_eq!(handles("b"), [(2, false), (3, false), (4, false), (6, true)]);
        assert_eq!(handles("c"), [(5, false), (6, true)]);
        let popup = popup_frame(&rect(100.0, 800.0, 70.0, 50.0), &rect(0.0, 0.0, 1920.0, 1080.0), 760.0, false, 1.0);
        let body = Rect { top: popup.top + 46.0, ..grow(&popup, -14.0) };
        let handles = |id| model.popup_windows(id).iter().map(|(window, _)| window.hwnd.0).collect::<Vec<_>>();
        assert_eq!(handles("a"), [1, 2, 4]);
        assert_eq!(handles("b"), [2, 3, 4]);
        assert_eq!(handles("c"), [5]);
        let empty = Model { rows: vec![row(&["empty"])], pinned: vec![window(6, false)], current: "empty".into() };
        assert!(empty.popup_windows("empty").is_empty());
        let tiles = layout_tiles(&model.popup_windows("b"), &body, 0.55, &HashMap::new());
        assert_eq!(tiles.len(), 3);
        for tile in tiles {
            assert!(tile.frame.left >= body.left - 0.5 && tile.frame.right <= body.right + 0.5);
            assert!(tile.frame.top >= body.top - 0.5 && tile.frame.bottom <= body.bottom + 0.5);
        }
    }

    #[test]
    fn preview_weights_share_one_cell_maximum_and_keep_title_widths() {
        let mut rows = vec![row(&["a", "b"])];
        rows[0].cells[0].windows = vec![window(1, false), window(2, false)];
        rows[0].cells[1].windows = vec![window(3, true)];
        for cell in &mut rows[0].cells {
            for w in &mut cell.windows {
                w.icon_key = 9;
                w.rect = RECT { right: 80, bottom: 50, ..Default::default() };
            }
        }
        let model = Model { rows, pinned: vec![], current: "a".into() };
        let mut grid = Grid::default();
        let area = rect(0.0, 0.0, 1600.0, 900.0);
        let no_data = model.window_weights("a", &grid);
        assert!(no_data.values().all(|weight| *weight == 1.0));
        assert_eq!(model.window_weights("b", &grid), HashMap::from([(3, 1.0)]));
        let now = usage::now();
        for (hwnd, count) in [(2, 10), (3, 5)] {
            grid.window_usage.entry(hwnd).or_default().record(now, count as f64, usage::WINDOW_HALF_LIFE);
        }
        let weights = model.window_weights("a", &grid);
        assert_eq!(weights[&1], 1.0);
        assert_eq!(weights[&2], 4.0);
        assert!((weights[&3] - (1.0 + 3.0 * 0.5_f32.sqrt())).abs() < 0.0001);
        let other_cell = model.window_weights("b", &grid);
        assert_eq!(other_cell, HashMap::from([(3, 4.0)]));
        // A part gets the whole cell's weights even when its winner is elsewhere.
        let part = vec![(&model.rows[0].cells[0].windows[0], false)];
        let tiles = layout_tiles(&part, &area, 1.0, &weights);
        assert!(size(&tiles[0].frame).0 >= tile_least(1.0));
        let popup = layout_tiles(&model.popup_windows("a"), &area, 1.0, &weights);
        assert_eq!(popup.len(), 3);
        assert!(popup.iter().all(|tile| size(&tile.frame).0 >= tile_least(1.0)));
    }

    #[test]
    fn preview_usage_scales_area_instead_of_each_side() {
        let mut windows = [window(1, false), window(2, false)];
        windows[0].rect = RECT { right: 320, bottom: 180, ..Default::default() };
        windows[1].rect = RECT { right: 3840, bottom: 2160, ..Default::default() };
        let windows: Vec<_> = windows.iter().map(|w| (w, false)).collect();
        let weights = HashMap::from([(1, usage::preview_factor(0.0, 1.0)), (2, usage::preview_factor(1.0, 1.0))]);
        for scale in [1.0, 0.55] {
            let tiles = layout_tiles(&windows, &rect(0.0, 0.0, 2400.0, 1400.0), scale, &weights);
            assert_eq!(tiles.len(), 2);
            let (w0, h0) = size(&tiles[0].thumb);
            let (w1, h1) = size(&tiles[1].thumb);
            let side_ratio = 2.0;
            assert!((w1 / w0 - side_ratio).abs() < 0.0001);
            assert!((h1 / h0 - side_ratio).abs() < 0.0001);
            assert!((w1 * h1 / (w0 * h0) - 4.0).abs() < 0.0001);
        }
    }

    #[test]
    fn previews_without_history_have_equal_area_and_keep_aspect_ratios() {
        let mut windows = [window(1, false), window(2, false), window(3, false)];
        for (window, (w, h)) in windows.iter_mut().zip([(320, 180), (1920, 1200), (2400, 1800)]) {
            window.rect = RECT { right: w, bottom: h, ..Default::default() };
        }
        let windows: Vec<_> = windows.iter().map(|w| (w, false)).collect();
        for scale in [1.0, 0.55] {
            let tiles = layout_tiles(&windows, &rect(0.0, 0.0, 2400.0, 1400.0), scale, &HashMap::new());
            assert_eq!(tiles.len(), windows.len());
            let (w, h) = size(&tiles[0].thumb);
            for (tile, (window, _)) in tiles.iter().zip(&windows) {
                let (tw, th) = size(&tile.thumb);
                assert!((tw * th / (w * h) - 1.0).abs() < 0.0001);
                let aspect = window.rect.right as f32 / window.rect.bottom as f32;
                assert!((tw / th - aspect).abs() < 0.0001);
            }
        }
    }

    #[test]
    fn apps_are_grouped_by_icon_with_executable_fallback() {
        let cases = [
            (vec![("brave.exe", 1), ("brave.exe", 2)], vec![(1, 1), (2, 1)]),
            (vec![("brave.exe", 1), ("brave.exe", 1)], vec![(1, 2)]),
            (vec![("brave.exe", 1), ("brave.exe", 2), ("brave.exe", 1)], vec![(1, 2), (2, 1)]),
            (vec![("brave.exe", 1), ("other.exe", 1)], vec![(1, 2)]),
            (vec![("brave.exe", 0), ("brave.exe", 0), ("other.exe", 0)], vec![(1, 2), (3, 1)]),
            (vec![("brave.exe", 1), ("brave.exe", 0)], vec![(1, 1), (2, 1)]),
            (vec![("brave.exe", 0), ("brave.exe", 1)], vec![(1, 1), (2, 1)]),
            (vec![("", 0), ("", 0)], vec![(1, 1), (2, 1)]),
        ];
        for (entries, expected) in cases {
            let windows: Vec<WindowModel> = entries
                .into_iter()
                .enumerate()
                .map(|(i, (app, icon_key))| WindowModel {
                    hwnd: HWND((i + 1) as isize),
                    rect: RECT::default(),
                    minimized: false,
                    title: String::new(),
                    app: app.into(),
                    icon_key,
                    app_name: String::new(),
                    follows: false,
                    topmost: false,
                })
                .collect();
            let groups: Vec<_> = group_apps(windows.iter()).into_iter().map(|(window, count)| (window.hwnd.0, count)).collect();
            assert_eq!(groups, expected);
        }
    }

    fn row(cells: &[&str]) -> RowModel {
        RowModel {
            name: String::new(),
            memory: 0,
            asleep: false,
            cells: cells.iter().map(|id| CellModel { id: id.to_string(), emphasised: false, usage: 0.0, windows: Vec::new() }).collect(),
        }
    }

    fn overlaps(a: &Rect, b: &Rect) -> bool {
        a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom
    }

    /// Every + of the map is on the screen and clear of every cell, so that
    /// it can be seen and pointed at, whatever the rows' lengths and the
    /// cells they are lined up on.
    #[test]
    fn plus_buttons_are_on_screen_and_off_the_cells() {
        let rows = vec![row(&["a"]), row(&["b"]), row(&["c", "d", "e"]), row(&["f"]), row(&["g", "h"])];
        let model = Model { rows, pinned: Vec::new(), current: "d".into() };
        let size = (1920.0, 1200.0 - FOOTER);
        for anchors in [vec![0, 0, 1, 0, 1], vec![0, 0, 2, 0, 0], vec![0, 0, 0, 0, 1]] {
            let layout = layout_map(&model, &anchors, size, 1.0);
            let screen = rect(0.0, 0.0, size.0, size.1);
            let cells: Vec<&Rect> = layout.rows.iter().flat_map(|r| &r.cells).map(|c| &c.body).collect();
            let pluses = layout.rows.iter().flat_map(|r| [&r.plus, &r.plus_left]).chain([&layout.add_row, &layout.add_row_top]);
            for plus in pluses {
                let inside = plus.left >= screen.left && plus.right <= screen.right && plus.top >= screen.top && plus.bottom <= screen.bottom;
                assert!(inside, "a + off the screen with anchors {anchors:?}");
                assert!(cells.iter().all(|c| !overlaps(c, plus)), "a + on a cell with anchors {anchors:?}");
            }
        }
    }
}
