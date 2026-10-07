//! The minimap shown while navigating: a translucent panel with one rounded
//! square per desktop and a highlight that slides to the current one.
//!
//! It is a click-through layered tool window (tool windows are shown on every
//! virtual desktop) drawn with Direct2D into a premultiplied-alpha bitmap.

use crate::grid::{Dir, Pos};
use std::{ffi::c_void, time::Instant};
use windows::{
    core::{w, Result},
    Win32::{
        Foundation::{BOOL, COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM},
        Graphics::{
            Dwm::DwmFlush,
            Direct2D::{
                Common::{D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_RECT_F},
                D2D1CreateFactory, ID2D1DCRenderTarget, ID2D1Factory, D2D1_FACTORY_TYPE_SINGLE_THREADED,
                D2D1_FEATURE_LEVEL_DEFAULT, D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_DEFAULT,
                D2D1_RENDER_TARGET_USAGE_GDI_COMPATIBLE, D2D1_ROUNDED_RECT,
            },
            Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
            Gdi::{
                CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, EnumDisplayMonitors, GetMonitorInfoW,
                HMONITOR, MonitorFromPoint,
                SelectObject, AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION,
                DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ, MONITORINFO, MONITOR_DEFAULTTOPRIMARY,
            },
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI},
            WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, PeekMessageW, RegisterClassW, SetWindowPos,
                ShowWindow, HWND_TOPMOST, MSG, PM_REMOVE,
                SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW,
                UpdateLayeredWindow, SW_HIDE, ULW_ALPHA, WNDCLASSW, WS_EX_LAYERED,
                WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
            },
        },
    },
};


const CELL_W: f32 = 46.0;
const CELL_H: f32 = 30.0;
const GAP: f32 = 7.0;
const PAD: f32 = 20.0;
/// Time constants (seconds) of the exponential easing.
const SLIDE_TAU: f32 = 0.018;
const FADE_TAU: f32 = 0.025;

const ACCENT: (f32, f32, f32) = (0.34, 0.62, 1.0);

/// What the minimap should show. `rows[r][c]` is true for a cell that was
/// just created and is still empty.
#[derive(Clone, Default)]
pub struct View {
    pub rows: Vec<Vec<bool>>,
    /// Per row, the column a vertical move would land on. Rows are shifted
    /// sideways so that these line up in one column.
    pub anchors: Vec<usize>,
    pub cur: Option<Pos>,
    /// The user is pushing against this edge; one more push creates a cell.
    pub pushing: Option<Dir>,
}

pub struct Overlay {
    hwnd: HWND,
    target: ID2D1DCRenderTarget,
    dc: HDC,
    bitmap: HBITMAP,
    stock: HGDIOBJ,
    bits: *mut c_void,
    size: (i32, i32),
    view: View,
    /// Highlight position in (column, row) cell units, eased toward `view.cur`.
    /// The column is measured on screen, i.e. after the row's shift.
    highlight: (f32, f32),
    /// Each row's sideways shift in cells, eased toward `View::shift`.
    shifts: Vec<f32>,
    alpha: f32,
    visible: bool,
    hide_at: Option<Instant>,
    last_tick: Instant,
    scale: f32,
    monitor: RECT,
    /// The same picture is shown on every other monitor through one more
    /// window each: (window, that monitor's work area).
    others: Vec<(HWND, RECT)>,
}

fn color(rgb: (f32, f32, f32), a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r: rgb.0, g: rgb.1, b: rgb.2, a }
}

fn rounded(x: f32, y: f32, w: f32, h: f32, radius: f32) -> D2D1_ROUNDED_RECT {
    D2D1_ROUNDED_RECT {
        rect: D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + h },
        radiusX: radius,
        radiusY: radius,
    }
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

    /// Width of the whole shifted grid, in cells.
    fn columns(&self) -> usize {
        let tail = self.rows.iter().enumerate().map(|(r, row)| row.len().saturating_sub(self.anchor(r))).max().unwrap_or(1);
        (self.lead() + tail).max(1)
    }
}

impl Overlay {
    pub fn new() -> Result<Overlay> {
        unsafe extern "system" fn wndproc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
            DefWindowProcW(hwnd, message, wparam, lparam)
        }
        unsafe {
            let instance = GetModuleHandleW(None)?;
            let class = w!("flick.overlay");
            RegisterClassW(&WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: instance.into(),
                lpszClassName: class,
                ..Default::default()
            });
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
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
            let factory: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let target = factory.CreateDCRenderTarget(&D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 0.0,
                dpiY: 0.0,
                usage: D2D1_RENDER_TARGET_USAGE_GDI_COMPATIBLE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            })?;
            Ok(Overlay {
                hwnd,
                target,
                dc: CreateCompatibleDC(None),
                bitmap: HBITMAP(0),
                stock: HGDIOBJ(0),
                bits: std::ptr::null_mut(),
                size: (0, 0),
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
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    /// Shows `view`. When the overlay was hidden the highlight starts at
    /// `from` (the cell just left) so the first move is animated too.
    /// `linger_ms` hides it again after that long; `None` keeps it up until
    /// `hide_after` is called.
    pub fn show(&mut self, view: View, from: Option<Pos>, linger_ms: Option<u64>) {
        if !self.visible {
            self.place_on_cursor_monitor();
            let start = from.or(view.cur).unwrap_or(Pos { row: 0, col: 0 });
            self.shifts = (0..view.rows.len()).map(|r| view.shift(r)).collect();
            self.highlight = (start.col as f32 + view.shift(start.row), start.row as f32);
            self.visible = true;
            self.last_tick = Instant::now();
        }
        self.view = view;
        self.hide_at = linger_ms.map(|ms| Instant::now() + std::time::Duration::from_millis(ms));
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
            self.hide_at = Some(Instant::now() + std::time::Duration::from_millis(ms));
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

    /// Finds the monitors: the one under the cursor sets the scale, and each
    /// of the others gets a window of its own to show the minimap too.
    fn place_on_cursor_monitor(&mut self) {
        unsafe extern "system" fn collect(monitor: HMONITOR, _: HDC, _: *mut RECT, out: LPARAM) -> BOOL {
            (*(out.0 as *mut Vec<HMONITOR>)).push(monitor);
            true.into()
        }
        unsafe {
            // Scale by the primary monitor: the minimap is the same on every
            // screen, wherever the cursor happens to be.
            let monitor = MonitorFromPoint(POINT::default(), MONITOR_DEFAULTTOPRIMARY);
            let work_area = |monitor: HMONITOR| {
                let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
                GetMonitorInfoW(monitor, &mut info).as_bool().then_some(info.rcWork)
            };
            if let Some(area) = work_area(monitor) {
                self.monitor = area;
            }
            let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
            let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
            self.scale = dpi_x as f32 / 96.0;

            let mut all: Vec<HMONITOR> = Vec::new();
            EnumDisplayMonitors(None, None, Some(collect), LPARAM(&mut all as *mut _ as isize));
            let areas: Vec<RECT> = all.into_iter().filter(|m| *m != monitor).filter_map(work_area).collect();
            while self.others.len() > areas.len() {
                if let Some((window, _)) = self.others.pop() {
                    let _ = DestroyWindow(window);
                }
            }
            while self.others.len() < areas.len() {
                let Ok(instance) = GetModuleHandleW(None) else { break };
                let window = CreateWindowExW(
                    WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                    w!("flick.overlay"),
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
                self.others.push((window, RECT::default()));
            }
            for ((_, area), new) in self.others.iter_mut().zip(areas) {
                *area = new;
            }
        }
    }

    fn wanted_size(&self) -> (i32, i32) {
        let cols = self.view.columns() as f32;
        let rows = self.view.rows.len().max(1) as f32;
        let w = PAD * 2.0 + cols * CELL_W + (cols - 1.0) * GAP;
        let h = PAD * 2.0 + rows * CELL_H + (rows - 1.0) * GAP;
        ((w * self.scale).ceil() as i32, (h * self.scale).ceil() as i32)
    }

    fn ensure_bitmap(&mut self, size: (i32, i32)) -> bool {
        if size == self.size && self.bitmap.0 != 0 {
            return true;
        }
        unsafe {
            if self.bitmap.0 != 0 {
                SelectObject(self.dc, self.stock);
                DeleteObject(self.bitmap);
                self.bitmap = HBITMAP(0);
            }
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: size.0,
                    biHeight: -size.1,
                    biPlanes: 1,
                    biBitCount: 32,
                    ..Default::default()
                },
                ..Default::default()
            };
            let Ok(bitmap) = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut self.bits, None, 0) else {
                return false;
            };
            self.bitmap = bitmap;
            self.stock = SelectObject(self.dc, bitmap);
            self.size = size;
        }
        true
    }

    fn render(&mut self) {
        let size = self.wanted_size();
        if !self.ensure_bitmap(size) || self.draw().is_err() {
            return;
        }
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: (self.alpha.clamp(0.0, 1.0) * 255.0) as u8,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let windows = std::iter::once((self.hwnd, self.monitor)).chain(self.others.iter().copied());
        for (window, area) in windows {
            // Centred on each monitor.
            let origin = POINT { x: (area.left + area.right - size.0) / 2, y: (area.top + area.bottom - size.1) / 2 };
            unsafe {
                let _ = UpdateLayeredWindow(
                    window,
                    None,
                    Some(&origin),
                    Some(&SIZE { cx: size.0, cy: size.1 }),
                    self.dc,
                    Some(&POINT::default()),
                    COLORREF(0),
                    Some(&blend),
                    ULW_ALPHA,
                );
            }
        }
    }

    fn draw(&self) -> Result<()> {
        let s = self.scale;
        let (w, h) = (self.size.0 as f32, self.size.1 as f32);
        let cell_x = |col: f32| (PAD + col * (CELL_W + GAP)) * s;
        let cell_y = |row: f32| (PAD + row * (CELL_H + GAP)) * s;
        let (cw, ch, radius) = (CELL_W * s, CELL_H * s, 7.0 * s);
        const WHITE: (f32, f32, f32) = (1.0, 1.0, 1.0);

        unsafe {
            let t = &self.target;
            t.BindDC(self.dc, &RECT { left: 0, top: 0, right: self.size.0, bottom: self.size.1 })?;
            t.BeginDraw();
            t.Clear(Some(&color((0.0, 0.0, 0.0), 0.0)));
            let brush = t.CreateSolidColorBrush(&color(WHITE, 1.0), None)?;
            let fill = |shape: &D2D1_ROUNDED_RECT, c: D2D1_COLOR_F| {
                brush.SetColor(&c);
                t.FillRoundedRectangle(shape, &brush);
            };

            // Panel with a hairline border.
            fill(&rounded(0.5, 0.5, w - 1.0, h - 1.0, 16.0 * s), color((0.075, 0.08, 0.095), 0.86));
            brush.SetColor(&color(WHITE, 0.10));
            t.DrawRoundedRectangle(&rounded(0.5, 0.5, w - 1.0, h - 1.0, 16.0 * s), &brush, 1.0, None);

            let cur = self.view.cur;
            for (r, row) in self.view.rows.iter().enumerate() {
                let in_current_row = cur.is_some_and(|c| c.row == r);
                let shift = self.shifts.get(r).copied().unwrap_or_else(|| self.view.shift(r));
                for (c, &fresh) in row.iter().enumerate() {
                    let shape = rounded(cell_x(c as f32 + shift), cell_y(r as f32), cw, ch, radius);
                    if fresh {
                        // A cell that will vanish again if left empty: outline only.
                        brush.SetColor(&color(WHITE, 0.30));
                        t.DrawRoundedRectangle(&shape, &brush, 1.2 * s, None);
                    } else {
                        fill(&shape, color(WHITE, if in_current_row { 0.17 } else { 0.08 }));
                    }
                }
            }

            if cur.is_some() {
                let (x, y) = (cell_x(self.highlight.0), cell_y(self.highlight.1));
                for (grow, a) in [(5.0, 0.07), (3.0, 0.12), (1.5, 0.20)] {
                    let g = grow * s;
                    fill(&rounded(x - g, y - g, cw + 2.0 * g, ch + 2.0 * g, radius + g), color(ACCENT, a));
                }
                fill(&rounded(x, y, cw, ch, radius), color(ACCENT, 1.0));

                // Pushing against an edge: a bar on that side of the current cell.
                if let Some(dir) = self.view.pushing {
                    let (bar, off) = (4.0 * s, 8.0 * s);
                    let shape = match dir {
                        Dir::Left => rounded(x - off - bar, y + ch * 0.2, bar, ch * 0.6, bar / 2.0),
                        Dir::Right => rounded(x + cw + off, y + ch * 0.2, bar, ch * 0.6, bar / 2.0),
                        Dir::Up => rounded(x + cw * 0.25, y - off - bar, cw * 0.5, bar, bar / 2.0),
                        Dir::Down => rounded(x + cw * 0.25, y + ch + off, cw * 0.5, bar, bar / 2.0),
                    };
                    fill(&shape, color(ACCENT, 0.95));
                }
            }
            t.EndDraw(None, None)
        }
    }

    /// Draws `view` at full opacity and returns the premultiplied BGRA pixels,
    /// for checking the design without showing a window.
    pub fn render_to_pixels(&mut self, view: View, highlight: (f32, f32), scale: f32) -> Option<(i32, i32, Vec<u8>)> {
        self.shifts = (0..view.rows.len()).map(|r| view.shift(r)).collect();
        self.highlight = (highlight.0 + view.shift(highlight.1 as usize), highlight.1);
        self.view = view;
        self.scale = scale;
        let size = self.wanted_size();
        if !self.ensure_bitmap(size) || self.draw().is_err() {
            return None;
        }
        let len = (size.0 * size.1 * 4) as usize;
        let pixels = unsafe { std::slice::from_raw_parts(self.bits as *const u8, len) }.to_vec();
        Some((size.0, size.1, pixels))
    }
}

impl Drop for Overlay {
    fn drop(&mut self) {
        unsafe {
            if self.bitmap.0 != 0 {
                SelectObject(self.dc, self.stock);
                DeleteObject(self.bitmap);
            }
            DeleteDC(self.dc);
        }
    }
}

/// What the app asks of the minimap.
enum Request {
    Show(View, Option<Pos>, Option<u64>),
    Update(View),
    HideAfter(u64),
}

/// The minimap on a thread of its own. Switching desktops keeps the app's
/// thread busy for tens of milliseconds at a time; drawn from there the
/// highlight moved in jerks. Here it is redrawn once per screen refresh no
/// matter what the app is doing.
pub struct Minimap {
    requests: std::sync::mpsc::Sender<Request>,
}

impl Minimap {
    pub fn spawn() -> Minimap {
        let (requests, inbox) = std::sync::mpsc::channel::<Request>();
        std::thread::spawn(move || {
            let Ok(mut overlay) = Overlay::new() else { return };
            let apply = |overlay: &mut Overlay, request: Request| match request {
                Request::Show(view, from, linger) => overlay.show(view, from, linger),
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
                            std::thread::sleep(std::time::Duration::from_millis(8));
                        }
                    }
                }
            }
        });
        Minimap { requests }
    }

    pub fn show(&self, view: View, from: Option<Pos>, linger_ms: Option<u64>) {
        let _ = self.requests.send(Request::Show(view, from, linger_ms));
    }

    pub fn update(&self, view: View) {
        let _ = self.requests.send(Request::Update(view));
    }

    pub fn hide_after(&self, ms: u64) {
        let _ = self.requests.send(Request::HideAfter(ms));
    }
}
