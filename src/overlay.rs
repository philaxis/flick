//! The minimap shown while navigating: a translucent panel with one rounded
//! square per desktop. The highlight stays in the middle and the grid slides
//! under it, following the hand.
//!
//! It is a click-through layered tool window (tool windows are shown on every
//! virtual desktop) drawn with Direct2D into a premultiplied-alpha bitmap.

use crate::{
    grid::Pos,
    input::Hand,
    paint::{self, accent, rect, rgba, warm, white, DcTarget, Dib, Painter},
    vd,
};
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};
use windows::{
    core::{w, Error, Result, PCWSTR},
    Win32::{
        Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM},
        Graphics::{
            Direct2D::Common::D2D1_ALPHA_MODE_PREMULTIPLIED,
            DirectWrite::{IDWriteTextFormat, DWRITE_TEXT_ALIGNMENT_CENTER},
            Dwm::DwmFlush,
            Gdi::{AC_SRC_ALPHA, AC_SRC_OVER, BLENDFUNCTION},
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, PeekMessageW, RegisterClassW,
            SetWindowPos, ShowWindow, UpdateLayeredWindow, HWND_TOPMOST, MSG, PM_REMOVE, SWP_NOACTIVATE, SWP_NOMOVE,
            SWP_NOSIZE, SWP_SHOWWINDOW, SW_HIDE, ULW_ALPHA, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
            WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
        },
    },
};

const WINDOW_CLASS: PCWSTR = w!("flick.overlay");

const CELL_W: f32 = 46.0;
const CELL_H: f32 = 30.0;
const GAP: f32 = 7.0;
const PAD: f32 = 20.0;
/// Room under the grid for the shelf name.
const TITLE_H: f32 = 26.0;
/// Time constants (seconds) of the exponential easing: of a jump in where
/// the grid is drawn, and of the fading in and out.
const SETTLE_TAU: f32 = 0.045;
const FADE_TAU: f32 = 0.025;
/// How far past the end of its row the highlight is pushed, in cells.
const EDGE_GIVE: f32 = 0.16;
/// Cells shown either side of the highlight at most, sideways and up and
/// down; the ones beyond fade out at the edge of the minimap.
const REACH: usize = 3;

/// One cell of the minimap.
#[derive(Clone, Copy, Default, PartialEq)]
pub struct CellView {
    /// Marked by the user to stand out.
    pub emphasised: bool,
}

/// What the minimap should show.
#[derive(Clone, Default, PartialEq)]
pub struct View {
    pub rows: Vec<Vec<CellView>>,
    /// Per row, the column a vertical move would land on. Rows are shifted
    /// sideways so that these line up in one column.
    pub anchors: Vec<usize>,
    pub cur: Option<Pos>,
    /// Name of the current shelf, shown under the grid.
    pub title: String,
}

impl View {
    fn anchor(&self, row: usize) -> usize {
        self.anchors.get(row).copied().unwrap_or(0)
    }

    /// Cells shown either side of the highlight, sideways and up and down.
    fn reach(&self) -> (usize, usize) {
        let longest = self.rows.iter().map(Vec::len).max().unwrap_or(1);
        (longest.saturating_sub(1).min(REACH), self.rows.len().saturating_sub(1).min(REACH))
    }
}

/// Where the grid is drawn: the highlight stays in the middle and the grid
/// moves under it. The current row follows the hand sideways; the others
/// stand with the cell a move up or down lands on (`View::anchors`) in line
/// with the highlight. Up and down, the grid leans a little the way the hand
/// is going and goes over to the next row in one quick move once it is come
/// to (the row is a whole step in the goal, eased away).
///
/// The hand's place is counted from the cell the app is on, which changes
/// in a step, sometimes a frame or more apart from the count itself. What
/// the goal jumps by then is not shown at once but eased away, so the grid
/// never jumps, nor stops for that frame: it goes on at the pace it had.
/// While nothing changes in a step, it follows the hand as is.
#[derive(Default)]
struct Camera {
    /// Per row, where its first cell is drawn, in cells to the right of the
    /// highlight, as the goal was in the last frame.
    goal_x: Vec<f32>,
    /// Where the first row is drawn, in rows below the highlight, likewise.
    goal_y: f32,
    /// What is left to ease away of jumps in the goal.
    rest_x: Vec<f32>,
    rest_y: f32,
    /// Jumps put by in frames where what the goal is counted from changed,
    /// to be eased from the next frame on: a frame drawn while the app's
    /// view and the count of the hand disagree is taken back in the next,
    /// and what is easing meanwhile goes on easing.
    put_x: Vec<f32>,
    put_y: f32,
    /// How fast the goal went, in cells a second, when last it went with
    /// nothing changing in a step.
    pace_x: Vec<f32>,
    pace_y: f32,
    /// What the goal was last worked out from, but for the hand's place.
    counted_from: Option<(u64, u64)>,
}

impl Camera {
    /// Where the grid goes with the hand at `hand` (along, across).
    fn goals(view: &View, hand: (f32, f32)) -> (Vec<f32>, f32) {
        let mut xs: Vec<f32> = (0..view.rows.len()).map(|r| -(view.anchor(r) as f32)).collect();
        let Some(cur) = view.cur.filter(|cur| cur.row < view.rows.len()) else { return (xs, 0.0) };
        let last_col = view.rows[cur.row].len().saturating_sub(1) as f32;
        xs[cur.row] = -(cur.col as f32 + hand.0).clamp(-EDGE_GIVE, last_col + EDGE_GIVE);
        let last_row = view.rows.len().saturating_sub(1) as f32;
        (xs, -(cur.row as f32 + hand.1).clamp(-EDGE_GIVE, last_row + EDGE_GIVE))
    }

    /// Starts over with the grid as it was with the user on `from`, so that
    /// the move from there is eased in.
    fn start(view: &View, from: Option<Pos>) -> Camera {
        let Some(from) = from else { return Camera::default() };
        let was = View { cur: Some(from), ..view.clone() };
        let (goal_x, goal_y) = Camera::goals(&was, (0.0, 0.0));
        let zeros = vec![0.0; goal_x.len()];
        Camera { goal_x, goal_y, rest_x: zeros.clone(), rest_y: 0.0, put_x: zeros.clone(), put_y: 0.0, pace_x: zeros, pace_y: 0.0, counted_from: None }
    }

    /// One frame, `dt` seconds after the last. `counted_from` changes
    /// whenever the view or what the hand is counted from does.
    fn advance(&mut self, view: &View, hand: (f32, f32), counted_from: (u64, u64), dt: f32) {
        let (goal_x, goal_y) = Camera::goals(view, hand);
        if goal_x.len() != self.goal_x.len() {
            // Rows came or went: nothing to ease from.
            self.rest_x = vec![0.0; goal_x.len()];
            self.put_x = vec![0.0; goal_x.len()];
            self.pace_x = vec![0.0; goal_x.len()];
            (self.rest_y, self.put_y, self.pace_y) = (0.0, 0.0, 0.0);
        } else if self.counted_from != Some(counted_from) {
            // The jump is put by, less the way the hand went meanwhile. Not
            // eased yet in this frame: the app tells the minimap of the cell
            // it got to a moment before it tells the count of the hand, and a
            // frame drawn in between is taken back in the next.
            for (((put, before), now), pace) in self.put_x.iter_mut().zip(&self.goal_x).zip(&goal_x).zip(&self.pace_x) {
                *put += before + pace * dt - now;
            }
            self.put_y += self.goal_y + self.pace_y * dt - goal_y;
            let keep = (-dt / SETTLE_TAU).exp();
            for rest in &mut self.rest_x {
                *rest *= keep;
            }
            self.rest_y *= keep;
        } else {
            for (rest, put) in self.rest_x.iter_mut().zip(&mut self.put_x) {
                *rest += std::mem::take(put);
            }
            self.rest_y += std::mem::take(&mut self.put_y);
            if dt > 0.0 {
                for ((pace, before), now) in self.pace_x.iter_mut().zip(&self.goal_x).zip(&goal_x) {
                    *pace = (now - before) / dt;
                }
                self.pace_y = (goal_y - self.goal_y) / dt;
            }
            let keep = (-dt / SETTLE_TAU).exp();
            for rest in &mut self.rest_x {
                *rest *= keep;
            }
            self.rest_y *= keep;
        }
        (self.goal_x, self.goal_y, self.counted_from) = (goal_x, goal_y, Some(counted_from));
    }

    /// Where the first cell of `row` is drawn, in cells right of the highlight.
    fn x(&self, row: usize) -> f32 {
        let at = |v: &Vec<f32>| v.get(row).copied().unwrap_or(0.0);
        at(&self.goal_x) + at(&self.rest_x) + at(&self.put_x)
    }

    /// Where the first row is drawn, in rows below the highlight.
    fn y(&self) -> f32 {
        self.goal_y + self.rest_y + self.put_y
    }
}

/// How much of a cell `apart` cells from the highlight shows, where `reach`
/// cells are shown in full: it fades out beyond them.
fn fade(apart: f32, reach: usize) -> f32 {
    ((reach as f32 + 0.6 - apart.abs()) / 0.45).clamp(0.0, 1.0)
}

/// One hidden, click-through window; `Overlay::render` gives it its picture
/// and its place.
fn create_window() -> Result<HWND> {
    unsafe {
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            WINDOW_CLASS,
            w!("Flick"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            GetModuleHandleW(None)?,
            None,
        );
        if hwnd.0 == 0 {
            return Err(Error::from_win32());
        }
        Ok(hwnd)
    }
}

pub struct Overlay {
    hwnd: HWND,
    target: DcTarget,
    /// Text format for the shelf name, made for `text_scale`.
    text: Option<IDWriteTextFormat>,
    text_scale: f32,
    /// The bitmap the minimap is drawn into, remade when its size changes.
    canvas: Option<Dib>,
    view: View,
    /// Counts the views given, for `Camera::advance`.
    views: u64,
    camera: Camera,
    alpha: f32,
    visible: bool,
    /// When to start fading out; `None` keeps the minimap up.
    hide_at: Option<Instant>,
    last_tick: Instant,
    scale: f32,
    /// Work area of the primary monitor, which `hwnd` is centred on.
    monitor: RECT,
    /// The same picture is shown on every other monitor through one more
    /// window each: (window, that monitor's work area).
    others: Vec<(HWND, RECT)>,
}

impl Overlay {
    pub fn new() -> Result<Overlay> {
        unsafe extern "system" fn wndproc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
            DefWindowProcW(hwnd, message, wparam, lparam)
        }
        unsafe {
            RegisterClassW(&WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: GetModuleHandleW(None)?.into(),
                lpszClassName: WINDOW_CLASS,
                ..Default::default()
            });
        }
        Ok(Overlay {
            hwnd: create_window()?,
            target: DcTarget::new(&paint::d2d_factory()?, D2D1_ALPHA_MODE_PREMULTIPLIED)?,
            text: None,
            text_scale: 0.0,
            canvas: None,
            view: View::default(),
            views: 0,
            camera: Camera::default(),
            alpha: 0.0,
            visible: false,
            hide_at: None,
            last_tick: Instant::now(),
            scale: 1.0,
            monitor: RECT::default(),
            others: Vec::new(),
        })
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// Shows `view` until `hide_after` is called. When the overlay was hidden
    /// the grid starts as it was on `from` (the cell just left) so the first
    /// move is animated too.
    pub fn show(&mut self, view: View, from: Option<Pos>) {
        if !self.visible {
            self.place_on_monitors();
            self.camera = Camera::start(&view, from);
            self.visible = true;
            self.last_tick = Instant::now();
        }
        self.view = view;
        self.views += 1;
        self.hide_at = None;
        self.tick();
        unsafe {
            // Shown and raised: the minimap stays above the sliding picture
            // of the desktop being left.
            let raise = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW;
            let _ = SetWindowPos(self.hwnd, HWND_TOPMOST, 0, 0, 0, 0, raise);
            for (window, _) in &self.others {
                let _ = SetWindowPos(*window, HWND_TOPMOST, 0, 0, 0, 0, raise);
            }
        }
    }

    /// Replaces the content without changing visibility or timing.
    pub fn update(&mut self, view: View) {
        if self.visible {
            self.view = view;
            self.views += 1;
        }
    }

    pub fn hide_after(&mut self, ms: u64) {
        if self.visible {
            self.hide_at = Some(Instant::now() + Duration::from_millis(ms));
        }
    }

    /// Advances the animations by the time elapsed and repaints.
    pub fn tick(&mut self) {
        if !self.visible {
            return;
        }
        let now = Instant::now();
        let dt = now.duration_since(self.last_tick).as_secs_f32().min(0.1);
        self.last_tick = now;

        let hiding = self.hide_at.is_some_and(|at| now >= at);
        let goal = if hiding { 0.0 } else { 1.0 };
        self.alpha += (goal - self.alpha) * (1.0 - (-dt / FADE_TAU).exp());
        let hand = crate::input::hand();
        self.camera.advance(&self.view, (hand.along, hand.across), (self.views, hand.counted_from), dt);
        #[cfg(feature = "debug-log")]
        if hand.along != 0.0 || hand.across != 0.0 || self.camera.rest_y.abs() > 0.001 {
            // Frame by frame where the grid is drawn while it follows the hand.
            log_frame(&self.view, &self.camera, hand);
        }

        if hiding && self.alpha < 0.02 {
            self.alpha = 0.0;
            self.visible = false;
            unsafe {
                ShowWindow(self.hwnd, SW_HIDE);
                for (window, _) in &self.others {
                    ShowWindow(*window, SW_HIDE);
                }
            }
            return;
        }
        self.render();
    }

    /// Finds the monitors: the primary one sets the scale, and each of the
    /// others gets a window of its own to show the minimap too. The primary
    /// monitor rather than the one under the cursor, so that the minimap is
    /// the same on every screen wherever the cursor is.
    fn place_on_monitors(&mut self) {
        let monitors = vd::monitors();
        if let Some(primary) = monitors.iter().find(|m| m.primary) {
            self.monitor = primary.work;
            self.scale = primary.scale;
        }
        let areas: Vec<RECT> = monitors.iter().filter(|m| !m.primary).map(|m| m.work).collect();
        while self.others.len() > areas.len() {
            if let Some((window, _)) = self.others.pop() {
                unsafe {
                    let _ = DestroyWindow(window);
                }
            }
        }
        while self.others.len() < areas.len() {
            let Ok(window) = create_window() else { break };
            self.others.push((window, RECT::default()));
        }
        for ((_, area), new) in self.others.iter_mut().zip(areas) {
            *area = new;
        }
    }

    /// The same whichever cell the user is on, so the minimap stands still.
    fn wanted_size(&self) -> (i32, i32) {
        let (across, down) = self.view.reach();
        let (cols, rows) = ((2 * across + 1) as f32, (2 * down + 1) as f32);
        let w = PAD * 2.0 + cols * CELL_W + (cols - 1.0) * GAP;
        let title = if self.view.title.is_empty() { 0.0 } else { TITLE_H };
        let h = PAD * 2.0 + rows * CELL_H + (rows - 1.0) * GAP + title;
        ((w * self.scale).ceil() as i32, (h * self.scale).ceil() as i32)
    }

    /// Draws the minimap as it is now into the canvas, which is remade when
    /// the size it needs has changed. `None` when that failed.
    fn draw_to_canvas(&mut self) -> Option<&Dib> {
        if self.text.is_none() || self.text_scale != self.scale {
            self.text = text_format(self.scale).ok();
            self.text_scale = self.scale;
        }
        let size = self.wanted_size();
        if !self.canvas.as_ref().is_some_and(|canvas| canvas.size == size) {
            self.canvas = Dib::new(size).ok();
        }
        let canvas = self.canvas.as_ref()?;
        self.draw(canvas).ok()?;
        Some(canvas)
    }

    /// Redraws the minimap and hands the picture to its windows.
    fn render(&mut self) {
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: (self.alpha.clamp(0.0, 1.0) * 255.0) as u8,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let windows: Vec<(HWND, RECT)> = std::iter::once((self.hwnd, self.monitor)).chain(self.others.iter().copied()).collect();
        let Some(canvas) = self.draw_to_canvas() else { return };
        let size = canvas.size;
        for (window, area) in windows {
            // Centred on each monitor.
            let origin = POINT { x: (area.left + area.right - size.0) / 2, y: (area.top + area.bottom - size.1) / 2 };
            unsafe {
                let _ = UpdateLayeredWindow(
                    window,
                    None,
                    Some(&origin),
                    Some(&SIZE { cx: size.0, cy: size.1 }),
                    canvas.dc,
                    Some(&POINT::default()),
                    COLORREF(0),
                    Some(&blend),
                    ULW_ALPHA,
                );
            }
        }
    }

    fn draw(&self, canvas: &Dib) -> Result<()> {
        let s = self.scale;
        let (w, h) = (canvas.size.0 as f32, canvas.size.1 as f32);
        let (cw, ch, radius) = (CELL_W * s, CELL_H * s, 7.0 * s);
        let (reach_x, reach_y) = self.view.reach();
        // The middle of the highlight, and cells `apart` (sideways, down) from it.
        let (mx, my) = (w / 2.0, (PAD + reach_y as f32 * (CELL_H + GAP)) * s + ch / 2.0);
        let cell_at = |(x, y): (f32, f32)| rect(mx + x * (CELL_W + GAP) * s - cw / 2.0, my + y * (CELL_H + GAP) * s - ch / 2.0, cw, ch);

        self.target.bind(canvas)?;
        let target = &self.target.target;
        unsafe {
            target.BeginDraw();
            target.Clear(Some(&rgba(0.0, 0.0, 0.0, 0.0)));
        }
        let p = Painter::new(target)?;

        // Panel with a hairline border.
        let panel = rect(0.5, 0.5, w - 1.0, h - 1.0);
        p.fill(&panel, 16.0 * s, rgba(0.075, 0.08, 0.095, 0.86));
        p.stroke(&panel, 16.0 * s, 1.0, white(0.10));

        let mut emphasised = Vec::new();
        for (r, row) in self.view.rows.iter().enumerate() {
            let down = r as f32 + self.camera.y();
            // The row the hand is on is lit, and the light goes with the hand.
            let near = (1.0 - down.abs()).clamp(0.0, 1.0);
            let x = self.camera.x(r);
            for (c, view) in row.iter().enumerate() {
                let apart = (c as f32 + x, down);
                let shown = fade(apart.0, reach_x) * fade(apart.1, reach_y);
                if shown <= 0.0 {
                    continue;
                }
                let cell = cell_at(apart);
                if view.emphasised {
                    p.fill(&cell, radius, warm((0.30 + 0.12 * near) * shown));
                    emphasised.push((cell, shown));
                } else {
                    // Bright enough that the other cells are seen against
                    // any wallpaper, not only the one being moved to.
                    p.fill(&cell, radius, white((0.22 + 0.10 * near) * shown));
                }
            }
        }

        if self.view.cur.is_some() {
            let highlight = cell_at((0.0, 0.0));
            let (x, y) = (highlight.left, highlight.top);
            for (grow, a) in [(5.0, 0.07), (3.0, 0.12), (1.5, 0.20)] {
                let g = grow * s;
                p.fill(&rect(x - g, y - g, cw + 2.0 * g, ch + 2.0 * g), radius + g, accent(a));
            }
            p.fill(&highlight, radius, accent(1.0));
        }
        // Over the highlight, so that the mark shows on the current cell too.
        for (cell, shown) in &emphasised {
            let inset = 0.75 * s;
            let ring = rect(cell.left + inset, cell.top + inset, cw - 2.0 * inset, ch - 2.0 * inset);
            p.stroke(&ring, radius - inset, 1.5 * s, warm(0.95 * shown));
        }
        if let (Some(format), false) = (&self.text, self.view.title.is_empty()) {
            let bottom = h - PAD * 0.55 * s;
            let area = paint::Rect { left: PAD * s, top: h - (PAD * 0.55 + TITLE_H) * s, right: w - PAD * s, bottom };
            p.text(&self.view.title, format, &area, white(0.88));
        }
        unsafe { target.EndDraw(None, None) }
    }

    /// Draws `view` at full opacity and returns the size and the
    /// premultiplied BGRA pixels, for checking the design without showing a
    /// window. The hand is `hand` (along, across) from the current cell.
    pub fn render_to_pixels(&mut self, view: View, hand: (f32, f32), scale: f32) -> Option<((i32, i32), Vec<u8>)> {
        self.camera = Camera::default();
        self.camera.advance(&view, hand, (0, 0), 0.0);
        self.view = view;
        self.scale = scale;
        let canvas = self.draw_to_canvas()?;
        Some((canvas.size, canvas.pixels().to_vec()))
    }

    /// One frame of a made-up hold, `dt` seconds after the one before, drawn
    /// as `render_to_pixels` does; with where the grid is drawn: each row's
    /// first cell sideways, and the first row down, from the highlight.
    pub fn frame_to_pixels(&mut self, view: View, hand: Hand, dt: f32, scale: f32) -> Option<((i32, i32), Vec<u8>, (Vec<f32>, f32))> {
        if view != self.view {
            self.views += 1;
        }
        self.camera.advance(&view, (hand.along, hand.across), (self.views, hand.counted_from), dt);
        let drawn = ((0..view.rows.len()).map(|r| self.camera.x(r)).collect(), self.camera.y());
        self.view = view;
        self.scale = scale;
        let canvas = self.draw_to_canvas()?;
        Some((canvas.size, canvas.pixels().to_vec(), drawn))
    }
}

/// Tells the debug record where the grid is drawn this frame.
#[cfg(feature = "debug-log")]
fn log_frame(view: &View, camera: &Camera, hand: Hand) {
    let Some(cur) = view.cur else { return };
    let rows: Vec<String> = (0..view.rows.len()).map(|r| format!("{:.3}", camera.x(r))).collect();
    debug_log!(
        "minimap: frame cur {},{} hand {:+.3},{:+.3} rows x [{}] y {:.3}",
        cur.row,
        cur.col,
        hand.along,
        hand.across,
        rows.join(" "),
        camera.y()
    );
}

fn text_format(scale: f32) -> Result<IDWriteTextFormat> {
    paint::text_format(&paint::dwrite_factory()?, 13.0 * scale, 600, DWRITE_TEXT_ALIGNMENT_CENTER)
}

/// What the app asks of the minimap.
enum Request {
    Show(View, Option<Pos>),
    Update(View),
    HideAfter(u64),
}

/// The minimap on a thread of its own. Switching desktops keeps the app's
/// thread busy for tens of milliseconds at a time; drawn from there the
/// highlight moved in jerks. Here it is redrawn once per screen refresh no
/// matter what the app is doing.
pub struct Minimap {
    requests: mpsc::Sender<Request>,
}

impl Minimap {
    pub fn spawn() -> Minimap {
        let (requests, inbox) = mpsc::channel::<Request>();
        std::thread::spawn(move || {
            let mut overlay = match Overlay::new() {
                Ok(overlay) => overlay,
                Err(e) => return crate::app::log(&format!("minimap failed: {e}")),
            };
            let apply = |overlay: &mut Overlay, request: Request| match request {
                Request::Show(view, from) => overlay.show(view, from),
                Request::Update(view) => overlay.update(view),
                Request::HideAfter(ms) => overlay.hide_after(ms),
            };
            loop {
                // Nothing on screen: sleep until asked for something.
                if !overlay.is_visible() {
                    match inbox.recv() {
                        Ok(request) => apply(&mut overlay, request),
                        Err(_) => return,
                    }
                }
                while let Ok(request) = inbox.try_recv() {
                    apply(&mut overlay, request);
                }
                unsafe {
                    let mut message = MSG::default();
                    while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                        DispatchMessageW(&message);
                    }
                    if overlay.is_visible() {
                        overlay.tick();
                        // Wait for the next refresh of the screen.
                        if DwmFlush().is_err() {
                            std::thread::sleep(Duration::from_millis(8));
                        }
                    }
                }
            }
        });
        Minimap { requests }
    }

    pub fn show(&self, view: View, from: Option<Pos>) {
        let _ = self.requests.send(Request::Show(view, from));
    }

    pub fn update(&self, view: View) {
        let _ = self.requests.send(Request::Update(view));
    }

    pub fn hide_after(&self, ms: u64) {
        let _ = self.requests.send(Request::HideAfter(ms));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One cell right and one row down at a steady hand, the app getting to
    /// each a few frames late and the count of the hand catching up with it
    /// a frame later still (as it does: the view is sent before the app
    /// tells the hook where it is). No row may move further in a frame than
    /// the hand does and a little (it jumped a whole cell, or the 0.4 of one
    /// a row is let go at), nor stand still while the hand goes on (it did
    /// for a frame or two), nor stop short of where the hand ends.
    #[test]
    fn the_grid_does_not_jump_when_the_app_gets_to_a_cell_or_a_row() {
        let view = |cur: Pos, anchors: Vec<usize>| View {
            rows: vec![vec![CellView::default(); 3], vec![CellView::default(); 2], vec![CellView::default(); 4]],
            anchors,
            cur: Some(cur),
            title: String::new(),
        };
        let mut camera = Camera::default();
        let (mut before, mut views): (Option<(Vec<f32>, f32)>, u64) = (None, 0);
        let speed = 1.2 / 20.0;
        for frame in 0..80 {
            let t = frame as f32;
            // The hand, in the grid's cells: along row 1, then down to row 2.
            let col = (t * speed).min(1.2);
            let row = 1.0 + ((t - 30.0) * speed).clamp(0.0, 1.0);
            // When the hand came to each, by frames; the app shows it 3 frames
            // on and the count follows a frame after that.
            let came_col = (0.6 / speed).ceil() as i32;
            let came_row = 30 + (0.6 / speed).ceil() as i32;
            let f = frame as i32;
            let (view_col, told_col) = (if f >= came_col + 3 { 1 } else { 0 }, if f >= came_col + 4 { 1 } else { 0 });
            let (view_row, told_row) = (if f >= came_row + 3 { 2 } else { 1 }, if f >= came_row + 4 { 2 } else { 1 });
            // A new stretch sideways begins as the hand comes to row 2.
            let stretch = (f >= came_row) as u64;
            let along = if stretch == 1 { 0.0 } else { col - told_col as f32 };
            let cur = if view_row == 2 { Pos { row: 2, col: 2 } } else { Pos { row: 1, col: view_col } };
            let v = view(cur, vec![1, view_col, 2]);
            views += (f == came_col + 3 || f == came_row + 3) as u64;
            camera.advance(&v, (along, row - told_row as f32), (views, stretch << 8 | (told_col as u64) << 4 | told_row as u64), 1.0 / 60.0);
            let now: (Vec<f32>, f32) = ((0..3).map(|r| camera.x(r)).collect(), camera.y());
            if let Some((xs, y)) = &before {
                for (r, (a, b)) in xs.iter().zip(&now.0).enumerate() {
                    assert!((a - b).abs() <= speed * 1.25, "row {r} jumped {:.3} in frame {frame}", b - a);
                }
                assert!((y - now.1).abs() <= speed * 1.25, "the grid jumped {:.3} down in frame {frame}", now.1 - y);
                // While the hand goes along row 1, and then down.
                if (1..=19).contains(&frame) {
                    assert!((xs[1] - now.0[1]).abs() >= speed * 0.5, "row 1 stood still in frame {frame}");
                }
                if (31..=46).contains(&frame) {
                    assert!((y - now.1).abs() >= speed * 0.5, "the grid stood still in frame {frame}");
                }
            }
            before = Some(now);
        }
        let (xs, y) = before.unwrap();
        assert!((y + 2.0).abs() < 0.01 && (xs[1] + 1.0).abs() < 0.01 && (xs[2] + 2.0).abs() < 0.01, "ends at {xs:?} {y}");
    }
}
