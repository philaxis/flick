//! The settings window: the directions of the gesture drawn as a dial, the
//! trigger, the distances and whether to start with Windows.
//!
//! It owns layout, hit-testing and drawing, in the look of the board. It
//! changes nothing itself; what the user does is turned into an `Action` for
//! the app, which answers with a new `View`.

use crate::{
    config::VERTICAL_ANGLES,
    paint::{self, accent, on_circle, rect, rgba, white, Canvas, Painter, Rect},
    vd,
};
use windows::{
    core::{w, Error, Result},
    Win32::{
        Foundation::{BOOL, COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM},
        Graphics::{
            Direct2D::{Common::D2D1_COLOR_F, ID2D1Factory, ID2D1RenderTarget},
            DirectWrite::{
                IDWriteTextFormat, DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING,
                DWRITE_TEXT_ALIGNMENT_TRAILING,
            },
            Dwm::{DwmSetWindowAttribute, DWMWA_CAPTION_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE},
            Gdi::{BitBlt, GetDC, InvalidateRect, ReleaseDC, ValidateRect, SRCCOPY},
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            HiDpi::AdjustWindowRectExForDpi,
            Input::KeyboardAndMouse::{
                ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VK_ESCAPE,
            },
            WindowsAndMessaging::{
                CreateWindowExW, GetClientRect, IsWindowVisible, LoadCursorW, RegisterClassW, SetWindowPos,
                ShowWindow, HWND_TOPMOST, IDC_ARROW, SWP_NOACTIVATE, SWP_NOZORDER, SWP_SHOWWINDOW, SW_HIDE,
                WM_DPICHANGED, WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_PAINT,
                WNDCLASSW, WS_CAPTION, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP, WS_SYSMENU,
            },
        },
    },
};

/// Sent when the cursor leaves a window that asked to be told
/// (`TrackMouseEvent`).
const WM_MOUSELEAVE: u32 = 0x02A3;

/// Size of the window's inside, in unscaled units.
const SIZE: (f32, f32) = (400.0, 658.0);
/// The dial: its centre and its radius.
const DIAL: ((f32, f32), f32) = ((200.0, 146.0), 104.0);
/// Top of the first row and the height of each.
const ROWS: (f32, f32) = (330.0, 46.0);
/// Left edge of the labels and right edge of the controls.
const EDGES: (f32, f32) = (28.0, 372.0);
/// What the distance sliders span, and the step they move in.
const DISTANCES: (i32, i32, i32) = (80, 600, 10);
/// The same for how long to rest on a cell, in milliseconds; none is "off".
const RESTS: (i32, i32, i32) = (0, 1500, 100);

/// What the window shows.
#[derive(Clone, Default)]
pub struct View {
    /// The trigger in words.
    pub trigger: String,
    /// Waiting for the user to press what the trigger should become.
    pub capturing: bool,
    /// See `Config::vertical_angle`, `step_x`, `step_y`, `vertical_sticky`.
    pub angle: f32,
    pub step_x: i32,
    pub step_y: i32,
    pub sticky: bool,
    /// See `Config::dwell_ms`.
    pub dwell_ms: i32,
    pub autostart: bool,
}

/// What the user asked for; carried out by the app.
pub enum Action {
    None,
    /// Use these for the gesture right away; a control is being dragged.
    Try { angle: f32, step_x: i32, step_y: i32 },
    /// Write a value to the config file: its key and the value as TOML.
    Set(&'static str, String),
    /// Start (or give up) waiting for a new trigger to be pressed.
    Capture(bool),
    Autostart(bool),
    /// Open the config file, which has the settings not shown here.
    OpenFile,
    Close,
}

#[derive(Clone, Copy, PartialEq, Default)]
enum Hit {
    #[default]
    Nothing,
    Dial,
    Trigger,
    StepX,
    /// One half of the vertical mode switch: flick (true) or distance.
    Mode(bool),
    StepY,
    Dwell,
    Autostart,
    File,
}

/// Where the controls are, in pixels.
struct Layout {
    dial: ((f32, f32), f32),
    trigger: Rect,
    step_x: Rect,
    mode: [Rect; 2],
    step_y: Rect,
    dwell: Rect,
    autostart: Rect,
    file: Rect,
}

impl Layout {
    fn new(s: f32) -> Layout {
        let row = |i: f32| ROWS.0 + i * ROWS.1;
        // A control `w` wide and `h` high at the right of row `i`.
        let right = |i: f32, w: f32, h: f32| rect((EDGES.1 - w) * s, (row(i) + (ROWS.1 - h) / 2.0) * s, w * s, h * s);
        let slider = |i: f32| rect(150.0 * s, row(i) * s, 166.0 * s, ROWS.1 * s);
        let mode = right(2.0, 160.0, 30.0);
        let half = (mode.right - mode.left) / 2.0;
        Layout {
            dial: ((DIAL.0 .0 * s, DIAL.0 .1 * s), DIAL.1 * s),
            trigger: right(0.0, 64.0, 30.0),
            step_x: slider(1.0),
            mode: [Rect { right: mode.left + half, ..mode }, Rect { left: mode.left + half, ..mode }],
            step_y: slider(3.0),
            dwell: slider(4.0),
            autostart: right(5.0, 44.0, 24.0),
            file: rect(EDGES.0 * s, (row(6.0) + 8.0) * s, 150.0 * s, 26.0 * s),
        }
    }

    fn hit(&self, x: f32, y: f32, s: f32) -> Hit {
        let inside = |r: &Rect| x >= r.left && x < r.right && y >= r.top && y < r.bottom;
        let (centre, radius) = self.dial;
        // A little past the rim, where the handles stick out; not the very
        // middle, where the direction of the cursor means nothing.
        let out = (x - centre.0).hypot(y - centre.1);
        if out >= 14.0 * s && out <= radius + 12.0 * s {
            return Hit::Dial;
        }
        [
            (&self.trigger, Hit::Trigger),
            (&self.step_x, Hit::StepX),
            (&self.mode[0], Hit::Mode(true)),
            (&self.mode[1], Hit::Mode(false)),
            (&self.step_y, Hit::StepY),
            (&self.dwell, Hit::Dwell),
            (&self.autostart, Hit::Autostart),
            (&self.file, Hit::File),
        ]
        .into_iter()
        .find(|(area, _)| inside(area))
        .map_or(Hit::Nothing, |(_, hit)| hit)
    }
}

struct Fonts {
    label: IDWriteTextFormat,
    value: IDWriteTextFormat,
    button: IDWriteTextFormat,
    heading: IDWriteTextFormat,
    hint: IDWriteTextFormat,
}

impl Fonts {
    fn new(s: f32) -> Result<Fonts> {
        let factory = paint::dwrite_factory()?;
        Ok(Fonts {
            label: paint::text_format(&factory, 13.5 * s, 400, DWRITE_TEXT_ALIGNMENT_LEADING)?,
            value: paint::text_format(&factory, 13.5 * s, 600, DWRITE_TEXT_ALIGNMENT_TRAILING)?,
            button: paint::text_format(&factory, 12.5 * s, 600, DWRITE_TEXT_ALIGNMENT_CENTER)?,
            heading: paint::text_format(&factory, 17.0 * s, 600, DWRITE_TEXT_ALIGNMENT_CENTER)?,
            hint: paint::text_format(&factory, 12.0 * s, 400, DWRITE_TEXT_ALIGNMENT_CENTER)?,
        })
    }
}

/// The colour of the up and down slices of the dial.
fn vertical(a: f32) -> D2D1_COLOR_F {
    rgba(0.62, 0.45, 1.0, a)
}

pub struct Settings {
    hwnd: HWND,
    factory: ID2D1Factory,
    canvas: Option<Canvas>,
    fonts: Option<Fonts>,
    view: View,
    size: (i32, i32),
    scale: f32,
    hover: Hit,
    press: Option<Press>,
}

/// The mouse button is down.
#[derive(Clone, Copy)]
struct Press {
    hit: Hit,
    /// For a control that is dragged, its value when the button went down.
    before: f32,
    /// For the dial, how far its angle was from the cursor's then: it is
    /// dragged from wherever it is taken hold of, without jumping there.
    offset: f32,
}

impl Settings {
    pub fn new(
        wndproc: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT,
        icon: windows::Win32::UI::WindowsAndMessaging::HICON,
    ) -> Result<Settings> {
        unsafe {
            let instance = GetModuleHandleW(None)?;
            let class = w!("flick.settings");
            RegisterClassW(&WNDCLASSW {
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                hIcon: icon,
                lpfnWndProc: Some(wndproc),
                hInstance: instance.into(),
                lpszClassName: class,
                ..Default::default()
            });
            // A tool window belongs to no desktop, so it stays in sight while
            // the gesture is tried out; on top, so the window that gets the
            // focus on each desktop does not cover it.
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                class,
                w!("Flick 설정"),
                WS_POPUP | WS_CAPTION | WS_SYSMENU,
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
            // The title bar in the colour of the inside.
            let (dark, colour) = (BOOL::from(true), paint::backdrop());
            let bgr = |c: f32, shift: u32| ((c * 255.0).round() as u32) << shift;
            let caption = COLORREF(bgr(colour.r, 0) | bgr(colour.g, 8) | bgr(colour.b, 16));
            let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const _ as *const _, 4);
            let _ = DwmSetWindowAttribute(hwnd, DWMWA_CAPTION_COLOR, &caption as *const _ as *const _, 4);
            Ok(Settings {
                hwnd,
                factory: paint::d2d_factory()?,
                canvas: None,
                fonts: None,
                view: View::default(),
                size: (0, 0),
                scale: 1.0,
                hover: Hit::Nothing,
                press: None,
            })
        }
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    pub fn is_open(&self) -> bool {
        unsafe { IsWindowVisible(self.hwnd) }.as_bool()
    }

    /// Shows the window in the middle of the primary monitor; one already
    /// open stays where it is.
    pub fn open(&mut self, view: View) {
        self.set_view(view);
        if self.is_open() {
            return;
        }
        let monitors = vd::monitors();
        let Some(home) = monitors.iter().find(|m| m.primary) else { return };
        self.set_scale(home.scale);
        self.hover = Hit::Nothing;
        self.press = None;
        let mut frame = RECT { left: 0, top: 0, right: self.size.0, bottom: self.size.1 };
        unsafe {
            let dpi = (home.scale * 96.0).round() as u32;
            let _ = AdjustWindowRectExForDpi(&mut frame, WS_POPUP | WS_CAPTION | WS_SYSMENU, false, WS_EX_TOOLWINDOW, dpi);
            let (w, h) = (frame.right - frame.left, frame.bottom - frame.top);
            let (x, y) = ((home.work.left + home.work.right - w) / 2, (home.work.top + home.work.bottom - h) / 2);
            let _ = SetWindowPos(self.hwnd, HWND_TOPMOST, x, y, w, h, SWP_SHOWWINDOW);
        }
    }

    pub fn close(&mut self) {
        self.press = None;
        unsafe {
            let _ = ReleaseCapture();
            ShowWindow(self.hwnd, SW_HIDE);
        }
    }

    pub fn set_view(&mut self, view: View) {
        self.view = view;
        self.invalidate();
    }

    fn set_scale(&mut self, scale: f32) {
        self.scale = scale;
        self.size = ((SIZE.0 * scale).round() as i32, (SIZE.1 * scale).round() as i32);
        self.fonts = None;
    }

    fn invalidate(&self) {
        unsafe {
            InvalidateRect(self.hwnd, None, false);
        }
    }

    // ---- input ------------------------------------------------------------

    /// Handles a window message. `None` means "not ours, use the default".
    pub fn handle(&mut self, message: u32, wparam: WPARAM, lparam: LPARAM) -> Option<Action> {
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
            WM_ERASEBKGND => Action::None,
            WM_LBUTTONDOWN => self.mouse_down(x, y),
            WM_MOUSEMOVE => self.mouse_move(x, y),
            WM_LBUTTONUP => self.mouse_up(x, y),
            WM_MOUSELEAVE => {
                self.hover = Hit::Nothing;
                self.invalidate();
                Action::None
            }
            WM_KEYDOWN if wparam.0 as u16 == VK_ESCAPE.0 => Action::Close,
            WM_DPICHANGED => {
                // Moved to a monitor with another scaling: Windows says where
                // the window should be now.
                self.set_scale((wparam.0 & 0xFFFF) as f32 / 96.0);
                unsafe {
                    let to = *(lparam.0 as *const RECT);
                    let (w, h) = (to.right - to.left, to.bottom - to.top);
                    let _ = SetWindowPos(self.hwnd, None, to.left, to.top, w, h, SWP_NOZORDER | SWP_NOACTIVATE);
                }
                self.invalidate();
                Action::None
            }
            _ => return None,
        })
    }

    /// The value of the control `hit` stands for, if it is one that is dragged.
    fn value(&self, hit: Hit) -> Option<f32> {
        match hit {
            Hit::Dial => Some(self.view.angle),
            Hit::StepX => Some(self.view.step_x as f32),
            Hit::StepY => Some(self.view.step_y as f32),
            Hit::Dwell => Some(self.view.dwell_ms as f32),
            _ => None,
        }
    }

    /// How many degrees the point is from straight up or down, as seen from
    /// the middle of the dial.
    fn degrees_at(&self, x: f32, y: f32) -> f32 {
        let (centre, _) = Layout::new(self.scale).dial;
        (x - centre.0).abs().atan2((y - centre.1).abs()).to_degrees()
    }

    /// Moves the control that is being dragged along with the cursor.
    fn drag(&mut self, x: f32, y: f32) -> Action {
        let Some(Press { hit, offset, .. }) = self.press else { return Action::None };
        let layout = Layout::new(self.scale);
        let along = |track: &Rect, (low, high, step): (i32, i32, i32)| {
            let (from, to) = slider_ends(track, self.scale);
            let at = ((x - from) / (to - from)).clamp(0.0, 1.0);
            ((low as f32 + at * (high - low) as f32) / step as f32).round() as i32 * step
        };
        let distance = |track: &Rect| along(track, DISTANCES);
        match hit {
            Hit::Dial => {
                let degrees = (self.degrees_at(x, y) + offset).round();
                self.view.angle = degrees.clamp(VERTICAL_ANGLES.0, VERTICAL_ANGLES.1);
            }
            Hit::StepX => self.view.step_x = distance(&layout.step_x),
            Hit::StepY => self.view.step_y = distance(&layout.step_y),
            Hit::Dwell => {
                // Nothing of the gesture: only shown until the drag ends.
                self.view.dwell_ms = along(&layout.dwell, RESTS);
                self.invalidate();
                return Action::None;
            }
            _ => return Action::None,
        }
        self.invalidate();
        Action::Try { angle: self.view.angle, step_x: self.view.step_x, step_y: self.view.step_y }
    }

    fn mouse_down(&mut self, x: f32, y: f32) -> Action {
        let hit = Layout::new(self.scale).hit(x, y, self.scale);
        let before = self.value(hit).unwrap_or(0.0);
        self.press = Some(Press { hit, before, offset: self.view.angle - self.degrees_at(x, y) });
        unsafe {
            SetCapture(self.hwnd);
        }
        self.invalidate();
        // A slider's knob comes to where its track was pressed.
        if hit == Hit::Dial { Action::None } else { self.drag(x, y) }
    }

    fn mouse_move(&mut self, x: f32, y: f32) -> Action {
        if self.press.is_some() {
            return self.drag(x, y);
        }
        let hover = Layout::new(self.scale).hit(x, y, self.scale);
        if hover != self.hover {
            self.hover = hover;
            self.invalidate();
            // Ask to be told when the cursor leaves, to put the hover out.
            let mut track = TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: self.hwnd,
                dwHoverTime: 0,
            };
            unsafe {
                let _ = TrackMouseEvent(&mut track);
            }
        }
        Action::None
    }

    fn mouse_up(&mut self, x: f32, y: f32) -> Action {
        unsafe {
            let _ = ReleaseCapture();
        }
        let Some(Press { hit, before, .. }) = self.press.take() else { return Action::None };
        self.invalidate();
        if let Some(now) = self.value(hit) {
            // A drag ended: what it arrived at is kept.
            let key = match hit {
                Hit::Dial => "vertical_angle",
                Hit::StepX => "step_x",
                Hit::Dwell => "dwell_ms",
                _ => "step_y",
            };
            return if now == before { Action::None } else { Action::Set(key, format!("{now}")) };
        }
        if Layout::new(self.scale).hit(x, y, self.scale) != hit {
            return Action::None;
        }
        match hit {
            Hit::Trigger => Action::Capture(!self.view.capturing),
            Hit::Mode(sticky) if sticky != self.view.sticky => Action::Set("vertical_sticky", sticky.to_string()),
            Hit::Autostart => Action::Autostart(!self.view.autostart),
            Hit::File => Action::OpenFile,
            _ => Action::None,
        }
    }

    // ---- drawing ----------------------------------------------------------

    fn draw_to_canvas(&mut self) -> Option<Result<()>> {
        if !self.canvas.as_ref().is_some_and(|c| c.dib.size == self.size) {
            self.canvas = Canvas::new(&self.factory, self.size)
                .map_err(|e| crate::app::log(&format!("settings canvas failed: {e}")))
                .ok();
        }
        if self.fonts.is_none() {
            self.fonts = Fonts::new(self.scale).map_err(|e| crate::app::log(&format!("settings fonts failed: {e}"))).ok();
        }
        let target = self.canvas.as_ref()?.target.target.clone();
        unsafe {
            target.BeginDraw();
            let drawn = self.draw(&target);
            Some(target.EndDraw(None, None).and(drawn))
        }
    }

    fn paint(&mut self) {
        // Drawn as large as the window's inside really is.
        let mut inside = RECT::default();
        if unsafe { GetClientRect(self.hwnd, &mut inside) }.is_ok() && inside.right > 0 {
            self.size = (inside.right, inside.bottom);
        }
        match self.draw_to_canvas() {
            Some(Ok(())) => {}
            Some(Err(e)) => {
                crate::app::log(&format!("settings draw failed: {e}"));
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
            }
        }
    }

    fn draw(&self, target: &ID2D1RenderTarget) -> Result<()> {
        let Some(fonts) = &self.fonts else { return Ok(()) };
        unsafe {
            target.Clear(Some(&paint::backdrop()));
        }
        let p = Painter::new(target)?;
        let s = self.scale;
        let layout = Layout::new(s);
        let view = &self.view;
        self.draw_dial(&p, fonts, &layout)?;

        let row = |i: f32| rect(EDGES.0 * s, (ROWS.0 + i * ROWS.1) * s, (EDGES.1 - EDGES.0) * s, ROWS.1 * s);
        let label = |i: f32, text: &str| p.text(text, &fonts.label, &row(i), white(0.62));

        label(0.0, "트리거");
        let (words, colour) = match view.capturing {
            true => ("쓸 버튼이나 키를 누르세요", accent(1.0)),
            false => (view.trigger.as_str(), white(0.92)),
        };
        p.text(words, &fonts.value, &Rect { right: layout.trigger.left - 12.0 * s, ..row(0.0) }, colour);
        self.draw_button(&p, fonts, &layout.trigger, if view.capturing { "취소" } else { "바꾸기" }, Hit::Trigger);

        label(1.0, "좌우 거리");
        self.draw_slider(&p, fonts, &layout.step_x, view.step_x, DISTANCES, &view.step_x.to_string(), Hit::StepX);

        label(2.0, "위아래");
        // One switch with two halves; the half in use is lit.
        p.fill(&Rect { right: layout.mode[1].right, ..layout.mode[0] }, 7.0 * s, white(0.08));
        for (area, sticky, text) in [(&layout.mode[0], true, "휙 밀기"), (&layout.mode[1], false, "거리")] {
            let on = view.sticky == sticky;
            let hot = self.hover == Hit::Mode(sticky);
            if on {
                p.fill(area, 7.0 * s, accent(0.95));
            }
            p.text(text, &fonts.button, area, white(if on || hot { 1.0 } else { 0.55 }));
        }
        // A flick is judged by how far it gets in a moment, so the same
        // number sets how hard it must be.
        label(3.0, if view.sticky { "휙 세기" } else { "위아래 거리" });
        self.draw_slider(&p, fonts, &layout.step_y, view.step_y, DISTANCES, &view.step_y.to_string(), Hit::StepY);

        // In the board: resting the cursor on a cell of the map shows it.
        label(4.0, "칸에 머물러 보기");
        let rest = match view.dwell_ms {
            0 => "끔".to_owned(),
            ms => format!("{:.1}초", ms as f32 / 1000.0),
        };
        self.draw_slider(&p, fonts, &layout.dwell, view.dwell_ms, RESTS, &rest, Hit::Dwell);

        label(5.0, "윈도우 시작 시 실행");
        let switch = &layout.autostart;
        let (h, on) = (switch.bottom - switch.top, view.autostart);
        let hot = self.hover == Hit::Autostart;
        p.fill(switch, h / 2.0, if on { accent(0.95) } else { white(if hot { 0.22 } else { 0.14 }) });
        let knob = if on { switch.right - h / 2.0 } else { switch.left + h / 2.0 };
        p.disc((knob, switch.top + h / 2.0), h / 2.0 - 3.0 * s, white(0.95));

        let hot = self.hover == Hit::File;
        p.text("그 밖의 설정: config.toml", &fonts.label, &layout.file, white(if hot { 0.85 } else { 0.38 }));
        Ok(())
    }

    /// The directions of the gesture as the hooks tell them apart: up and
    /// down are the slices `angle` degrees either side of vertical, sideways
    /// is the rest. Dragging in the dial moves the boundaries.
    fn draw_dial(&self, p: &Painter, fonts: &Fonts, layout: &Layout) -> Result<()> {
        let s = self.scale;
        let (centre, radius) = layout.dial;
        let a = self.view.angle;
        let active = self.hover == Hit::Dial || matches!(self.press, Some(Press { hit: Hit::Dial, .. }));
        let slices = [(-a, a, true), (a, 180.0 - a, false), (180.0 - a, 180.0 + a, true), (180.0 + a, 360.0 - a, false)];
        for (from, to, up_or_down) in slices {
            let slice = paint::pie(&self.factory, centre, radius, from, to)?;
            p.fill_shape(&slice, if up_or_down { vertical(0.80) } else { accent(0.30) });
        }
        // The four boundaries, each with a handle on the rim.
        for degrees in [a, 180.0 - a, 180.0 + a, 360.0 - a] {
            let rim = on_circle(centre, radius, degrees);
            p.line(centre, rim, 2.0 * s, paint::backdrop());
            p.disc(rim, if active { 7.0 } else { 5.5 } * s, paint::backdrop());
            p.disc(rim, if active { 5.0 } else { 3.5 } * s, white(if active { 1.0 } else { 0.80 }));
        }
        p.disc(centre, 5.0 * s, paint::backdrop());
        p.disc(centre, 2.5 * s, white(0.90));

        let name = |text: &str, degrees: f32, out: f32| {
            let at = on_circle(centre, radius * out, degrees);
            p.text(text, &fonts.button, &rect(at.0 - 40.0 * s, at.1 - 12.0 * s, 80.0 * s, 24.0 * s), white(0.95));
        };
        name("위", 0.0, 0.66);
        name("아래", 180.0, 0.66);
        name("왼쪽", 270.0, 0.56);
        name("오른쪽", 90.0, 0.56);

        let below = |top: f32, height: f32| rect(0.0, (DIAL.0 .1 + DIAL.1 + top) * s, SIZE.0 * s, height * s);
        p.text(&format!("위아래 ±{a:.0}°"), &fonts.heading, &below(18.0, 26.0), white(0.92));
        p.text("경계를 끌면 위아래로 치는 폭이 바뀝니다", &fonts.hint, &below(44.0, 20.0), white(0.40));
        Ok(())
    }

    fn draw_button(&self, p: &Painter, fonts: &Fonts, area: &Rect, text: &str, hit: Hit) {
        let hot = self.hover == hit;
        p.fill(area, 7.0 * self.scale, white(if hot { 0.20 } else { 0.10 }));
        p.text(text, &fonts.button, area, white(if hot { 1.0 } else { 0.85 }));
    }

    /// A value within `span`: a track with a knob, and the value in words
    /// after it.
    #[allow(clippy::too_many_arguments)]
    fn draw_slider(&self, p: &Painter, fonts: &Fonts, area: &Rect, value: i32, span: (i32, i32, i32), words: &str, hit: Hit) {
        let s = self.scale;
        let (from, to) = slider_ends(area, s);
        let (low, high, _) = span;
        let at = from + (to - from) * ((value - low) as f32 / (high - low) as f32).clamp(0.0, 1.0);
        let middle = (area.top + area.bottom) / 2.0;
        let active = self.hover == hit || matches!(self.press, Some(Press { hit: pressed, .. }) if pressed == hit);
        p.fill(&rect(from, middle - 2.0 * s, to - from, 4.0 * s), 2.0 * s, white(0.12));
        p.fill(&rect(from, middle - 2.0 * s, at - from, 4.0 * s), 2.0 * s, accent(0.95));
        p.disc((at, middle), if active { 8.0 } else { 6.5 } * s, white(0.95));
        let number = Rect { left: area.right, right: EDGES.1 * s, ..*area };
        p.text(words, &fonts.value, &number, white(0.92));
    }

    /// Draws the window showing `view` into a bitmap and returns its size and
    /// the BGRA pixels, for reviewing it without showing it.
    pub fn render_to_pixels(&mut self, view: View, scale: f32) -> Option<((i32, i32), Vec<u8>)> {
        self.view = view;
        self.set_scale(scale);
        let _ = self.draw_to_canvas()?;
        Some((self.size, self.canvas.as_ref()?.dib.pixels().to_vec()))
    }
}

/// Where a slider's knob travels: from and to, inside its row.
fn slider_ends(area: &Rect, s: f32) -> (f32, f32) {
    (area.left + 8.0 * s, area.right - 14.0 * s)
}
