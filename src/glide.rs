//! The screen following the hand between desktops: while a hold moves, a
//! picture of the desktops slides along with it, and on letting go it
//! settles on one, which the app then switches to, once (`follow`).
//!
//! The picture is made of live DWM thumbnails of the windows of the desktop
//! the hold began on and of the desktops around it, over the shell's own
//! desktop window (wallpaper and icons) for each, in one click-through
//! window per monitor above everything but the minimap. Only where each
//! thumbnail is drawn changes from frame to frame. Windows shown on every
//! desktop stay where they are; those that go along their row (`App::follows`)
//! stay put sideways and go up and down with their row.
//!
//! Only the row the hand is on goes sideways (`Follow::offset`). The desktop
//! in the middle stands out: the others are darkened by thumbnails of a
//! black window over them and it has a thin edge of thumbnails of a white
//! one (`follow::standing_out`). Above the picture the name of the row is
//! shown while receded, in a small window of its own (`Banner`).
//!
//! With `FLICK_TRACE` set to a file, each frame is written to it as a line
//! of CSV (`Trace`), kept in memory until the picture goes.
//!
//! It runs on a thread of its own, as the minimap does: switching desktops
//! keeps the app's thread busy for tens of milliseconds at a time.

use crate::{
    follow::{self, Follow, Label, Name, Step},
    paint::{self, rgba, white, DcTarget, Dib, Painter},
};
use std::{
    io::Write,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use windows::{
    core::{w, PCWSTR},
    Win32::{
        Foundation::{COLORREF, FILETIME, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM},
        Graphics::{
            Direct2D::{Common::D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE},
            DirectWrite::{IDWriteFactory, IDWriteTextFormat, DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_METRICS},
            Dwm::{
                DwmFlush, DwmQueryThumbnailSourceSize, DwmRegisterThumbnail, DwmUnregisterThumbnail,
                DwmUpdateThumbnailProperties, DWM_THUMBNAIL_PROPERTIES, DWM_TNP_OPACITY, DWM_TNP_RECTDESTINATION,
                DWM_TNP_RECTSOURCE, DWM_TNP_SOURCECLIENTAREAONLY, DWM_TNP_VISIBLE,
            },
            Gdi::{
                GetStockObject, AC_SRC_ALPHA, AC_SRC_OVER, BLACK_BRUSH, BLENDFUNCTION, HBRUSH, WHITE_BRUSH,
            },
        },
        System::{
            Com::{CoInitializeEx, COINIT_MULTITHREADED},
            LibraryLoader::GetModuleHandleW,
            Performance::{QueryPerformanceCounter, QueryPerformanceFrequency},
            Threading::{
                GetCurrentProcessId, GetCurrentThread, GetThreadTimes,
            },
        },
        UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, EnumWindows, FindWindowW, GetClassNameW,
            GetSystemMetrics, GetWindowRect, GetWindowThreadProcessId, PeekMessageW, PostMessageW, RegisterClassW,
            SetLayeredWindowAttributes, SetWindowPos, ShowWindow, UpdateLayeredWindow, HWND_TOPMOST, LWA_ALPHA, MSG,
            PM_REMOVE, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SWP_HIDEWINDOW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
            SWP_NOZORDER, SWP_ASYNCWINDOWPOS, SWP_SHOWWINDOW, SW_SHOWNOACTIVATE, ULW_ALPHA, WM_APP, WNDCLASSW, WS_EX_LAYERED,
            WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
        },
    },
};

/// Told to the app when a hold is let go of: the desktop the picture
/// settles on, sideways and up and down from the one it began on
/// (`target_of`). lparam: the hold.
pub const WM_GLIDE_TARGET: u32 = WM_APP + 30;
/// The picture has settled there: the app switches to it, and says when
/// Windows is there (`Glide::arrived`). As `WM_GLIDE_TARGET`.
pub const WM_GLIDE_SETTLED: u32 = WM_APP + 31;

/// At most this many window thumbnails, the nearest desktops first: the
/// rest of the desktops show their wallpaper alone.
pub const MOST_THUMBNAILS: usize = 60;
/// How long the picture waits for Windows to be on the desktop it settled
/// on before going anyway.
const ARRIVAL_WAIT: Duration = Duration::from_millis(1500);

const WINDOW_CLASS: PCWSTR = w!("flick.glide");
const WHITE_CLASS: PCWSTR = w!("flick.glide.white");
/// Desktops in sight at once, at most: three each way with the screen receded.
const WALLS: usize = 9;
/// Desktops with an edge of light at once, at most: the four the middle of
/// the screen is between.
const LIT: usize = 4;
/// The name of the row above the picture: its window's size and its place
/// below the top of the screen, unscaled, and the size of its letters.
const BANNER: (f32, f32) = (560.0, 30.0);
const BANNER_TOP: f32 = 14.0;
const BANNER_TEXT: f32 = 13.0;
/// Room either side of the name in its box.
const BANNER_PAD: f32 = 14.0;
/// Up and down travel after which windows going along their row recede and
/// slide with it, as the rest does; before, they stay where they are.
const ROW_WINDOWS_JOIN: f64 = 0.15;

/// Where a window is drawn as the picture moves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Place {
    /// On the desktop that many rows down and cells along from the first
    /// (along from the cell a move to that row lands on).
    Cell(i32, i32),
    /// Going along its row, `Cell`'s row: up and down with it, never sideways.
    Row(i32),
    /// Shown on every desktop: where it is.
    Fixed,
}

pub struct Shot {
    pub hwnd: HWND,
    /// Its visible frame on the screen (`DWMWA_EXTENDED_FRAME_BOUNDS`).
    pub rect: RECT,
    pub place: Place,
}

/// What a hold shows: the desktops around the one it began on, and their
/// windows, bottom first.
pub struct Scene {
    pub hold: u32,
    pub origin: crate::grid::Pos,
    pub anchors: Vec<usize>,
    /// The rows up and down from the first, and for each the cells along
    /// it (`Follow::new`).
    pub rows: (i32, i32),
    pub cols: Vec<(i32, i32)>,
    /// How firm the notches are each way (`follow::firmness`).
    pub firm: (f64, f64),
    pub shots: Vec<Shot>,
    /// The name of each row from `rows.0` to `rows.1`.
    pub names: Vec<String>,
}

enum Request {
    Begin(Scene),
    /// A single step to a desktop (a hotkey), or one more upon the step
    /// under way (`Retarget`).
    Step(Scene, (i32, i32)),
    Retarget(u32, (i32, i32)),
    /// The app went to another desktop a way the hand is not followed: the
    /// picture goes on from there.
    Rebase(Scene),
    Arrived(u32),
    Cancel,
}

/// The desktop a hold settled on, once decided: hold << 32 | row << 16 |
/// cell, for the app to finish it at once when the next hold begins.
static DECIDED: AtomicU64 = AtomicU64::new(0);

fn pack((col, row): (i32, i32)) -> usize {
    (row as i16 as u16 as usize) << 16 | col as i16 as u16 as usize
}

/// The cell and the row in the wparam of `WM_GLIDE_TARGET` and `WM_GLIDE_SETTLED`.
pub fn target_of(wparam: usize) -> (i32, i32) {
    (wparam as u16 as i16 as i32, (wparam >> 16) as u16 as i16 as i32)
}

/// Where `hold` was decided to settle, if it has been.
pub fn decided(hold: u32) -> Option<(i32, i32)> {
    let decided = DECIDED.load(Ordering::SeqCst);
    ((decided >> 32) as u32 == hold && hold != 0).then(|| target_of(decided as u32 as usize))
}

/// The picture's thread.
pub struct Glide {
    requests: mpsc::Sender<Request>,
}

impl Glide {
    /// `app`: the window told of `WM_GLIDE_TARGET` and `WM_GLIDE_SETTLED`.
    pub fn spawn(app: HWND) -> Glide {
        let (requests, inbox) = mpsc::channel();
        let app = app.0;
        std::thread::Builder::new()
            .name("glide".into())
            .spawn(move || run(HWND(app), inbox))
            .map_err(|e| crate::app::log(&format!("the sliding picture could not start: {e}")))
            .ok();
        Glide { requests }
    }

    pub fn begin(&self, scene: Scene) {
        let _ = self.requests.send(Request::Begin(scene));
    }

    /// One step to `to` (sideways, up and down from the scene's desktop).
    pub fn step(&self, scene: Scene, to: (i32, i32)) {
        let _ = self.requests.send(Request::Step(scene, to));
    }

    /// The step `hold` goes on to `to` instead, from where it is.
    pub fn retarget(&self, hold: u32, to: (i32, i32)) {
        let _ = self.requests.send(Request::Retarget(hold, to));
    }

    pub fn rebase(&self, scene: Scene) {
        let _ = self.requests.send(Request::Rebase(scene));
    }

    /// Windows is on the desktop `hold` settled on (or will not be).
    pub fn arrived(&self, hold: u32) {
        let _ = self.requests.send(Request::Arrived(hold));
    }

    pub fn cancel(&self) {
        let _ = self.requests.send(Request::Cancel);
    }
}

/// A monitor's window and where it is (the monitor's work area).
struct Screen {
    hwnd: HWND,
    area: RECT,
    /// Its scaling setting: 1.0 at 100%.
    scale: f32,
    /// The wallpaper, one for each desktop that can be in sight at once
    /// (`WALLS`).
    walls: Vec<Thumb>,
    /// The name of the row above the picture.
    banner: Option<Banner>,
}

impl Screen {
    /// Out of sight at once, before anything in it goes: taking thumbnails
    /// away from a window on screen one by one shows it without them.
    fn hide(&self) {
        unsafe {
            let flags = SWP_HIDEWINDOW | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE;
            let _ = SetWindowPos(self.hwnd, None, 0, 0, 0, 0, flags);
            if let Some(banner) = &self.banner {
                let _ = SetWindowPos(banner.hwnd, None, 0, 0, 0, 0, flags);
            }
        }
    }
}

struct Thumb {
    id: isize,
    /// Where it is drawn on its screen with the picture where the hold began.
    dest: RECT,
    source: Option<RECT>,
    /// Where it was last drawn and how opaque; `None` while not drawn.
    drawn: Option<(RECT, u8)>,
}

impl Thumb {
    fn new(screen: &Screen, source_window: HWND, at: (i32, i32), source: Option<RECT>) -> Option<Thumb> {
        let id = unsafe { DwmRegisterThumbnail(screen.hwnd, source_window) }.ok()?;
        // Drawn one to one from the corner of the frame; its size is the
        // source's, which can be a pixel off the frame's.
        let size = match source {
            Some(source) => (source.right - source.left, source.bottom - source.top),
            None => match unsafe { DwmQueryThumbnailSourceSize(id) } {
                Ok(size) => (size.cx, size.cy),
                Err(_) => {
                    unsafe {
                        let _ = DwmUnregisterThumbnail(id);
                    }
                    return None;
                }
            },
        };
        let (left, top) = (at.0 - screen.area.left, at.1 - screen.area.top);
        Some(Thumb { id, dest: RECT { left, top, right: left + size.0, bottom: top + size.1 }, source, drawn: None })
    }

    fn edges(&self) -> [f64; 4] {
        [self.dest.left as f64, self.dest.top as f64, self.dest.right as f64, self.dest.bottom as f64]
    }

    /// Draws it at `at` `opacity` opaque, or not at all; only when that
    /// changed. Its size is rounded on its own, not as the difference of
    /// its rounded edges, which would make it a pixel larger or smaller
    /// from frame to frame as it slides: the picture in it would shimmer.
    fn draw(&mut self, at: Option<[f64; 4]>, size: (i32, i32), opacity: u8) -> bool {
        let rect = at.map(|[l, t, r, b]| {
            let (left, top) = (l.round() as i32, t.round() as i32);
            RECT { left, top, right: left + (r - l).round() as i32, bottom: top + (b - t).round() as i32 }
        });
        let rect = rect.filter(|r| r.right > 0 && r.bottom > 0 && r.left < size.0 && r.top < size.1 && r.right > r.left && r.bottom > r.top);
        let drawn = rect.map(|rect| (rect, opacity)).filter(|_| opacity > 0);
        if drawn == self.drawn {
            return false;
        }
        self.drawn = drawn;
        let mut props = DWM_THUMBNAIL_PROPERTIES {
            dwFlags: DWM_TNP_RECTDESTINATION | DWM_TNP_VISIBLE | DWM_TNP_OPACITY | DWM_TNP_SOURCECLIENTAREAONLY,
            rcDestination: rect.unwrap_or_default(),
            opacity,
            fVisible: drawn.is_some().into(),
            fSourceClientAreaOnly: false.into(),
            ..Default::default()
        };
        if let Some(source) = self.source {
            props.dwFlags |= DWM_TNP_RECTSOURCE;
            props.rcSource = source;
        }
        unsafe {
            let _ = DwmUpdateThumbnailProperties(self.id, &props);
        }
        true
    }
}

impl Drop for Thumb {
    fn drop(&mut self) {
        unsafe {
            let _ = DwmUnregisterThumbnail(self.id);
        }
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        self.hide();
        self.walls.clear();
        self.banner = None;
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

/// What darkens the desktops not in the middle of a screen (thumbnails of
/// a black window) and lights the edge of the one in the middle (of a white
/// one, four to a desktop), above the desktops' windows and below the ones
/// going along their row or shown on every desktop.
#[derive(Default)]
struct Veil {
    shades: Vec<Thumb>,
    edges: Vec<Thumb>,
}

/// The windows the veils are thumbnails of: plain black and white, out of
/// sight past the screens, for as long as the picture is up.
struct Paints {
    black: HWND,
    white: HWND,
}

impl Paints {
    fn new() -> Option<Paints> {
        let make = |class: PCWSTR| unsafe {
            let (x, y) = (GetSystemMetrics(SM_XVIRTUALSCREEN) - 200, GetSystemMetrics(SM_YVIRTUALSCREEN) - 200);
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT,
                class,
                w!("Flick"),
                WS_POPUP,
                x,
                y,
                16,
                16,
                None,
                None,
                GetModuleHandleW(None).ok()?,
                None,
            );
            if hwnd.0 == 0 {
                return None;
            }
            ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            let _ = crate::vdapi::pin_window(hwnd);
            Some(hwnd)
        };
        let black = make(WINDOW_CLASS)?;
        let Some(white) = make(WHITE_CLASS) else {
            unsafe {
                let _ = DestroyWindow(black);
            }
            return None;
        };
        Some(Paints { black, white })
    }
}

impl Drop for Paints {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.black);
            let _ = DestroyWindow(self.white);
        }
    }
}

/// The name of the row above the picture on one screen: a layered window
/// drawn anew only while the name changes, made more or less opaque with
/// the screen receding and coming forward.
struct Banner {
    hwnd: HWND,
    dib: Option<Dib>,
    /// What it shows and how opaque it is, as last given to the window.
    drawn: Option<(BannerKey, u8)>,
}

/// The names shown and how far each has come, in thousandths.
#[derive(Clone, Copy, PartialEq)]
struct BannerKey {
    coming: (i32, i32, i32),
    going: Option<(i32, i32, i32)>,
}

impl BannerKey {
    fn of((coming, going): (Name, Option<Name>)) -> BannerKey {
        let key = |(row, shown, slide): Name| (row, (shown * 1000.0).round() as i32, (slide * 1000.0).round() as i32);
        BannerKey { coming: key(coming), going: going.map(key) }
    }
}

impl Banner {
    fn new() -> Option<Banner> {
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT | WS_EX_LAYERED,
                WINDOW_CLASS,
                w!("Flick"),
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                GetModuleHandleW(None).ok()?,
                None,
            )
        };
        if hwnd.0 == 0 {
            return None;
        }
        let _ = crate::vdapi::pin_window(hwnd);
        Some(Banner { hwnd, dib: None, drawn: None })
    }
}

impl Drop for Banner {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

/// What the banners are drawn with, made once.
struct Ink {
    target: DcTarget,
    dwrite: IDWriteFactory,
    /// The letters for each scale seen.
    formats: Vec<(f32, IDWriteTextFormat)>,
}

impl Ink {
    fn new() -> Option<Ink> {
        let target = DcTarget::new(&paint::d2d_factory().ok()?, D2D1_ALPHA_MODE_PREMULTIPLIED).ok()?;
        Some(Ink { target, dwrite: paint::dwrite_factory().ok()?, formats: Vec::new() })
    }

    fn format(&mut self, scale: f32) -> Option<IDWriteTextFormat> {
        if let Some((_, format)) = self.formats.iter().find(|(s, _)| *s == scale) {
            return Some(format.clone());
        }
        let format = paint::text_format(&self.dwrite, BANNER_TEXT * scale, 600, DWRITE_TEXT_ALIGNMENT_CENTER).ok()?;
        self.formats.push((scale, format.clone()));
        Some(format)
    }

    /// How wide `text` is in `format`.
    fn width(&self, text: &str, format: &IDWriteTextFormat) -> f32 {
        let wide: Vec<u16> = text.encode_utf16().collect();
        unsafe {
            let Ok(layout) = self.dwrite.CreateTextLayout(&wide, format, 4000.0, 200.0) else { return 0.0 };
            let mut metrics = DWRITE_TEXT_METRICS::default();
            if layout.GetMetrics(&mut metrics).is_err() {
                return 0.0;
            }
            metrics.widthIncludingTrailingWhitespace
        }
    }

    /// Draws `names` (`Label::at`) of rows called `called` into `dib`, at `scale`.
    fn draw(&mut self, dib: &Dib, scale: f32, (coming, going): (Name, Option<Name>), called: &dyn Fn(i32) -> String) -> Option<()> {
        let format = self.format(scale)?;
        let (w, h) = (dib.size.0 as f32, dib.size.1 as f32);
        let names: Vec<(String, f64, f64)> = std::iter::once(coming).chain(going).map(|(row, shown, slide)| (called(row), shown, slide)).collect();
        // The box as wide as the names in it, going from one to the other.
        let wide = names.iter().map(|(name, shown, _)| (self.width(name, &format) + 2.0 * BANNER_PAD * scale) * *shown as f32).sum::<f32>();
        let wide = wide.clamp(h * 2.0, w - 2.0);
        let area = paint::rect((w - wide) / 2.0, 0.5, wide, h - 1.0);
        self.target.bind(dib).ok()?;
        let target = &self.target.target;
        unsafe {
            target.BeginDraw();
            target.Clear(Some(&rgba(0.0, 0.0, 0.0, 0.0)));
        }
        if let Ok(p) = Painter::new(target) {
            p.fill(&area, h / 2.0, rgba(0.06, 0.065, 0.08, 0.56));
            p.stroke(&area, h / 2.0, 1.0, white(0.10));
            unsafe {
                target.PushAxisAlignedClip(&area, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            }
            for (name, shown, slide) in &names {
                let down = *slide as f32 * h;
                let line = paint::Rect { top: area.top + down, bottom: area.bottom + down, ..area };
                p.text(name, &format, &line, white(0.92 * *shown as f32));
            }
            unsafe {
                target.PopAxisAlignedClip();
            }
        }
        unsafe { target.EndDraw(None, None) }.ok()
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Following,
    /// Let go of: settling on a desktop, then coming forward from then on.
    Settling((i32, i32), f64),
    Stepping(Step),
    /// Settled on a desktop, waiting for Windows to be there, since then.
    Arriving((i32, i32), Instant),
}

/// What frames cost, for the record (`FLICK_GLIDE_STATS`).
#[derive(Default)]
struct Stats {
    frames: u32,
    updates: u32,
    cpu: Vec<f64>,
    work: Vec<f64>,
    /// From one frame shown to the next.
    gaps: Vec<f64>,
    setup: f64,
    thumbnails: usize,
}

/// Where everything is drawn in a frame: up and down, each row sideways
/// (from `Follow::rows.0`), and how far the screen has receded.
#[derive(Clone, Debug, Default, PartialEq)]
struct Strip {
    y: f64,
    along: Vec<f64>,
    receded: f64,
}

impl Strip {
    fn along(&self, rows: (i32, i32), row: i32) -> f64 {
        usize::try_from(row - rows.0).ok().and_then(|at| self.along.get(at)).copied().unwrap_or(0.0)
    }
}

/// How the last refresh was waited for (`View::pace`).
#[derive(Clone, Copy, Default)]
struct Paced {
    /// DwmFlush said it waited, and for how long.
    flushed: bool,
    flush: f64,
    /// The timer finished the refresh, for how long.
    timer: Option<f64>,
}

/// Frame by frame, for `FLICK_TRACE`: kept in memory, written as the
/// picture goes, so that writing takes no time from the frames.
struct Trace {
    lines: Vec<String>,
    last: Option<f64>,
}

const TRACE_HEADER: &str =
    "hold,qpc_ms,gap_ms,vblank,flush_ms,timer_ms,phase,hand_x,hand_y,x,y,scale,work_ms,thumbnail_ms,moves,thumbnails";

/// The performance counter now, in ms.
fn qpc_ms() -> f64 {
    let (mut count, mut frequency) = (0i64, 0i64);
    unsafe {
        let _ = QueryPerformanceCounter(&mut count);
        let _ = QueryPerformanceFrequency(&mut frequency);
    }
    count as f64 * 1000.0 / frequency.max(1) as f64
}

struct Session {
    hold: u32,
    origin: crate::grid::Pos,
    anchors: Vec<usize>,
    follow: Follow,
    screens: Vec<Screen>,
    /// The windows' thumbnails, bottom first, and where each is drawn.
    shots: Vec<(Thumb, usize, Place)>,
    /// Each screen's.
    veils: Vec<Veil>,
    /// The thumbnails of the hold before, while this one's are not drawn
    /// yet: taken away with this one's first frame, not before, or the
    /// picture would be without windows for a moment.
    leftover: Vec<Thumb>,
    phase: Phase,
    began: Instant,
    /// Where the hand was last seen.
    hand: (f64, f64),
    replay: std::collections::VecDeque<(f64, f64, usize)>,
    /// As drawn.
    shown: Option<Strip>,
    /// Where the picture of the hold before was from where this one's is
    /// (sideways, along the first row, and up and down), eased away in
    /// `CARRY_TIME`, and how far it had receded, which this one goes on from.
    receded_from: f64,
    /// The farthest up or down the strip went (`ROW_WINDOWS_JOIN`).
    farthest_y: f64,
    names: Vec<String>,
    label: Label,
    last_frame: Option<Instant>,
    visible: bool,
    stats: Stats,
    trace: Option<Trace>,
}

impl Session {
    /// What its thumbnails are, its screens kept for the next.
    fn retire(self, path: Option<&std::path::Path>) -> (Vec<Screen>, Vec<Thumb>) {
        if let (Some(path), Some(trace)) = (path, &self.trace) { write_trace(path, &trace.lines); }
        let mut thumbs: Vec<Thumb> = self.leftover;
        thumbs.extend(self.shots.into_iter().map(|(thumb, ..)| thumb));
        for veil in self.veils {
            thumbs.extend(veil.shades);
            thumbs.extend(veil.edges);
        }
        (self.screens, thumbs)
    }

    /// Where it was going, from where it began, and where it is drawn.
    fn heading(&self) -> Option<((i32, i32), &Strip)> {
        let to = match self.phase {
            Phase::Following => (0, 0),
            Phase::Settling(to, _) | Phase::Arriving(to, _) => to,
            Phase::Stepping(step) => step.to,
        };
        Some((to, self.shown.as_ref()?))
    }

    fn called(&self, row: i32) -> String {
        usize::try_from(row - self.follow.rows.0).ok().and_then(|at| self.names.get(at)).cloned().unwrap_or_default()
    }
}

struct View {
    app: HWND,
    session: Option<Session>,
    pacer: crate::overlay::FramePacer,
    period: Duration,
    frame: Instant,
    paints: Option<Paints>,
    ink: Option<Ink>,
    trace: Option<std::path::PathBuf>,
}

fn run(app: HWND, inbox: mpsc::Receiver<Request>) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        unsafe extern "system" fn wndproc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
            DefWindowProcW(hwnd, message, wparam, lparam)
        }
        for (class, brush) in [(WINDOW_CLASS, BLACK_BRUSH), (WHITE_CLASS, WHITE_BRUSH)] {
            RegisterClassW(&WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: GetModuleHandleW(None).unwrap_or_default().into(),
                lpszClassName: class,
                hbrBackground: HBRUSH(GetStockObject(brush).0),
                ..Default::default()
            });
        }
    }
    let Ok(pacer) = crate::overlay::FramePacer::new() else {
        return crate::app::log("the sliding picture has no frame clock");
    };
    let trace = std::env::var_os("FLICK_TRACE").filter(|path| !path.is_empty()).map(std::path::PathBuf::from);
    let mut view = View { app, session: None, pacer, period: Duration::from_secs_f64(1.0 / 60.0), frame: Instant::now(), paints: None, ink: Ink::new(), trace };
    loop {
        unsafe {
            let mut message = MSG::default();
            while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() { DispatchMessageW(&message); }
        }
        if view.session.is_none() {
            match inbox.recv_timeout(Duration::from_millis(16)) {
                Ok(request) => view.take(request),
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
        while let Ok(request) = inbox.try_recv() {
            view.take(request);
        }
        if view.session.is_some() {
            view.tick();
        }
    }
}

fn thread_cpu() -> f64 {
    let (mut created, mut exited, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    let _ = unsafe { GetThreadTimes(GetCurrentThread(), &mut created, &mut exited, &mut kernel, &mut user) };
    let ms = |t: FILETIME| ((t.dwHighDateTime as u64) << 32 | t.dwLowDateTime as u64) as f64 / 10_000.0;
    ms(kernel) + ms(user)
}

fn new_window(area: RECT) -> Option<HWND> {
    unsafe {
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT | WS_EX_LAYERED,
            WINDOW_CLASS,
            w!("Flick"),
            WS_POPUP,
            area.left,
            area.top,
            area.right - area.left,
            area.bottom - area.top,
            None,
            None,
            GetModuleHandleW(None).ok()?,
            None,
        );
        if hwnd.0 == 0 {
            return None;
        }
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
        // Tool windows are shown on every desktop already; pinned, the
        // shell is told so as well.
        let _ = crate::vdapi::pin_window(hwnd);
        Some(hwnd)
    }
}

/// The minimap uses the exact displayed rows rather than the hold's origin row.
#[derive(Clone)]
pub struct Drawn {
    pub title: String,
    pub origin: crate::grid::Pos,
    pub anchors: Vec<usize>,
    pub rows: (i32, i32),
    pub along: Vec<f64>,
    pub y: f64,
}
static DRAWN: std::sync::Mutex<Option<Drawn>> = std::sync::Mutex::new(None);
pub fn drawn() -> Option<Drawn> { DRAWN.lock().ok()?.clone() }

/// The minimap's windows, to keep them above the picture.
fn minimap_windows() -> Vec<HWND> {
    unsafe extern "system" fn each(hwnd: HWND, out: LPARAM) -> windows::Win32::Foundation::BOOL {
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == GetCurrentProcessId() {
            let mut class = [0u16; 32];
            let n = GetClassNameW(hwnd, &mut class).max(0) as usize;
            if String::from_utf16_lossy(&class[..n]) == "flick.overlay" {
                (*(out.0 as *mut Vec<HWND>)).push(hwnd);
            }
        }
        true.into()
    }
    let mut found = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&mut found as *mut _ as isize));
    }
    found
}

impl View {
    fn take(&mut self, request: Request) {
        match request {
            Request::Begin(scene) => {
                // Monitors as they were are kept, so the picture stays up
                // from one hold to the next, going on from where it was.
                let (_, receded) = self.carried();
                let (screens, leftover) = self.session.take().map(|s| s.retire(self.trace.as_deref())).unwrap_or_default();
                self.begin(scene, screens, leftover, Phase::Following);
                if let Some(new) = &mut self.session {
                    new.receded_from = receded;
                }
            }
            Request::Step(scene, to) => {
                let (carry, receded) = self.carried();
                let (screens, leftover) = self.session.take().map(|s| s.retire(self.trace.as_deref())).unwrap_or_default();
                let step = Step { from: carry, receded, ..Step::new(0.0, to) };
                self.begin(scene, screens, leftover, Phase::Stepping(step));
            }
            Request::Retarget(hold, to) => {
                if let Some(session) = self.session.as_mut().filter(|session| session.hold == hold) {
                    if let Phase::Stepping(step) = session.phase {
                        session.phase = Phase::Stepping(step.then(session.began.elapsed().as_secs_f64(), to));
                    }
                }
            }
            Request::Rebase(scene) => {
                let Some(mut old) = self.session.take() else { return };
                if scene.hold != old.hold || old.phase != Phase::Following {
                    self.session = Some(old);
                    return;
                }
                let (hand, visible, began, farthest_y, trace) = (old.hand, old.visible, old.began, old.farthest_y, old.trace.take());
                let histories = (std::mem::take(&mut old.follow.x.history), std::mem::take(&mut old.follow.y.history));
                let (screens, leftover) = old.retire(self.trace.as_deref());
                self.begin(scene, screens, leftover, Phase::Following);
                if let Some(new) = &mut self.session {
                    (new.follow.x.history, new.follow.y.history) = histories;
                    (new.hand, new.visible, new.began, new.farthest_y) = (hand, visible, began, farthest_y);
                    new.trace = trace.or(new.trace.take());
                }
            }
            Request::Arrived(hold) => {
                if self.session.as_ref().is_some_and(|session| session.hold == hold) {
                    self.finish(true);
                }
            }
            Request::Cancel => self.finish(false),
        }
    }

    /// Where the picture up now is from the desktop it was going to, which
    /// the next hold begins on, and how far it has receded.
    fn carried(&self) -> ((f64, f64), f64) {
        let Some(session) = &self.session else { return ((0.0, 0.0), 0.0) };
        let Some((to, strip)) = session.heading() else { return ((0.0, 0.0), 0.0) };
        let along = strip.along(session.follow.rows, to.1);
        ((along - to.0 as f64, strip.y - to.1 as f64), strip.receded)
    }

    fn begin(&mut self, scene: Scene, mut screens: Vec<Screen>, mut leftover: Vec<Thumb>, phase: Phase) {
        let started = Instant::now();
        let monitors = crate::vd::monitors();
        if screens.len() != monitors.len() || screens.iter().zip(&monitors).any(|(screen, m)| screen.area != m.work) {
            // Thumbnails before their windows.
            leftover.clear();
            screens.clear();
            for monitor in &monitors {
                if let Some(hwnd) = new_window(monitor.work) {
                    screens.push(Screen { hwnd, area: monitor.work, scale: monitor.scale, walls: Vec::new(), banner: Banner::new() });
                }
            }
        }
        if screens.is_empty() {
            return;
        }
        // The wallpaper and icons of the desktop, the same on every one.
        let progman = unsafe { FindWindowW(w!("Progman"), None) };
        let mut whole = RECT::default();
        if progman.0 != 0 && unsafe { GetWindowRect(progman, &mut whole) }.is_ok() {
            for screen in &mut screens {
                if screen.walls.is_empty() {
                    let a = screen.area;
                    let source = RECT { left: a.left - whole.left, top: a.top - whole.top, right: a.right - whole.left, bottom: a.bottom - whole.top };
                    let walls: Vec<Thumb> = (0..WALLS).filter_map(|_| Thumb::new(screen, progman, (a.left, a.top), Some(source))).collect();
                    screen.walls = walls;
                }
            }
        }
        // Each desktop's windows in their order; over them what darkens and
        // lights the desktops, then the windows going along their row,
        // which stay put as the row slides by under them, and in front of
        // all the ones on every desktop.
        let rank = |place: &Place| match place {
            Place::Cell(..) => 0,
            Place::Row(_) => 1,
            Place::Fixed => 2,
        };
        let mut order: Vec<&Shot> = scene.shots.iter().collect();
        order.sort_by_key(|shot| rank(&shot.place));
        let mut shots = Vec::new();
        let mut veils: Vec<Veil> = Vec::new();
        if self.paints.is_none() {
            self.paints = Paints::new();
        }
        for shot in order {
            if rank(&shot.place) > 0 && veils.is_empty() {
                veils = self.veils(&screens);
            }
            let (cx, cy) = ((shot.rect.left + shot.rect.right) / 2, (shot.rect.top + shot.rect.bottom) / 2);
            let inside = |a: &RECT| cx >= a.left && cx < a.right && cy >= a.top && cy < a.bottom;
            let on = monitors.iter().position(|m| inside(&m.bounds)).unwrap_or(0).min(screens.len() - 1);
            if let Some(thumb) = Thumb::new(&screens[on], shot.hwnd, (shot.rect.left, shot.rect.top), None) {
                shots.push((thumb, on, shot.place));
            }
        }
        if veils.is_empty() {
            veils = self.veils(&screens);
        }
        let stats = Stats { setup: started.elapsed().as_secs_f64() * 1000.0, thumbnails: shots.len(), ..Stats::default() };
        let follow = Follow::new(scene.rows, scene.cols, scene.firm);
        let visible = screens.iter().any(|screen| screen.walls.iter().any(|wall| wall.drawn.is_some()));
        let trace = self.trace.as_ref().map(|_| Trace { lines: Vec::with_capacity(512), last: None });
        self.session = Some(Session {
            hold: scene.hold,
            origin: scene.origin,
            anchors: scene.anchors,
            follow,
            screens,
            shots,
            veils,
            leftover,
            phase,
            began: Instant::now(),
            hand: (0.0, 0.0),
            replay: crate::input::follow_hold(scene.hold).filter(|s| !s.hand.holding).map(|s| s.path.into()).unwrap_or_default(),
            shown: None,
            receded_from: 0.0,
            farthest_y: 0.0,
            names: scene.names,
            label: Label::new(0),
            last_frame: None,
            visible,
            stats,
            trace,
        });
        // Every overlay is drawn together: the fastest covered output supplies
        // the clock, so a secondary 120 Hz screen is never capped at 60 Hz.
        let mut fastest = None;
        for monitor in &monitors {
            self.pacer.place(monitor.work);
            if fastest.as_ref().map_or(true, |(_, period)| self.pacer.period < *period) {
                fastest = Some((monitor.work, self.pacer.period));
            }
        }
        if let Some((area, period)) = fastest {
            self.pacer.place(area);
            self.period = period;
        }
        self.frame = Instant::now();
    }

    /// Each screen's veil, none where the windows it is made of are not.
    fn veils(&self, screens: &[Screen]) -> Vec<Veil> {
        let Some(paints) = &self.paints else { return screens.iter().map(|_| Veil::default()).collect() };
        screens
            .iter()
            .map(|screen| {
                let at = (screen.area.left, screen.area.top);
                Veil {
                    shades: (0..WALLS).filter_map(|_| Thumb::new(screen, paints.black, at, None)).collect(),
                    edges: (0..LIT * 4).filter_map(|_| Thumb::new(screen, paints.white, at, None)).collect(),
                }
            })
            .collect()
    }

    /// Takes the picture down; `flushed` once what Windows shows under it
    /// is on screen.
    fn finish(&mut self, flushed: bool) {
        let Some(session) = self.session.take() else { return };
        *DRAWN.lock().unwrap() = None;
        if flushed {
            unsafe {
                let _ = DwmFlush();
                let _ = DwmFlush();
            }
        }
        // Out of sight in one go first; then what was in it.
        for screen in &session.screens {
            screen.hide();
        }
        let stats = &session.stats;
        let summary = |v: &[f64]| {
            let most = v.iter().copied().fold(0.0, f64::max);
            let mean = if v.is_empty() { 0.0 } else { v.iter().sum::<f64>() / v.len() as f64 };
            let mut sorted = v.to_vec();
            sorted.sort_by(f64::total_cmp);
            let p95 = sorted.get(sorted.len() * 95 / 100).copied().unwrap_or(0.0);
            format!("mean {mean:.2} p95 {p95:.2} most {most:.2} ms")
        };
        let long = stats.gaps.iter().filter(|gap| **gap > self.period.as_secs_f64() * 1500.0).count();
        let line = format!(
            "glide {}: {} thumbnails set up in {:.1} ms; {} frames over {:.0} ms ({:.1} ms a refresh), {} thumbnail moves; \
             frame to frame {}, {long} longer than 1.5 refreshes; cpu {}; work {}",
            session.hold,
            stats.thumbnails,
            stats.setup,
            stats.frames,
            session.began.elapsed().as_secs_f64() * 1000.0,
            self.period.as_secs_f64() * 1000.0,
            stats.updates,
            summary(&stats.gaps),
            summary(&stats.cpu),
            summary(&stats.work)
        );
        debug_log!("{line}");
        if std::env::var_os("FLICK_GLIDE_STATS").is_some() {
            crate::app::log(&line);
        }
        // Thumbnails before their windows.
        let (screens, thumbs) = session.retire(self.trace.as_deref());
        drop(thumbs);
        drop(screens);
        self.paints = None;
    }

    /// One frame: where the hand is, or where the picture settles, drawn;
    /// then the next refresh is waited for.
    fn tick(&mut self) {
        unsafe {
            let mut message = MSG::default();
            while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                DispatchMessageW(&message);
            }
        }
        let (cpu, work) = (thread_cpu(), Instant::now());
        let app = self.app;
        let Some(session) = &mut self.session else { return };
        let now = session.began.elapsed().as_secs_f64();
        let mut done = false;
        let tell = |message: u32, to: (i32, i32), hold: u32| unsafe {
            let _ = PostMessageW(app, message, WPARAM(pack(to)), LPARAM(hold as isize));
        };
        let receding = follow::recede_from(session.receded_from, now);
        // Where the strip is, how far receded, and whether the rows go as
        // in a single step (`follow::step_along`).
        let ((x, y), receded, stepping) = match session.phase {
            Phase::Following => {
                let saved = crate::input::follow_hold(session.hold);
                let hand = saved.as_ref().map_or_else(crate::input::followed, |s| s.hand);
                if hand.hold == session.hold {
                    session.hand = (hand.x, hand.y);
                }
                if let Some(&(x, y, axis)) = session.replay.front() {
                    let at = session.follow.track_locked(now, (x, y), axis);
                    if session.follow.caught_up((x, y), axis) { session.replay.pop_front(); }
                    (at, receding, false)
                } else if hand.hold == session.hold && hand.holding {
                    (session.follow.track_locked(now, session.hand, saved.as_ref().map_or(0, |s| s.axis)), receding, false)
                } else {
                    let to = session.follow.let_go(now, session.hand);
                    DECIDED.store((session.hold as u64) << 32 | pack(to) as u32 as u64, Ordering::SeqCst);
                    debug_log!("glide {}: let go of at {:?}, settling on {to:?}", session.hold, session.hand);
                    tell(WM_GLIDE_TARGET, to, session.hold);
                    let settle = [session.follow.x.snap, session.follow.y.snap].iter().flatten().map(|snap| snap.time).fold(0.0, f64::max);
                    let forward = now + follow::forward_after(settle);
                    session.phase = Phase::Settling(to, forward);
                    (session.follow.settle(now).0, receding, false)
                }
            }
            Phase::Settling(to, forward) => {
                let (at, settled) = session.follow.settle(now);
                let receded = receding.min(follow::come_forward(now - forward));
                if settled && now >= forward + follow::FORWARD_TIME {
                    if to == (0, 0) {
                        done = true;
                    } else {
                        tell(WM_GLIDE_SETTLED, to, session.hold);
                        session.phase = Phase::Arriving(to, Instant::now());
                    }
                }
                (at, receded, false)
            }
            Phase::Stepping(step) => {
                let (at, receded) = step.at(now);
                if step.done(now) {
                    tell(WM_GLIDE_SETTLED, step.to, session.hold);
                    session.phase = Phase::Arriving(step.to, Instant::now());
                }
                (at, receded, true)
            }
            Phase::Arriving(_, since) => {
                done = since.elapsed() >= ARRIVAL_WAIT;
                let strip = session.shown.clone().unwrap_or_default();
                ((0.0, strip.y), strip.receded, false)
            }
        };
        if done {
            return self.finish(false);
        }
        let Some(session) = &mut self.session else { return };
        let strip = match session.phase {
            Phase::Arriving(..) => session.shown.clone().unwrap_or_default(),
            _ => {
                let rows = session.follow.rows;
                let along = (rows.0..=rows.1)
                    .map(|row| {
                        let along = if stepping { follow::step_along(x, y, row) } else { session.follow.offset(row) };
                        along
                    })
                    .collect();
                Strip { y, along, receded }
            }
        };
        // The row whose name is shown: the one the hand is on, or the one
        // the picture goes to.
        let row = match session.phase {
            Phase::Following => session.follow.row,
            Phase::Settling(to, _) | Phase::Arriving(to, _) => to.1,
            Phase::Stepping(step) => step.to.1,
        };
        session.label.show(now, row);
        session.farthest_y = session.farthest_y.max(strip.y.abs());
        let mut moves = 0;
        let drawing = Instant::now();
        if session.shown.as_ref() != Some(&strip) {
            moves = draw(session, &strip);
            session.stats.updates += moves;
            session.shown = Some(strip.clone());
            *DRAWN.lock().unwrap() = Some(Drawn { title: session.called(row), origin: session.origin, anchors: session.anchors.clone(), rows: session.follow.rows, along: strip.along.clone(), y: strip.y });
        }
        if self.ink.is_none() {
            self.ink = Ink::new();
        }
        if let Some(ink) = &mut self.ink {
            show_banners(ink, session, now);
        }
        let drawn_in = drawing.elapsed().as_secs_f64() * 1000.0;
        // The hold before's thumbnails go now this one's are drawn over them.
        session.leftover.clear();
        if !session.visible {
            session.visible = true;
            unsafe {
                let raise = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW;
                for screen in &session.screens {
                    let _ = SetWindowPos(screen.hwnd, HWND_TOPMOST, 0, 0, 0, 0, raise);
                }
                for banner in session.screens.iter().filter_map(|screen| screen.banner.as_ref()) {
                    let _ = SetWindowPos(banner.hwnd, HWND_TOPMOST, 0, 0, 0, 0, raise);
                }
                for minimap in minimap_windows() {
                    let _ = SetWindowPos(minimap, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS);
                }
            }
        }
        if session.stats.frames == 0 {
            // Thumbnail/font setup is not animation time: the next visible frame starts from zero.
            session.began = Instant::now();
            self.frame = session.began;
        }
        session.stats.frames += 1;
        session.stats.cpu.push(thread_cpu() - cpu);
        let work_ms = work.elapsed().as_secs_f64() * 1000.0;
        session.stats.work.push(work_ms);
        let paced = self.pace();
        let Some(session) = &mut self.session else { return };
        let shown_at = Instant::now();
        if let Some(last) = session.last_frame.replace(shown_at) {
            session.stats.gaps.push(shown_at.duration_since(last).as_secs_f64() * 1000.0);
        }
        if let Some(trace) = &mut session.trace {
            let at = qpc_ms();
            let gap = trace.last.map_or(0.0, |last| at - last);
            trace.last = Some(at);
            let phase = match session.phase {
                Phase::Following => "following",
                Phase::Settling(..) => "settling",
                Phase::Stepping(_) => "stepping",
                Phase::Arriving(..) => "arriving",
            };
            let strip = session.shown.as_ref().cloned().unwrap_or_default();
            let along = strip.along(session.follow.rows, session.follow.row);
            let vblank = if paced.flushed { "dxgi" } else if paced.timer.is_some() { "timer" } else { "none" };
            trace.lines.push(format!(
                "{},{at:.3},{gap:.3},{vblank},{:.3},{:.3},{phase},{:.4},{:.4},{along:.4},{:.4},{:.4},{work_ms:.3},{drawn_in:.3},{moves},{}",
                session.hold,
                paced.flush,
                paced.timer.unwrap_or(0.0),
                session.hand.0,
                session.hand.1,
                strip.y,
                follow::scale(strip.receded),
                session.shots.len()
            ));
        }
    }

    /// Waits for this output's DXGI refresh, with a bounded timer fallback.
    fn pace(&mut self) -> Paced {
        let before = Instant::now();
        let (fallback, vblank) = self.pacer.wait(self.frame).unwrap_or_default();
        self.period = self.pacer.period;
        let now = Instant::now();
        self.frame = if vblank { now } else { (self.frame + self.period).max(now - self.period) };
        Paced { flushed: vblank, flush: before.elapsed().as_secs_f64() * 1000.0, timer: (!fallback.is_zero()).then_some(fallback.as_secs_f64() * 1000.0) }
    }

}

/// Adds `lines` to the trace at `path`, with the names of the columns
/// first in a new file.
fn write_trace(path: &std::path::Path, lines: &[String]) {
    let new = !path.exists();
    let written = std::fs::OpenOptions::new().create(true).append(true).open(path).and_then(|file| {
        let mut out = std::io::BufWriter::new(file);
        if new {
            writeln!(out, "{TRACE_HEADER}")?;
        }
        for line in lines {
            writeln!(out, "{line}")?;
        }
        out.flush()
    });
    if let Err(e) = written {
        crate::app::log(&format!("the frame trace could not be written to {}: {e}", path.display()));
    }
}

/// The name of the row over each screen, as opaque as the screen has
/// receded: drawn anew only while it changes.
fn show_banners(ink: &mut Ink, session: &mut Session, now: f64) {
    let names = session.label.at(now);
    let key = BannerKey::of(names);
    let alpha = (session.shown.as_ref().map_or(0.0, |strip| strip.receded).clamp(0.0, 1.0) * 255.0).round() as u8;
    let called: Vec<String> = (session.follow.rows.0..=session.follow.rows.1).map(|row| session.called(row)).collect();
    let rows = session.follow.rows;
    let called = |row: i32| usize::try_from(row - rows.0).ok().and_then(|at| called.get(at)).cloned().unwrap_or_default();
    for screen in &mut session.screens {
        let (area, scale) = (screen.area, screen.scale);
        let Some(banner) = &mut screen.banner else { continue };
        if banner.drawn == Some((key, alpha)) {
            continue;
        }
        let size = ((BANNER.0 * scale).round() as i32, (BANNER.1 * scale).round() as i32);
        if !banner.dib.as_ref().is_some_and(|dib| dib.size == size) {
            banner.dib = Dib::new(size).ok();
            banner.drawn = None;
        }
        let Some(dib) = &banner.dib else { continue };
        if banner.drawn.map(|(key, _)| key) != Some(key) && ink.draw(dib, scale, names, &called).is_none() {
            continue;
        }
        let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: alpha, AlphaFormat: AC_SRC_ALPHA as u8 };
        let origin = POINT { x: (area.left + area.right - size.0) / 2, y: area.top + (BANNER_TOP * scale).round() as i32 };
        let shown = unsafe {
            UpdateLayeredWindow(
                banner.hwnd,
                None,
                Some(&origin),
                Some(&SIZE { cx: size.0, cy: size.1 }),
                dib.dc,
                Some(&POINT::default()),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            )
        };
        banner.drawn = shown.is_ok().then_some((key, alpha));
    }
}

/// One desktop in sight: how far from the middle of the screen (sideways,
/// up and down), and how it stands out (`follow::standing_out`).
struct Desk {
    apart: (f64, f64),
    emphasis: f64,
    lift: f64,
    dark: f64,
}

/// The desktops of `follow` that may be in sight with the strip at
/// `strip`: the nearest two each way of each row's own place and one more.
fn in_sight(follow: &Follow, strip: &Strip) -> Vec<Desk> {
    let exists = |row: i32, col: i32| {
        (follow.rows.0..=follow.rows.1).contains(&row) && {
            let cols = follow.cols_of(row);
            (cols.0..=cols.1).contains(&col)
        }
    };
    let near = |at: f64| (at.floor() as i32 - 1)..=(at.floor() as i32 + 2);
    let mut sight = Vec::new();
    for row in near(strip.y) {
        let along = strip.along(follow.rows, row);
        for col in near(along) {
            if exists(row, col) {
                let apart = (col as f64 - along, row as f64 - strip.y);
                let emphasis = follow::emphasis(apart);
                let (lift, dark) = follow::standing_out(emphasis, strip.receded);
                sight.push(Desk { apart, emphasis, lift, dark });
            }
        }
    }
    sight
}

fn opacity(share: f64) -> u8 {
    (share.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// Draws every thumbnail of `session` for `strip`; how many moved.
fn draw(session: &mut Session, strip: &Strip) -> u32 {
    let mut moved = 0;
    let scale = follow::scale(strip.receded);
    let sight = in_sight(&session.follow, strip);
    let rows = session.follow.rows;
    // Windows going along their row stay where they are until the strip
    // has gone up or down a little, then go with it.
    let with_row = (session.farthest_y / ROW_WINDOWS_JOIN).min(1.0);
    let mut empty = Veil::default();
    for (index, screen) in session.screens.iter_mut().enumerate() {
        let size = (screen.area.right - screen.area.left, screen.area.bottom - screen.area.top);
        let whole = (size.0 as f64, size.1 as f64);
        let edge = (2.0 * screen.scale as f64).round().max(1.0);
        let veil = session.veils.get_mut(index).unwrap_or(&mut empty);
        let (mut walls, mut shades, mut edges) = (screen.walls.iter_mut(), veil.shades.iter_mut(), veil.edges.iter_mut());
        for desk in &sight {
            let at = follow::placed([0.0, 0.0, whole.0, whole.1], whole, scale, desk.apart, desk.lift);
            let inside = at[2] > 0.0 && at[3] > 0.0 && at[0] < whole.0 && at[1] < whole.1;
            if !inside {
                continue;
            }
            if let Some(wall) = walls.next() {
                moved += wall.draw(Some(at), size, 255) as u32;
            }
            if desk.dark > 0.004 {
                if let Some(shade) = shades.next() {
                    moved += shade.draw(Some(at), size, opacity(desk.dark)) as u32;
                }
            }
            let glow = follow::GLOW * desk.emphasis * strip.receded;
            if glow > 0.01 {
                let [l, t, r, b] = at;
                let lines = [[l - edge, t - edge, r + edge, t], [l - edge, b, r + edge, b + edge], [l - edge, t, l, b], [r, t, r + edge, b]];
                for line in lines {
                    if let Some(light) = edges.next() {
                        moved += light.draw(Some(line), size, opacity(glow)) as u32;
                    }
                }
            }
        }
        for thumb in walls.chain(shades).chain(edges) {
            moved += thumb.draw(None, size, 0) as u32;
        }
    }
    for (thumb, on, place) in &mut session.shots {
        let area = session.screens[*on].area;
        let size = (area.right - area.left, area.bottom - area.top);
        let whole = (size.0 as f64, size.1 as f64);
        let edges = thumb.edges();
        let (at, shown) = match *place {
            Place::Cell(row, col) => {
                let apart = (col as f64 - strip.along(rows, row), row as f64 - strip.y);
                let (lift, _) = follow::standing_out(follow::emphasis(apart), strip.receded);
                (follow::placed(edges, whole, scale, apart, lift), 1.0)
            }
            Place::Row(row) => {
                let at = follow::between(edges, follow::on_strip(edges, whole, scale, (0.0, row as f64 - strip.y)), with_row);
                // Gone with a row not in the middle, as dark as it is.
                let near = (1.0 - (row as f64 - strip.y).abs()).clamp(0.0, 1.0);
                (at, 1.0 - follow::DIM * (1.0 - near) * strip.receded * with_row)
            }
            Place::Fixed => (edges, 1.0),
        };
        moved += thumb.draw(Some(at), size, opacity(shown)) as u32;
    }
    moved
}
