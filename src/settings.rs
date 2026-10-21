//! The settings window: the trigger, starting with Windows, running as
//! administrator, and how long a flick may take. All else is left as it is
//! set, in config.toml.
//!
//! It owns layout, hit-testing and drawing, in the look of the board. It
//! changes nothing itself; what the user does is turned into an `Action` for
//! the app, which answers with a new `View`.

use crate::{
    paint::{self, accent, rect, white, Canvas, Painter, Rect},
    vd,
    i18n::text as tr,
};
use windows::{
    core::{w, Error, Result},
    Win32::{
        Foundation::{BOOL, COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM},
        Graphics::{
            Direct2D::{ID2D1Factory, ID2D1RenderTarget},
            DirectWrite::{
                IDWriteTextFormat, DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING,
                DWRITE_TEXT_ALIGNMENT_TRAILING, DWRITE_WORD_WRAPPING_WRAP,
            },
            Dwm::{DwmSetWindowAttribute, DWMWA_CAPTION_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE},
            Gdi::{BitBlt, GetDC, InvalidateRect, ReleaseDC, ValidateRect, SRCCOPY},
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            HiDpi::AdjustWindowRectExForDpi,
            Input::KeyboardAndMouse::{
                GetKeyState, ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VK_ESCAPE, VK_TAB,
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

/// Size of the window's inside, in unscaled units: the rows, the note under
/// the last and a margin.
const SIZE: (f32, f32) = (560.0, ROWS.0 + (FLICK_ROW + 3.0) * ROWS.1 + 14.0);
/// The flick time's row, under the trigger and the two switches.
const FLICK_ROW: f32 = 3.0;
/// Top of the first row and the height of each.
const ROWS: (f32, f32) = (14.0, 44.0);
/// Left edge of the labels and right edge of the controls.
const EDGES: (f32, f32) = (28.0, 532.0);
/// How long a flick may take (`experimental.flick_ms`), in milliseconds:
/// what the slider spans and the step it moves in.
const FLICK_MS: (i32, i32, i32) = (80, 500, 10);
/// The width of the flick time's label before its slider, and of its value
/// after it.
const FLICK_LABEL: f32 = 200.0;
const VALUE: f32 = 70.0;

/// What the window shows.
#[derive(Clone, Default)]
pub struct View {
    /// The trigger in words.
    pub trigger: String,
    /// Waiting for the user to press what the trigger should become.
    pub capturing: bool,
    pub autostart: bool,
    pub admin: bool,
    /// See `config::Experimental::flick_ms`.
    pub flick_ms: u32,
}

/// What the user asked for; carried out by the app.
pub enum Action {
    None,
    /// Write a value to the config file: its key and the value as TOML.
    Set(&'static str, String),
    /// Start (or give up) waiting for a new trigger to be pressed.
    Capture(bool),
    Autostart(bool),
    Admin(bool),
    Close,
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
enum Hit {
    #[default]
    Nothing,
    Trigger,
    Autostart,
    Admin,
    FlickMs,
}

/// Where the controls are, in pixels.
struct Layout {
    trigger: Rect,
    autostart: Rect,
    admin: Rect,
    flick: Rect,
}

impl Layout {
    fn new(s: f32) -> Layout {
        let row = |i: f32| ROWS.0 + i * ROWS.1;
        Layout {
            trigger: rect((EDGES.1 - 64.0) * s, (row(0.0) + (ROWS.1 - 30.0) / 2.0) * s, 64.0 * s, 30.0 * s),
            autostart: rect((EDGES.1 - 44.0) * s, (row(1.0) + (ROWS.1 - 24.0) / 2.0) * s, 44.0 * s, 24.0 * s),
            admin: rect((EDGES.1 - 44.0) * s, (row(2.0) + (ROWS.1 - 24.0) / 2.0) * s, 44.0 * s, 24.0 * s),
            flick: rect((EDGES.0 + FLICK_LABEL) * s, row(FLICK_ROW) * s, (EDGES.1 - EDGES.0 - FLICK_LABEL - VALUE) * s, ROWS.1 * s),
        }
    }

    fn controls(&self) -> [(Rect, Hit); 4] {
        [(self.trigger, Hit::Trigger), (self.autostart, Hit::Autostart), (self.admin, Hit::Admin), (self.flick, Hit::FlickMs)]
    }

    fn hit(&self, x: f32, y: f32) -> Hit {
        self.controls().into_iter()
            .find(|(r, _)| x >= r.left && x < r.right && y >= r.top && y < r.bottom)
            .map_or(Hit::Nothing, |(_, hit)| hit)
    }
}

struct Fonts {
    label: IDWriteTextFormat,
    value: IDWriteTextFormat,
    button: IDWriteTextFormat,
    notice: IDWriteTextFormat,
}

impl Fonts {
    fn new(s: f32) -> Result<Fonts> {
        let factory = paint::dwrite_factory()?;
        let notice = paint::text_format(&factory, 13.5 * s, 400, DWRITE_TEXT_ALIGNMENT_LEADING)?;
        unsafe { notice.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP)?; }
        Ok(Fonts {
            label: paint::text_format(&factory, 13.5 * s, 400, DWRITE_TEXT_ALIGNMENT_LEADING)?,
            value: paint::text_format(&factory, 13.5 * s, 600, DWRITE_TEXT_ALIGNMENT_TRAILING)?,
            button: paint::text_format(&factory, 12.5 * s, 600, DWRITE_TEXT_ALIGNMENT_CENTER)?,
            notice,
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
    focus: Hit,
}

/// The mouse button is down.
#[derive(Clone, Copy)]
struct Press {
    hit: Hit,
    /// For the slider, its value when the button went down.
    before: i32,
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
                &windows::core::HSTRING::from(tr("Flick Settings", "Flick 설정", "Flick 设置")),
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
                focus: Hit::Trigger,
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
        self.set_scale(fitting_scale(home.scale, home.work.right - home.work.left));
        self.focus = Hit::Trigger;
        self.hover = Hit::Nothing;
        self.press = None;
        let mut frame = RECT { left: 0, top: 0, right: self.size.0, bottom: self.size.1 };
        unsafe {
            let dpi = (home.scale * 96.0).round() as u32;
            let _ = AdjustWindowRectExForDpi(&mut frame, WS_POPUP | WS_CAPTION | WS_SYSMENU, false, WS_EX_TOOLWINDOW, dpi);
            let (w, h) = (frame.right - frame.left, frame.bottom - frame.top);
            let fitted = fit_frame((home.work.left + home.work.right - w) / 2, (home.work.top + home.work.bottom - h) / 2, w, h, &home.work);
            let _ = SetWindowPos(self.hwnd, HWND_TOPMOST, fitted.left, fitted.top, fitted.right - fitted.left, fitted.bottom - fitted.top, SWP_SHOWWINDOW);
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
        let capturing = self.view.capturing;
        let pressed = |code: u8| message == WM_KEYDOWN && wparam.0 == code as usize && !capturing;
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
            _ if pressed(VK_TAB.0 as u8) => {
                self.focus = next_focus(&Layout::new(self.scale).controls(), self.focus, unsafe { GetKeyState(0x10) } < 0);
                self.invalidate();
                Action::None
            }
            _ if pressed(0x20) || pressed(0x0d) => self.activate(self.focus),
            _ if pressed(0x25) || pressed(0x26) => self.adjust_slider(-1),
            _ if pressed(0x27) || pressed(0x28) => self.adjust_slider(1),
            WM_DPICHANGED => {
                // Moved to a monitor with another scaling: Windows says where
                // the window should be now.
                unsafe {
                    let to = *(lparam.0 as *const RECT);
                    let monitors = vd::monitors();
                    if let Some(home) = monitors.iter().max_by_key(|m| overlap(&to, &m.bounds)) {
                        self.set_scale(fitting_scale((wparam.0 & 0xffff) as f32 / 96.0, home.work.right - home.work.left));
                        let mut frame = RECT { right: self.size.0, bottom: self.size.1, ..Default::default() };
                        let _ = AdjustWindowRectExForDpi(&mut frame, WS_POPUP | WS_CAPTION | WS_SYSMENU, false, WS_EX_TOOLWINDOW, (wparam.0 & 0xffff) as u32);
                        let fitted = fit_frame(to.left, to.top, frame.right - frame.left, frame.bottom - frame.top, &home.work);
                        let _ = SetWindowPos(self.hwnd, None, fitted.left, fitted.top, fitted.right - fitted.left, fitted.bottom - fitted.top, SWP_NOZORDER | SWP_NOACTIVATE);
                    }
                }
                self.invalidate();
                Action::None
            }
            _ => return None,
        })
    }

    /// Moves the slider being dragged along with the cursor; written when
    /// the drag ends.
    fn drag(&mut self, x: f32) -> Action {
        if !matches!(self.press, Some(Press { hit: Hit::FlickMs, .. })) { return Action::None }
        let (from, to) = slider_ends(&Layout::new(self.scale).flick, self.scale);
        let at = ((x - from) / (to - from)).clamp(0.0, 1.0);
        let (low, high, step) = FLICK_MS;
        self.view.flick_ms = (((low as f32 + at * (high - low) as f32) / step as f32).round() as i32 * step) as u32;
        self.invalidate();
        Action::None
    }

    fn mouse_down(&mut self, x: f32, y: f32) -> Action {
        let hit = Layout::new(self.scale).hit(x, y);
        self.focus = if hit == Hit::Nothing { self.focus } else { hit };
        self.press = Some(Press { hit, before: self.view.flick_ms as i32 });
        unsafe {
            SetCapture(self.hwnd);
        }
        self.invalidate();
        // The slider's knob comes to where its track was pressed.
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
        let Some(Press { hit, before }) = self.press.take() else { return Action::None };
        self.invalidate();
        if hit == Hit::FlickMs {
            let now = self.view.flick_ms as i32;
            return if now == before { Action::None } else { Action::Set("experimental.flick_ms", now.to_string()) };
        }
        if Layout::new(self.scale).hit(x, y) != hit {
            return Action::None;
        }
        self.activate(hit)
    }

    fn activate(&mut self, hit: Hit) -> Action {
        match hit {
            Hit::Trigger => Action::Capture(!self.view.capturing),
            Hit::Autostart => Action::Autostart(!self.view.autostart),
            Hit::Admin => Action::Admin(!self.view.admin),
            _ => Action::None,
        }
    }

    fn adjust_slider(&mut self, direction: i32) -> Action {
        if self.focus != Hit::FlickMs { return Action::None }
        let next = slider_step(self.view.flick_ms as i32, FLICK_MS, direction);
        Action::Set("experimental.flick_ms", next.to_string())
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
            let (w, h) = self.size;
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

        label(0.0, tr("Trigger", "트리거", "触发键"));
        let (words, colour) = match view.capturing {
            true => (tr("Press the button or key to use", "사용할 버튼이나 키를 누르세요", "请按要使用的按钮或按键"), accent(1.0)),
            false => (view.trigger.as_str(), white(0.92)),
        };
        p.text(words, &fonts.value, &Rect { right: layout.trigger.left - 12.0 * s, ..row(0.0) }, colour);
        self.draw_button(&p, fonts, &layout.trigger, if view.capturing { tr("Cancel", "취소", "取消") } else { tr("Change", "변경", "更改") }, Hit::Trigger);

        label(1.0, tr("Start with Windows", "Windows와 함께 시작", "随 Windows 启动"));
        self.draw_switch(&p, &layout.autostart, view.autostart, Hit::Autostart);
        label(2.0, tr("Run as administrator (off takes effect after restart)", "관리자 권한 실행 (끄기는 재시작 후 적용)", "以管理员身份运行（关闭后重启生效）"));
        self.draw_switch(&p, &layout.admin, view.admin, Hit::Admin);

        label(FLICK_ROW, tr("Flick time", "넘김 인정 시간", "翻页判定时间"));
        let flick_words = format!("{} ms", view.flick_ms);
        self.draw_slider(&p, fonts, &layout.flick, EDGES.1 * s, view.flick_ms as i32, FLICK_MS, &flick_words, Hit::FlickMs);
        p.text(tr("A quick move let go of within this time goes one desktop, the screen staying whole. Held on, the screen zooms out gently from 60% of it, and you go as far as you move. Shorter: fewer flicks.",
            "이 시간 안에 휙 움직였다 놓으면 화면 그대로 한 칸 넘어갑니다. 더 누르고 있으면 그 시간의 60%부터 화면이 살살 물러나며, 움직인 만큼 이동합니다. 짧을수록 섬세 이동이 쉬워집니다.",
            "在此时间内快速移动并松开，画面不缩小地切换一格。继续按住，画面从该时间的 60% 起缓缓后退，移动多少就走多远。越短越容易精细移动。"),
            &fonts.notice, &Rect { bottom: row(FLICK_ROW + 1.0).top + 2.0 * ROWS.1 * s, ..row(FLICK_ROW + 1.0) }, white(0.62));

        if let Some((area, _)) = layout.controls().into_iter().find(|(_, hit)| *hit == self.focus) {
            let ring = Rect { left: area.left - 3.0 * s, top: area.top - 3.0 * s, right: area.right + 3.0 * s, bottom: area.bottom + 3.0 * s };
            p.stroke(&ring, 5.0 * s, 2.0 * s, accent(1.0));
        }
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
    /// after it, up to `right`.
    #[allow(clippy::too_many_arguments)]
    fn draw_slider(&self, p: &Painter, fonts: &Fonts, area: &Rect, right: f32, value: i32, span: (i32, i32, i32), words: &str, hit: Hit) {
        let s = self.scale;
        let (from, to) = slider_ends(area, s);
        let (low, high, _) = span;
        let at = from + (to - from) * ((value - low) as f32 / (high - low) as f32).clamp(0.0, 1.0);
        let middle = (area.top + area.bottom) / 2.0;
        let active = self.hover == hit || matches!(self.press, Some(Press { hit: pressed, .. }) if pressed == hit);
        p.fill(&rect(from, middle - 2.0 * s, to - from, 4.0 * s), 2.0 * s, white(0.12));
        p.fill(&rect(from, middle - 2.0 * s, at - from, 4.0 * s), 2.0 * s, accent(0.95));
        p.disc((at, middle), if active { 8.0 } else { 6.5 } * s, white(0.95));
        let number = Rect { left: area.right, right, ..*area };
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

fn overlap(a: &RECT, b: &RECT) -> i64 {
    (a.right.min(b.right) - a.left.max(b.left)).max(0) as i64
        * (a.bottom.min(b.bottom) - a.top.max(b.top)).max(0) as i64
}
fn fit_frame(x: i32, y: i32, width: i32, height: i32, work: &RECT) -> RECT {
    let w = width.min(work.right - work.left);
    let h = height.min(work.bottom - work.top);
    let left = x.clamp(work.left, work.right - w);
    let top = y.clamp(work.top, work.bottom - h);
    RECT { left, top, right: left + w, bottom: top + h }
}
fn fitting_scale(dpi_scale: f32, work_width: i32) -> f32 {
    dpi_scale.min(((work_width - 40).max(1) as f32) / SIZE.0)
}
fn next_focus(controls: &[(Rect, Hit)], focus: Hit, backwards: bool) -> Hit {
    let index = controls.iter().position(|(_, hit)| *hit == focus);
    let next = match index { Some(i) if backwards => (i + controls.len() - 1) % controls.len(),
        Some(i) => (i + 1) % controls.len(), None if backwards => controls.len() - 1, None => 0 };
    controls[next].1
}
fn slider_step(value: i32, (low, high, step): (i32, i32, i32), direction: i32) -> i32 {
    (value + step * direction).clamp(low, high)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn focus_visits_every_control_and_wraps_both_ways() {
        let controls = Layout::new(1.0).controls();
        assert_eq!(next_focus(&controls, Hit::Trigger, false), Hit::Autostart);
        assert_eq!(next_focus(&controls, Hit::Admin, false), Hit::FlickMs);
        assert_eq!(next_focus(&controls, Hit::FlickMs, false), Hit::Trigger);
        assert_eq!(next_focus(&controls, Hit::Trigger, true), Hit::FlickMs);
    }
    #[test]
    fn the_flick_time_slider_clamps() {
        assert_eq!(slider_step(500, FLICK_MS, 1), 500);
        assert_eq!(slider_step(80, FLICK_MS, -1), 80);
        assert_eq!(slider_step(150, FLICK_MS, 1), 160);
    }
    #[test]
    fn high_dpi_content_fits_a_narrow_work_area() {
        assert!((fitting_scale(2.0, 1920) - 2.0).abs() < 1e-6);
        assert!(SIZE.0 * fitting_scale(2.0, 800) <= 760.01);
        let work = RECT { left: -1920, top: 0, right: 0, bottom: 1040 };
        let fitted = fit_frame(-1600, -100, 1312, 1400, &work);
        assert!(fitted.left >= work.left && fitted.right <= work.right);
        assert_eq!((fitted.top, fitted.bottom), (0, 1040));
        assert!(overlap(&fitted, &work) > overlap(&fitted, &RECT { left: 0, top: 0, right: 1920, bottom: 1080 }));
    }
}
