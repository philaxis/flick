//! The full-screen view: the selected cell's windows as live previews, each
//! with area set by recent active-stay credits and each window's aspect
//! ratio preserved, and a small map of all rows and cells on every monitor.
//! Windows can be dragged onto cells and monitors, cells around the map.
//!
//! The board owns layout, hit-testing, drawing and the DWM thumbnails. It does
//! not touch desktops itself; input is turned into an `Action` for the app.

mod sheet;
mod strip;

use crate::{
    grid::{self, CellId, Dir, Grid, Pos},
    pin::Pin,
    paint::{self, accent, rect, rgba, warm, white, Canvas, Dib, Glow, Painter, Rect},
    tiles::{self, Space},
    usage,
    vd,
};
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    hash::{DefaultHasher, Hash, Hasher},
    io::Write as _,
    sync::{
        atomic::{AtomicBool, AtomicIsize, Ordering},
        Arc, OnceLock,
    },
    time::{Duration, Instant},
};
use strip::{Place, Strip};
use windows::{
    core::{w, ComInterface, Error, Result},
    Foundation::Numerics::Matrix3x2,
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
        Graphics::{
            Direct2D::{
                Common::{
                    D2D1_ALPHA_MODE_IGNORE, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_SIZE_U,
                },
                ID2D1Bitmap, ID2D1Factory, ID2D1RenderTarget, ID2D1HwndRenderTarget, D2D1_HWND_RENDER_TARGET_PROPERTIES,
                D2D1_RENDER_TARGET_PROPERTIES, D2D1_PRESENT_OPTIONS_IMMEDIATELY, D2D1_ANTIALIAS_MODE_ALIASED, D2D1_BITMAP_PROPERTIES,
            },
            DirectWrite::{
                IDWriteTextFormat, DWRITE_TEXT_ALIGNMENT, DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING,
                DWRITE_TRIMMING, DWRITE_TRIMMING_GRANULARITY_CHARACTER,
            },
            Dwm::{
                DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS,
                DWM_THUMBNAIL_PROPERTIES, DWM_TNP_OPACITY, DWM_TNP_RECTDESTINATION,
                DWM_TNP_RECTSOURCE, DWM_TNP_SOURCECLIENTAREAONLY, DWM_TNP_VISIBLE,
            },
            Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
            Gdi::{
                BitBlt, GdiFlush, GetDC, InvalidateRect, ReleaseDC, ValidateRect, SRCCOPY,
            },
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
                CreateWindowExW, PostMessageW, WM_APP, DrawIconEx, GetClassLongPtrW, GetWindowRect, LoadCursorW, RegisterClassW, SendMessageTimeoutW,
                KillTimer, SetTimer, SetWindowPos, ShowWindow, CS_DBLCLKS, DI_NORMAL, GCLP_HICON, HICON, HWND_TOPMOST,
                GetCursorPos, GetAncestor, GetClassNameW, GetWindowThreadProcessId, WindowFromPoint, GA_ROOT, SW_SHOWMINNOACTIVE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOACTIVATE, ICON_BIG, IDC_ARROW, SMTO_ABORTIFHUNG, SW_SHOWNOACTIVATE, SW_HIDE, WM_ACTIVATE, WM_CHAR, WM_ERASEBKGND,
                WM_GETICON, WM_IME_COMPOSITION, WM_IME_STARTCOMPOSITION, WM_KEYDOWN, WM_LBUTTONDBLCLK,
                WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_PAINT, WM_RBUTTONUP, WM_TIMER, WNDCLASSW,
                WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
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
    /// The app owns this pin; a window button cannot remove it.
    pub pin_from_app: bool,
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

    /// The desktop has no windows of its own, only those pinned, as known
    /// from the model rather than from previews still on their way.
    fn has_no_own_windows(&self, id: &str) -> bool {
        self.cell(id).is_some_and(|cell| cell.windows.iter().all(|w| w.follows))
    }

    /// Normalize across the whole cell, before splitting monitors or pinned
    /// sections, so a window has the same weight wherever it is shown.
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
    /// Esc or a click on nothing: close and go to the selected cell, or,
    /// that being the current desktop, give focus back.
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
/// cursor is or does (shown the first time, and with ?), in unscaled units.
const FOOTER: f32 = 44.0;

/// While the board is up, how often it checks that nothing came over it.
const ON_TOP_TIMER_ID: usize = 2;
const ON_TOP_MS: u32 = 250;
/// How long the cursor rests on a preview before its window's title shows.
const TITLE_TIMER_ID: usize = 3;
const TITLE_MS: u32 = 900;
/// A cell of the map pointed at is shown in the preview area, this much
/// smaller and as bright, and goes back or forward in this long.
const PEEK_SCALE: f32 = 0.93;
const PEEK_SHADE: f32 = 0.62;
const PEEK_MS: f32 = 170.0;
/// Under each preview: its app's icon and, on hover, its buttons and title.
const FOOT: f32 = 30.0;
/// While what moved eases into place, how often it is drawn again.
const MOTION_TIMER_ID: usize = 4;
/// Requested board animation interval; WM_TIMER also runs when vblank stops.
const MOTION_MS: u32 = 15;
/// Natural period in seconds of the critically damped board, tile and popup
/// springs: a jump from rest covers 98% of its distance in about 0.2 seconds.
const MOTION_RESPONSE: f32 = 0.22;
/// Angular frequency (rad/s); damping is exactly critical, with no bounce.
const MOTION_OMEGA: f32 = std::f32::consts::TAU / MOTION_RESPONSE;
/// Under this much left to go, in pixels, a part is in place.
const SETTLED: f32 = 0.5;
/// A near-target spring must also be this slow (px/s) before its tail is cut.
const SETTLED_SPEED: f32 = SETTLED * MOTION_OMEGA;
/// The longest name a shelf can be given, in characters.
const NAME_LENGTH: usize = 24;
/// Sent after the cursor leaves a window that called `TrackMouseEvent`.
const WM_MOUSELEAVE: u32 = 0x02A3;
/// Posted to the board by its `FrameClock` after a vblank while it moves.
const WM_BOARD_FRAME: u32 = WM_APP + 40;
/// After a frame brought by a vblank, how long the motion timer's own
/// ticks are not drawn: only when vblanks stop coming does it draw.
const VBLANK_TRUSTED: Duration = Duration::from_millis(40);

/// Wakes the board at each vblank of the primary monitor while it moves,
/// so that it draws once a refresh, in step with the screen, and never
/// waits for one on the UI thread (`WaitForVBlank` has no timeout and
/// blocks only this clock's own thread). At most one wake-up is waiting
/// in the board's queue at a time.
#[derive(Default)]
struct FrameClock {
    shared: Arc<ClockShared>,
    thread: Option<std::thread::Thread>,
}

#[derive(Default)]
struct ClockShared {
    running: AtomicBool,
    posted: AtomicBool,
    /// Find the primary monitor's output again (the board was opened anew).
    fresh: AtomicBool,
    hwnd: AtomicIsize,
}

impl FrameClock {
    fn start(&mut self, hwnd: HWND) {
        let shared = &self.shared;
        shared.hwnd.store(hwnd.0, Ordering::SeqCst);
        shared.running.store(true, Ordering::SeqCst);
        match &self.thread {
            Some(thread) => thread.unpark(),
            None => {
                let shared = self.shared.clone();
                self.thread = std::thread::Builder::new()
                    .name("board vblank".into())
                    .spawn(move || clock_loop(&shared))
                    .ok()
                    .map(|handle| handle.thread().clone());
            }
        }
    }

    fn stop(&mut self) {
        self.shared.running.store(false, Ordering::SeqCst);
        self.shared.posted.store(false, Ordering::SeqCst);
    }

    /// The board took the wake-up out of its queue.
    fn frame_taken(&self) {
        self.shared.posted.store(false, Ordering::SeqCst);
    }

    fn refresh(&self) {
        self.shared.fresh.store(true, Ordering::SeqCst);
    }
}

fn clock_loop(shared: &ClockShared) {
    let Ok(mut pacer) = crate::overlay::FramePacer::new() else { return };
    let mut frame = Instant::now();
    loop {
        while !shared.running.load(Ordering::SeqCst) { std::thread::park(); }
        if shared.fresh.swap(false, Ordering::SeqCst) {
            let mut fastest = None;
            for monitor in crate::vd::monitors() {
                pacer.place(monitor.work);
                if fastest.as_ref().map_or(true, |(_, period)| pacer.period < *period) { fastest = Some((monitor.work, pacer.period)); }
            }
            if let Some((area, _)) = fastest { pacer.place(area); }
            frame = Instant::now();
        }
        // Some drivers return from WaitForVBlank immediately; share the validated
        // refresh clock instead of flooding the UI queue with unpaced frames.
        let (_, vblank) = pacer.wait(frame).unwrap_or_default();
        let now = Instant::now();
        frame = if vblank { now } else { (frame + pacer.period).max(now - pacer.period) };
        if shared.running.load(Ordering::SeqCst) && !shared.posted.swap(true, Ordering::SeqCst) {
            let hwnd = HWND(shared.hwnd.load(Ordering::SeqCst));
            if unsafe { PostMessageW(hwnd, WM_BOARD_FRAME, WPARAM(vblank as usize), LPARAM(0)) }.is_err() {
                shared.posted.store(false, Ordering::SeqCst);
            }
        }
    }
}

/// One drawn frame of the board's motion, as `FLICK_TRACE` has it.
struct TraceFrame {
    at: Duration,
    interval: Option<Duration>,
    by_vblank: bool,
    /// The hand as the input thread has it (`input::Hand`), in cells.
    hand: (f32, f32),
    /// Where the selected cell's picture is drawn on the home monitor (its
    /// top left corner, px) and its size, and how far back the strip is.
    drawn: (f32, f32, f32),
    depth: f32,
    draw: Duration,
    dwm: Duration,
    submit: Duration,
    thumbs: usize,
    updates: usize,
    /// First selected thumbnail: handle, DWM destination/source, transformed tile.
    geometry: Option<(isize, RECT, RECT, Rect)>,
    submitted_qpc_ms: f64,
}

/// The board's frames written to the CSV file `FLICK_TRACE` names: kept in
/// memory while it moves, and written in one go once it is still, so that
/// no file is touched while drawing.
struct Trace {
    path: std::path::PathBuf,
    frames: Vec<TraceFrame>,
    last: Option<Instant>,
}

/// What every trace's times are counted from (`Instant` is the
/// performance counter).
fn trace_epoch() -> Instant {
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}

impl Trace {
    fn from_env() -> Option<Trace> {
        let path = std::env::var_os("FLICK_TRACE").filter(|path| !path.is_empty())?;
        trace_epoch();
        Some(Trace { path: path.into(), frames: Vec::with_capacity(1024), last: None })
    }

    fn record(&mut self, frame: TraceFrame, started: Instant) {
        let interval = self.last.map(|last| started.saturating_duration_since(last));
        self.last = Some(started);
        self.frames.push(TraceFrame { interval, ..frame });
    }

    fn flush(&mut self) {
        self.last = None;
        if self.frames.is_empty() {
            return;
        }
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        let mut out = String::new();
        let new = std::fs::metadata(&self.path).map_or(true, |m| m.len() == 0);
        if new {
            out.push_str("qpc_ms,interval_ms,wake,hand_along,hand_across,drawn_x,drawn_y,drawn_scale,depth,draw_ms,dwm_ms,thumbnails,submit_ms,thumbnail_updates,absolute_qpc_ms\n");
        }
        let geometry_path = self.path.with_extension("geometry.csv");
        let mut geometry = if geometry_path.exists() { String::new() } else { "qpc_ms,hwnd,dest_l,dest_t,dest_r,dest_b,source_l,source_t,source_r,source_b,tile_l,tile_t,tile_r,tile_b,absolute_qpc_ms\n".into() };
        for f in self.frames.drain(..) {
            if let Some((hwnd, d, s, tile)) = f.geometry {
                geometry.push_str(&format!("{:.3},{hwnd:x},{},{},{},{},{},{},{},{},{:.3},{:.3},{:.3},{:.3},{:.3}\n", ms(f.at),
                    d.left, d.top, d.right, d.bottom, s.left, s.top, s.right, s.bottom, tile.left, tile.top, tile.right, tile.bottom, f.submitted_qpc_ms));
            }
            out.push_str(&format!("{:.3},{},{},{:.4},{:.4},{:.1},{:.1},{:.4},{:.4},{:.3},{:.3},{},{:.3},{},{:.3}\n",
                ms(f.at), f.interval.map_or(String::new(), |i| format!("{:.3}", ms(i))),
                if f.by_vblank { "vblank" } else { "timer" }, f.hand.0, f.hand.1, f.drawn.0, f.drawn.1, f.drawn.2,
                f.depth, ms(f.draw), ms(f.dwm), f.thumbs, ms(f.submit), f.updates, f.submitted_qpc_ms));
        }
        let written = std::fs::OpenOptions::new().create(true).append(true).open(&self.path)
            .and_then(|mut file| file.write_all(out.as_bytes()));
        if let Err(e) = written {
            crate::app::log(&format!("board trace not written: {e}"));
        }
        if let Err(e) = std::fs::OpenOptions::new().create(true).append(true).open(geometry_path).and_then(|mut file| file.write_all(geometry.as_bytes())) {
            crate::app::log(&format!("board geometry trace not written: {e}"));
        }
    }
}

/// A cell's windows laid out as when it is the one selected: their tiles
/// and the parts they are in.
#[derive(Default)]
struct CellTiles {
    tiles: Vec<TileLayout>,
    sections: Vec<Section>,
}

/// A part of the preview area: the cell's own windows, or one of the
/// places of the pinned ones, with, when they did not all fit, the cell
/// that stands for the rest (a few of their windows, and how many).
struct Section {
    part: Rect,
    carried: Carried,
    more: Option<(Rect, Vec<HWND>, usize)>,
}

/// A cell beside the selection, shown while the strip moves.
struct StripCell {
    id: CellId,
    pos: Pos,
    cell: CellTiles,
}

/// What a tile moves with when the selection moves.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Carried {
    /// On every desktop: it never moves.
    Fixed,
    /// Pinned to its row: it stays while the selection moves in the row.
    Row,
    /// The cell's own.
    Own,
}

impl Carried {
    fn of(window: &WindowModel, everywhere: bool) -> Carried {
        if everywhere { Carried::Fixed } else if window.follows { Carried::Row } else { Carried::Own }
    }

    /// Whether a cell beside the selection, in the selected row or not,
    /// shows it: what moves with the selection is shown once, there.
    fn beside(self, in_selected_row: bool) -> bool {
        match self {
            Carried::Fixed => false,
            Carried::Row => !in_selected_row,
            Carried::Own => true,
        }
    }
}

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

/// The colour of what is pinned to all rows, on its pin and its place
/// (`paint::PIN_ALL`).
fn everywhere(a: f32) -> D2D1_COLOR_F {
    paint::pinned(true, a)
}

/// The colour of what is pinned to its row (`paint::PIN_ROW`).
fn on_row(a: f32) -> D2D1_COLOR_F {
    paint::pinned(false, a)
}

/// The shadows and the glows the board is drawn with, made for its target.
struct Glows {
    shadow: Glow,
    light: Glow,
    /// The light of what is pinned to every shelf, and to its own.
    all: Glow,
    row: Glow,
    /// The glow of the mark of a row pinned to.
    row_mark: Glow,
    accent: Glow,
    warm: Glow,
}

impl Glows {
    fn new(target: &ID2D1RenderTarget) -> Option<Glows> {
        Some(Glows {
            shadow: Glow::new(target, rgba(0.0, 0.0, 0.0, 1.0))?,
            light: Glow::new(target, rgba(0.80, 0.86, 1.0, 1.0))?,
            all: Glow::new(target, paint::lit(true, 1.0))?,
            row: Glow::new(target, paint::lit(false, 1.0))?,
            row_mark: Glow::new(target, on_row(1.0))?,
            accent: Glow::new(target, accent(1.0))?,
            warm: Glow::new(target, warm(1.0))?,
        })
    }

    fn of(&self, pin: Pin) -> Option<&Glow> {
        match pin {
            Pin::Everywhere => Some(&self.all),
            Pin::Row => Some(&self.row),
            Pin::Off => None,
        }
    }
}

/// What a pin is shown in.
fn pin_colour(pin: Pin, a: f32) -> D2D1_COLOR_F {
    match pin {
        Pin::Everywhere => everywhere(a),
        Pin::Row => on_row(a),
        Pin::Off => white(0.70 * a),
    }
}

fn shifted_rect(r: &Rect, dx: f32, dy: f32) -> Rect {
    Rect { left: r.left + dx, top: r.top + dy, right: r.right + dx, bottom: r.bottom + dy }
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

impl Pin {
    fn of(window: &WindowModel, everywhere: bool) -> Pin {
        Pin::from_flags(window.follows, everywhere)
    }
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
    /// Some windows are pinned to the row: they are in whichever of its
    /// cells the user is, so no cell shows them, and a dot by its name says so.
    pinned: bool,
}

/// One window of the selected cell, shown large: its live preview with its
/// app's icon under the corner, or, where there is no room for one, the icon
/// alone.
struct TileLayout {
    hwnd: HWND,
    /// The preview and the line under it.
    frame: Rect,
    icon: Rect,
    /// Where its title is written, after resting on it a while.
    title: Rect,
    thumb: Rect,
    /// The "above the others", pin and close buttons at the right of the
    /// line under the preview, shown under the cursor.
    top: Rect,
    pin: Rect,
    close: Rect,
    /// Shown by its icon: there was no room for a preview.
    icon_only: bool,
}

/// What moves as one when the selection does.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
enum Part {
    /// A row's cells and its + buttons on a monitor's map, known by the
    /// row's first cell.
    Row(usize, CellId),
    /// The row names and the apps pinned to the rows, on a monitor's map.
    Names(usize),
    /// The column vertical moves travel along, with the + buttons on it.
    Spine(usize),
    /// The frame of the selected cell, on a monitor's map.
    Selection(usize),
}

/// One axis of a critically damped spring, relative to its target, in px and px/s.
#[derive(Clone, Copy, Default, Debug)]
struct Spring {
    offset: f32,
    velocity: f32,
}

impl Spring {
    /// Exact solution of x'' + 2ωx' + ω²x = 0, independent of frame subdivision.
    fn sample(self, seconds: f32) -> Self {
        let carry = self.velocity + MOTION_OMEGA * self.offset;
        let decay = (-MOTION_OMEGA * seconds).exp();
        Self {
            offset: (self.offset + carry * seconds) * decay,
            velocity: (self.velocity - MOTION_OMEGA * carry * seconds) * decay,
        }
    }

    fn settled(self) -> bool {
        self.offset.abs() < SETTLED && self.velocity.abs() < SETTLED_SPEED
    }
}

/// Each part is drawn at its layout plus the spring's remaining displacement
/// and the held trigger's direct pull between cells.
#[derive(Default)]
struct Motion {
    rest: HashMap<Part, [Spring; 2]>,
    /// How far the hand pulled each part in the last frame, in pixels.
    hand: HashMap<Part, (f32, f32)>,
    /// What the hand was counted from then (`input::Hand::counted_from`).
    counted_from: Option<u64>,
    last: Option<Instant>,
    /// Frames drawn and the time spent drawing them, since it last stood still.
    frames: u32,
    drawing: Duration,
    slowest: Duration,
}

impl Motion {
    /// Where a part is drawn, relative to where it is laid out.
    fn offset(&self, part: &Part) -> (f32, f32) {
        let (rest, hand) = (self.rest.get(part).copied().unwrap_or_default(), self.hand.get(part).copied().unwrap_or_default());
        (rest[0].offset + hand.0, rest[1].offset + hand.1)
    }

    fn is_moving(&self) -> bool {
        !self.rest.is_empty() || !self.hand.is_empty()
    }

    /// The parts were laid out at `before` and are now at `after`: each is
    /// still drawn where it was and eases from there. A part that came or
    /// went has nothing to ease from.
    fn moved(&mut self, before: &HashMap<Part, (f32, f32)>, after: &HashMap<Part, (f32, f32)>, now: Instant) {
        self.elapse(now);
        let rest = after.iter().filter_map(|(part, at)| {
            let mut left = self.rest.get(part).copied().unwrap_or_default();
            let jump = before.get(part).map_or((0.0, 0.0), |was| (was.0 - at.0, was.1 - at.1));
            left[0].offset += jump.0;
            left[1].offset += jump.1;
            (!left.iter().all(|spring| spring.settled())).then(|| (part.clone(), left))
        });
        self.rest = rest.collect();
    }

    fn elapse(&mut self, now: Instant) {
        let step = self.last.map_or(0.0, |last| now.saturating_duration_since(last).as_secs_f32());
        self.last = Some(now);
        self.rest.retain(|_, rest| {
            *rest = rest.map(|spring| spring.sample(step));
            !rest.iter().all(|spring| spring.settled())
        });
    }

    /// One frame at `now`, with the hand pulling the parts by `hand`. When
    /// what the hand is counted from changed (it came to a cell, or let go),
    /// its jump is eased away like any other, so nothing jumps. Returns
    /// whether anything is still moving.
    fn advance(&mut self, hand: HashMap<Part, (f32, f32)>, counted_from: u64, now: Instant) -> bool {
        self.elapse(now);
        if self.counted_from.is_some_and(|from| from != counted_from) {
            let parts: std::collections::HashSet<Part> = self.hand.keys().chain(hand.keys()).cloned().collect();
            for part in parts {
                let (was, is) = (self.hand.get(&part).copied().unwrap_or_default(), hand.get(&part).copied().unwrap_or_default());
                let rest = self.rest.entry(part).or_default();
                rest[0].offset += was.0 - is.0;
                rest[1].offset += was.1 - is.1;
            }
        }
        self.rest.retain(|_, rest| !rest.iter().all(|spring| spring.settled()));
        (self.hand, self.counted_from) = (hand, Some(counted_from));
        self.is_moving()
    }

    fn lock_axis(&mut self, axis: usize) {
        for rest in self.rest.values_mut() { rest[1 - axis] = Spring::default(); }
        for hand in self.hand.values_mut() { if axis == 0 { hand.1 = 0.0; } else { hand.0 = 0.0; } }
    }

    fn stop(&mut self) {
        self.rest.clear();
        self.hand.clear();
        self.counted_from = None;
        self.last = None;
    }
}

/// A hold over the board the screen follows (`input::FOLLOWING`), its hand
/// read as the normal view reads it (`follow::Follow::take`): the latches
/// counted from the cell it began on as a picture of a hold counts them
/// (`Grid::counted`, in `grid` as it was then), and the flick judge
/// (`quick::Judge`). Not yet unsure, the selection stays on the cell it
/// began on; unsure, it is the latches' cell, easing there as any move
/// does. Let go of, a flick goes exactly one cell its way (`Board::let_go`).
struct Hold {
    id: u32,
    began: Instant,
    start: CellId,
    from: Pos,
    grid: Grid,
    follow: crate::follow::Follow,
    judge: Option<crate::quick::Judge>,
    /// Where the hand is (`input::Followed`), and when it was let go of.
    hand: (f64, f64),
    released: Option<f64>,
    /// The cell shown, as the latches count it, and the rows shown turned
    /// on the way there (`Follow::turns`).
    shown: (i32, i32),
    turns: Vec<(i32, i32)>,
}

impl Hold {
    fn still(&self) -> bool { self.judge.as_ref().is_some_and(|judge| judge.mind != crate::quick::Mind::Unsure) }
}

/// How far the hand pulls the selected row sideways and the selection up or
/// down, in cells and rows. Unavailable directions never pull the board.
fn hand_pull(grid: &Grid, at: Pos, hand: (f32, f32)) -> (f32, f32) {
    let pull = |by: f32, negative, positive| {
        let dir = if by < 0.0 { negative } else { positive };
        if grid.target(at, dir).and_then(|to| grid.id_at(to)).is_some() { by } else { 0.0 }
    };
    (pull(hand.0, Dir::Left, Dir::Right), pull(hand.1, Dir::Up, Dir::Down))
}

/// The map of all rows and cells, in the coordinates of the monitor it is on.
#[derive(Default)]
struct MapLayout {
    top: f32,
    /// All of it, the + buttons included.
    bounds: Rect,
    /// The column on which every row's landing cell is lined up.
    spine: Rect,
    rows: Vec<RowLayout>,
    /// The "add a row" buttons below and above the rows.
    add_row: Rect,
    add_row_top: Rect,
    /// From one cell to the next, and from one row to the next.
    pitch: f32,
    row_pitch: f32,
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
    /// The selected shelf's name, at the top.
    label: IDWriteTextFormat,
    small: IDWriteTextFormat,
    centered: IDWriteTextFormat,
    /// How many windows a cell of icons stands for.
    tiny: IDWriteTextFormat,
    /// The word in a desktop's picture when it has no windows of its own.
    empty: IDWriteTextFormat,
}

#[derive(Clone, Copy)]
struct ThumbnailSource { full: (f32, f32), frame: Rect }

pub struct Board {
    hwnd: HWND,
    /// A cell of the map pointed at, shown in the preview area in place of
    /// the selected one without selecting it; how far back it is drawn
    /// (`PEEK_SCALE`, from 0 to 1) in the frame being drawn, and where that
    /// eases from and to, since when.
    peek: Option<CellId>,
    peek_shown: f32,
    peek_glide: (f32, f32, Instant),
    /// What the line at the bottom says is shown (the first time, or ?).
    hints: bool,
    /// The preview the cursor rests on, and since when.
    resting: Option<(isize, Instant)>,
    glows: Option<Glows>,
    /// The map and the tiles easing into place after the selection moved,
    /// and following the held trigger; drawn by `MOTION_TIMER_ID` meanwhile.
    motion: Motion,
    motion_timer: bool,
    /// Brings the frames of the motion at each vblank.
    clock: FrameClock,
    /// When the last frame brought by a vblank was drawn.
    vblank_frame: Option<Instant>,
    /// The preview area as a strip of desktops while the selection moves
    /// (`strip`), in seconds from `strip_epoch`; the time of the frame
    /// being drawn; whether it is moving, with the cells beside the
    /// selection laid out and their live previews registered.
    strip: Strip,
    /// A hold over the board the screen follows, while it goes on: see
    /// `Hold`.
    hold: Option<Hold>,
    #[cfg(feature = "motion-probe")]
    probe_marker: Cell<Option<f32>>,
    strip_frame: Option<Strip>,
    strip_input: Option<(crate::input::Hand, bool)>,
    strip_samples: Vec<(f64,f64,f64,usize)>,
    strip_epoch: Instant,
    strip_at: f32,
    strip_on: bool,
    strip_cells: Vec<StripCell>,
    /// (cell, source window, thumbnail handle) of the cells beside.
    strip_thumbs: Vec<(CellId, isize, isize)>,
    /// Where each row's column 0 is from the selection (`strip::marks`).
    strip_marks: Vec<(f32, f32)>,
    /// Per monitor (as `monitors`), where the tiles go, and the plate the
    /// strip moves in: that, but for the part of the windows on every
    /// desktop.
    areas: Vec<Rect>,
    plates: Vec<Rect>,
    /// How large each thumbnail's window is, asked of DWM once while moving.
    source_sizes: RefCell<HashMap<isize, Option<ThumbnailSource>>>,
    trace: Option<Trace>,
    factory: ID2D1Factory,
    canvas: Option<Canvas>,
    window_target: Option<ID2D1HwndRenderTarget>,
    window_failed: bool,
    text_cache: paint::TextCache,
    paint_origin: std::cell::Cell<(f32, f32)>,
    thumbnail_state: RefCell<HashMap<isize, (RECT, RECT, u8, bool, u32)>>,
    thumbnail_updates: Cell<usize>,
    fonts: Option<Fonts>,
    open: bool,
    model: Model,
    /// The app's grid as last supplied, for laying out and labelling cells.
    grid: Grid,
    selected: CellId,
    /// The selected cell's windows, the main content of the board, in the
    /// board window's coordinates.
    tiles: Vec<TileLayout>,
    /// Where the selected cell is named, in the home monitor's coordinates.
    label: Rect,
    /// The map on every monitor, the home monitor's first. Never empty.
    maps: Vec<Map>,
    size: (i32, i32),
    scale: f32,
    /// (source window, thumbnail handle) of every tile, in stacking order.
    thumbs: Vec<(isize, isize)>,
    icons: HashMap<isize, Option<ID2D1Bitmap>>,
    /// The icons again, for thumbnails over the previews (`sheet`).
    sheet: Option<sheet::IconSheet>,
    /// The thumbnail of a preview's icon by the preview's own thumbnail.
    badges: HashMap<isize, isize>,
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
    /// The parts the tiles are placed in: the places of the pinned windows
    /// and the rest.
    sections: Vec<Section>,
    /// Per monitor (as `monitors`), the places of the windows pinned to
    /// every desktop and to the row, where it has them.
    columns: Vec<Option<(Rect, Rect)>>,
    /// Shell windows put out of the way while the board is open
    /// (`keep_on_top`), to be given back when it closes.
    pushed_aside: Vec<HWND>,
}

/// The parts of `area` the tiles go into: all in one, or with the column
/// of the places of the pinned windows at the left (`tiles::pin_column`),
/// of one size whatever is in them, where they stay whichever cell is
/// looked at.
fn split_area<'a>(
    here: &[(&'a WindowModel, bool)],
    column: bool,
    area: &Rect,
    s: f32,
) -> Vec<(Rect, Carried, Vec<(&'a WindowModel, bool)>)> {
    if !column {
        return vec![(*area, Carried::Own, here.to_vec())];
    }
    let pick = |carried: Carried| -> Vec<(&'a WindowModel, bool)> { here.iter().filter(|(w, e)| Carried::of(w, *e) == carried).copied().collect() };
    let parts = tiles::pin_column(size(area), 28.0 * s, (210.0 * s, 400.0 * s));
    let place = |(x, y, w, h): tiles::Part| rect(area.left + x, area.top + y, w, h);
    vec![
        (place(parts.all), Carried::Fixed, pick(Carried::Fixed)),
        (place(parts.row), Carried::Row, pick(Carried::Row)),
        (place(parts.rest), Carried::Own, pick(Carried::Own)),
    ]
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
            Ok(Board {
                hwnd,
                peek: None,
                peek_shown: 0.0,
                peek_glide: (0.0, 0.0, Instant::now()),
                hints: false,
                resting: None,
                glows: None,
                motion: Motion::default(),
                motion_timer: false,
                clock: FrameClock::default(),
                vblank_frame: None,
                strip: Strip::default(),
                hold: None,
                #[cfg(feature = "motion-probe")]
                probe_marker: Cell::new(None),
                strip_frame: None,
                strip_input: None,
                strip_samples: Vec::new(),
                strip_epoch: Instant::now(),
                strip_at: 0.0,
                strip_on: false,
                strip_cells: Vec::new(),
                strip_thumbs: Vec::new(),
                strip_marks: Vec::new(),
                areas: Vec::new(),
                plates: Vec::new(),
                source_sizes: RefCell::new(HashMap::new()),
                trace: Trace::from_env(),
                factory: paint::d2d_factory()?,
                canvas: None,
                window_target: None,
                window_failed: false,
                text_cache: paint::TextCache::new()?,
                paint_origin: std::cell::Cell::new((0.0, 0.0)),
                thumbnail_state: RefCell::default(),
                thumbnail_updates: Cell::new(0),
                fonts: None,
                open: false,
                model: Model { rows: Vec::new(), pinned: Vec::new(), current: String::new() },
                grid: Grid::default(),
                selected: String::new(),
                tiles: Vec::new(),
                label: Rect::default(),
                maps: vec![Map::default()],
                size: (0, 0),
                scale: 1.0,
                thumbs: Vec::new(),
                icons: HashMap::new(),
                sheet: sheet::IconSheet::new().map_err(|e| crate::app::log(&format!("board icon sheet failed: {e}"))).ok(),
                badges: HashMap::new(),
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
                columns: Vec::new(),
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
        let cold = self.window_target.is_none();
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
            self.text_cache.clear();
        }
        self.scale = home.scale;
        self.size = (right - left, bottom - top);
        self.origin = (left, top);
        self.selected = model.current.clone();
        self.hover = Hit::Nothing;
        self.press = None;
        self.end_peek();
        self.hints = !grid.board_hints_seen;
        self.resting = None;
        self.motion.stop();
        self.settle_strip(Instant::now());
        self.clock.refresh();
        self.refused = None;
        self.stop_editing();
        self.open = true;
        self.window_failed = false;
        crate::input::set_board_open(true);
        unsafe {
            let _ = SetWindowPos(self.hwnd, HWND_TOPMOST, left, top, self.size.0, self.size.1, SWP_NOACTIVATE);
            SetTimer(self.hwnd, ON_TOP_TIMER_ID, ON_TOP_MS, None);
        }
        self.set_model(model, grid);
        if cold {
            // Compile transformed/clipped drawing paths before the first gesture.
            let row = self.grid.find(&self.selected).map_or(0, |at| at.row);
            self.strip.follow((0.1, 0.0), row, 0, true, 0.0);
            self.strip.follow((0.1, 0.0), row, 0, true, 0.1);
            self.strip_at = 0.1;
            self.strip_on = true;
            self.lay_out_strip(None);
            self.paint();
            self.settle_strip(Instant::now());
        }
        // Warm drawing resources and the clock before the first held movement.
        self.paint();
        unsafe { ShowWindow(self.hwnd, SW_SHOWNOACTIVATE); }
        self.start_motion();
    }

    /// Replaces the content, keeping the selection when its cell still exists.
    pub fn set_model(&mut self, model: Model, mut grid: Grid) {
        if !self.open {
            return;
        }
        self.model = model;
        self.source_sizes.borrow_mut().clear();
        // A hold going on is counted in the grid as it is now, and shows
        // the rows it turned as it did.
        let mut lost = false;
        if let Some(hold) = &mut self.hold {
            match grid.find(&hold.start) {
                Some(from) => {
                    (hold.grid, hold.from) = (grid.clone(), from);
                    grid.keep_turns(from, &hold.turns);
                }
                None => lost = true,
            }
        }
        if lost {
            self.hold = None;
        }
        self.grid = grid;
        if self.grid.find(&self.selected).is_none() {
            self.selected = self.model.current.clone();
        }
        self.press = None;
        self.end_peek();
        // What changed is not a move: it is shown as it is. A strip that
        // moves goes on (a hold may be under way), with its cells anew.
        self.motion.stop();
        self.relayout();
        if self.strip_on {
            self.lay_out_strip(None);
        }
        self.hover_under_cursor();
        self.register_thumbnails();
        self.update_peek(Instant::now());
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
        self.hold = None;
        crate::input::set_board_open(false);
        self.end_peek();
        self.stop_motion();
        self.settle_strip(Instant::now());
        self.clear_thumbnails();
        self.icons.clear();
        if let Some(sheet) = &mut self.sheet {
            sheet.clear();
        }
        // Keep the D2D target for repeated trigger presses; ensure_canvas handles resizing.
        self.press = None;
        self.stop_motion();
        unsafe {
            let _ = ReleaseCapture();
            let _ = KillTimer(self.hwnd, ON_TOP_TIMER_ID);
            ShowWindow(self.hwnd, SW_HIDE);
            for window in self.pushed_aside.drain(..) {
                crate::vd::put_notice_away(window);
            }
        }
    }

    /// Shows a selection whose row anchors the app has already saved. What
    /// moved is drawn where it was and eases into place, as in the minimap:
    /// the selection stays on the spine and its row slides under it, or the
    /// selection goes along the spine to the next row. Tiles of windows
    /// shown before and after (on every desktop, or in the row while the
    /// row stays) stay; those of the cell gone to come in from its side.
    pub fn select(&mut self, id: CellId, grid: Grid) {
        self.show_selected(id, grid);
    }

    /// Hold `id`, which the screen follows, began over the open board: its
    /// hand is read from now on as the normal view reads it (`Hold`), the
    /// latches given the rows and cells a picture of it would have.
    pub fn hold(&mut self, id: u32) {
        if !self.open || self.hold.as_ref().is_some_and(|hold| hold.id == id) {
            return;
        }
        let Some(saved) = crate::input::follow_hold(id).filter(|saved| saved.hand.hold == id) else { return };
        let Some(from) = self.grid.find(&self.selected) else { return };
        let (rows, cols) = self.grid.reach(from, true, true);
        self.hold = Some(Hold {
            id,
            began: saved.started.unwrap_or_else(Instant::now),
            start: self.selected.clone(),
            from,
            grid: self.grid.clone(),
            follow: crate::follow::Follow::new(rows, cols),
            judge: (crate::quick::mode() != crate::config::QuickFlick::Off).then(crate::quick::Judge::default),
            hand: (0.0, 0.0),
            released: None,
            shown: (0, 0),
            turns: Vec::new(),
        });
        self.follow_hand();
    }

    /// Whether hold `id` over the board has gone to another cell by now:
    /// let go of, it is a move, never a click.
    pub fn moved(&mut self, id: u32) -> bool {
        self.take_hold(Instant::now());
        self.hold.as_ref().is_some_and(|hold| hold.id == id && hold.follow.selected() != (0, 0))
    }

    /// The hold over the board let go of (`Hold`), landing as the normal
    /// view's does (`Follow::land`): the cell it began on, the cell it
    /// lands on and the rows it turned, as `Grid::land` takes them. `None`
    /// when no hold the screen follows went on.
    pub fn let_go(&mut self) -> Option<(CellId, (i32, i32), Vec<(i32, i32)>)> {
        self.take_hold(Instant::now());
        let mut hold = self.hold.take()?;
        let at = hold.released.unwrap_or_else(|| hold.began.elapsed().as_secs_f64());
        let (to, _) = hold.follow.land(hold.judge.as_mut(), at, hold.hand);
        Some((hold.start, to, hold.follow.turns()))
    }

    /// The hold over the board (`Hold`) brought up to `now`: the hand it
    /// went meanwhile taken in, and the cell it is on shown, eased to as
    /// any move is (`show_selected`). Returns the hand as the board follows
    /// it (how far it leans from the cell shown, as `input::Hand` has it),
    /// whether it is still held, and its axis; `None` without one.
    fn take_hold(&mut self, now: Instant) -> Option<(crate::input::Hand, bool, Option<usize>)> {
        let hold = self.hold.as_mut()?;
        let t = now.saturating_duration_since(hold.began).as_secs_f64();
        if hold.released.is_none() {
            if let Some(saved) = crate::input::follow_frame(hold.id) {
                let (_, released) = hold.follow.take(hold.judge.as_mut(), &saved, hold.began, t, |_, _| {});
                hold.hand = (saved.hand.x, saved.hand.y);
                hold.released = released.or_else(|| (!saved.hand.holding).then_some(t));
            }
        }
        let at = hold.released.unwrap_or(t);
        if let Some(judge) = &mut hold.judge { judge.check(at); }
        let holding = hold.released.is_none();
        // Not yet unsure, on the cell it began on, the board still.
        let (cell, lean) = if hold.still() {
            ((0, 0), (0.0, 0.0))
        } else {
            let (cell, aim) = (hold.follow.selected(), hold.follow.aim());
            (cell, (aim.0 - cell.0 as f64, aim.1 - cell.1 as f64))
        };
        let lean = if holding { lean } else { (0.0, 0.0) };
        let (id, axis) = (hold.id, hold.follow.axis());
        if cell != hold.shown {
            let turns = if hold.still() { Vec::new() } else { hold.follow.turns() };
            let to = hold.grid.counted(hold.from, cell).and_then(|at| hold.grid.id_at(at)).cloned();
            if let Some(to) = to {
                let mut grid = hold.grid.clone();
                grid.keep_turns(hold.from, &turns);
                (hold.shown, hold.turns) = (cell, turns);
                self.show_selected(to, grid);
            }
        }
        // Changes with the cell shown and with letting go, as `input::Hand`'s does.
        let counted_from = (id as u64) << 33 | (holding as u64) << 32 | (cell.1 as i16 as u16 as u64) << 16 | cell.0 as i16 as u16 as u64;
        Some((crate::input::Hand { along: lean.0 as f32, across: lean.1 as f32, counted_from }, holding, Some(axis)))
    }

    fn show_selected(&mut self, id: CellId, grid: Grid) {
        let now = Instant::now();
        let before = self.marks();
        let (strip_before, from) = (self.strip_marks.clone(), self.grid.find(&self.selected));
        self.grid = grid;
        self.selected = id;
        self.end_peek();
        self.relayout();
        self.motion.moved(&before, &self.marks(), now);
        let t = self.strip_time(now);
        self.strip_frame = None;
        self.strip.moved(&strip_before, &self.strip_marks, t);
        self.strip_at = t;
        self.strip_on |= self.strip.active(t);
        if self.strip_on {
            self.lay_out_strip(from);
        }
        self.hover_under_cursor();
        self.register_thumbnails();
        if self.motion.is_moving() || self.strip_on {
            // The preview comes up over the cell under the cursor once it
            // is in place; the frames draw it meanwhile, at each vblank.
            self.start_motion();
        } else {
            self.update_peek(now);
            self.invalidate();
        }
    }

    /// Seconds into the strip's motion.
    fn strip_time(&self, now: Instant) -> f32 {
        now.saturating_duration_since(self.strip_epoch).as_secs_f32()
    }

    /// Lays out the cells beside the selection, and those beside `from`
    /// where it moved from.
    fn lay_out_strip(&mut self, from: Option<Pos>) {
        let Some(to) = self.grid.find(&self.selected) else {
            self.strip_cells.clear();
            return;
        };
        self.strip_cells = strip::shown(&self.grid, from, to).into_iter()
            .map(|(id, pos)| StripCell { cell: self.lay_out_cell(&id), id, pos })
            .collect();
    }

    /// The strip is still: the cells beside go, with their live previews,
    /// and its clock starts again from `now`.
    fn settle_strip(&mut self, now: Instant) {
        self.strip = Strip::default();
        self.strip_frame = None;
        self.strip_input = None;
        self.strip_samples.clear();
        self.strip_epoch = now;
        self.strip_at = 0.0;
        self.strip_on = false;
        self.strip_cells.clear();
        unregister_strip_thumbnails(&mut self.strip_thumbs);
        self.source_sizes.borrow_mut().clear();
        self.thumbnail_state.borrow_mut().clear();
    }

    /// How far the hand pulls the selection, in cells and rows.
    fn drawn_strip(&self) -> &Strip { self.strip_frame.as_ref().unwrap_or(&self.strip) }

    fn strip_pull(&self, hand: (f32, f32)) -> (f32, f32) {
        let Some(at) = self.grid.find(&self.selected).filter(|_| hand != (0.0, 0.0)) else { return (0.0, 0.0) };
        hand_pull(&self.grid, at, hand)
    }

    /// The held trigger began a hold over the open board: its rows follow
    /// the hand from now on, as the minimap's do.
    pub fn follow_hand(&mut self) {
        if self.open {
            if self.end_peek() {
                self.relayout();
                self.register_thumbnails();
            }
            self.start_motion();
        }
    }

    fn start_motion(&mut self) {
        if !self.motion_timer {
            unsafe {
                SetTimer(self.hwnd, MOTION_TIMER_ID, MOTION_MS, None);
            }
            self.motion_timer = true;
            self.clock.start(self.hwnd);
        }
    }

    fn stop_motion(&mut self) {
        self.motion.stop();
        self.clock.stop();
        self.vblank_frame = None;
        if let Some(trace) = &mut self.trace {
            trace.flush();
        }
        if self.motion_timer {
            unsafe {
                let _ = KillTimer(self.hwnd, MOTION_TIMER_ID);
            }
            self.motion_timer = false;
        }
    }

    /// Where each part that moves is laid out: its top left corner.
    fn marks(&self) -> HashMap<Part, (f32, f32)> {
        let corner = |r: &Rect| (r.left, r.top);
        let mut marks = HashMap::new();
        for map in &self.maps {
            let m = map.monitor;
            marks.insert(Part::Spine(m), corner(&map.layout.spine));
            if let Some(row) = map.layout.rows.first() {
                marks.insert(Part::Names(m), corner(&row.header));
            }
            for row in &map.layout.rows {
                if let Some(first) = row.cells.first() {
                    marks.insert(Part::Row(m, first.id.clone()), corner(&first.body));
                }
                if let Some(cell) = row.cells.iter().find(|cell| cell.id == self.selected) {
                    marks.insert(Part::Selection(m), corner(&cell.body));
                }
            }
        }
        marks
    }

    /// How far the hand pulls the selected row and the selection, in pixels.
    fn hand_parts(&self, hand: (f32, f32)) -> HashMap<Part, (f32, f32)> {
        let mut parts = HashMap::new();
        let Some(at) = self.grid.find(&self.selected).filter(|_| hand != (0.0, 0.0)) else { return parts };
        for map in &self.maps {
            let Some(row) = map.layout.rows.get(at.row) else { continue };
            let Some(first) = row.cells.first() else { continue };
            let (x, y) = hand_pull(&self.grid, at, hand);
            if x != 0.0 {
                parts.insert(Part::Row(map.monitor, first.id.clone()), (-x * map.layout.pitch, 0.0));
            }
            if y != 0.0 {
                parts.insert(Part::Selection(map.monitor), (0.0, y * map.layout.row_pitch));
            }
        }
        parts
    }

    /// One frame of the motion at `now`, brought by a vblank or by the
    /// timer standing in for one: follows the hand, eases what jumped,
    /// moves the strip, and draws. Where everything is comes from the time,
    /// never from how many frames there were. Stops once all is in place
    /// and no hold goes on.
    fn tick_motion(&mut self, now: Instant, by_vblank: bool) {
        let (hand, holding, axis, samples) = crate::input::board_frame();
        let now = samples.last().map_or(now, |s| now.max(s.at));
        // A hold the screen follows is read as the normal view reads it
        // (`Hold`), not from where the app last answered.
        let (hand, holding, axis, samples) = match self.take_hold(now) {
            Some((hand, holding, axis)) => (hand, holding, axis, Vec::new()),
            None => (hand, holding, axis, samples),
        };
        let dragging = self.press.as_ref().is_some_and(|press| press.dragging);
        let t = self.strip_time(now);
        let row = self.grid.find(&self.selected).map_or(0, |at| at.row);
        let was_strip = self.strip_on;
        self.advance_peek(now);
        let changed_origin = self.strip_input.is_none_or(|(last, _)| last.counted_from != hand.counted_from);
        if changed_origin { self.strip_samples.clear(); }
        for sample in &samples {
            let at = self.strip_time(sample.at);
            let h = sample.hand;
            self.strip.follow(self.strip_pull((h.along, h.across)), row, h.counted_from, sample.followed.holding, at);
            if self.strip_samples.len() == 3 { self.strip_samples.remove(0); }
            self.strip_samples.push((at as f64,h.along as f64,h.across as f64,sample.axis));
        }
        if samples.is_empty() && self.strip_input != Some((hand, holding)) {
            self.strip.follow(self.strip_pull((hand.along, hand.across)), row, hand.counted_from, holding, t);
        }
        self.strip_input = Some((hand, holding));
        self.strip.advance_depth(t);
        let mut frame = self.strip.clone();
        if holding && !changed_origin {
            if let Some(((x,y),at)) = crate::follow::resample(&self.strip_samples,t as f64) {
                // Keep prediction inside this cell; only real input may cross a commit.
                if x.floor() == (hand.along as f64).floor() && y.floor() == (hand.across as f64).floor() {
                    frame.follow(self.strip_pull((x as f32,y as f32)),row,hand.counted_from,true,at as f32);
                }
            }
        }
        self.strip_frame = Some(frame);

        self.strip_at = t;
        let strip_moving = self.strip.active(t);
        if strip_moving && !self.strip_on {
            self.strip_on = true;
            self.lay_out_strip(None);
            self.register_thumbnails();
        }
        let moving = self.motion.advance(self.hand_parts((hand.along, hand.across)), hand.counted_from, now) || strip_moving
            || self.peek_shown != self.peek_glide.1;
        if holding { if let Some(axis) = axis { self.motion.lock_axis(axis); } }
        {
            // Also submit the final frame after a drag or hold stops.
            let started = Instant::now();
            let rendered = self.draw_to_canvas();
            let prepared = started.elapsed();
            match rendered {
                Some(Ok(())) => self.present_canvas(),
                Some(Err(e)) => { crate::app::log(&format!("board draw failed: {e}")); self.discard_target(); }
                None => {}
            }
            let submitted = started.elapsed();
            self.thumbnail_updates.set(0);
            let geometry = self.update_thumbnails();
            if let Err(e) = crate::composition::commit(self.hwnd) { crate::app::log(&format!("board composition commit failed: {e}")); }
            let dwm = started.elapsed() - submitted;
            unsafe {
                ValidateRect(self.hwnd, None);
            }
            let took = started.elapsed();
            self.motion.frames += 1;
            self.motion.drawing += took;
            self.motion.slowest = self.motion.slowest.max(took);
            if self.trace.is_some() {
                let drawn = self.selected_place(self.home_index()).map_or((0.0, 0.0, 1.0), |(place, plate)| {
                    let r = place.rect(&plate);
                    (r.left, r.top, place.s)
                });
                let frame = TraceFrame {
                    at: now.saturating_duration_since(trace_epoch()),
                    interval: None,
                    by_vblank,
                    hand: (hand.along, hand.across),
                    drawn,
                    depth: self.drawn_strip().depth(t),
                    draw: took - dwm,
                    dwm,
                    submit: took - prepared,
                    thumbs: self.thumbs.len() + self.strip_thumbs.len(),
                    updates: self.thumbnail_updates.get(),
                    geometry,
                    submitted_qpc_ms: crate::glide::qpc_ms(),
                };
                if let Some(trace) = &mut self.trace {
                    trace.record(frame, now);
                }
            }
            debug_log!("board: motion frame {} drawn in {:.2} ms, {} parts easing, at most {:.1} px left, hand {:.3},{:.3} {:?}, strip {:.3} back",
                self.motion.frames, took.as_secs_f32() * 1000.0, self.motion.rest.len(),
                self.motion.rest.values().flatten().map(|spring| spring.offset.abs()).fold(0.0, f32::max), hand.along, hand.across,
                self.motion.hand.values().next(), self.drawn_strip().depth(t));
        }
        if !moving && !holding && !dragging {
            debug_log!("board: motion settled after {} frames, drawing {:.2} ms a frame on average and {:.2} at most",
                self.motion.frames, self.motion.drawing.as_secs_f32() * 1000.0 / self.motion.frames.max(1) as f32,
                self.motion.slowest.as_secs_f32() * 1000.0);
            (self.motion.frames, self.motion.drawing, self.motion.slowest) = (0, Duration::ZERO, Duration::ZERO);
            self.stop_motion();
            // Clear any blur trail of the last moving frame.
            if crate::composition::trails_shown() { self.update_thumbnails(); }
            if was_strip {
                self.settle_strip(now);
            }
            self.hover_under_cursor();
            self.update_peek(now);
            self.invalidate();
        }
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
        self.model.tile_windows(self.shown())
    }

    /// The cell whose windows are shown: the one pointed at in the map,
    /// or the selected one.
    fn shown(&self) -> &CellId {
        self.peek.as_ref().unwrap_or(&self.selected)
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
        let anchors = map_anchors(&self.grid, &self.selected);
        let mut maps = Vec::new();
        let mut areas = Vec::new();
        for (m, monitor) in self.monitors.iter().enumerate() {
            let is_home = *monitor == self.home;
            let (w, h) = size(monitor);
            let layout = layout_map(&self.model, &anchors, (w, h - FOOTER * s), s);
            // The home monitor has the shelf's name above its tiles.
            areas.push(Rect {
                left: monitor.left + margin,
                top: monitor.top + if is_home { 62.0 * s } else { margin },
                right: monitor.right - margin,
                bottom: monitor.top + layout.top - 26.0 * s,
            });
            maps.push(Map { monitor: m, area: *monitor, layout });
        }
        maps.sort_by_key(|map| map.area != self.home);
        self.plates = areas.clone();
        self.areas = areas;
        let shown = self.shown().clone();
        let CellTiles { tiles, sections } = self.lay_out_cell(&shown);
        // The strip moves in what is left of the area beside the places of
        // the pinned windows: those of every desktop never move, and those
        // of the row move in their own place.
        self.columns = vec![None; self.monitors.len()];
        for (m, plate) in self.plates.iter_mut().enumerate() {
            let monitor = self.monitors[m];
            let on_m = |section: &&Section| contains(&monitor, centre(&section.part).0, centre(&section.part).1);
            let place = |carried| sections.iter().filter(on_m).find(|section| section.carried == carried).map(|section| section.part);
            if let (Some(all), Some(row)) = (place(Carried::Fixed), place(Carried::Row)) {
                plate.left = plate.left.max(all.right.max(row.right) + 28.0 * s);
                self.columns[m] = Some((all, row));
            }
        }
        self.strip_marks = self.grid.find(&self.selected).map_or_else(Vec::new, |at| strip::marks(&self.grid, at));
        self.label = rect(margin, 18.0 * s, size(&self.home).0 - 2.0 * margin, 30.0 * s);
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
        self.maps = maps;
    }

    /// The windows of cell `id` laid out as when it is selected, in the
    /// areas `relayout` found for each monitor. The home monitor always has
    /// the places of the pinned windows, to drop windows into; the others
    /// when windows are pinned there.
    fn lay_out_cell(&self, id: &str) -> CellTiles {
        let s = self.scale;
        let windows = self.model.tile_windows(id);
        let weights = self.model.window_weights(id, &self.grid);
        let mut out = CellTiles::default();
        for (m, monitor) in self.monitors.iter().enumerate() {
            let Some(area) = self.areas.get(m) else { continue };
            let here: Vec<(&WindowModel, bool)> = windows.iter().filter(|(w, _)| self.monitor_of(w) == m).copied().collect();
            let pinned_here = self.model.pinned.iter().any(|w| self.monitor_of(w) == m)
                || self.model.rows.iter().flat_map(|row| &row.cells).flat_map(|cell| &cell.windows).any(|w| w.follows && self.monitor_of(w) == m);
            for (part, carried, group) in split_area(&here, *monitor == self.home || pinned_here, area, s) {
                let (tiles, more) = match carried {
                    Carried::Own => (layout_tiles(&group, &part, s, &weights), None),
                    _ => layout_slot(&group, &part, s),
                };
                out.tiles.extend(tiles);
                out.sections.push(Section { part, carried, more });
            }
        }
        out
    }

    /// The monitor a thing laid out at `r` is on, as `monitors` counts them.
    fn monitor_at(&self, r: &Rect) -> usize {
        let (x, y) = centre(r);
        self.monitors.iter().position(|m| contains(m, x, y)).unwrap_or_else(|| self.home_index())
    }

    /// How the picture of the cell at `pos` is drawn on monitor `m` in
    /// the frame being drawn.
    fn picture_place(&self, pos: Pos, m: usize) -> Place {
        let Some(plate) = self.plates.get(m) else { return Place::SAME };
        if !self.strip_on {
            return self.peek_place(plate);
        }
        let Some(mark) = self.strip_marks.get(pos.row) else { return Place::SAME };
        let t = self.strip_at;
        let (x, y) = self.drawn_strip().offset(pos.row, t);
        let size = strip::scale(self.drawn_strip().depth(t));
        strip::place(plate, (mark.0 + pos.col as f32 + x, mark.1 + y), size, size)
    }

    fn picture_shade(&self, pos: Pos) -> f32 {
        if !self.strip_on {
            return self.peek_shade();
        }
        let Some(mark) = self.strip_marks.get(pos.row) else { return 1.0 };
        let (x, y) = self.drawn_strip().offset(pos.row, self.strip_at);
        strip::shade((mark.0 + pos.col as f32 + x).hypot(mark.1 + y))
    }

    /// How the windows pinned to row `row` are drawn on monitor `m`: in
    /// their own place, still while the selection moves in the selected row,
    /// going back and up or down with it to another.
    fn row_place(&self, row: usize, m: usize) -> Place {
        let zone = self.columns.get(m).copied().flatten().map(|(_, row)| row);
        let Some(plate) = zone.or(self.plates.get(m).copied()) else { return Place::SAME };
        if !self.strip_on {
            return self.peek_place(&plate);
        }
        let Some(mark) = self.strip_marks.get(row) else { return Place::SAME };
        let t = self.strip_at;
        let spacing = strip::scale(self.drawn_strip().depth(t));
        let in_selected = self.grid.find(&self.selected).is_some_and(|at| at.row == row);
        let size = strip::scale(if in_selected { self.drawn_strip().row_depth(t) } else { self.drawn_strip().depth(t) });
        strip::place(&plate, (0.0, mark.1 + self.drawn_strip().offset(row, t).1), size, spacing)
    }

    /// How a cell's own windows, or those pinned to its row, are drawn
    /// while a cell of the map is pointed at: a little back.
    fn peek_place(&self, plate: &Rect) -> Place {
        if self.peek_shown <= 0.0 {
            return Place::SAME;
        }
        strip::place(plate, (0.0, 0.0), 1.0 - (1.0 - PEEK_SCALE) * self.peek_shown, 1.0)
    }

    fn peek_shade(&self) -> f32 {
        1.0 - (1.0 - PEEK_SHADE) * self.peek_shown
    }

    /// How a tile of the cell at `pos` that moves as `carried` is drawn,
    /// and the plate it is cut off at; `None` when it is not shown there.
    fn tile_place(&self, pos: Pos, carried: Carried, tile: &Rect) -> Option<(Place, Option<Rect>)> {
        let m = self.monitor_at(tile);
        let selected = self.grid.find(self.shown());
        let middle = selected == Some(pos);
        if !middle && !carried.beside(selected.is_some_and(|at| at.row == pos.row)) {
            return None;
        }
        let (place, clip) = match carried {
            Carried::Fixed => (Place::SAME, self.plates[m]),
            Carried::Row => (self.row_place(pos.row, m), self.columns.get(m).copied().flatten().map_or(self.plates[m], |(_, row)| row)),
            Carried::Own => (self.picture_place(pos, m), self.plates[m]),
        };
        Some((place, (!place.is_same()).then_some(clip)))
    }

    /// How bright the live preview of a tile moving as `carried` is.
    fn tile_shade(&self, pos: Pos, carried: Carried) -> f32 {
        match carried {
            Carried::Own => self.picture_shade(pos),
            Carried::Row if !self.strip_on => self.peek_shade(),
            _ => 1.0,
        }
    }

    /// The selected cell's picture on monitor `m` and its plate.
    fn selected_place(&self, m: usize) -> Option<(Place, Rect)> {
        let at = self.grid.find(self.shown())?;
        Some((self.picture_place(at, m), *self.plates.get(m)?))
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

    /// Shows the selected cell again at once, with nothing easing: says
    /// whether another was shown (and the tiles are to be laid out anew).
    fn end_peek(&mut self) -> bool {
        self.peek_shown = 0.0;
        self.peek_glide = (0.0, 0.0, Instant::now());
        self.peek.take().is_some()
    }

    /// Where the cell shown is in its easing back or forward at `now`.
    fn advance_peek(&mut self, now: Instant) {
        let (from, to, since) = self.peek_glide;
        let t = (now.saturating_duration_since(since).as_secs_f32() * 1000.0 / PEEK_MS).min(1.0);
        let ease = 1.0 - (1.0 - t).powi(3);
        self.peek_shown = if t >= 1.0 { to } else { from + (to - from) * ease };
    }

    /// Follows the cell of the map under the cursor without changing the
    /// selection: the preview area shows its windows at once, a little
    /// back, and the selected cell's again, easing forward, once the cursor
    /// is off the map. Pressed, it comes forward as it is gone to.
    fn update_peek(&mut self, now: Instant) {
        if self.strip_on || self.motion.is_moving() || crate::input::holding() {
            return;
        }
        let (map, x, y) = self.map_at(self.cursor.0, self.cursor.1);
        let on_map = contains(&map.layout.bounds, x, y);
        let pressed = |id: &CellId| matches!(&self.press, Some(Press { hit: Hit::Cell(on), dragging: false, .. }) if on == id);
        let wanted = match &self.hover {
            _ if self.editing.is_some() => None,
            _ if self.press.is_some() => self.peek.clone().filter(pressed),
            Hit::Cell(id) | Hit::CellClose(id) => Some(id.clone()).filter(|id| *id != self.selected),
            _ if on_map => self.peek.clone(),
            _ => None,
        };
        let back = if wanted.is_some() && !self.peek.as_ref().is_some_and(pressed) { 1.0 } else { 0.0 };
        if wanted == self.peek && back == self.peek_glide.1 {
            return;
        }
        self.advance_peek(now);
        let from = if self.peek.is_none() { 0.0 } else { self.peek_shown };
        let changed = wanted != self.peek;
        self.peek = wanted;
        self.peek_glide = (from, back, now);
        self.peek_shown = from;
        if changed {
            self.relayout();
            if self.open {
                self.register_thumbnails();
            }
        }
        if self.open {
            self.start_motion();
        }
        self.invalidate();
    }

    fn clear_thumbnails(&mut self) {
        unregister_badges(&mut self.badges);
        unregister_strip_thumbnails(&mut self.strip_thumbs);
        unregister_thumbnails(&mut self.thumbs);
        self.source_sizes.borrow_mut().clear();
        self.thumbnail_state.borrow_mut().clear();
    }

    /// Registers a live thumbnail for every tile and, while the strip
    /// moves, for as many of the tiles of the cells beside as
    /// `strip::MAX_THUMBS` allows. Thumbnails stack in the order they are
    /// registered: those beside go under the selected cell's, the windows
    /// on every desktop, which never move, over them, and the apps' icons
    /// over all the previews.
    fn register_thumbnails(&mut self) {
        let mut old = std::mem::take(&mut self.thumbs);
        let mut old_strip = std::mem::take(&mut self.strip_thumbs);
        let mut old_badges = std::mem::take(&mut self.badges);
        self.thumbnail_state.borrow_mut().clear();
        if self.strip_on {
            let wanted: Vec<Vec<HWND>> = self.strip_cells.iter().map(|cell| {
                let windows = self.model.tile_windows(&cell.id);
                cell.cell.tiles.iter()
                    .filter(|tile| windows.iter().any(|(w, everywhere)| w.hwnd == tile.hwnd && self.tile_place(cell.pos, Carried::of(w, *everywhere), &tile.frame).is_some()))
                    .map(|tile| tile.hwnd)
                    .collect()
            }).collect();
            let given = strip::budget(&wanted.iter().map(Vec::len).collect::<Vec<_>>(), strip::MAX_THUMBS);
            for ((cell, hwnds), given) in self.strip_cells.iter().zip(&wanted).zip(given) {
                for hwnd in &hwnds[..given] {
                    if let Ok(thumb) = unsafe { crate::composition::register(self.hwnd, *hwnd, crate::composition::board_enabled()) } {
                        self.strip_thumbs.push((cell.id.clone(), hwnd.0, thumb));
                    }
                }
            }
        }
        let mut wanted: Vec<(HWND, bool)> = self.tile_windows().into_iter().map(|(w, everywhere)| (w.hwnd, everywhere)).collect();
        wanted.sort_by_key(|(_, everywhere)| *everywhere);
        for (hwnd, _) in wanted {
            if let Ok(thumb) = unsafe { crate::composition::register(self.hwnd, hwnd, crate::composition::board_enabled()) } {
                self.thumbs.push((hwnd.0, thumb));
            }
        }
        self.register_badges();
        self.update_thumbnails();
        // Keep the old pictures until their replacements are visible.
        unregister_badges(&mut old_badges);
        unregister_strip_thumbnails(&mut old_strip);
        unregister_thumbnails(&mut old);
    }

    /// Registers the icon of every preview that has none yet and whose icon
    /// is on the sheet, over all the previews. Says whether there was one.
    fn register_badges(&mut self) -> bool {
        let Some(sheet) = &self.sheet else { return false };
        let previews = self.thumbs.iter().copied().chain(self.strip_thumbs.iter().map(|(_, hwnd, thumb)| (*hwnd, *thumb)));
        let wanted: Vec<isize> = previews.filter(|(hwnd, thumb)| !self.badges.contains_key(thumb) && sheet.slot(*hwnd).is_some()).map(|(_, thumb)| thumb).collect();
        let mut added = false;
        for thumb in wanted {
            if let Ok(badge) = unsafe { crate::composition::register(self.hwnd, sheet.hwnd, crate::composition::board_enabled()) } {
                self.badges.insert(thumb, badge);
                added = true;
            }
        }
        added
    }

    /// How large the window of `thumb` is. Asked of DWM once while the
    /// strip moves, every time otherwise (a window may change size).
    fn source_rect(&self, thumb: isize, hwnd: HWND) -> Option<ThumbnailSource> {
        if self.strip_on {
            if let Some(size) = self.source_sizes.borrow().get(&hwnd.0) {
                return *size;
            }
        }
        let size = unsafe { crate::composition::source_size(thumb) }.ok().map(|s| {
            let mut outer = RECT::default();
            let mut visible = RECT::default();
            let queried = (s.cx.max(1) as f32, s.cy.max(1) as f32);
            let mut full = queried;
            let mut frame = rect(0.0, 0.0, full.0, full.1);
            if unsafe { GetWindowRect(hwnd, &mut outer) }.is_ok()
                && unsafe { DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, &mut visible as *mut _ as _, std::mem::size_of::<RECT>() as u32) }.is_ok()
                && visible.right > visible.left && visible.bottom > visible.top {
                let outer_size = (outer.right - outer.left, outer.bottom - outer.top);
                let visible_size = (visible.right - visible.left, visible.bottom - visible.top);
                // DWM can report visible size while rcSource still includes resize borders.
                if outer_size == (s.cx, s.cy) || visible_size == (s.cx, s.cy) {
                    full = (outer_size.0.max(1) as f32, outer_size.1.max(1) as f32);
                    frame = Rect { left: (visible.left - outer.left).max(0) as f32, top: (visible.top - outer.top).max(0) as f32,
                        right: (visible.right - outer.left).min(outer_size.0) as f32, bottom: (visible.bottom - outer.top).min(outer_size.1) as f32 };
                }
            }
            if frame.right <= frame.left || frame.bottom <= frame.top { frame = rect(0.0, 0.0, full.0, full.1); }
            ThumbnailSource { full, frame }
        });
        if self.strip_on {
            self.source_sizes.borrow_mut().insert(hwnd.0, size);
        }
        size
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

    /// Puts every thumbnail where its tile is drawn, or under the cursor
    /// while its window is being dragged.
    fn update_thumbnails(&self) -> Option<(isize, RECT, RECT, Rect)> {
        let mut geometry = None;
        let at = self.grid.find(self.shown());
        let windows = self.tile_windows();
        let placing = |hwnd: isize| {
            let (window, everywhere) = windows.iter().find(|(w, _)| w.hwnd.0 == hwnd)?;
            let tile = self.tiles.iter().find(|tile| tile.hwnd.0 == hwnd)?;
            let Some(at) = at else { return Some((Place::SAME, None, 1.0)) };
            let carried = Carried::of(window, *everywhere);
            let (place, clip) = self.tile_place(at, carried, &tile.frame)?;
            Some((place, clip, self.tile_shade(at, carried)))
        };
        #[cfg(feature = "debug-log")]
        let mut placed = Vec::new();
        for &(hwnd, thumb) in &self.thumbs {
            // A window shown by its icon has no preview.
            let tile = self.tiles.iter().find(|tile| tile.hwnd.0 == hwnd).filter(|tile| !tile.icon_only);
            let minimized = windows.iter().any(|(w, _)| w.hwnd.0 == hwnd && w.minimized);
            let place = placing(hwnd);
            let dragged = self.dragged_window() == Some(hwnd);
            let _shown = self.put_thumbnail(thumb, tile, minimized, (0.0, 0.0), dragged, 1.0, place);
            self.put_badge(thumb, tile, minimized, _shown.is_some() && !dragged, place);
            if self.trace.is_some() && geometry.is_none() {
                if let (Some((dest, source)), Some(tile), Some((place, _, _))) = (_shown, tile, place) {
                    geometry = Some((hwnd, dest, source, place.rect(&tile.thumb)));
                }
            }
            #[cfg(feature = "debug-log")]
            placed.push(format!("{hwnd:x}={_shown:?}"));
        }
        for (id, hwnd, thumb) in &self.strip_thumbs {
            let Some(cell) = self.strip_cells.iter().find(|cell| cell.id == *id) else {
                self.put_thumbnail(*thumb, None, false, (0.0, 0.0), false, 1.0, None);
                continue;
            };
            let windows = self.model.tile_windows(id);
            let window = windows.iter().find(|(w, _)| w.hwnd.0 == *hwnd);
            let tile = cell.cell.tiles.iter().find(|tile| tile.hwnd.0 == *hwnd).filter(|tile| !tile.icon_only);
            let placing = window.zip(tile).and_then(|((w, everywhere), tile)| {
                let carried = Carried::of(w, *everywhere);
                self.tile_place(cell.pos, carried, &tile.frame).map(|(place, clip)| (place, clip, self.tile_shade(cell.pos, carried)))
            });
            let minimized = window.is_some_and(|(w, _)| w.minimized);
            let shown = self.put_thumbnail(*thumb, tile, minimized, (0.0, 0.0), false, 1.0, placing);
            self.put_badge(*thumb, tile, minimized, shown.is_some(), placing);
        }
        debug_log!("board: thumbnails {}", placed.join(" "));
        geometry
    }

    /// Shows the live picture `thumb` of a tile's window where the tile is
    /// drawn (`placing`: how it is moved, where it is cut off, how bright),
    /// or under the cursor while `dragged`; hides it with no tile or no
    /// place. `offset` is where the window it is shown in is. Says where it
    /// went.
    #[allow(clippy::too_many_arguments)]
    fn put_thumbnail(&self, thumb: isize, tile: Option<&TileLayout>, minimized: bool, offset: (f32, f32), dragged: bool, opacity: f32, placing: Option<(Place, Option<Rect>, f32)>) -> Option<(RECT, RECT)> {
        // Round the transformed edges exactly as the screen plate does.
        let to_rect = |r: &Rect| {
            let (left, top) = ((r.left - offset.0).round() as i32, (r.top - offset.1).round() as i32);
            RECT { left, top, right: (r.right - offset.0).round() as i32, bottom: (r.bottom - offset.1).round() as i32 }
        };
        let mut precise = [0.0; 4];
        let mut props = DWM_THUMBNAIL_PROPERTIES {
            dwFlags: DWM_TNP_RECTDESTINATION | DWM_TNP_VISIBLE | DWM_TNP_OPACITY | DWM_TNP_SOURCECLIENTAREAONLY,
            opacity: 255,
            fVisible: true.into(),
            fSourceClientAreaOnly: false.into(),
            ..Default::default()
        };
        let source_frame = tile.and_then(|tile| self.source_rect(thumb, tile.hwnd));
        let source = source_frame.map(|source| source.full);
        if let Some((w, h)) = source {
            props.rcSource = RECT { left: 0, top: 0, right: w.round() as i32, bottom: h.round() as i32 };
            props.dwFlags |= DWM_TNP_RECTSOURCE;
        }
        let fitted = |slot: &Rect| source_frame.map_or(*slot, |source| strip::fit_source(slot, &source.frame, source.full));
        match (tile, placing) {
            (Some(tile), _) if dragged => {
                // Follows the cursor, small enough to see the map under it.
                let (tw, th) = size(&tile.thumb);
                let (cx, cy) = self.cursor;
                let w = tw.min(200.0 * self.scale);
                let dest = fitted(&rect(cx - w / 2.0, cy - th / 2.0, w, th));
                precise = [dest.left-offset.0,dest.top-offset.1,dest.right-offset.0,dest.bottom-offset.1];
                props.rcDestination = to_rect(&dest);
                props.opacity = 215;
            }
            (Some(tile), Some((place, clip, bright))) => {
                // A tile has the shape of its window and the picture
                // fills it; in one that had to be made wider or higher
                // for its buttons the picture sits in the middle.
                let dest = place.rect(&fitted(&tile.thumb));
                precise = [dest.left-offset.0,dest.top-offset.1,dest.right-offset.0,dest.bottom-offset.1];
                match (clip, source) {
                    (Some(clip), Some(source)) => match strip::clip(&dest, &clip, source) {
                        Some((shown, from)) => {
                            precise = [shown.left-offset.0,shown.top-offset.1,shown.right-offset.0,shown.bottom-offset.1];
                            props.rcDestination = to_rect(&shown);
                            props.rcSource = RECT {
                                left: from.left.round() as i32,
                                top: from.top.round() as i32,
                                right: from.right.round() as i32,
                                bottom: from.bottom.round() as i32,
                            };
                            props.dwFlags |= DWM_TNP_RECTSOURCE;
                        }
                        None => props.fVisible = false.into(),
                    },
                    _ => props.rcDestination = to_rect(&dest),
                }
                // A minimized window shows what it last looked like, faded.
                // (Without a kept picture the source is only a sliver.)
                if minimized {
                    props.opacity = 150;
                    if !source.is_some_and(|(_, sh)| sh >= 80.0) {
                        props.fVisible = false.into();
                    }
                }
                props.opacity = (props.opacity as f32 * bright).round() as u8;
            }
            _ => props.fVisible = false.into(),
        }
        unsafe {
            props.opacity = (props.opacity as f32 * opacity).round() as u8;
            let visible = props.fVisible.as_bool();
            let state = if visible { (props.rcDestination, props.rcSource, props.opacity, true, props.dwFlags) }
                else { (RECT::default(), RECT::default(), 0, false, 0) };
            if (crate::composition::needs_update(thumb) || self.thumbnail_state.borrow().get(&thumb) != Some(&state)) && crate::composition::update(thumb, &props, precise).is_ok() {
                self.thumbnail_state.borrow_mut().insert(thumb, state);
                self.thumbnail_updates.set(self.thumbnail_updates.get() + 1);
            }
        }
        props.fVisible.as_bool().then_some((props.rcDestination, props.rcSource))
    }

    /// Puts the app's icon over the preview `thumb` of `tile`'s window: on
    /// its lower left corner, or large in its middle when the preview tells
    /// little (minimized, or of a window with nothing to show yet). Hidden
    /// with the preview, unless it is hidden for being minimized.
    fn put_badge(&self, thumb: isize, tile: Option<&TileLayout>, minimized: bool, shown: bool, placing: Option<(Place, Option<Rect>, f32)>) {
        let Some(&badge) = self.badges.get(&thumb) else { return };
        let mut props = DWM_THUMBNAIL_PROPERTIES {
            dwFlags: DWM_TNP_RECTDESTINATION | DWM_TNP_RECTSOURCE | DWM_TNP_VISIBLE | DWM_TNP_OPACITY,
            opacity: 255,
            fVisible: false.into(),
            ..Default::default()
        };
        let mut precise = [0.0; 4];
        let slot = tile.and_then(|tile| self.sheet.as_ref()?.slot(tile.hwnd.0));
        if let (Some(tile), Some((x, y, side)), Some((place, clip, bright))) = (tile, slot, placing) {
            let source = self.source_rect(thumb, tile.hwnd);
            // A minimized window with a kept picture shows it (`put_thumbnail`),
            // so its icon goes on the corner like any other.
            let kept = source.is_some_and(|source| source.full.1 >= 80.0);
            let plain = (minimized && !kept) || source.is_none_or(|source| source.frame.right - source.frame.left < 40.0 || source.frame.bottom - source.frame.top < 40.0);
            if shown || plain {
                let r = match source {
                    _ if plain => middle_icon(&tile.thumb, self.scale),
                    Some(source) => corner_icon(&shown_frame(&tile.thumb, &source), self.scale),
                    None => tile.icon,
                };
                let dest = place.rect(&r);
                let cut = match clip {
                    Some(clip) => strip::clip(&dest, &clip, (side, side)),
                    None => Some((dest, rect(0.0, 0.0, side, side))),
                };
                if let Some((dest, from)) = cut {
                    precise = [dest.left, dest.top, dest.right, dest.bottom];
                    props.rcDestination = RECT { left: dest.left.round() as i32, top: dest.top.round() as i32, right: dest.right.round() as i32, bottom: dest.bottom.round() as i32 };
                    props.rcSource = RECT {
                        left: (x + from.left).round() as i32,
                        top: (y + from.top).round() as i32,
                        right: (x + from.right).round() as i32,
                        bottom: (y + from.bottom).round() as i32,
                    };
                    props.opacity = (255.0 * bright).round() as u8;
                    props.fVisible = true.into();
                }
            }
        }
        let visible = props.fVisible.as_bool();
        let state = if visible { (props.rcDestination, props.rcSource, props.opacity, true, props.dwFlags) }
            else { (RECT::default(), RECT::default(), 0, false, 0) };
        unsafe {
            if (crate::composition::needs_update(badge) || self.thumbnail_state.borrow().get(&badge) != Some(&state)) && crate::composition::update(badge, &props, precise).is_ok() {
                self.thumbnail_state.borrow_mut().insert(badge, state);
                self.thumbnail_updates.set(self.thumbnail_updates.get() + 1);
            }
        }
    }

    /// Thumbnails stack in registration order; re-register the one that will
    /// follow the cursor to lift it above the rest.
    fn raise_thumbnail(&mut self, hwnd: isize) {
        if let Some(i) = self.thumbs.iter().position(|&(h, _)| h == hwnd) {
            let (_, old) = self.thumbs.remove(i);
            unsafe {
                // Its icon stays under it; a dragged window shows none.
                if let Some(badge) = self.badges.remove(&old) {
                    let _ = crate::composition::unregister(badge);
                }
                let _ = crate::composition::unregister(old);
                if let Ok(thumb) = crate::composition::register(self.hwnd, HWND(hwnd), crate::composition::board_enabled()) {
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
                // While it moves, the next frame draws it all within a
                // refresh; drawing here as well would come between frames.
                if !self.motion_timer {
                    self.paint();
                }
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
                self.cursor = (-1.0e6, -1.0e6);
                self.resting = None;
                self.update_peek(Instant::now());
                self.invalidate();
                Action::None
            }
            WM_LBUTTONUP => self.mouse_up(x, y),
            WM_RBUTTONUP => match self.hit_test(x, y) {
                Hit::Cell(id) | Hit::CellClose(id) => Action::ToggleEmphasis(id),
                _ => self.row_header_at(x, y).map_or(Action::None, Action::RowMenu),
            },
            WM_KEYDOWN => self.key_down(wparam.0 as u16),
            WM_TIMER if wparam.0 == TITLE_TIMER_ID => {
                unsafe { let _ = KillTimer(self.hwnd, TITLE_TIMER_ID); }
                self.invalidate();
                Action::None
            }
            WM_TIMER if wparam.0 == MOTION_TIMER_ID => {
                // Draws only when vblanks stopped coming (`FrameClock`).
                let now = Instant::now();
                if !self.vblank_frame.is_some_and(|at| now.saturating_duration_since(at) < VBLANK_TRUSTED) {
                    self.tick_motion(now, false);
                }
                Action::None
            }
            WM_BOARD_FRAME => {
                self.clock.frame_taken();
                if self.motion_timer {
                    let now = Instant::now();
                    self.vblank_frame = Some(now);
                    self.tick_motion(now, wparam.0 != 0);
                }
                Action::None
            }
            WM_TIMER if wparam.0 == ON_TOP_TIMER_ID => {
                self.keep_on_top();
                Action::None
            }
            WM_CHAR => {
                match (&mut self.editing, char::from_u32(wparam.0 as u32)) {
                    (Some((_, text)), Some(c)) => {
                        if !c.is_control() && text.chars().count() < NAME_LENGTH {
                            text.push(c);
                            self.invalidate();
                        }
                    }
                    // What can be done is said at the bottom, or no longer.
                    (None, Some('?')) => {
                        self.hints = !self.hints;
                        self.invalidate();
                    }
                    _ => {}
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
            if self.end_peek() {
                self.relayout();
                self.register_thumbnails();
            }
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
        self.press = Some(Press { hit, x, y, dragging: false });
        self.update_peek(Instant::now());
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
            // A window is dragged to a cell, a monitor, or a place of the
            // pinned windows (or out of one).
            let draggable = matches!(&press.hit, Hit::Window { .. } | Hit::Cell(_) | Hit::RowName(_));
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
                self.start_motion();
                self.update_peek(Instant::now());
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
            // Resting on a preview a while brings its title.
            let tile = match &self.hover {
                Hit::Window { hwnd, .. } | Hit::Pin(hwnd) | Hit::Top(hwnd) | Hit::TileClose(hwnd) => Some(*hwnd),
                _ => None,
            };
            if tile != self.resting.map(|(hwnd, _)| hwnd) {
                self.resting = tile.map(|hwnd| (hwnd, Instant::now()));
                unsafe {
                    let _ = KillTimer(self.hwnd, TITLE_TIMER_ID);
                    if tile.is_some() {
                        SetTimer(self.hwnd, TITLE_TIMER_ID, TITLE_MS, None);
                    }
                }
            }
        }
        self.update_peek(Instant::now());
    }

    fn mouse_up(&mut self, x: f32, y: f32) -> Action {
        unsafe {
            let _ = ReleaseCapture();
        }
        let Some(press) = self.press.take() else { return Action::None };
        // Where a drag ends is judged by the board as it was dragged over,
        // before the cell released on is peeked at and its windows shown.
        let dropped = press.dragging.then(|| match &press.hit {
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
        });
        self.hover = self.hit_test(x, y);
        // Released on the cell pointed at, it is gone to as it is shown.
        if !matches!(&press, Press { hit: Hit::Cell(id), dragging: false, .. } if *id == self.hover_cell()) {
            self.update_peek(Instant::now());
        }
        self.invalidate();
        if let Some(action) = dropped {
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

    /// The cell under the cursor in the map, if any.
    fn hover_cell(&self) -> CellId {
        match &self.hover {
            Hit::Cell(id) | Hit::CellClose(id) => id.clone(),
            _ => CellId::new(),
        }
    }

    /// The place of the pinned windows a point is in: how a window dropped
    /// there is pinned.
    fn zone_at(&self, x: f32, y: f32) -> Option<Pin> {
        self.columns.iter().flatten().find_map(|(all, row)| {
            if contains(&grow(all, 14.0 * self.scale), x, y) { Some(Pin::Everywhere) }
            else if contains(&grow(row, 14.0 * self.scale), x, y) { Some(Pin::Row) }
            else { None }
        })
    }

    /// A window of cell `from` was dragged to a point (`window_drop`).
    fn drop_window(&self, from: &str, hwnd: isize, x: f32, y: f32) -> Action {
        let windows = self.tile_windows();
        let Some(&(window, everywhere)) = windows.iter().find(|(w, _)| w.hwnd.0 == hwnd) else { return Action::None };
        let on_map = self.cell_at(x, y).is_some() || {
            let (map, mx, my) = self.map_at(x, y);
            contains(&map.layout.bounds, mx, my)
        };
        let (to_cell, to_monitor) = match self.cell_at(x, y) {
            Some((target, m)) => (Some(target.id.as_str()), m),
            None => (None, self.map_at(x, y).0.monitor),
        };
        let spot = DropSpot {
            zone: self.zone_at(x, y),
            on_map,
            on_plate: self.plates.iter().any(|plate| contains(plate, x, y)),
            cell: to_cell,
            other_monitor: to_monitor != self.monitor_of(window),
        };
        match window_drop(from, Pin::of(window, everywhere), &spot) {
            WindowDrop::Nothing => Action::None,
            WindowDrop::Pin(to) => {
                let (row, all) = to.flags();
                Action::SetPin { window: HWND(hwnd), row, all }
            }
            WindowDrop::Move { cell, monitor } => Action::MoveWindow {
                window: HWND(hwnd),
                cell,
                monitor: if monitor { self.monitor_on_screen(to_monitor) } else { None },
            },
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
                let (row, all) = Pin::of(window, *everywhere).next().flags();
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
            // A click on nothing dismisses the board, like Task View, at its
            // selection.
            Hit::Nothing => Action::Cancel,
        }
    }

    // ---- drawing ----------------------------------------------------------

    /// Present the full board on the GPU; keep the bitmap path for exports
    /// and machines that cannot create a window target.
    fn ensure_canvas(&mut self) -> Option<ID2D1RenderTarget> {
        if self.open && !self.window_failed {
            let size = D2D_SIZE_U { width: self.size.0 as u32, height: self.size.1 as u32 };
            if self.window_target.as_ref().is_some_and(|t| unsafe { t.GetPixelSize() } != size) {
                self.window_target = None;
            }
            if self.window_target.is_none() {
                self.icons.clear();
                self.glows = None;
                let made = unsafe { self.factory.CreateHwndRenderTarget(
                    &D2D1_RENDER_TARGET_PROPERTIES {
                        pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_IGNORE },
                        dpiX: 96.0, dpiY: 96.0, ..Default::default()
                    },
                    &D2D1_HWND_RENDER_TARGET_PROPERTIES { hwnd: self.hwnd, pixelSize: size, presentOptions: D2D1_PRESENT_OPTIONS_IMMEDIATELY },
                ) };
                match made {
                    Ok(target) => { self.window_target = Some(target); self.canvas = None; }
                    Err(e) => { crate::app::log(&format!("board GPU target failed, using bitmap: {e}")); self.window_failed = true; }
                }
            }
            if let Some(target) = &self.window_target { return target.cast().ok(); }
        }
        if !self.canvas.as_ref().is_some_and(|c| c.dib.size == self.size) {
            self.icons.clear();
            self.glows = None;
            self.canvas = Canvas::new(&self.factory, self.size)
                .map_err(|e| crate::app::log(&format!("board canvas failed: {e}"))).ok();
        }
        self.canvas.as_ref().map(|c| c.target.target.clone())
    }

    fn discard_target(&mut self) {
        self.window_target = None;
        self.canvas = None;
        self.icons.clear();
        self.glows = None;
    }

    /// Draws the board into the canvas; `None` when there is no canvas.
    fn draw_to_canvas(&mut self) -> Option<Result<()>> {
        if self.open && crate::composition::board_enabled() {
            if self.window_target.is_some() || self.canvas.is_some() { self.discard_target(); }
            match unsafe { crate::composition::begin_paint(self.hwnd, self.size) } {
                Ok((surface, target, offset)) => {
                    self.paint_origin.set((offset.x as f32, offset.y as f32));
                    if self.fonts.is_none() { self.fonts = make_fonts(self.scale).ok(); }
                    self.load_icons(&target);
                    unsafe { self.set_transform(&target, Matrix3x2::identity()); }
                    let drawn = self.draw(&target);
                    self.paint_origin.set((0.0, 0.0));
                    return Some(unsafe { surface.EndDraw() }.and(drawn));
                }
                Err(e) => return Some(Err(e)),
            }
        }
        let target = self.ensure_canvas()?;
        if self.fonts.is_none() {
            self.text_cache.clear();
            self.fonts = make_fonts(self.scale).map_err(|e| crate::app::log(&format!("board fonts failed: {e}"))).ok();
        }
        self.load_icons(&target);
        if self.glows.is_none() {
            self.glows = Glows::new(&target);
        }
        unsafe {
            target.BeginDraw();
            let drawn = self.draw(&target);
            Some(target.EndDraw(None, None).and(drawn))
        }
    }

    fn paint(&mut self) {
        self.advance_peek(Instant::now());
        match self.draw_to_canvas() {
            Some(Ok(())) => {}
            Some(Err(e)) => {
                crate::app::log(&format!("board draw failed: {e}"));
                // Rebuild everything on the next paint.
                self.discard_target();
                return;
            }
            None => return,
        }
        self.present_canvas();
        let _ = crate::composition::commit(self.hwnd);
    }

    fn present_canvas(&mut self) {
        if self.window_target.is_none() && !crate::composition::board_enabled() {
            if let Some(canvas) = &self.canvas {
                let (w, h) = canvas.dib.size;
                unsafe {
                    let window = GetDC(self.hwnd);
                    let _ = BitBlt(window, 0, 0, w, h, canvas.dib.dc, 0, 0, SRCCOPY);
                    ReleaseDC(self.hwnd, window);
                    let _ = GdiFlush();
                }
            }
        }
    }

    fn load_icons(&mut self, target: &ID2D1RenderTarget) {
        // Large enough for the middle of a preview that tells little.
        let size = (64.0 * self.scale).round() as i32;
        let in_cells = self.model.rows.iter().flat_map(|r| &r.cells).flat_map(|c| &c.windows);
        for window in in_cells.chain(&self.model.pinned) {
            // Cache absent icons too: asking another process every frame stalls motion.
            if self.icons.contains_key(&window.hwnd.0) {
                continue;
            }
            let pixels = icon_pixels(window.hwnd, size);
            if let (Some(sheet), Some(pixels)) = (&mut self.sheet, &pixels) {
                sheet.add(window.hwnd.0, size, pixels);
            }
            self.icons.insert(window.hwnd.0, pixels.and_then(|pixels| icon_bitmap(target, &pixels, size)));
        }
        if let Some(sheet) = &mut self.sheet {
            if let Err(e) = sheet.update() {
                crate::app::log(&format!("board icon sheet: {e}"));
            }
        }
        // The previews are registered before their icons are first loaded.
        if self.open && self.register_badges() {
            self.update_thumbnails();
        }
    }

    unsafe fn set_transform(&self, target: &ID2D1RenderTarget, mut matrix: Matrix3x2) {
        let origin = self.paint_origin.get();
        matrix.M31 += origin.0; matrix.M32 += origin.1;
        target.SetTransform(&matrix);
    }

    fn draw(&self, target: &ID2D1RenderTarget) -> Result<()> {
        let Some(fonts) = &self.fonts else { return Ok(()) };
        unsafe {
            target.Clear(Some(&paint::backdrop()));
        }
        let p = Painter::cached(target, &self.text_cache)?;
        // The tiles are placed in the window's coordinates; each map, and the
        // label and the hint of the home monitor, in its monitor's own.
        #[cfg(feature = "motion-probe")]
        self.probe_marker.set(None);
        self.draw_tiles(&p, fonts);
        for map in &self.maps {
            unsafe {
                self.set_transform(target, Matrix3x2::translation(map.area.left, map.area.top));
            }
            self.draw_map(&p, fonts, map);
        }
        unsafe {
            self.set_transform(target, Matrix3x2::translation(self.home.left, self.home.top));
        }
        self.draw_captions(&p, fonts);
        unsafe {
            self.set_transform(target, Matrix3x2::identity());
        }
        #[cfg(feature = "motion-probe")]
        if let Some(x) = self.probe_marker.get() { p.fill_square(&rect(x-2.0,20.0,4.0,12.0),rgba(0.0,1.0,0.0,1.0)); }
        Ok(())
    }

    /// What the line at the bottom says, shown the first time and when ?
    /// is pressed: what can be done with the thing under the cursor.
    fn footer(&self) -> Option<(String, f32)> {
        if !self.hints {
            return None;
        }
        let hint = |text: &str| Some((text.to_owned(), 0.46));
        if self.editing.is_some() {
            return hint("Enter: save   ·   Esc: cancel");
        }
        if self.dragged_window().is_some() {
            return hint(if self.monitors.len() > 1 {
                "Drop up left to pin to all shelves, below that to this shelf, among the previews to unpin, on a cell to move it there, or on another monitor"
            } else {
                "Drop up left to pin to all shelves, below that to this shelf, among the previews to unpin, or on a cell to move it there"
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
            Hit::Pin(_) => hint("Click: pin to this shelf, then to all shelves, then unpin"),
            Hit::TileClose(_) => hint("Click: close the window"),
            Hit::Window { .. } => hint("Click: go to it   ·   Drag: to a cell, or up left to pin it"),
            Hit::Cell(_) | Hit::CellClose(_) => {
                hint("Hover: show windows   ·   Click: go   ·   Drag: move   ·   Right-click: highlight")
            }
            Hit::RowName(_) => hint("Drag: reorder   ·   Double-click: rename   ·   Right-click: close windows, remove"),
            Hit::Plus(_) | Hit::PlusLeft(_) => hint("Add a cell"),
            Hit::AddRow | Hit::AddRowTop => hint("Add a shelf"),
            Hit::Nothing => hint("Point at a cell to see its windows   ·   Drag windows up left to pin them   ·   ?: hide these hints"),
        }
    }

    /// The shelf's name at the top and the line at the bottom of the home monitor.
    fn draw_captions(&self, p: &Painter, fonts: &Fonts) {
        let s = self.scale;
        let (w, h) = size(&self.home);
        if let Some(pos) = self.grid.find(self.shown()) {
            let row = grid::row_title(self.model.rows.get(pos.row).map_or("", |row| &row.name), pos.row);
            p.text(&row, &fonts.label, &self.label, white(if self.peek.is_some() { 0.62 } else { 0.86 }));
        }
        if let Some((line, alpha)) = self.footer() {
            p.text(&line, &fonts.centered, &rect(0.0, h - (FOOTER - 8.0) * s, w, 26.0 * s), white(alpha));
        }
    }

    /// A surface standing off the dark: its soft shadow (`lift` from 0 to
    /// 1, how high it stands), a face of `face` brightness, lighter at its
    /// top, and a faint line of light along its top edge.
    fn surface(&self, p: &Painter, r: &Rect, radius: f32, face: f32, alpha: f32, lift: f32) {
        let s = self.scale;
        if let Some(glows) = &self.glows {
            p.glow(&glows.shadow, &shifted_rect(r, 0.0, (5.0 + 5.0 * lift) * s), (34.0 + 26.0 * lift) * s, (0.36 + 0.24 * lift) * alpha);
            p.glow(&glows.shadow, &shifted_rect(r, 0.0, 1.0 * s), (8.0 + 4.0 * lift) * s, 0.50 * alpha);
        }
        p.fill(r, radius, rgba(face, face + 0.006, face + 0.02, alpha));
        p.sheen(r, radius, 0.055 * alpha);
        let inset = radius.max(2.0 * s);
        if r.right - r.left > 2.0 * inset {
            p.line((r.left + inset, r.top + 0.75 * s), (r.right - inset, r.top + 0.75 * s), 1.0 * s, white(0.07 * alpha));
        }
    }

    /// The selected cell's windows as large previews, those pinned in their
    /// places at the left. While the selection moves, the preview area is a
    /// strip of desktops: a faint well round it, the cells beside at its
    /// edges, darker, all moving as `tile_place` has it.
    fn draw_tiles(&self, p: &Painter, fonts: &Fonts) {
        let s = self.scale;
        let depth = if self.strip_on { self.drawn_strip().depth(self.strip_at) } else { 0.0 };
        let at = self.grid.find(self.shown());
        for section in self.sections.iter().filter(|section| section.carried != Carried::Own) {
            self.draw_zone(p, section);
        }
        if depth > 0.001 {
            for plate in &self.plates {
                let frame = grow(plate, 10.0 * s);
                p.fill(&frame, 18.0 * s, rgba(0.0, 0.0, 0.0, 0.20 * depth));
                p.sheen(&frame, 18.0 * s, 0.02 * depth);
            }
        }
        let selected = CellTiles::default();
        for m in 0..self.plates.len() {
            // Each cell's own windows in the plate, those pinned to the rows
            // in their own place.
            let zone = self.columns.get(m).copied().flatten().map(|(_, row)| grow(&row, 12.0 * s));
            for (which, clip) in [(Carried::Own, self.plates[m]), (Carried::Row, zone.unwrap_or(self.plates[m]))] {
                if self.strip_on {
                    unsafe {
                        p.target.PushAxisAlignedClip(&clip, D2D1_ANTIALIAS_MODE_ALIASED);
                    }
                }
                for cell in &self.strip_cells {
                    self.draw_picture(p, fonts, m, cell.pos, &cell.id, &cell.cell, depth, which);
                }
                if let Some(at) = at {
                    self.draw_picture(p, fonts, m, at, self.shown(), &selected, depth, which);
                }
                if self.strip_on {
                    unsafe {
                        p.target.PopAxisAlignedClip();
                    }
                }
            }
        }
        // What is on every desktop never moves, and is over all that does.
        let shown = self.shown();
        self.draw_group(p, fonts, shown, &self.sections, &self.tiles, |carried, _| (carried == Carried::Fixed).then_some(Place::SAME));
        if at.is_none() {
            self.draw_group(p, fonts, shown, &self.sections, &self.tiles, |carried, _| (carried != Carried::Fixed).then_some(Place::SAME));
        }
    }

    /// The picture of the cell `id` at `pos` on monitor `m`: its windows
    /// that move as `which` and, for its own, its panel while the strip
    /// moves. The cell shown's layout is the board's own.
    #[allow(clippy::too_many_arguments)]
    fn draw_picture(&self, p: &Painter, fonts: &Fonts, m: usize, pos: Pos, id: &str, cell: &CellTiles, depth: f32, which: Carried) {
        let s = self.scale;
        let middle = id == self.shown();
        let (tiles, sections) = if middle { (&self.tiles, &self.sections) } else { (&cell.tiles, &cell.sections) };
        let place = self.picture_place(pos, m);
        let plate = self.plates[m];
        let on_m = |r: &Rect| self.monitor_at(r) == m;
        let own = which == Carried::Own;
        if own && depth > 0.001 {
            self.placed(p, place, || {
                let radius = 16.0 * s / place.s;
                if let (true, Some(glows)) = (middle, &self.glows) {
                    p.glow(&glows.light, &plate, 60.0 * s / place.s, 0.10 * depth);
                }
                self.surface(p, &plate, radius, 0.075, depth, 0.6);
            });
        }
        if own {
            self.draw_empty(p, fonts, m, id, sections, place);
        }
        let shows = |carried: Carried, r: &Rect| {
            if carried != which || !on_m(r) {
                return None;
            }
            self.tile_place(pos, carried, r).map(|(place, _)| place)
        };
        self.draw_group(p, fonts, id, sections, tiles, shows);
        if own && depth > 0.001 {
            self.placed(p, place, || p.fill(&plate, 16.0 * s / place.s, rgba(0.0, 0.0, 0.0, 1.0 - self.picture_shade(pos))));
        }
    }

    /// A faint word in the middle of the picture of a desktop with no
    /// windows of its own, on the home monitor, under any pinned there; so
    /// that it does not look like one whose previews are late. Left out
    /// where the picture is drawn too small for it.
    fn draw_empty(&self, p: &Painter, fonts: &Fonts, m: usize, id: &str, sections: &[Section], place: Place) {
        let s = self.scale;
        if m != self.home_index() || !self.model.has_no_own_windows(id) {
            return;
        }
        let on_m = |r: &Rect| self.monitor_at(r) == m;
        let part = sections.iter().find(|section| section.carried == Carried::Own && on_m(&section.part)).map_or(self.plates[m], |section| section.part);
        let (w, h) = size(&part);
        if w * place.s < 160.0 * s || h * place.s < 60.0 * s {
            return;
        }
        let (_, cy) = centre(&part);
        let line = rect(part.left, cy - 12.0 * s, w, 24.0 * s);
        self.placed(p, place, || p.text(crate::i18n::text("No windows", "창 없음", "无窗口"), &fonts.empty, &line, white(0.40)));
    }

    /// The tiles of cell `id`, and the cells standing for the windows that
    /// did not fit in their place, for which `place` says how they are drawn.
    fn draw_group(&self, p: &Painter, fonts: &Fonts, id: &str, sections: &[Section], tiles: &[TileLayout], place: impl Fn(Carried, &Rect) -> Option<Place>) {
        let windows = self.model.tile_windows(id);
        let middle = id == self.shown();
        for tile in tiles {
            if let Some((window, everywhere)) = windows.iter().find(|(w, _)| w.hwnd == tile.hwnd) {
                if let Some(at) = place(Carried::of(window, *everywhere), &tile.frame) {
                    let preview = if tile.icon_only { None } else if middle {
                        self.thumbs.iter().find(|&&(hwnd, _)| hwnd == tile.hwnd.0).map(|&(_, thumb)| thumb)
                    } else {
                        self.strip_thumbs.iter().find(|(cell, hwnd, _)| cell == id && *hwnd == tile.hwnd.0).map(|&(_, _, thumb)| thumb)
                    };
                    let badged = preview.is_some_and(|thumb| self.badges.contains_key(&thumb));
                    self.placed(p, at, || self.draw_tile(p, fonts, tile, window, Pin::of(window, *everywhere), preview.is_some(), badged));
                    #[cfg(feature = "motion-probe")]
                    if window.title == "F motion 0" {
                        let dest = at.rect(&tile.thumb);
                        let x = (dest.left + dest.right) * 0.5;
                        self.probe_marker.set(Some(x));
                    }
                }
            }
        }
        for section in sections {
            if let Some((r, hwnds, count)) = &section.more {
                if let Some(at) = place(section.carried, r) {
                    self.placed(p, at, || self.draw_more(p, fonts, r, hwnds, *count));
                }
            }
        }
    }

    /// A place of the pinned windows: a faint pool of its light on its
    /// floor, brighter while a window is dragged, and most under it, and
    /// over it a tint of its colour while one is.
    fn draw_zone(&self, p: &Painter, section: &Section) {
        let s = self.scale;
        let Some(glows) = &self.glows else { return };
        let (glow, colour, reach): (&Glow, fn(f32) -> D2D1_COLOR_F, f32) = match section.carried {
            Carried::Fixed => (&glows.all, everywhere, paint::PIN_ALL_REACH),
            _ => (&glows.row, on_row, paint::PIN_ROW_REACH),
        };
        let dragging = self.dragged_window().is_some();
        let over = dragging && contains(&grow(&section.part, 14.0 * s), self.cursor.0, self.cursor.1);
        let part = section.part;
        let (w, _) = size(&part);
        // Its windows have their own glow; an empty place has a pool of it.
        let empty = !self.tiles.iter().any(|tile| contains(&part, centre(&tile.frame).0, centre(&tile.frame).1)) && section.more.is_none();
        if empty || dragging {
            let floor = rect(part.left + w * 0.12, part.bottom - 10.0 * s, w * 0.76, 6.0 * s);
            p.glow(glow, &floor, 90.0 * reach * s, if over { 0.70 } else if dragging { 0.40 } else { 0.22 });
        }
        if dragging {
            p.fill(&part, 16.0 * s, colour(if over { 0.075 } else { 0.035 }));
            p.sheen(&part, 16.0 * s, if over { 0.05 } else { 0.02 });
        }
    }

    /// Draws with everything moved as `place` says, the target having no
    /// transform before and after.
    fn placed(&self, p: &Painter, place: Place, draw: impl FnOnce()) {
        if place.is_same() {
            return draw();
        }
        unsafe {
            self.set_transform(p.target, Matrix3x2 { M11: place.s, M12: 0.0, M21: 0.0, M22: place.s, M31: place.dx, M32: place.dy });
        }
        draw();
        unsafe {
            self.set_transform(p.target, Matrix3x2::identity());
        }
    }

    /// One window: what is under its live preview (its shadow, the glow of
    /// its pin, a face for when there is nothing live to show), its app's
    /// icon unless one is put over the preview (`put_badge`): large in the
    /// middle of a preview that tells little, on its lower left corner
    /// otherwise, and under the cursor its buttons and, after a while, its
    /// title. Kept above the others, it stands higher.
    #[allow(clippy::too_many_arguments)]
    fn draw_tile(&self, p: &Painter, fonts: &Fonts, tile: &TileLayout, window: &WindowModel, pin: Pin, live: bool, badged: bool) {
        let s = self.scale;
        let hot = self.hover == self.tile_hit(tile.hwnd);
        // The cursor is on the tile or on one of its buttons.
        let pointed = hot || matches!(&self.hover, Hit::Pin(hwnd) | Hit::Top(hwnd) | Hit::TileClose(hwnd) if *hwnd == tile.hwnd.0);
        let dragged = self.dragged_window() == Some(tile.hwnd.0);
        let dim = if dragged { 0.35 } else { 1.0 };
        let body = if tile.icon_only { tile.frame } else { tile.thumb };
        let (bw, _) = size(&body);
        // What of its window shows, where its light is cast (`visible`).
        let shown = match pin {
            Pin::Off => None,
            _ if tile.icon_only => None,
            _ => crate::visible::shown(tile.hwnd),
        }
        .map_or(body, |shown| {
            let [left, top, right, bottom] = crate::visible::within([body.left, body.top, body.right, body.bottom].map(f64::from), shown).map(|v| v as f32);
            Rect { left, top, right, bottom }
        });
        let lift = if window.topmost { 1.0 } else if pointed { 0.4 } else { 0.0 };
        if let Some(glows) = &self.glows {
            // Pinned: a glow of its place's colour under it.
            if let Some(glow) = glows.of(pin) {
                // None within it, which would show through a window seen through.
                p.outside(&shown, paint::PIN_FRAME * s, || {
                    for (area, corner, opacity) in paint::pin_glows(&shown, s, pin == Pin::Everywhere) {
                        p.glow(glow, &area, corner, opacity * dim);
                    }
                });
            }
            if pointed && !dragged {
                p.glow(&glows.light, &body, 30.0 * s, 0.16);
            }
        }
        self.surface(p, &body, if tile.icon_only { 10.0 * s } else { 8.0 * s }, if pointed { 0.12 } else { 0.085 }, dim, lift);
        let icon = self.icons.get(&tile.hwnd.0).and_then(Option::as_ref);
        let centred = |k: f32| { let (cx, cy) = centre(&body); rect(cx - k / 2.0, cy - k / 2.0, k, k) };
        if tile.icon_only {
            match icon {
                Some(bitmap) => p.bitmap(bitmap, &centred(bw * 0.62), dim),
                None => p.disc(centre(&body), bw * 0.16, white(0.20 * dim)),
            }
            return;
        }
        if let Some(bitmap) = icon.filter(|_| !badged) {
            // With nothing live to show, its app's icon in the middle says what it is.
            if !live || window.minimized {
                p.bitmap(bitmap, &middle_icon(&body, s), 0.85 * dim);
            } else {
                p.bitmap(bitmap, &tile.icon, dim);
            }
        }
        let resting = self.resting.is_some_and(|(hwnd, at)| hwnd == tile.hwnd.0 && at.elapsed().as_millis() + 30 >= TITLE_MS as u128);
        if pointed && resting && !dragged {
            let title = display_title(&window.title, &window.app_name, (size(&tile.title).0 / (6.6 * s)) as usize);
            let text = if window.app_name.is_empty() || title.is_empty() || title.eq_ignore_ascii_case(&window.app_name) {
                if title.is_empty() { window.app_name.clone() } else { title }
            } else {
                format!("{}  —  {}", window.app_name, title)
            };
            let line = Rect { right: tile.top.left - 6.0 * s, ..tile.title };
            p.text(&text, &fonts.small, &line, white(0.46));
        }
        if pointed && !dragged {
            self.draw_tile_buttons(p, tile, window, pin);
        }
        if self.refused.is_some_and(|(hwnd, at)| hwnd == tile.hwnd.0 && at.elapsed().as_millis() < REFUSAL_MS) {
            // Under the picture: the live picture of the window covers the rest.
            let note = Rect { top: tile.thumb.bottom + 2.0 * s, ..tile.frame };
            p.fill(&note, 6.0 * s, danger(0.90));
            p.text("Runs as administrator: cannot be changed", &fonts.centered, &note, white(1.0));
        }
    }

    /// A tile's three buttons, as small signs: kept above the others (two
    /// cards, one over the other), its pin in its place's colour, and close.
    fn draw_tile_buttons(&self, p: &Painter, tile: &TileLayout, window: &WindowModel, pin: Pin) {
        let s = self.scale;
        let h = tile.hwnd.0;
        let hot = |hit: Hit| self.hover == hit;
        for (area, hit) in [(tile.top, Hit::Top(h)), (tile.pin, Hit::Pin(h)), (tile.close, Hit::TileClose(h))] {
            p.disc(centre(&area), size(&area).0 / 2.0, white(if hot(hit) { 0.13 } else { 0.055 }));
        }
        let (cx, cy) = centre(&tile.top);
        let lit = if window.topmost { 0.95 } else if hot(Hit::Top(h)) { 0.85 } else { 0.50 };
        let back = rect(cx - 2.5 * s, cy - 5.0 * s, 7.5 * s, 6.0 * s);
        let front = rect(cx - 5.0 * s, cy - 1.5 * s, 7.5 * s, 6.0 * s);
        p.stroke(&back, 1.5 * s, 1.2 * s, white(lit * 0.75));
        p.fill(&front, 1.5 * s, if window.topmost { white(lit) } else { rgba(0.13, 0.14, 0.16, 1.0) });
        p.stroke(&front, 1.5 * s, 1.2 * s, white(lit));

        let (cx, cy) = centre(&tile.pin);
        let ink = pin_colour(pin, if hot(Hit::Pin(h)) { 1.0 } else { 0.85 });
        // A pushpin: needle, flange, body and head end to end (none drawn
        // over another), leaning while it pins nothing, upright once it does.
        let (ux, uy) = if pin == Pin::Off { (0.71, -0.71) } else { (0.0, -1.0) };
        let at = |v: f32, w: f32| (cx + (ux * (v + 0.9) - uy * w) * s, cy + (uy * (v + 0.9) + ux * w) * s);
        let along = |from: f32, to: f32, width: f32| p.line(at(from, 0.0), at(to, 0.0), width * s, ink);
        let across = |v: f32, half: f32, width: f32| p.line(at(v, -half), at(v, half), width * s, ink);
        along(-6.0, -2.1, 1.2);
        across(-1.5, 3.5, 1.2);
        along(-0.9, 2.6, 3.4);
        across(3.4, 3.0, 1.6);
        if window.pin_from_app {
            // The app's pin, which a window cannot take off.
            p.disc((cx + 5.5 * s, cy + 4.0 * s), 1.4 * s, ink);
        }

        let (cx, cy) = centre(&tile.close);
        let arm = 3.2 * s;
        let ink = if hot(Hit::TileClose(h)) { danger(1.0) } else { white(0.60) };
        p.line((cx - arm, cy - arm), (cx + arm, cy + arm), 1.4 * s, ink);
        p.line((cx - arm, cy + arm), (cx + arm, cy - arm), 1.4 * s, ink);
    }

    /// The cell standing for the windows that did not fit in a place: a
    /// few of their icons one over another, and how many there are.
    fn draw_more(&self, p: &Painter, fonts: &Fonts, r: &Rect, hwnds: &[HWND], count: usize) {
        let (w, h) = size(r);
        self.surface(p, r, 8.0 * self.scale, 0.085, 1.0, 0.0);
        let k = w.min(h) * 0.46;
        for (i, hwnd) in hwnds.iter().take(3).enumerate().rev() {
            let off = i as f32 * k * 0.24;
            if let Some(bitmap) = self.icons.get(&hwnd.0).and_then(Option::as_ref) {
                p.bitmap(bitmap, &rect(r.left + w * 0.12 + off, r.top + h * 0.10 + off, k, k), 0.9 - 0.2 * i as f32);
            }
        }
        let label = Rect { left: r.left + w * 0.30, top: r.top + h * 0.55, ..*r };
        p.text(&format!("+{count}"), &fonts.tiny, &label, white(0.80));
    }

    /// Draws with everything moved by `by`, the target's translation being
    /// `base` before and after.
    fn shifted(&self, p: &Painter, base: (f32, f32), by: (f32, f32), draw: impl FnOnce()) {
        if by == (0.0, 0.0) {
            return draw();
        }
        unsafe {
            self.set_transform(p.target, Matrix3x2::translation(base.0 + by.0, base.1 + by.1));
        }
        draw();
        unsafe {
            self.set_transform(p.target, Matrix3x2::translation(base.0, base.1));
        }
    }

    /// The map of all rows and cells: squares holding each cell's apps
    /// (icon and count), the selected one brighter and larger, with the
    /// controls appearing on hover.
    fn draw_map(&self, p: &Painter, fonts: &Fonts, map: &Map) {
        let s = self.scale;
        let layout = &map.layout;
        let selected_row = self.grid.find(&self.selected).map(|pos| pos.row);
        // The cell a dragged window would be dropped on.
        let drop_cell = self.dragged_window().and_then(|_| self.cell_at(self.cursor.0, self.cursor.1)).map(|(c, _)| c.id.as_str());
        let (cx, cy) = (self.cursor.0 - map.area.left, self.cursor.1 - map.area.top);
        let on_map = contains(&grow(&layout.bounds, 12.0 * s), cx, cy) && self.press.is_none();

        // Each part where the motion has it (`Motion`), the selection's frame
        // apart from its cell: under the cells, and its light over them.
        let base = (map.area.left, map.area.top);
        let at = |part: Part| self.motion.offset(&part);
        let (spine, names, selection) = (at(Part::Spine(map.monitor)), at(Part::Names(map.monitor)), at(Part::Selection(map.monitor)));
        let selected = layout.rows.iter().flat_map(|row| &row.cells).find(|cell| cell.id == self.selected);

        // Windows on every desktop: a thin white line over the map, under
        // the previews.
        if !self.model.pinned.is_empty() {
            let y = layout.top - 6.0 * s;
            p.fading_line((layout.bounds.left, y), (layout.bounds.right, y), 1.2 * s, white(0.65));
        }
        // The column vertical moves travel along.
        self.shifted(p, base, spine, || {
            p.fill(&layout.spine, 12.0 * s, rgba(0.0, 0.0, 0.0, 0.16));
            p.sheen(&layout.spine, 12.0 * s, 0.018);
        });
        if let Some(cell) = selected {
            self.shifted(p, base, selection, || self.draw_selection(p, cell, false));
        }
        for (r, (row, model)) in layout.rows.iter().zip(&self.model.rows).enumerate() {
            let in_selected_row = selected_row == Some(r);
            self.shifted(p, base, names, || self.draw_row_name(p, fonts, r, row, &model.name, in_selected_row));
            let shift = row.cells.first().map_or((0.0, 0.0), |first| at(Part::Row(map.monitor, first.id.clone())));
            // How far the selected cell is drawn from its frame, in cells:
            // away from it, it is filled as the others are.
            let apart = (shift.0 - selection.0).hypot(shift.1 - selection.1) / layout.pitch.max(1.0);
            let near = on_map && cy >= row.top && cy < row.bottom;
            self.shifted(p, base, shift, || {
                for cell in &row.cells {
                    self.draw_cell(p, fonts, cell, in_selected_row, drop_cell == Some(cell.id.as_str()), apart.min(1.0));
                }
                self.draw_plus(p, &row.plus, Hit::Plus(r), near);
                self.draw_plus(p, &row.plus_left, Hit::PlusLeft(r), near);
            });
        }
        self.shifted(p, base, spine, || {
            self.draw_plus(p, &layout.add_row, Hit::AddRow, on_map);
            self.draw_plus(p, &layout.add_row_top, Hit::AddRowTop, on_map);
        });
        if let Some(cell) = selected {
            self.shifted(p, base, selection, || self.draw_selection(p, cell, true));
        }

        // What is being dragged is drawn only on the monitor the cursor is on.
        if contains(&map.area, self.cursor.0, self.cursor.1) {
            self.draw_drag(p, map);
        }
    }

    /// A row's name, on a faint surface as wide as its text while hovered or
    /// edited; a dot of their colour after it when windows are pinned to the row.
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
            let area = grow(&Rect { right: row.name.left + width.max(size(&row.name).0), ..row.name }, 4.0 * s);
            p.fill(&area, 6.0 * s, white(if editing.is_some() { 0.09 } else { 0.045 }));
            p.sheen(&area, 6.0 * s, 0.05);
        }
        let wide = Rect { right: row.header.right, ..row.name };
        p.text(&name, &fonts.small, &wide, white(if in_selected_row || hot { 0.90 } else { 0.45 }));
        if row.pinned && editing.is_none() {
            let (x, y) = ((row.name.right + 8.0 * s).min(row.header.right - 4.0 * s), centre(&row.name).1);
            if let Some(glows) = &self.glows {
                p.glow(&glows.row_mark, &rect(x - 1.5 * s, y - 1.5 * s, 3.0 * s, 3.0 * s), 10.0 * s, 0.45);
            }
            p.disc((x, y), 2.6 * s, on_row(0.95));
        }
    }

    /// The frame of the selected cell: a brighter surface, a little larger
    /// than a cell and standing higher, under the cells; or the line of
    /// light along its top, over them. It is drawn apart from the cell, so
    /// that it can stay while the row slides under it.
    fn draw_selection(&self, p: &Painter, cell: &CellLayout, outline: bool) {
        let s = self.scale;
        let body = grow(&cell.body, 3.0 * s);
        let radius = 11.0 * s;
        let dim = if self.dragged_cell() == Some(cell.id.as_str()) { 0.35 } else { 1.0 };
        if outline {
            p.line((body.left + radius, body.top + 0.75 * s), (body.right - radius, body.top + 0.75 * s), 1.0 * s, white(0.20 * dim));
            return;
        }
        if let Some(glows) = &self.glows {
            p.glow(&glows.light, &body, 30.0 * s, 0.14 * dim);
        }
        self.surface(p, &body, radius, 0.215, dim, 1.0);
    }

    /// A cell of the map, brighter the more it is used. The selected one is
    /// shown by its frame (`draw_selection`), and as the others are only
    /// where it is drawn `apart` from that frame, in cells (up to one).
    fn draw_cell(&self, p: &Painter, fonts: &Fonts, cell: &CellLayout, in_selected_row: bool, drop_target: bool, apart: f32) {
        let s = self.scale;
        let radius = 9.0 * s;
        let dim = if self.dragged_cell() == Some(cell.id.as_str()) { 0.35 } else { 1.0 };
        let is_selected = cell.id == self.selected;
        let hovering = matches!(&self.hover, Hit::Cell(id) | Hit::CellClose(id) if *id == cell.id);
        let face = 0.10 + 0.075 * cell.usage
            + if in_selected_row { 0.012 } else { 0.0 }
            + if hovering { 0.035 } else { 0.0 }
            + if drop_target { 0.04 } else { 0.0 };
        let alpha = if is_selected { apart } else { 1.0 } * dim;
        if let Some(glows) = &self.glows {
            if drop_target {
                p.glow(&glows.accent, &cell.body, 26.0 * s, 0.55);
            }
            if cell.emphasised {
                p.glow(&glows.warm, &shifted_rect(&cell.body, 0.0, 3.0 * s), 22.0 * s, 0.40 * dim);
            }
        }
        if alpha > 0.0 {
            self.surface(p, &cell.body, radius, face, alpha, 0.0);
        }
        if cell.emphasised {
            p.fill(&cell.body, radius, warm(0.11 * dim));
        }
        if cell.id == self.model.current {
            // Where the user actually is, as opposed to what is selected.
            let dot = (cell.body.left + 8.0 * s, cell.body.top + 8.0 * s);
            if let Some(glows) = &self.glows {
                p.glow(&glows.accent, &rect(dot.0 - 1.5 * s, dot.1 - 1.5 * s, 3.0 * s, 3.0 * s), 10.0 * s, 0.5 * dim);
            }
            p.disc(dot, 2.6 * s, accent(dim));
        }
        self.draw_badges(p, fonts, &cell.apps, dim);
        if cell.more > 0 {
            p.text(&format!("+{}", cell.more), &fonts.centered, &cell.more_label, white(0.50 * dim));
        }
        if hovering && self.model.cell_count() > 1 && self.press.is_none() {
            let hot = self.hover == Hit::CellClose(cell.id.clone());
            let (cx, cy) = centre(&cell.close);
            p.disc((cx, cy), 8.0 * s, rgba(0.16, 0.17, 0.20, 1.0));
            p.sheen(&rect(cx - 8.0 * s, cy - 8.0 * s, 16.0 * s, 16.0 * s), 8.0 * s, 0.06);
            let arm = 2.8 * s;
            let ink = if hot { danger(1.0) } else { white(0.70) };
            p.line((cx - arm, cy - arm), (cx + arm, cy + arm), 1.3 * s, ink);
            p.line((cx - arm, cy + arm), (cx + arm, cy - arm), 1.3 * s, ink);
        }
    }

    /// App icons, each with how many windows it has when more than one.
    fn draw_badges(&self, p: &Painter, fonts: &Fonts, badges: &[AppBadge], dim: f32) {
        for badge in badges {
            match self.icons.get(&badge.hwnd.0).and_then(Option::as_ref) {
                Some(bitmap) => p.bitmap(bitmap, &badge.icon, dim),
                None => p.fill(&badge.icon, 4.0 * self.scale, white(0.18 * dim)),
            }
            if badge.count > 1 {
                p.text(&badge.count.to_string(), &fonts.small, &badge.label, white(0.75 * dim));
            }
        }
    }

    /// One of the "+" buttons, which `hit` is the hit-test result of:
    /// faint while the cursor is `near`, a round button under it, and
    /// nothing otherwise.
    fn draw_plus(&self, p: &Painter, area: &Rect, hit: Hit, near: bool) {
        let hot = self.hover == hit;
        if !hot && !near {
            return;
        }
        let s = self.scale;
        let (cx, cy) = centre(area);
        let half = size(area).0.min(size(area).1) / 2.0;
        if hot {
            let arm = half * 0.30;
            p.disc((cx, cy), half * 0.72, white(0.12));
            p.line((cx - arm, cy), (cx + arm, cy), 1.6 * s, white(0.92));
            p.line((cx, cy - arm), (cx, cy + arm), 1.6 * s, white(0.92));
        } else {
            let arm = half * 0.20;
            p.line((cx - arm, cy), (cx + arm, cy), 1.2 * s, white(0.24));
            p.line((cx, cy - arm), (cx, cy + arm), 1.2 * s, white(0.24));
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
                p.fill(&rect(row.header.left, line - 1.5 * s, right - row.header.left, 3.0 * s), 1.5 * s, accent(0.9));
            }
        }
        if let Some(id) = self.dragged_cell() {
            match self.slot_at(id, self.cursor.0, self.cursor.1) {
                Some(Slot::Row { row, x, .. }) => {
                    let band = &map.layout.rows[row];
                    let body = band.cells.first().map_or(band.plus, |c| c.body);
                    p.fill(&rect(x - 1.5 * s, body.top, 3.0 * s, size(&body).1), 1.5 * s, accent(0.9));
                }
                Some(Slot::NewRow { y, .. }) => {
                    let spine = &map.layout.spine;
                    p.fill(&rect(spine.left, y - 1.5 * s, size(spine).0, 3.0 * s), 1.5 * s, accent(0.9));
                }
                None => {}
            }
            let held = rect(cx - 40.0 * s, cy - 25.0 * s, 80.0 * s, 50.0 * s);
            if let Some(glows) = &self.glows {
                p.glow(&glows.accent, &held, 24.0 * s, 0.40);
            }
            self.surface(p, &held, 9.0 * s, 0.17, 0.92, 1.0);
        }
        if let Some(hwnd) = self.dragged_window() {
            // With no thumbnail to follow the cursor (it could not be
            // registered), the window's icon does.
            if !self.thumbs.iter().any(|&(h, _)| h == hwnd) {
                if let Some(bitmap) = self.icons.get(&hwnd).and_then(Option::as_ref) {
                    p.bitmap(bitmap, &rect(cx - 16.0 * s, cy - 16.0 * s, 32.0 * s, 32.0 * s), 1.0);
                }
            }
        }
    }

    /// Draws the board (without live thumbnails) into a bitmap and returns the
    /// BGRA pixels, for reviewing the layout without showing a window.
    /// With `step` the selection has just come from that cell to the current
    /// one, as by an arrow key, and the picture is that many milliseconds
    /// into the motion.
    #[allow(clippy::too_many_arguments)]
    pub fn render_to_pixels(&mut self, model: Model, grid: Grid, size: (i32, i32), scale: f32, hover: Option<grid::Pos>, glide: Option<(grid::Pos, u64)>, step: Option<(grid::Pos, u64)>) -> Option<Vec<u8>> {
        self.size = size;
        self.scale = scale;
        self.home = rect(0.0, 0.0, size.0 as f32, size.1 as f32);
        self.monitors = vec![self.home];
        self.selected = model.current.clone();
        self.model = model;
        self.source_sizes.borrow_mut().clear();
        self.grid = grid;
        let now = Instant::now();
        if let Some((from, ms)) = step.filter(|(from, _)| self.grid.id_at(*from).is_some()) {
            let (current, grid) = (self.selected.clone(), self.grid.clone());
            self.selected = self.grid.id_at(from)?.clone();
            self.grid.visit(&self.selected);
            self.relayout();
            let (before, strip_before) = (self.marks(), self.strip_marks.clone());
            (self.selected, self.grid) = (current, grid);
            self.relayout();
            self.motion.moved(&before, &self.marks(), now);
            let mut at = now;
            while at < now + Duration::from_millis(ms) {
                at = (at + Duration::from_millis(MOTION_MS as u64)).min(now + Duration::from_millis(ms));
                self.motion.advance(HashMap::new(), 0, at);
            }
            // The strip `ms` into the move, with the cells beside both.
            self.strip = Strip::default();
        self.strip_frame = None;
        self.strip_input = None;
        self.strip_samples.clear();
            self.strip.moved(&strip_before, &self.strip_marks, 0.0);
            self.strip_at = ms as f32 / 1000.0;
            self.strip_on = self.strip.active(self.strip_at);
            if self.strip_on {
                self.lay_out_strip(Some(from));
            }
            debug_log!("board: rendered {ms} ms into a move, {:?}, strip {:?}", self.motion.rest, self.strip);
        } else {
            self.settle_strip(now);
            self.relayout();
        }
        let point_at = |board: &mut Self, pos: Option<grid::Pos>| {
            board.hover = Hit::Nothing;
            if let Some(cell) = pos.and_then(|pos| board.maps[0].layout.rows.get(pos.row)?.cells.get(pos.col)) {
                board.cursor = centre(&cell.body);
                board.hover = Hit::Cell(cell.id.clone());
            }
        };
        // A cell pointed at is shown in the preview area, `ms` into its
        // going back (all the way without `glide`).
        self.end_peek();
        point_at(self, hover);
        self.update_peek(now);
        let ms = glide.map_or(PEEK_MS as u64, |(_, ms)| ms);
        self.advance_peek(now + Duration::from_millis(ms));
        self.hints = std::env::var_os("FLICK_HINTS").is_some();
        self.fonts = None;
        let _ = self.draw_to_canvas()?;
        Some(self.canvas.as_ref()?.dib.pixels().to_vec())
    }
}

fn unregister_strip_thumbnails(thumbs: &mut Vec<(CellId, isize, isize)>) {
    for (_, _, thumb) in thumbs.drain(..) {
        unsafe {
            let _ = crate::composition::unregister(thumb);
        }
    }
}

fn unregister_badges(badges: &mut HashMap<isize, isize>) {
    for (_, badge) in badges.drain() {
        unsafe { let _ = crate::composition::unregister(badge); }
    }
}

fn unregister_thumbnails(thumbs: &mut Vec<(isize, isize)>) {
    for (_, thumb) in thumbs.drain(..) {
        unsafe { let _ = crate::composition::unregister(thumb); }
    }
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
        // As the normal view's banner has it.
        label: make(13.5, 600, DWRITE_TEXT_ALIGNMENT_CENTER)?,
        small: make(12.0, 400, DWRITE_TEXT_ALIGNMENT_LEADING)?,
        centered: make(12.0, 400, DWRITE_TEXT_ALIGNMENT_CENTER)?,
        tiny: make(10.0, 600, DWRITE_TEXT_ALIGNMENT_CENTER)?,
        empty: make(15.0, 400, DWRITE_TEXT_ALIGNMENT_CENTER)?,
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

/// The window's icon, `size` pixels square, premultiplied.
pub(crate) fn icon_pixels(hwnd: HWND, size: i32) -> Option<Vec<u8>> {
    let icon = window_icon(hwnd)?;
    unsafe {
        let mut dib = Dib::new((size, size)).ok()?;
        DrawIconEx(dib.dc, 0, 0, icon, size, size, 0, None, DI_NORMAL).ok()?;
        // Finish GDI drawing before reading the bitmap's memory.
        let _ = GdiFlush();
        // Icons without an alpha channel leave alpha at zero; make what they drew opaque.
        let pixels = dib.pixels_mut();
        if pixels.chunks_exact(4).all(|p| p[3] == 0) {
            for p in pixels.chunks_exact_mut(4).filter(|p| p[0] | p[1] | p[2] != 0) {
                p[3] = 255;
            }
        }
        Some(pixels.to_vec())
    }
}

/// An icon's `pixels` as a Direct2D bitmap of `size` pixels.
pub(crate) fn icon_bitmap(target: &ID2D1RenderTarget, pixels: &[u8], size: i32) -> Option<ID2D1Bitmap> {
    unsafe {
        let (bits, pitch) = (pixels.as_ptr() as *const std::ffi::c_void, size as u32 * 4);
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

/// Not narrower than the line under a preview needs for its icon and buttons.
fn tile_least(s: f32) -> f32 {
    120.0 * s
}

/// An app's icon on the lower left corner of the preview `picture`, a
/// little over its edges, as Mission Control has it.
fn corner_icon(picture: &Rect, s: f32) -> Rect {
    let side = 32.0 * s;
    rect(picture.left - 6.0 * s, picture.bottom - side + 10.0 * s, side, side)
}

/// An app's icon large in the middle of `picture`, for a preview that
/// tells little.
fn middle_icon(picture: &Rect, s: f32) -> Rect {
    let (w, h) = size(picture);
    let side = (w.min(h) * 0.42).min(64.0 * s);
    let (cx, cy) = centre(picture);
    rect(cx - side / 2.0, cy - side / 2.0, side, side)
}

/// Where in a tile's `slot` the window of `source` shows, without the
/// borders round it that DWM gives too (`strip::fit_source`).
fn shown_frame(slot: &Rect, source: &ThumbnailSource) -> Rect {
    let full = strip::fit_source(slot, &source.frame, source.full);
    let k = (full.right - full.left) / source.full.0.max(1.0);
    Rect {
        left: full.left + source.frame.left * k,
        top: full.top + source.frame.top * k,
        right: full.left + source.frame.right * k,
        bottom: full.top + source.frame.bottom * k,
    }
}

/// A window's tile with its preview at `x, y`, `w` by `h`, its app's icon
/// on the preview's lower left corner, the line under it holding its title
/// (after resting on it a while) and, at the right, its buttons. One kept
/// above the others stands a little higher.
fn tile_at(window: &WindowModel, x: f32, y: f32, w: f32, h: f32, s: f32) -> TileLayout {
    let y = if window.topmost { y - 4.0 * s } else { y };
    let foot = FOOT * s;
    let (line, button) = (22.0 * s, 24.0 * s);
    let bottom = y + h;
    let by = bottom + (foot - button) / 2.0;
    let close = rect(x + w - button, by, button, button);
    let pin = rect(close.left - 4.0 * s - button, by, button, button);
    let top = rect(pin.left - 4.0 * s - button, by, button, button);
    let icon = corner_icon(&rect(x, y, w, h), s);
    TileLayout {
        hwnd: window.hwnd,
        frame: rect(x, y, w, h + foot),
        title: rect(icon.right + 6.0 * s, bottom + (foot - line) / 2.0, (x + w - icon.right - 6.0 * s).max(0.0), line),
        icon,
        thumb: rect(x, y, w, h),
        top,
        pin,
        close,
        icon_only: false,
    }
}

/// Importance sets area; real window dimensions supply only the aspect ratio.
/// All previews share a fit scale and keep the minimum width (see `tiles`).
fn layout_tiles(windows: &[(&WindowModel, bool)], area: &Rect, s: f32, weights: &HashMap<isize, f32>) -> Vec<TileLayout> {
    let (aw, ah) = size(area);
    let (foot, gap) = (FOOT * s, 26.0 * s);
    let sizes: Vec<(f32, f32)> = windows.iter().map(|(w, _)| {
        let aspect = aspect(w);
        let area = (aw * ah).max(1.0) * weights.get(&w.hwnd.0).copied().unwrap_or(1.0);
        ((area * aspect).sqrt(), (area / aspect).sqrt())
    }).collect();
    let min_widths: Vec<f32> = vec![tile_least(s); windows.len()];
    let space = Space {
        area: (aw, ah),
        title: foot,
        gap,
        max: (aw * TILE_MAX.0, (TILE_MAX.1 * s).min(ah - foot).max(1.0)),
        min_height: TILE_MIN_HEIGHT * s,
    };
    // The line of each is under its preview, where `arrange` keeps room for a title.
    tiles::arrange(&sizes, &min_widths, &space)
        .into_iter()
        .zip(windows)
        .map(|(at, &(window, _))| tile_at(window, area.left + at.x, area.top + at.y, at.w, at.h, s))
        .collect()
}

fn aspect(window: &WindowModel) -> f32 {
    (window.rect.right - window.rect.left).max(1) as f32 / (window.rect.bottom - window.rect.top).max(1) as f32
}

/// The windows of a place of the pinned ones, which is of one size
/// (`tiles::fill_slot`): previews while they come out large enough, the
/// apps' icons where they would not, and when even those do not fit, the
/// last cell standing for the rest.
fn layout_slot(windows: &[(&WindowModel, bool)], part: &Rect, s: f32) -> (Vec<TileLayout>, Option<(Rect, Vec<HWND>, usize)>) {
    let rules = tiles::SlotRules { gap: 16.0 * s, foot: FOOT * s, least: (110.0 * s, 62.0 * s), icon: (24.0 * s, 56.0 * s) };
    let fill = tiles::fill_slot(windows.len(), size(part), &rules);
    let at = |(x, y, w, h): tiles::Part| rect(part.left + x, part.top + y, w, h);
    let shown = fill.cells.len() - if fill.more > 0 { 1 } else { 0 };
    let tiles = fill.cells.iter().zip(windows).take(shown).map(|(cell, (window, _))| {
        let cell = at(*cell);
        if fill.previews {
            // As large as fits in the cell above its line, of its own shape.
            let (cw, ch) = (size(&cell).0, size(&cell).1 - FOOT * s);
            let w = cw.min(ch * aspect(window));
            let h = w / aspect(window);
            tile_at(window, cell.left + (cw - w) / 2.0, cell.top + (ch - h) / 2.0, w, h, s)
        } else {
            let none = Rect::default();
            TileLayout { hwnd: window.hwnd, frame: cell, icon: cell, title: none, thumb: none, top: none, pin: none, close: none, icon_only: true }
        }
    }).collect();
    let more = (fill.more > 0).then(|| {
        let rest: Vec<HWND> = windows[shown..].iter().take(3).map(|(w, _)| w.hwnd).collect();
        (at(fill.cells[shown]), rest, fill.more)
    });
    (tiles, more)
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

/// Lays out the map at the bottom of a monitor: rows top to bottom, each
/// shifted sideways so that its landing cell (`anchors[row]`) sits on a common
/// column, the spine. `size` is the space above the footer; `s` the scale.
///
/// Where the names and the spine are does not depend on the anchors while
/// there is room: room is kept for any row to be shifted as far as it can
/// go, so that moving about only slides rows under a still spine. Only rows
/// too long for that are packed by their anchors, and the names then move.
/// Each row of the map is shifted so that the cell it was last turned to
/// (where a vertical move lands) sits in the same column for all rows; the
/// selected row by the selection, which may not be that cell when the board
/// was left without going to it and Windows stayed where it was.
fn map_anchors(grid: &Grid, selected: &str) -> Vec<usize> {
    let at = grid.find(selected);
    grid.rows.iter().enumerate().map(|(r, row)| match at {
        Some(at) if at.row == r => at.col,
        _ => row.anchor(),
    }).collect()
}

fn layout_map(model: &Model, anchors: &[usize], size: (f32, f32), s: f32) -> MapLayout {
    let (w, h) = size;
    let (margin, header_w, gap) = (60.0 * s, 170.0 * s, 10.0 * s);
    /// Share of the monitor's height the map may take.
    const MAX_SHARE: f32 = 0.34;

    let anchor = |r: usize| anchors.get(r).copied().unwrap_or(0);
    let rows = model.rows.len().max(1) as f32;
    let fit_w = |columns: usize| (w - 2.0 * margin - header_w) / (columns as f32 + 1.2) - gap;
    let fit_h = (h * MAX_SHARE) / (rows + 1.2) - gap;
    // Squares of a fixed look, shrunk only when the grid would not fit.
    let natural = (62.0 * s).min(fit_h);
    let longest = model.rows.iter().map(|row| row.cells.len()).max().unwrap_or(1).max(1);
    let (lead, tail) = if fit_w(2 * longest - 1) * 0.62 >= natural {
        (longest - 1, longest)
    } else {
        let lead = (0..model.rows.len()).map(anchor).max().unwrap_or(0);
        let tail = model.rows.iter().enumerate().map(|(r, row)| row.cells.len().saturating_sub(anchor(r))).max().unwrap_or(1).max(1);
        (lead, tail)
    };
    let columns = (lead + tail) as f32;
    let cell_h = natural.min(fit_w(lead + tail) * 0.62).max(22.0 * s);
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
    let name_x = x0;

    let mut layout = MapLayout { top: y, bounds: rect(x0, y, total_w, total_h), ..MapLayout::default() };
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
        // Room after the name for the dot of the windows pinned to the row.
        let name_room = header.right - name_x - 22.0 * s;
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
            pinned: row.cells.iter().flat_map(|c| &c.windows).any(|w| w.follows),
        });
        y += row_pitch;
    }
    layout.spine = rect(spine_x - gap / 2.0, rows_top - gap / 2.0, pitch, (y - rows_top).max(0.0));
    layout.add_row = rect(spine_x + (cell_w - plus) / 2.0, y - gap / 2.0 + 2.0 * s, plus, plus);
    (layout.pitch, layout.row_pitch) = (pitch, row_pitch);
    layout
}

/// Where a dragged window was released, as `window_drop` judges it.
struct DropSpot<'a> {
    /// The place of the pinned windows it is in.
    zone: Option<Pin>,
    /// Over a map (on a cell or not).
    on_map: bool,
    /// Among the previews.
    on_plate: bool,
    /// The map cell under it.
    cell: Option<&'a str>,
    /// On another monitor than the window's.
    other_monitor: bool,
}

#[derive(Debug, PartialEq)]
enum WindowDrop {
    Nothing,
    Pin(Pin),
    /// To another cell, and to the monitor it was dropped on.
    Move { cell: Option<CellId>, monitor: bool },
}

/// What becomes of a window of cell `from`, pinned `pin`, released at
/// `spot`: in a place of the pinned windows it is pinned so; on a map cell
/// it goes to that cell, on the monitor whose map that was; elsewhere
/// among the previews it is no longer pinned, or only changes monitor.
fn window_drop(from: &str, pin: Pin, spot: &DropSpot) -> WindowDrop {
    let set = |to: Pin| if to == pin { WindowDrop::Nothing } else { WindowDrop::Pin(to) };
    if let Some(to) = spot.zone {
        return set(to);
    }
    // On every desktop it has no cell to go to.
    if pin == Pin::Everywhere {
        return if spot.on_map { WindowDrop::Nothing } else { set(Pin::Off) };
    }
    if pin == Pin::Row && !spot.on_map && spot.on_plate {
        return set(Pin::Off);
    }
    let cell = spot.cell.filter(|id| *id != from).map(CellId::from);
    if cell.is_none() && !spot.other_monitor {
        WindowDrop::Nothing
    } else {
        WindowDrop::Move { cell, monitor: spot.other_monitor }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::Row;

    fn window(hwnd: isize, follows: bool) -> WindowModel {
        WindowModel { hwnd: HWND(hwnd), rect: RECT { right: 1280, bottom: 720, ..Default::default() },
            follows, pin_from_app: false, minimized: false, title: format!("Window {hwnd}"), app: String::new(),
            icon_key: 0, app_name: String::new(), topmost: false }
    }

    #[test]
    fn a_desktop_is_empty_with_only_pinned_windows_or_none() {
        let mut rows = vec![row(&["a", "b", "c"])];
        rows[0].cells[0].windows = vec![window(1, true)];
        rows[0].cells[1].windows = vec![window(2, true), window(3, false)];
        let model = Model { rows, pinned: vec![window(4, false)], current: "a".into() };
        assert_eq!(["a", "b", "c", "gone"].map(|id| model.has_no_own_windows(id)), [true, false, true, false]);
    }

    #[test]
    fn tiles_keep_row_followers_and_windows_on_every_desktop() {
        let mut rows = vec![row(&["a", "b"]), row(&["c"])];
        rows[0].cells[0].windows = vec![window(1, false), window(2, true)];
        rows[0].cells[1].windows = vec![window(3, false), window(4, true)];
        rows[1].cells[0].windows = vec![window(5, true)];
        let model = Model { rows, pinned: vec![window(6, false)], current: "a".into() };
        let handles = |id| model.tile_windows(id).iter().map(|(window, all)| (window.hwnd.0, *all)).collect::<Vec<_>>();
        assert_eq!(handles("a"), [(1, false), (2, false), (4, false), (6, true)]);
        assert_eq!(handles("b"), [(2, false), (3, false), (4, false), (6, true)]);
        assert_eq!(handles("c"), [(5, false), (6, true)]);
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
        let all = layout_tiles(&model.tile_windows("a"), &area, 1.0, &weights);
        assert_eq!(all.len(), 3);
        assert!(all.iter().all(|tile| size(&tile.frame).0 >= tile_least(1.0)));
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
                    pin_from_app: false,
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

    /// With rows of a few cells, the names and the spine stay where they
    /// are wherever the rows are lined up: moving about only slides rows.
    #[test]
    fn names_and_spine_stay_while_rows_slide() {
        let rows = vec![row(&["a", "b", "c"]), row(&["d", "e", "f", "g"]), row(&["h"])];
        let model = Model { rows, pinned: Vec::new(), current: "a".into() };
        for (size, s) in [((1920.0, 1080.0 - FOOTER), 1.0), ((1280.0, 720.0 - FOOTER), 1.0), ((2560.0, 1440.0 - FOOTER * 1.5), 1.5)] {
            let layouts: Vec<MapLayout> = [[0, 0, 0], [2, 0, 0], [1, 3, 0], [0, 3, 0]].iter().map(|anchors| layout_map(&model, anchors, size, s)).collect();
            let first = &layouts[0];
            for layout in &layouts[1..] {
                assert_eq!(layout.spine, first.spine, "the spine moved at {size:?}");
                for (row, was) in layout.rows.iter().zip(&first.rows) {
                    assert_eq!((row.header, row.name), (was.header, was.name), "a name moved at {size:?}");
                }
                assert_eq!(layout.pitch, first.pitch);
            }
            // Going one cell right slides the row one cell left under the spine.
            let (before, after) = (&layouts[0], layout_map(&model, &[1, 0, 0], size, s));
            assert_eq!(after.rows[0].cells[0].body.left, before.rows[0].cells[0].body.left - before.pitch);
            assert_eq!(after.rows[0].cells[1].body.left, before.rows[0].cells[0].body.left);
            assert!(after.rows[0].cells[1].body.left >= after.spine.left);
        }
    }

    /// Rows too long to keep room for every shift are packed by their
    /// anchors instead: the names may move then, and the map still fits.
    #[test]
    fn long_rows_are_packed_and_still_fit() {
        let ids: Vec<String> = (0..14).map(|i| format!("c{i}")).collect();
        let long: Vec<&str> = ids.iter().map(String::as_str).collect();
        let model = Model { rows: vec![row(&long), row(&["x"])], pinned: Vec::new(), current: "c0".into() };
        let size = (1280.0, 720.0 - FOOTER);
        for anchors in [[0, 0], [7, 0], [13, 0]] {
            let layout = layout_map(&model, &anchors, size, 1.0);
            let right = layout.rows.iter().map(|row| row.plus.right).fold(0.0, f32::max);
            let left = layout.rows.iter().map(|row| row.header.left.min(row.plus_left.left)).fold(f32::MAX, f32::min);
            assert!(left >= 0.0 && right <= size.0, "off the screen with anchors {anchors:?}: {left}..{right}");
        }
    }

    #[test]
    fn spring_is_critical_and_arrives_without_overshoot() {
        for offset in [-200.0, 200.0] {
            let spring = Spring { offset, velocity: 0.0 };
            let mut previous = offset.abs();
            for ms in 0..1000 {
                let t = ms as f32 / 1000.0;
                let at = spring.sample(t);
                let expected = offset * (1.0 + MOTION_OMEGA * t) * (-MOTION_OMEGA * t).exp();
                assert!((at.offset - expected).abs() < 0.0001);
                assert!(at.offset * offset >= 0.0 && at.offset.abs() <= previous + 0.0001);
                previous = at.offset.abs();
            }
            let arrived = 1.0 - spring.sample(0.2).offset / offset;
            assert!((0.975..0.985).contains(&arrived), "{arrived}");
        }
    }

    #[test]
    fn spring_integration_is_independent_of_frame_subdivision() {
        for velocity in [-800.0, 0.0, 400.0] {
            let start = Spring { offset: 200.0, velocity };
            for frames in [1, 5, 20, 40] {
                let mut divided = start;
                for _ in 0..frames {
                    divided = divided.sample(0.016);
                }
                let single = start.sample(0.016 * frames as f32);
                assert!((single.offset - divided.offset).abs() < 0.0001);
                assert!((single.velocity - divided.velocity).abs() < 0.001);
            }
        }
        assert!(!Spring { offset: 0.0, velocity: 100.0 }.settled());
    }

    #[test]
    fn repeated_jumps_keep_the_current_position_and_velocity_on_both_axes() {
        let now = Instant::now();
        for (part, jump) in [(Part::Row(0, "a".into()), (100.0, 0.0)), (Part::Selection(0), (0.0, 100.0))] {
            let before = HashMap::from([(part.clone(), jump)]);
            let after = HashMap::from([(part.clone(), (0.0, 0.0))]);
            let mut motion = Motion::default();
            motion.moved(&before, &after, now);
            let at = now + Duration::from_millis(48);
            motion.advance(HashMap::new(), 0, at);
            let was = motion.rest[&part];
            assert!(was.iter().any(|spring| spring.velocity != 0.0));
            motion.moved(&before, &after, at);
            let is = motion.rest[&part];
            assert_eq!(is.map(|spring| spring.velocity), was.map(|spring| spring.velocity));
            assert_eq!(motion.offset(&part), (was[0].offset + jump.0, was[1].offset + jump.1));
        }
    }

    #[test]
    fn slow_frames_use_elapsed_time_then_cut_the_settled_tail() {
        let now = Instant::now();
        let row = Part::Row(0, "a".into());
        let mut motion = Motion::default();
        motion.moved(&HashMap::from([(row.clone(), (100.0, 0.0))]), &HashMap::from([(row.clone(), (0.0, 0.0))]), now);
        motion.advance(HashMap::new(), 0, now + Duration::from_millis(500));
        let expected = Spring { offset: 100.0, velocity: 0.0 }.sample(0.5).offset;
        assert!(expected < SETTLED);
        assert_eq!(motion.offset(&row).0, 0.0);
        for frame in 1..=40 {
            motion.advance(HashMap::new(), 0, now + Duration::from_millis(500 + frame * 16));
        }
        assert!(!motion.is_moving());
        assert_eq!(motion.offset(&row), (0.0, 0.0));
    }

    /// A part that stays where it was stays; one new to the layout has
    /// nothing to ease from; one that jumps again mid-way goes on from
    /// where it is drawn.
    #[test]
    fn kept_parts_stay_and_new_ones_have_nothing_to_ease_from() {
        let now = Instant::now();
        let (kept, new, moved) = (Part::Spine(0), Part::Names(0), Part::Selection(0));
        let before = HashMap::from([(kept.clone(), (10.0, 10.0)), (moved.clone(), (100.0, 0.0))]);
        let after = HashMap::from([(kept.clone(), (10.0, 10.0)), (new.clone(), (50.0, 50.0)), (moved.clone(), (60.0, 0.0))]);
        let mut motion = Motion::default();
        motion.moved(&before, &after, now);
        assert_eq!(motion.offset(&kept), (0.0, 0.0));
        assert_eq!(motion.offset(&new), (0.0, 0.0));
        assert_eq!(motion.offset(&moved), (40.0, 0.0));
        let again = HashMap::from([(moved.clone(), (0.0, 0.0))]);
        motion.moved(&after, &again, now);
        assert_eq!(motion.offset(&moved), (100.0, 0.0));
    }

    /// The row follows the hand between cells, and when the app gets to the
    /// next cell (the layout slides by a cell and the hand is counted from
    /// there) nothing jumps, in whichever order the two are heard of.
    #[test]
    fn coming_to_a_cell_while_held_does_not_jump() {
        let now = Instant::now();
        let row = Part::Row(0, "a".into());
        let pitch = 100.0;
        let drawn = |motion: &Motion, layout: f32| layout + motion.offset(&row).0;
        let mut motion = Motion::default();
        // A hold begins: the hand starts from where the row is.
        motion.advance(HashMap::new(), 1, now);
        motion.advance(HashMap::from([(row.clone(), (-0.9 * pitch, 0.0))]), 1, now);
        assert_eq!(drawn(&motion, 0.0), -0.9 * pitch);
        // The hand is past the cell; the app has not got there yet.
        motion.advance(HashMap::from([(row.clone(), (-1.02 * pitch, 0.0))]), 1, now);
        let was = drawn(&motion, 0.0);
        motion.moved(&HashMap::from([(row.clone(), (0.0, 0.0))]), &HashMap::from([(row.clone(), (-pitch, 0.0))]), now);
        assert!((drawn(&motion, -pitch) - was).abs() < 0.01);
        motion.advance(HashMap::from([(row.clone(), (-0.02 * pitch, 0.0))]), 2, now);
        assert!((drawn(&motion, -pitch) - was).abs() < 0.01);
        // Letting go: the row eases back onto the spine.
        motion.advance(HashMap::new(), 3, now);
        assert!((drawn(&motion, -pitch) - was).abs() < 0.01);
        let mut at = now;
        for _ in 0..40 {
            at += Duration::from_millis(16);
            motion.advance(HashMap::new(), 3, at);
        }
        assert_eq!(drawn(&motion, -pitch), -pitch);
    }

    #[test]
    fn hand_confirmation_is_continuous_in_either_order_on_both_axes() {
        let now = Instant::now();
        for (part, axis, sign) in [(Part::Row(0, "a".into()), 0, -1.0), (Part::Selection(0), 1, 1.0)] {
            let point = |distance| if axis == 0 { (distance, 0.0) } else { (0.0, distance) };
            let component = |motion: &Motion| if axis == 0 { motion.offset(&part).0 } else { motion.offset(&part).1 };
            for hand_first in [false, true] {
                let mut motion = Motion::default();
                motion.advance(HashMap::from([(part.clone(), point(sign * 40.0))]), 1, now);
                assert_eq!(component(&motion), sign * 40.0, "the first hand sample follows directly");
                let at = now + Duration::from_millis(16);
                motion.advance(HashMap::from([(part.clone(), point(sign * 90.0))]), 1, at);
                assert_eq!(component(&motion), sign * 90.0, "the hand follows directly");
                let confirm = at + Duration::from_millis(16);
                let moved = |motion: &mut Motion| motion.moved(
                    &HashMap::from([(part.clone(), point(0.0))]),
                    &HashMap::from([(part.clone(), point(sign * 100.0))]), confirm);
                let counted = |motion: &mut Motion| { motion.advance(
                    HashMap::from([(part.clone(), point(-sign * 10.0))]), 2, confirm); };
                if hand_first { counted(&mut motion); moved(&mut motion); }
                else { moved(&mut motion); counted(&mut motion); }
                assert!((sign * 100.0 + component(&motion) - sign * 90.0).abs() < 0.001);
                let release = confirm + Duration::from_millis(16);
                motion.advance(HashMap::new(), 3, release);
                assert!((sign * 100.0 + component(&motion) - sign * 90.0).abs() < 0.001);
                motion.advance(HashMap::new(), 3, release + Duration::from_millis(16));
                assert!(component(&motion).abs() < 10.0);
            }
        }
    }

    /// Left without going to its selection, the board opens again on the
    /// cell Windows stayed on: that row is laid out by it, the others by the
    /// cell each was last turned to.
    #[test]
    fn the_selected_row_lies_on_the_spine_even_off_its_anchor() {
        let mut grid = Grid { rows: vec![
            Row { cells: vec!["a1".into(), "a2".into(), "a3".into()], ..Default::default() },
            Row { cells: vec!["b1".into(), "b2".into()], ..Default::default() },
        ], ..Default::default() };
        grid.visit("a3");
        grid.visit("b2");
        assert_eq!(map_anchors(&grid, "a1"), [0, 1]);
        assert_eq!(map_anchors(&grid, "a3"), [2, 1]);
    }

    #[test]
    fn the_hand_stops_at_every_edge() {
        let grid = Grid { rows: vec![
            Row { cells: vec!["a".into(), "b".into()], ..Default::default() },
            Row::default(),
            Row { cells: vec!["c".into()], ..Default::default() },
        ], ..Default::default() };
        assert_eq!(hand_pull(&grid, Pos { row: 0, col: 0 }, (-100.0, -100.0)), (0.0, 0.0));
        assert_eq!(hand_pull(&grid, Pos { row: 0, col: 1 }, (100.0, 100.0)), (0.0, 0.0));
        assert_eq!(hand_pull(&grid, Pos { row: 2, col: 0 }, (100.0, 100.0)), (0.0, 0.0));
        assert_eq!(hand_pull(&grid, Pos { row: 0, col: 1 }, (-0.125, 0.0)), (-0.125, 0.0));
    }

    /// Going to a row with more or fewer windows pinned to it, or none, the
    /// places of the pinned windows stay as they are, and so do the tiles of
    /// the windows on every desktop.
    #[test]
    fn windows_on_every_desktop_stay_whichever_row() {
        let everywhere = window(1, false);
        let (followers, own) = ([window(2, true), window(3, true)], window(4, false));
        let area = rect(40.0, 64.0, 1840.0, 640.0);
        let parts = |row: &[WindowModel]| {
            let here: Vec<(&WindowModel, bool)> = [(&everywhere, true), (&own, false)].into_iter().chain(row.iter().map(|w| (w, false))).collect();
            let parts = split_area(&here, true, &area, 1.0);
            let part = |carried| parts.iter().find(|(_, c, _)| *c == carried).map(|(part, _, group)| (*part, group.len())).unwrap();
            let (all, row_part) = (part(Carried::Fixed), part(Carried::Row));
            let tiles = layout_slot(&[(&everywhere, true)], &all.0, 1.0).0;
            (all.0, row_part.0, tiles[0].frame, row_part.1)
        };
        let (none, one, two) = (parts(&[]), parts(&followers[..1]), parts(&followers));
        assert_eq!((none.0, none.1, none.2), (two.0, two.1, two.2));
        assert_eq!((one.0, one.1, one.2), (two.0, two.1, two.2));
        assert_eq!((none.3, one.3, two.3), (0, 1, 2));
    }

    #[test]
    fn a_dropped_window_goes_to_the_cell_or_the_pin_it_was_released_on() {
        let at = |zone, on_map, on_plate, cell| DropSpot { zone, on_map, on_plate, cell, other_monitor: false };
        let moved = |cell: &str| WindowDrop::Move { cell: Some(cell.to_string()), monitor: false };
        // On another cell of the map it goes there; on its own, nowhere.
        assert_eq!(window_drop("a", Pin::Off, &at(None, true, false, Some("b"))), moved("b"));
        assert_eq!(window_drop("a", Pin::Row, &at(None, true, false, Some("b"))), moved("b"));
        assert_eq!(window_drop("a", Pin::Off, &at(None, true, false, Some("a"))), WindowDrop::Nothing);
        // In a place of the pinned windows it is pinned so.
        assert_eq!(window_drop("a", Pin::Off, &at(Some(Pin::Everywhere), false, false, None)), WindowDrop::Pin(Pin::Everywhere));
        assert_eq!(window_drop("a", Pin::Off, &at(Some(Pin::Row), false, false, None)), WindowDrop::Pin(Pin::Row));
        assert_eq!(window_drop("a", Pin::Row, &at(Some(Pin::Row), false, false, None)), WindowDrop::Nothing);
        // Anywhere else nothing happens, but a pinned one is let go among the previews.
        assert_eq!(window_drop("a", Pin::Off, &at(None, false, true, None)), WindowDrop::Nothing);
        assert_eq!(window_drop("a", Pin::Off, &at(None, false, false, None)), WindowDrop::Nothing);
        assert_eq!(window_drop("a", Pin::Row, &at(None, false, true, None)), WindowDrop::Pin(Pin::Off));
        assert_eq!(window_drop("a", Pin::Everywhere, &at(None, true, false, Some("b"))), WindowDrop::Nothing);
        // Over another monitor's map, it only changes monitor.
        let other = DropSpot { other_monitor: true, ..at(None, true, false, None) };
        assert_eq!(window_drop("a", Pin::Off, &other), WindowDrop::Move { cell: None, monitor: true });
    }
}
