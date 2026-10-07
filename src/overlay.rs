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
        Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM},
        Graphics::{
            Direct2D::{
                Common::{D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_RECT_F},
                D2D1CreateFactory, ID2D1DCRenderTarget, ID2D1Factory, D2D1_FACTORY_TYPE_SINGLE_THREADED,
                D2D1_FEATURE_LEVEL_DEFAULT, D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_DEFAULT,
                D2D1_RENDER_TARGET_USAGE_GDI_COMPATIBLE, D2D1_ROUNDED_RECT,
            },
            Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
            Gdi::{
                CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetMonitorInfoW, MonitorFromPoint,
                SelectObject, AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION,
                DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ, MONITORINFO, MONITOR_DEFAULTTONEAREST,
            },
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI},
            WindowsAndMessaging::{
                CreateWindowExW, GetCursorPos, KillTimer, RegisterClassW, SetTimer, ShowWindow,
                UpdateLayeredWindow, SW_HIDE, SW_SHOWNOACTIVATE, ULW_ALPHA, WNDCLASSW, WS_EX_LAYERED,
                WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
            },
        },
    },
};

pub const TIMER_ID: usize = 1;

const CELL_W: f32 = 46.0;
const CELL_H: f32 = 30.0;
const GAP: f32 = 7.0;
const PAD: f32 = 20.0;
/// Time constants (seconds) of the exponential easing.
const SLIDE_TAU: f32 = 0.045;
const FADE_TAU: f32 = 0.040;

const ACCENT: (f32, f32, f32) = (0.34, 0.62, 1.0);

/// What the minimap should show. `rows[r][c]` is true for a cell that was
/// just created and is still empty.
#[derive(Clone, Default)]
pub struct View {
    pub rows: Vec<Vec<bool>>,
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
    highlight: (f32, f32),
    alpha: f32,
    visible: bool,
    hide_at: Option<Instant>,
    last_tick: Instant,
    scale: f32,
    monitor: RECT,
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

impl Overlay {
    pub fn new(wndproc: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT) -> Result<Overlay> {
        unsafe {
            let instance = GetModuleHandleW(None)?;
            let class = w!("kankan.overlay");
            RegisterClassW(&WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: instance.into(),
                lpszClassName: class,
                ..Default::default()
            });
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                class,
                w!("KanKan"),
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
                alpha: 0.0,
                visible: false,
                hide_at: None,
                last_tick: Instant::now(),
                scale: 1.0,
                monitor: RECT::default(),
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
            self.highlight = (start.col as f32, start.row as f32);
            self.visible = true;
            self.last_tick = Instant::now();
            unsafe {
                SetTimer(self.hwnd, TIMER_ID, 10, None);
            }
        }
        self.view = view;
        self.hide_at = linger_ms.map(|ms| Instant::now() + std::time::Duration::from_millis(ms));
        self.render();
        unsafe {
            ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
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
        if let Some(cur) = self.view.cur {
            let k = 1.0 - (-dt / SLIDE_TAU).exp();
            self.highlight.0 += (cur.col as f32 - self.highlight.0) * k;
            self.highlight.1 += (cur.row as f32 - self.highlight.1) * k;
        }

        if hiding && self.alpha < 0.02 {
            self.alpha = 0.0;
            self.visible = false;
            unsafe {
                let _ = KillTimer(self.hwnd, TIMER_ID);
                ShowWindow(self.hwnd, SW_HIDE);
            }
            return;
        }
        self.render();
    }

    fn place_on_cursor_monitor(&mut self) {
        unsafe {
            let mut cursor = POINT::default();
            let _ = GetCursorPos(&mut cursor);
            let monitor = MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST);
            let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            if GetMonitorInfoW(monitor, &mut info).as_bool() {
                self.monitor = info.rcWork;
            }
            let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
            let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
            self.scale = dpi_x as f32 / 96.0;
        }
    }

    fn wanted_size(&self) -> (i32, i32) {
        let cols = self.view.rows.iter().map(Vec::len).max().unwrap_or(1).max(1) as f32;
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
        let origin = POINT {
            x: (self.monitor.left + self.monitor.right - size.0) / 2,
            y: (self.monitor.top + self.monitor.bottom - size.1) / 2,
        };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: (self.alpha.clamp(0.0, 1.0) * 255.0) as u8,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        unsafe {
            let _ = UpdateLayeredWindow(
                self.hwnd,
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
                for (c, &fresh) in row.iter().enumerate() {
                    let shape = rounded(cell_x(c as f32), cell_y(r as f32), cw, ch, radius);
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
        self.view = view;
        self.highlight = highlight;
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
