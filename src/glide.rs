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
//! It runs on a thread of its own, as the minimap does: switching desktops
//! keeps the app's thread busy for tens of milliseconds at a time.

use crate::follow::{self, Follow, Step};
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use windows::{
    core::{w, PCWSTR},
    Win32::{
        Foundation::{CloseHandle, COLORREF, FILETIME, HANDLE, HWND, LPARAM, LRESULT, RECT, WPARAM},
        Graphics::{
            Dwm::{
                DwmFlush, DwmQueryThumbnailSourceSize, DwmRegisterThumbnail, DwmUnregisterThumbnail,
                DwmUpdateThumbnailProperties, DWM_THUMBNAIL_PROPERTIES, DWM_TNP_OPACITY, DWM_TNP_RECTDESTINATION,
                DWM_TNP_RECTSOURCE, DWM_TNP_SOURCECLIENTAREAONLY, DWM_TNP_VISIBLE,
            },
            Gdi::{EnumDisplaySettingsW, GetStockObject, BLACK_BRUSH, DEVMODEW, ENUM_CURRENT_SETTINGS, HBRUSH},
        },
        System::{
            Com::{CoInitializeEx, COINIT_MULTITHREADED},
            LibraryLoader::GetModuleHandleW,
            Threading::{
                CreateWaitableTimerExW, GetCurrentProcessId, GetCurrentThread, GetThreadTimes, SetWaitableTimerEx,
                WaitForSingleObject, CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, TIMER_ALL_ACCESS,
            },
        },
        UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, EnumWindows, FindWindowW, GetClassNameW,
            GetWindowRect, GetWindowThreadProcessId, PeekMessageW, PostMessageW, RegisterClassW, SetLayeredWindowAttributes,
            SetWindowPos, HWND_TOPMOST, LWA_ALPHA, MSG, PM_REMOVE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW,
            WM_APP, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT,
            WS_POPUP,
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
/// Desktops in sight at once, at most: three each way with the screen receded.
const WALLS: usize = 9;
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
    /// The rows up and down from the first, and for each the cells along
    /// it (`Follow::new`).
    pub rows: (i32, i32),
    pub cols: Vec<(i32, i32)>,
    /// How firm the notches are each way (`follow::firmness`).
    pub firm: (f64, f64),
    pub shots: Vec<Shot>,
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
    /// The wallpaper, one for each desktop that can be in sight at once
    /// (`WALLS`).
    walls: Vec<Thumb>,
}

struct Thumb {
    id: isize,
    /// Where it is drawn on its screen with the picture where the hold began.
    dest: RECT,
    source: Option<RECT>,
    /// Where it was last drawn; `None` while not drawn.
    drawn: Option<RECT>,
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

    /// Draws it at `at`, or not at all; only when that changed.
    fn draw(&mut self, at: Option<[f64; 4]>, size: (i32, i32)) -> bool {
        let rect = at.map(|[l, t, r, b]| RECT { left: l.round() as i32, top: t.round() as i32, right: r.round() as i32, bottom: b.round() as i32 });
        let rect = rect.filter(|r| r.right > 0 && r.bottom > 0 && r.left < size.0 && r.top < size.1 && r.right > r.left && r.bottom > r.top);
        if rect == self.drawn {
            return false;
        }
        self.drawn = rect;
        let mut props = DWM_THUMBNAIL_PROPERTIES {
            dwFlags: DWM_TNP_RECTDESTINATION | DWM_TNP_VISIBLE | DWM_TNP_OPACITY | DWM_TNP_SOURCECLIENTAREAONLY,
            rcDestination: rect.unwrap_or_default(),
            opacity: 255,
            fVisible: rect.is_some().into(),
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
        self.walls.clear();
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Following,
    /// Let go of: settling on a desktop, then coming forward from then on.
    Settling((i32, i32), f64),
    Stepping(Step),
    /// Settled, waiting for Windows to be there, since then.
    Arriving(Instant),
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

struct Session {
    hold: u32,
    follow: Follow,
    screens: Vec<Screen>,
    /// The windows' thumbnails, bottom first, and where each is drawn.
    shots: Vec<(Thumb, usize, Place)>,
    phase: Phase,
    began: Instant,
    /// Where the hand was last seen.
    hand: (f64, f64),
    /// Where the strip is and how far the screen has receded, as drawn.
    shown: Option<((f64, f64), f64)>,
    /// The farthest up or down the strip went (`ROW_WINDOWS_JOIN`).
    farthest_y: f64,
    last_frame: Option<Instant>,
    visible: bool,
    stats: Stats,
}

struct View {
    app: HWND,
    session: Option<Session>,
    timer: HANDLE,
    period: Duration,
    frame: Instant,
}

fn run(app: HWND, inbox: mpsc::Receiver<Request>) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        unsafe extern "system" fn wndproc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
            DefWindowProcW(hwnd, message, wparam, lparam)
        }
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: GetModuleHandleW(None).unwrap_or_default().into(),
            lpszClassName: WINDOW_CLASS,
            hbrBackground: HBRUSH(GetStockObject(BLACK_BRUSH).0),
            ..Default::default()
        });
    }
    let Ok(timer) = (unsafe { CreateWaitableTimerExW(None, None, CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, TIMER_ALL_ACCESS.0) }) else {
        return crate::app::log("the sliding picture has no timer");
    };
    let mut view = View { app, session: None, timer, period: refresh_period(), frame: Instant::now() };
    loop {
        if view.session.is_none() {
            match inbox.recv() {
                Ok(request) => view.take(request),
                Err(_) => break,
            }
        }
        while let Ok(request) = inbox.try_recv() {
            view.take(request);
        }
        if view.session.is_some() {
            view.tick();
        }
    }
    unsafe {
        let _ = CloseHandle(view.timer);
    }
}

/// How long one refresh of the screen takes.
fn refresh_period() -> Duration {
    let mut mode = DEVMODEW { dmSize: std::mem::size_of::<DEVMODEW>() as u16, ..Default::default() };
    let hz = unsafe { EnumDisplaySettingsW(PCWSTR::null(), ENUM_CURRENT_SETTINGS, &mut mode) }.as_bool().then_some(mode.dmDisplayFrequency);
    Duration::from_secs_f64(1.0 / hz.filter(|hz| *hz > 1).unwrap_or(60) as f64)
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
                // from one hold to the next.
                let screens = self.session.take().map(|old| old.screens).unwrap_or_default();
                self.begin(scene, screens, Phase::Following);
            }
            Request::Step(scene, to) => {
                let screens = self.session.take().map(|old| old.screens).unwrap_or_default();
                self.begin(scene, screens, Phase::Stepping(Step::new(0.0, to)));
            }
            Request::Retarget(hold, to) => {
                if let Some(session) = self.session.as_mut().filter(|session| session.hold == hold) {
                    if let Phase::Stepping(step) = session.phase {
                        session.phase = Phase::Stepping(step.then(session.began.elapsed().as_secs_f64(), to));
                    }
                }
            }
            Request::Rebase(scene) => {
                let Some(old) = self.session.take() else { return };
                if scene.hold != old.hold || old.phase != Phase::Following {
                    self.session = Some(old);
                    return;
                }
                let Session { follow, screens, hand, visible, began, farthest_y, .. } = old;
                self.begin(scene, screens, Phase::Following);
                if let Some(new) = &mut self.session {
                    new.follow.x.history = follow.x.history;
                    new.follow.y.history = follow.y.history;
                    (new.hand, new.visible, new.began, new.farthest_y) = (hand, visible, began, farthest_y);
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

    fn begin(&mut self, scene: Scene, mut screens: Vec<Screen>, phase: Phase) {
        let started = Instant::now();
        let monitors = crate::vd::monitors();
        if screens.len() != monitors.len() || screens.iter().zip(&monitors).any(|(screen, m)| screen.area != m.work) {
            screens.clear();
            for monitor in &monitors {
                if let Some(hwnd) = new_window(monitor.work) {
                    screens.push(Screen { hwnd, area: monitor.work, walls: Vec::new() });
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
        // Each desktop's windows in their order; above them the windows
        // going along their row, which stay put as the row slides by under
        // them, and in front of all the ones on every desktop.
        let rank = |place: &Place| match place {
            Place::Cell(..) => 0,
            Place::Row(_) => 1,
            Place::Fixed => 2,
        };
        let mut order: Vec<&Shot> = scene.shots.iter().collect();
        order.sort_by_key(|shot| rank(&shot.place));
        let mut shots = Vec::new();
        for shot in order {
            let (cx, cy) = ((shot.rect.left + shot.rect.right) / 2, (shot.rect.top + shot.rect.bottom) / 2);
            let inside = |a: &RECT| cx >= a.left && cx < a.right && cy >= a.top && cy < a.bottom;
            let on = monitors.iter().position(|m| inside(&m.bounds)).unwrap_or(0).min(screens.len() - 1);
            if let Some(thumb) = Thumb::new(&screens[on], shot.hwnd, (shot.rect.left, shot.rect.top), None) {
                shots.push((thumb, on, shot.place));
            }
        }
        let stats = Stats { setup: started.elapsed().as_secs_f64() * 1000.0, thumbnails: shots.len(), ..Stats::default() };
        let follow = Follow::new(scene.rows, scene.cols, scene.firm);
        let visible = screens.iter().any(|screen| screen.walls.iter().any(|wall| wall.drawn.is_some()));
        self.session = Some(Session {
            hold: scene.hold,
            follow,
            screens,
            shots,
            phase,
            began: Instant::now(),
            hand: (0.0, 0.0),
            shown: None,
            farthest_y: 0.0,
            last_frame: None,
            visible,
            stats,
        });
        self.period = refresh_period();
        self.frame = Instant::now();
    }

    /// Takes the picture down; `flushed` once what Windows shows under it
    /// is on screen.
    fn finish(&mut self, flushed: bool) {
        let Some(session) = self.session.take() else { return };
        if flushed {
            unsafe {
                let _ = DwmFlush();
                let _ = DwmFlush();
            }
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
        drop(session.shots);
        drop(session.screens);
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
        let shown: ((f64, f64), f64) = match session.phase {
            Phase::Following => {
                let hand = crate::input::followed();
                if hand.hold == session.hold {
                    session.hand = (hand.x, hand.y);
                }
                if hand.hold == session.hold && hand.holding {
                    (session.follow.track(now, session.hand), follow::recede(now))
                } else {
                    let to = session.follow.let_go(now, session.hand);
                    DECIDED.store((session.hold as u64) << 32 | pack(to) as u32 as u64, Ordering::SeqCst);
                    debug_log!("glide {}: let go of at {:?}, settling on {to:?}", session.hold, session.hand);
                    tell(WM_GLIDE_TARGET, to, session.hold);
                    let settle = [session.follow.x.snap, session.follow.y.snap].iter().flatten().map(|snap| snap.time).fold(0.0, f64::max);
                    let forward = now + follow::forward_after(settle);
                    session.phase = Phase::Settling(to, forward);
                    (session.follow.settle(now).0, follow::recede(now))
                }
            }
            Phase::Settling(to, forward) => {
                let (at, settled) = session.follow.settle(now);
                let receded = follow::recede(now).min(follow::come_forward(now - forward));
                if settled && now >= forward + follow::FORWARD_TIME {
                    if to == (0, 0) {
                        done = true;
                    } else {
                        tell(WM_GLIDE_SETTLED, to, session.hold);
                        session.phase = Phase::Arriving(Instant::now());
                    }
                }
                (at, receded)
            }
            Phase::Stepping(step) => {
                let (at, receded) = step.at(now);
                if step.done(now) {
                    tell(WM_GLIDE_SETTLED, step.to, session.hold);
                    session.phase = Phase::Arriving(Instant::now());
                }
                (at, receded)
            }
            Phase::Arriving(since) => {
                done = since.elapsed() >= ARRIVAL_WAIT;
                session.shown.unwrap_or_default()
            }
        };
        if done {
            return self.finish(false);
        }
        let Some(session) = &mut self.session else { return };
        session.farthest_y = session.farthest_y.max(shown.0 .1.abs());
        if session.shown != Some(shown) {
            session.shown = Some(shown);
            session.stats.updates += draw(session, shown);
        }
        if !session.visible {
            session.visible = true;
            unsafe {
                for screen in &session.screens {
                    let _ = SetWindowPos(screen.hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
                }
                for minimap in minimap_windows() {
                    let _ = SetWindowPos(minimap, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
                }
            }
        }
        session.stats.frames += 1;
        session.stats.cpu.push(thread_cpu() - cpu);
        session.stats.work.push(work.elapsed().as_secs_f64() * 1000.0);
        self.pace();
        let Some(session) = &mut self.session else { return };
        let shown_at = Instant::now();
        if let Some(last) = session.last_frame.replace(shown_at) {
            session.stats.gaps.push(shown_at.duration_since(last).as_secs_f64() * 1000.0);
        }
    }

    /// Waits for the next refresh: DWM's, or, when it does not wait (an
    /// idle display), the rest of a refresh's time.
    fn pace(&mut self) {
        unsafe {
            let _ = DwmFlush();
        }
        let next = self.frame + self.period;
        let left = next.saturating_duration_since(Instant::now());
        if left > self.period / 2 {
            let due = -(left.as_nanos().div_ceil(100) as i64);
            unsafe {
                if SetWaitableTimerEx(self.timer, &due, 0, None, None, None, 0).is_ok() {
                    WaitForSingleObject(self.timer, left.as_millis() as u32 + 2);
                }
            }
        }
        self.frame = Instant::now();
    }
}

/// Draws every thumbnail of `session` for the strip at `(x, y)` and the
/// screen receded `receded`; how many moved.
fn draw(session: &mut Session, ((x, y), receded): ((f64, f64), f64)) -> u32 {
    let mut moved = 0;
    let scale = follow::scale(receded);
    let follow = &session.follow;
    let exists = |row: i32, col: i32| {
        (follow.rows.0..=follow.rows.1).contains(&row) && {
            let cols = follow.cols_of(row);
            (cols.0..=cols.1).contains(&col)
        }
    };
    // The desktops that may be in sight: the nearest two each way and one more past them.
    let near = |at: f64| (at.floor() as i32 - 1)..=(at.floor() as i32 + 2);
    let mut sight: Vec<(i32, i32)> = Vec::new();
    for row in near(y) {
        for col in near(x) {
            if exists(row, col) {
                sight.push((row, col));
            }
        }
    }
    // Windows going along their row stay where they are until the strip
    // has gone up or down a little, then go with it.
    let with_row = (session.farthest_y / ROW_WINDOWS_JOIN).min(1.0);
    for screen in &mut session.screens {
        let size = (screen.area.right - screen.area.left, screen.area.bottom - screen.area.top);
        let whole = (size.0 as f64, size.1 as f64);
        let mut walls = screen.walls.iter_mut();
        for &(row, col) in &sight {
            let at = follow::on_strip([0.0, 0.0, whole.0, whole.1], whole, scale, (col as f64 - x, row as f64 - y));
            let inside = at[2] > 0.0 && at[3] > 0.0 && at[0] < whole.0 && at[1] < whole.1;
            if inside {
                if let Some(wall) = walls.next() {
                    moved += wall.draw(Some(at), size) as u32;
                }
            }
        }
        for wall in walls {
            moved += wall.draw(None, size) as u32;
        }
    }
    for (thumb, on, place) in &mut session.shots {
        let area = session.screens[*on].area;
        let size = (area.right - area.left, area.bottom - area.top);
        let whole = (size.0 as f64, size.1 as f64);
        let edges = thumb.edges();
        let at = match *place {
            Place::Cell(row, col) => follow::on_strip(edges, whole, scale, (col as f64 - x, row as f64 - y)),
            Place::Row(row) => follow::between(edges, follow::on_strip(edges, whole, scale, (0.0, row as f64 - y)), with_row),
            Place::Fixed => edges,
        };
        moved += thumb.draw(Some(at), size) as u32;
    }
    moved
}
