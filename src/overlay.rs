//! The minimap shown while navigating: a translucent panel with one rounded
//! square per desktop and a highlight that slides to the current one.
//!
//! It is a click-through layered tool window (tool windows are shown on every
//! virtual desktop) drawn with Direct2D into a premultiplied-alpha bitmap.

use crate::{
    grid::{Dir, Pos},
    paint::{self, accent, rect, rgba, white, DcTarget, Dib, Painter},
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
/// Room under the grid for the workspace name.
const TITLE_H: f32 = 26.0;
/// Time constants (seconds) of the exponential easing.
const SLIDE_TAU: f32 = 0.018;
const FADE_TAU: f32 = 0.025;

/// What the minimap should show. `rows[r][c]` is true for a cell that was
/// just created and is still empty.
#[derive(Clone, Default)]
pub struct View {
    pub rows: Vec<Vec<bool>>,
    /// Per row, the column a vertical move would land on. Rows are shifted
    /// sideways so that these line up in one column.
    pub anchors: Vec<usize>,
    pub cur: Option<Pos>,
    /// The user is pushing against this edge; enough pushes create a cell.
    pub pushing: Option<Dir>,
    /// Name of the current workspace, shown under the grid.
    pub title: String,
}

impl View {
    fn anchor(&self, row: usize) -> usize {
        self.anchors.get(row).copied().unwrap_or(0)
    }

    /// Cells in front of the lined-up column.
    fn lead(&self) -> usize {
        (0..self.rows.len()).map(|r| self.anchor(r)).max().unwrap_or(0)
    }

    /// How far `row` is shifted right, in cells.
    fn shift(&self, row: usize) -> f32 {
        (self.lead() - self.anchor(row)) as f32
    }

    fn shifts(&self) -> Vec<f32> {
        (0..self.rows.len()).map(|r| self.shift(r)).collect()
    }

    /// Width of the whole shifted grid, in cells.
    fn columns(&self) -> usize {
        let tail = self.rows.iter().enumerate().map(|(r, row)| row.len().saturating_sub(self.anchor(r))).max().unwrap_or(1);
        (self.lead() + tail).max(1)
    }
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
    /// Text format for the workspace name, made for `text_scale`.
    text: Option<IDWriteTextFormat>,
    text_scale: f32,
    /// The bitmap the minimap is drawn into, remade when its size changes.
    canvas: Option<Dib>,
    view: View,
    /// Highlight position in (column, row) cell units, eased toward `view.cur`.
    /// The column is measured on screen, i.e. after the row's shift.
    highlight: (f32, f32),
    /// Each row's sideways shift in cells, eased toward `View::shift`.
    shifts: Vec<f32>,
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
            highlight: (0.0, 0.0),
            shifts: Vec::new(),
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
    /// the highlight starts at `from` (the cell just left) so the first move
    /// is animated too.
    pub fn show(&mut self, view: View, from: Option<Pos>) {
        if !self.visible {
            self.place_on_monitors();
            let start = from.or(view.cur).unwrap_or(Pos { row: 0, col: 0 });
            self.shifts = view.shifts();
            self.highlight = (start.col as f32 + view.shift(start.row), start.row as f32);
            self.visible = true;
            self.last_tick = Instant::now();
        }
        self.view = view;
        self.hide_at = None;
        self.render();
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
        let k = 1.0 - (-dt / SLIDE_TAU).exp();
        self.shifts.resize(self.view.rows.len(), 0.0);
        for (r, shift) in self.shifts.iter_mut().enumerate() {
            *shift += (self.view.shift(r) - *shift) * k;
        }
        if let Some(cur) = self.view.cur {
            self.highlight.0 += (cur.col as f32 + self.view.shift(cur.row) - self.highlight.0) * k;
            self.highlight.1 += (cur.row as f32 - self.highlight.1) * k;
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

    fn wanted_size(&self) -> (i32, i32) {
        let cols = self.view.columns() as f32;
        let rows = self.view.rows.len().max(1) as f32;
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
        let cell_x = |col: f32| (PAD + col * (CELL_W + GAP)) * s;
        let cell_y = |row: f32| (PAD + row * (CELL_H + GAP)) * s;
        let (cw, ch, radius) = (CELL_W * s, CELL_H * s, 7.0 * s);

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

        let cur = self.view.cur;
        for (r, row) in self.view.rows.iter().enumerate() {
            let in_current_row = cur.is_some_and(|c| c.row == r);
            let shift = self.shifts.get(r).copied().unwrap_or_else(|| self.view.shift(r));
            for (c, &fresh) in row.iter().enumerate() {
                let cell = rect(cell_x(c as f32 + shift), cell_y(r as f32), cw, ch);
                if fresh {
                    // A cell that will vanish again if left empty: outline only.
                    p.stroke(&cell, radius, 1.2 * s, white(0.30));
                } else {
                    p.fill(&cell, radius, white(if in_current_row { 0.17 } else { 0.08 }));
                }
            }
        }

        if cur.is_some() {
            let (x, y) = (cell_x(self.highlight.0), cell_y(self.highlight.1));
            for (grow, a) in [(5.0, 0.07), (3.0, 0.12), (1.5, 0.20)] {
                let g = grow * s;
                p.fill(&rect(x - g, y - g, cw + 2.0 * g, ch + 2.0 * g), radius + g, accent(a));
            }
            p.fill(&rect(x, y, cw, ch), radius, accent(1.0));

            // Pushing against an edge: a bar on that side of the current cell.
            if let Some(dir) = self.view.pushing {
                let (bar, off) = (4.0 * s, 8.0 * s);
                let shape = match dir {
                    Dir::Left => rect(x - off - bar, y + ch * 0.2, bar, ch * 0.6),
                    Dir::Right => rect(x + cw + off, y + ch * 0.2, bar, ch * 0.6),
                    Dir::Up => rect(x + cw * 0.25, y - off - bar, cw * 0.5, bar),
                    Dir::Down => rect(x + cw * 0.25, y + ch + off, cw * 0.5, bar),
                };
                p.fill(&shape, bar / 2.0, accent(0.95));
            }
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
    /// window. `highlight` is in (column, row) cell units, the column counted
    /// within its row.
    pub fn render_to_pixels(&mut self, view: View, highlight: (f32, f32), scale: f32) -> Option<((i32, i32), Vec<u8>)> {
        self.shifts = view.shifts();
        self.highlight = (highlight.0 + view.shift(highlight.1 as usize), highlight.1);
        self.view = view;
        self.scale = scale;
        let canvas = self.draw_to_canvas()?;
        Some((canvas.size, canvas.pixels().to_vec()))
    }
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
