//! The settings window: the trigger, the distance and
//! whether to start with Windows.
//!
//! It owns layout, hit-testing and drawing, in the look of the board. It
//! changes nothing itself; what the user does is turned into an `Action` for
//! the app, which answers with a new `View`.

use crate::{
    paint::{self, accent, rect, white, Canvas, Painter, Rect},
    vd,
};
use windows::{
    core::{w, Error, Result},
    Win32::{
        Foundation::{BOOL, COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM},
        Graphics::{
            Direct2D::{ID2D1Factory, ID2D1RenderTarget},
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
/// Below the last row, the link to the file and a margin.
const SIZE: (f32, f32) = (400.0, ROWS.0 + (9.0 - BELOW_UP_AND_DOWN) * ROWS.1 + 52.0);
/// Whether up and down has settings of its own (a flick or a distance, and
/// how much). Not now: rows go by distance, a distance of their own. The
/// switch is kept hidden, and its key in config.toml read and left alone,
/// should that change back.
const UP_AND_DOWN_OF_ITS_OWN: bool = false;
/// The rows below them move up into their place while they are hidden.
const BELOW_UP_AND_DOWN: f32 = if UP_AND_DOWN_OF_ITS_OWN { 0.0 } else { 1.0 };
/// Top of the first row and the height of each.
const ROWS: (f32, f32) = (14.0, 46.0);
/// Left edge of the labels and right edge of the controls.
const EDGES: (f32, f32) = (28.0, 372.0);
/// What the distance sliders span, and the step they move in.
const DISTANCES: (i32, i32, i32) = (80, 600, 10);

/// What the window shows.
#[derive(Clone, Default)]
pub struct View {
    /// The trigger in words.
    pub trigger: String,
    /// Waiting for the user to press what the trigger should become.
    pub capturing: bool,
    /// See `Config::step_x`, `step_y`, `vertical_sticky`.
    pub step_x: i32,
    pub step_y: i32,
    pub sticky: bool,
    pub autostart: bool,
    pub touchpad: bool,
    pub touchpad_status: crate::touchpad::Status,
}

/// What the user asked for; carried out by the app.
pub enum Action {
    None,
    /// Use these for the gesture right away; a control is being dragged.
    Try { step_x: i32, step_y: i32 },
    /// Write a value to the config file: its key and the value as TOML.
    Set(&'static str, String),
    /// Start (or give up) waiting for a new trigger to be pressed.
    Capture(bool),
    Autostart(bool),
    /// Open the config file, which has the settings not shown here.
    OpenFile,
    TouchpadSettings,
    Close,
}

#[derive(Clone, Copy, PartialEq, Default)]
enum Hit {
    #[default]
    Nothing,
    Trigger,
    StepX,
    /// One half of the vertical mode switch: flick (true) or distance.
    Mode(bool),
    StepY,
    Autostart,
    Touchpad,
    TouchpadSettings,
    File,
}

/// Where the controls are, in pixels.
struct Layout {
    trigger: Rect,
    step_x: Rect,
    mode: [Rect; 2],
    step_y: Rect,
    autostart: Rect,
    touchpad: Rect,
    touchpad_settings: Rect,
    file: Rect,
}

impl Layout {
    fn new(s: f32) -> Layout {
        let row = |i: f32| ROWS.0 + i * ROWS.1;
        // A control `w` wide and `h` high at the right of row `i`.
        let right = |i: f32, w: f32, h: f32| rect((EDGES.1 - w) * s, (row(i) + (ROWS.1 - h) / 2.0) * s, w * s, h * s);
        let slider = |i: f32| rect(150.0 * s, row(i) * s, 166.0 * s, ROWS.1 * s);
        let hidden = |r: Rect| if UP_AND_DOWN_OF_ITS_OWN { r } else { Rect::default() };
        let mode = hidden(right(2.0, 160.0, 30.0));
        let half = (mode.right - mode.left) / 2.0;
        let up = BELOW_UP_AND_DOWN;
        Layout {
            trigger: right(0.0, 64.0, 30.0),
            step_x: slider(1.0),
            mode: [Rect { right: mode.left + half, ..mode }, Rect { left: mode.left + half, ..mode }],
            step_y: slider(if UP_AND_DOWN_OF_ITS_OWN { 3.0 } else { 2.0 }),
            autostart: right(4.0 - up, 44.0, 24.0),
            touchpad: right(5.0 - up, 44.0, 24.0),
            touchpad_settings: rect(EDGES.0 * s, (row(8.0 - up) + 4.0) * s, 240.0 * s, 30.0 * s),
            file: rect(EDGES.0 * s, (row(9.0 - up) + 8.0) * s, 190.0 * s, 26.0 * s),
        }
    }

    fn hit(&self, x: f32, y: f32) -> Hit {
        let inside = |r: &Rect| x >= r.left && x < r.right && y >= r.top && y < r.bottom;
        [
            (&self.trigger, Hit::Trigger),
            (&self.step_x, Hit::StepX),
            (&self.mode[0], Hit::Mode(true)),
            (&self.mode[1], Hit::Mode(false)),
            (&self.step_y, Hit::StepY),
            (&self.autostart, Hit::Autostart),
            (&self.touchpad, Hit::Touchpad),
            (&self.touchpad_settings, Hit::TouchpadSettings),
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
}

impl Fonts {
    fn new(s: f32) -> Result<Fonts> {
        let factory = paint::dwrite_factory()?;
        Ok(Fonts {
            label: paint::text_format(&factory, 13.5 * s, 400, DWRITE_TEXT_ALIGNMENT_LEADING)?,
            value: paint::text_format(&factory, 13.5 * s, 600, DWRITE_TEXT_ALIGNMENT_TRAILING)?,
            button: paint::text_format(&factory, 12.5 * s, 600, DWRITE_TEXT_ALIGNMENT_CENTER)?,
        })
    }
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
                w!("Flick Settings"),
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
        self.canvas = None;
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
            WM_KEYDOWN if wparam.0 == 0x54 && !self.view.capturing =>
                Action::Set("three_finger_touchpad", (!self.view.touchpad).to_string()),
            WM_KEYDOWN if wparam.0 == 0x57 && self.view.touchpad_status == crate::touchpad::Status::Conflict =>
                Action::TouchpadSettings,
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
            Hit::StepX => Some(self.view.step_x as f32),
            Hit::StepY => Some(self.view.step_y as f32),
            _ => None,
        }
    }

    /// Moves the control that is being dragged along with the cursor.
    fn drag(&mut self, x: f32) -> Action {
        let Some(Press { hit, .. }) = self.press else { return Action::None };
        let layout = Layout::new(self.scale);
        let along = |track: &Rect, (low, high, step): (i32, i32, i32)| {
            let (from, to) = slider_ends(track, self.scale);
            let at = ((x - from) / (to - from)).clamp(0.0, 1.0);
            ((low as f32 + at * (high - low) as f32) / step as f32).round() as i32 * step
        };
        let distance = |track: &Rect| along(track, DISTANCES);
        match hit {
            Hit::StepX => self.view.step_x = distance(&layout.step_x),
            Hit::StepY => self.view.step_y = distance(&layout.step_y),
            _ => return Action::None,
        }
        self.invalidate();
        Action::Try { step_x: self.view.step_x, step_y: self.view.step_y }
    }

    fn mouse_down(&mut self, x: f32, y: f32) -> Action {
        let hit = Layout::new(self.scale).hit(x, y);
        let before = self.value(hit).unwrap_or(0.0);
        self.press = Some(Press { hit, before });
        unsafe {
            SetCapture(self.hwnd);
        }
        self.invalidate();
        // A slider's knob comes to where its track was pressed.
        self.drag(x)
    }

    fn mouse_move(&mut self, x: f32, y: f32) -> Action {
        if self.press.is_some() {
            return self.drag(x);
        }
        let hover = Layout::new(self.scale).hit(x, y);
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
                Hit::StepX => "step_x",
                _ => "step_y",
            };
            return if now == before { Action::None } else { Action::Set(key, format!("{now}")) };
        }
        if Layout::new(self.scale).hit(x, y) != hit {
            return Action::None;
        }
        match hit {
            Hit::Trigger => Action::Capture(!self.view.capturing),
            Hit::Mode(sticky) if sticky != self.view.sticky => Action::Set("vertical_sticky", sticky.to_string()),
            Hit::Autostart => Action::Autostart(!self.view.autostart),
            Hit::Touchpad => Action::Set("three_finger_touchpad", (!self.view.touchpad).to_string()),
            Hit::TouchpadSettings if self.view.touchpad_status == crate::touchpad::Status::Conflict => Action::TouchpadSettings,
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

        let row = |i: f32| rect(EDGES.0 * s, (ROWS.0 + i * ROWS.1) * s, (EDGES.1 - EDGES.0) * s, ROWS.1 * s);
        let label = |i: f32, text: &str| p.text(text, &fonts.label, &row(i), white(0.62));

        label(0.0, "Trigger");
        let (words, colour) = match view.capturing {
            true => ("Press the button or key to use", accent(1.0)),
            false => (view.trigger.as_str(), white(0.92)),
        };
        p.text(words, &fonts.value, &Rect { right: layout.trigger.left - 12.0 * s, ..row(0.0) }, colour);
        self.draw_button(&p, fonts, &layout.trigger, if view.capturing { "Cancel" } else { "Change" }, Hit::Trigger);

        label(1.0, "Sideways");
        self.draw_slider(&p, fonts, &layout.step_x, view.step_x, DISTANCES, &view.step_x.to_string(), Hit::StepX);

        if UP_AND_DOWN_OF_ITS_OWN {
        label(2.0, "Up and down");
        // One switch with two halves; the half in use is lit.
        p.fill(&Rect { right: layout.mode[1].right, ..layout.mode[0] }, 7.0 * s, white(0.08));
        for (area, sticky, text) in [(&layout.mode[0], true, "Flick"), (&layout.mode[1], false, "Distance")] {
            let on = view.sticky == sticky;
            let hot = self.hover == Hit::Mode(sticky);
            if on {
                p.fill(area, 7.0 * s, accent(0.95));
            }
            p.text(text, &fonts.button, area, white(if on || hot { 1.0 } else { 0.55 }));
        }
        // A flick is judged by how far it gets in a moment, so the same
        // number sets how hard it must be.
        label(3.0, if view.sticky { "Flick strength" } else { "Vertical distance" });
        self.draw_slider(&p, fonts, &layout.step_y, view.step_y, DISTANCES, &view.step_y.to_string(), Hit::StepY);
        }
        if !UP_AND_DOWN_OF_ITS_OWN {
            label(2.0, "Up and down");
            self.draw_slider(&p, fonts, &layout.step_y, view.step_y, DISTANCES, &view.step_y.to_string(), Hit::StepY);
        }
        let up = BELOW_UP_AND_DOWN;

        label(4.0 - up, "Start with Windows");
        self.draw_switch(&p, &layout.autostart, view.autostart, Hit::Autostart);

        label(5.0 - up, "Three-finger touchpad (T)");
        self.draw_switch(&p, &layout.touchpad, view.touchpad, Hit::Touchpad);
        let notice = match view.touchpad_status {
            crate::touchpad::Status::Absent => "No precision touchpad detected",
            crate::touchpad::Status::Off => "Off. Enable to use three fingers as the trigger.",
            crate::touchpad::Status::Active => "Ready. Tap for the board; move about 1 cm per desktop.",
            crate::touchpad::Status::Conflict => crate::touchpad::NOTICE,
            crate::touchpad::Status::Unavailable => "Touchpad reports could not be read on this device.",
        };
        let area = Rect { bottom: row(8.0 - up).top, ..row(6.0 - up) };
        p.text(notice, &fonts.label, &area, white(0.62));
        if view.touchpad_status == crate::touchpad::Status::Conflict {
            self.draw_button(&p, fonts, &layout.touchpad_settings, "Open Windows touchpad settings (W)", Hit::TouchpadSettings);
        }

        let hot = self.hover == Hit::File;
        p.text("More settings: config.toml", &fonts.label, &layout.file, white(if hot { 0.85 } else { 0.38 }));
        Ok(())
    }

    fn draw_switch(&self, p: &Painter, switch: &Rect, on: bool, hit: Hit) {
        let h = switch.bottom - switch.top;
        let hot = self.hover == hit;
        p.fill(switch, h / 2.0, if on { accent(0.95) } else { white(if hot { 0.22 } else { 0.14 }) });
        let knob = if on { switch.right - h / 2.0 } else { switch.left + h / 2.0 };
        p.disc((knob, switch.top + h / 2.0), h / 2.0 - 3.0 * self.scale, white(0.95));
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
