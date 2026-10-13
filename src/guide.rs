//! The window shown on the first run, and again from the tray menu or when
//! Flick is started while it is already running: how it is used, played
//! over and over, and where its icon is.
//!
//! Drawn like the settings window. It changes nothing itself; closing it is
//! an `Action` for the app.

use crate::{
    paint::{self, accent, rect, rgba, white, Canvas, Painter, Rect},
    vd,
};
use std::time::Instant;
use windows::{
    core::{w, Error, Result},
    Win32::{
        Foundation::{BOOL, COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM},
        Globalization::GetUserDefaultUILanguage,
        Graphics::{
            Direct2D::{Common::D2D1_COLOR_F, ID2D1Factory, ID2D1RenderTarget},
            DirectWrite::{IDWriteTextFormat, DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING},
            Dwm::{DwmSetWindowAttribute, DWMWA_CAPTION_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE},
            Gdi::{BitBlt, GetDC, InvalidateRect, ReleaseDC, ValidateRect, SRCCOPY},
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            HiDpi::AdjustWindowRectExForDpi,
            Input::KeyboardAndMouse::{VK_ESCAPE, VK_RETURN},
            WindowsAndMessaging::{
                CreateWindowExW, GetClientRect, IsWindowVisible, KillTimer, LoadCursorW, RegisterClassW, SetTimer,
                SetWindowPos, ShowWindow, HICON, HWND_TOPMOST, IDC_ARROW, SET_WINDOW_POS_FLAGS, SWP_NOACTIVATE, SWP_NOZORDER,
                SWP_SHOWWINDOW, SW_HIDE, WM_DPICHANGED, WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP,
                WM_MOUSEMOVE, WM_PAINT, WM_TIMER,
                WNDCLASSW, WS_CAPTION, WS_EX_TOPMOST, WS_POPUP, WS_SYSMENU,
            },
        },
    },
};

/// Size of the window's inside, in unscaled units.
const SIZE: (f32, f32) = (520.0, 404.0);
/// How long each way of using it is played, and how often it is redrawn.
const SCENE_SECONDS: f32 = 3.2;
const FRAME_MS: u32 = 33;
const TIMER_ID: usize = 1;

/// The words of the window, in the language Windows is shown in.
pub struct Words {
    pub title: &'static str,
    /// What each scene shows: sideways, up and down, a click, pinning.
    pub scenes: [&'static str; 4],
    pub hold: &'static str,
    pub click: &'static str,
    pub screen: &'static str,
    /// Where the icon is, in two lines.
    pub tray: [&'static str; 2],
    pub button: &'static str,
    /// The notification once the window is closed.
    pub balloon: (&'static str, &'static str),
    /// The line of the tray menu that opens this window again.
    pub menu: &'static str,
    /// The notice when a window run as administrator is in front, so that
    /// the trigger cannot be heard.
    pub admin_front: (&'static str, &'static str),
}

const ENGLISH: Words = Words {
    title: "Flick is running",
    scenes: [
        "Hold the side button and move sideways: the next desktop",
        "Up and down goes to another row",
        "Click it once: every desktop and window",
        "Pin a window to keep it on every desktop",
    ],
    hold: "hold + move",
    click: "click once",
    screen: "your screen",
    tray: ["Flick lives by the clock, maybe inside ^.", "Drag it out to keep it in sight."],
    button: "Got it",
    balloon: ("Flick is running here", "Right-click this icon for Settings and How to use."),
    menu: "How to use",
    admin_front: ("Flick can't hear its button now", "A window run as administrator is in front. Click another window, or start Flick again and let it run as administrator."),
};

const KOREAN: Words = Words {
    title: "Flick 이 켜졌어요",
    scenes: [
        "옆 단추를 누른 채 옆으로: 옆 데스크톱",
        "위아래로: 다른 줄",
        "한 번 누르면: 모든 데스크톱과 창",
        "창을 고정하면 모든 데스크톱에 보여요",
    ],
    hold: "누른 채 움직이기",
    click: "한 번 누르기",
    screen: "지금 화면",
    tray: ["Flick 은 시계 옆에 있어요. ^ 안에 있을 수도 있어요.", "끌어서 꺼내 두면 늘 보여요."],
    button: "알겠어요",
    balloon: ("Flick 이 여기서 돌고 있어요", "이 아이콘을 오른쪽 클릭하면 설정과 쓰는 법이 있어요."),
    menu: "쓰는 법",
    admin_front: ("지금은 Flick 버튼이 안 들려요", "관리자 권한 창이 앞에 있어요. 다른 창을 누르거나, Flick 을 다시 켜고 관리자 권한을 허용하세요."),
};

const CHINESE: Words = Words {
    title: "Flick 已在运行",
    scenes: ["按住侧键左右移动：下一个桌面", "上下移动：换一行", "按一下：所有桌面和窗口", "固定窗口，让它出现在每个桌面"],
    hold: "按住并移动",
    click: "按一下",
    screen: "你的屏幕",
    tray: ["Flick 在时钟旁边，可能在 ^ 里。", "把它拖出来，随时可见。"],
    button: "知道了",
    balloon: ("Flick 正在这里运行", "右键点击此图标可打开设置和使用方法。"),
    menu: "使用方法",
    admin_front: ("Flick 现在听不到按键", "前面是以管理员身份运行的窗口。点一下别的窗口，或重新启动 Flick 并允许它以管理员身份运行。"),
};

/// The words for the language Windows is shown in: Korean, Chinese, or
/// English for any other.
pub fn words() -> &'static Words {
    match unsafe { GetUserDefaultUILanguage() } & 0x3FF {
        0x12 => &KOREAN,
        0x04 => &CHINESE,
        _ => &ENGLISH,
    }
}

/// What the user asked for; carried out by the app.
pub enum Action {
    None,
    Close,
}

struct Fonts {
    title: IDWriteTextFormat,
    text: IDWriteTextFormat,
    small: IDWriteTextFormat,
    small_centred: IDWriteTextFormat,
    button: IDWriteTextFormat,
    arrow: IDWriteTextFormat,
}

impl Fonts {
    fn new(s: f32) -> Result<Fonts> {
        let factory = paint::dwrite_factory()?;
        let left = DWRITE_TEXT_ALIGNMENT_LEADING;
        let centre = DWRITE_TEXT_ALIGNMENT_CENTER;
        Ok(Fonts {
            title: paint::text_format(&factory, 18.0 * s, 600, left)?,
            text: paint::text_format(&factory, 14.0 * s, 400, left)?,
            small: paint::text_format(&factory, 12.0 * s, 400, left)?,
            small_centred: paint::text_format(&factory, 11.5 * s, 400, centre)?,
            button: paint::text_format(&factory, 13.5 * s, 600, centre)?,
            arrow: paint::text_format(&factory, 24.0 * s, 600, centre)?,
        })
    }
}

/// The windows on each desktop of the little map: (colour, offset) each.
const DESKTOPS: [&[(usize, f32)]; 9] = [
    &[(0, 1.0)],
    &[(1, 0.0), (2, 3.0)],
    &[(3, 2.0)],
    &[(4, 1.0), (0, 4.0)],
    &[(5, 0.0)],
    &[(1, 3.0), (3, 1.0)],
    &[(2, 0.0)],
    &[(4, 4.0), (5, 2.0)],
    &[(0, 0.0), (2, 2.0)],
];

fn window_colour(i: usize, a: f32) -> D2D1_COLOR_F {
    let (r, g, b) = [(0.88, 0.42, 0.46), (0.38, 0.69, 0.94), (0.60, 0.76, 0.47), (0.78, 0.47, 0.87), (0.90, 0.75, 0.48), (0.34, 0.71, 0.76)][i];
    rgba(r, g, b, a)
}

/// The value at `t` along stops (time, value), moving smoothly from one to
/// the next over `glide` seconds.
fn along(t: f32, stops: &[(f32, f32)], glide: f32) -> f32 {
    let mut value = stops[0].1;
    for pair in stops.windows(2) {
        let (from, (at, to)) = (pair[0].1, pair[1]);
        let k = ((t - at) / glide).clamp(0.0, 1.0);
        let eased = k * k * (3.0 - 2.0 * k);
        if t >= at {
            value = from + (to - from) * eased;
        }
    }
    value
}

pub struct Guide {
    hwnd: HWND,
    factory: ID2D1Factory,
    canvas: Option<Canvas>,
    fonts: Option<Fonts>,
    words: &'static Words,
    size: (i32, i32),
    scale: f32,
    started: Instant,
    /// Seconds into the playing, when drawn for a picture instead.
    frozen: Option<f32>,
    hover: bool,
    /// The button was pressed and not let go yet: only a click that both
    /// starts and ends on it puts the window away.
    pressed: bool,
}

impl Guide {
    pub fn new(wndproc: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT, icon: HICON) -> Result<Guide> {
        let words = words();
        unsafe {
            let instance = GetModuleHandleW(None)?;
            let class = w!("flick.guide");
            RegisterClassW(&WNDCLASSW {
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                hIcon: icon,
                lpfnWndProc: Some(wndproc),
                hInstance: instance.into(),
                lpszClassName: class,
                ..Default::default()
            });
            // On the taskbar like any window, so it is not missed.
            let hwnd = CreateWindowExW(
                WS_EX_TOPMOST,
                class,
                w!("Flick"),
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
            let (dark, colour) = (BOOL::from(true), paint::backdrop());
            let bgr = |c: f32, shift: u32| ((c * 255.0).round() as u32) << shift;
            let caption = COLORREF(bgr(colour.r, 0) | bgr(colour.g, 8) | bgr(colour.b, 16));
            let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const _ as *const _, 4);
            let _ = DwmSetWindowAttribute(hwnd, DWMWA_CAPTION_COLOR, &caption as *const _ as *const _, 4);
            Ok(Guide {
                hwnd,
                factory: paint::d2d_factory()?,
                canvas: None,
                fonts: None,
                words,
                size: (0, 0),
                scale: 1.0,
                started: Instant::now(),
                frozen: None,
                hover: false,
                pressed: false,
            })
        }
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    pub fn words(&self) -> &'static Words {
        self.words
    }

    pub fn is_open(&self) -> bool {
        unsafe { IsWindowVisible(self.hwnd) }.as_bool()
    }

    /// Shows the window in the middle of the primary monitor, playing from
    /// the start. Not `activate`d it leaves the keyboard where it is, so that
    /// keys meant for another window (Enter, Esc) do not put it away unseen.
    pub fn open(&mut self, activate: bool) {
        self.started = Instant::now();
        self.hover = false;
        self.pressed = false;
        if self.is_open() {
            return self.invalidate();
        }
        let monitors = vd::monitors();
        let Some(home) = monitors.iter().find(|m| m.primary) else { return };
        self.set_scale(home.scale);
        let mut frame = RECT { left: 0, top: 0, right: self.size.0, bottom: self.size.1 };
        unsafe {
            let dpi = (home.scale * 96.0).round() as u32;
            let _ = AdjustWindowRectExForDpi(&mut frame, WS_POPUP | WS_CAPTION | WS_SYSMENU, false, WS_EX_TOPMOST, dpi);
            let (w, h) = (frame.right - frame.left, frame.bottom - frame.top);
            let (x, y) = ((home.work.left + home.work.right - w) / 2, (home.work.top + home.work.bottom - h) / 2);
            let quiet = if activate { SET_WINDOW_POS_FLAGS(0) } else { SWP_NOACTIVATE };
            let _ = SetWindowPos(self.hwnd, HWND_TOPMOST, x, y, w, h, SWP_SHOWWINDOW | quiet);
            SetTimer(self.hwnd, TIMER_ID, FRAME_MS, None);
        }
    }

    pub fn close(&mut self) {
        self.canvas = None;
        unsafe {
            let _ = KillTimer(self.hwnd, TIMER_ID);
            ShowWindow(self.hwnd, SW_HIDE);
        }
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

    fn button(&self) -> Rect {
        let s = self.scale;
        rect(392.0 * s, 362.0 * s, 104.0 * s, 30.0 * s)
    }

    /// Handles a window message. `None` means "not ours, use the default".
    pub fn handle(&mut self, message: u32, wparam: WPARAM, lparam: LPARAM) -> Option<Action> {
        let (x, y) = ((lparam.0 & 0xFFFF) as i16 as f32, ((lparam.0 >> 16) & 0xFFFF) as i16 as f32);
        let on_button = |b: &Rect| x >= b.left && x <= b.right && y >= b.top && y <= b.bottom;
        Some(match message {
            WM_PAINT => {
                self.paint();
                unsafe {
                    ValidateRect(self.hwnd, None);
                }
                Action::None
            }
            WM_ERASEBKGND => Action::None,
            WM_TIMER if wparam.0 == TIMER_ID => {
                self.invalidate();
                Action::None
            }
            WM_MOUSEMOVE => {
                self.hover = on_button(&self.button());
                Action::None
            }
            WM_LBUTTONDOWN => {
                self.pressed = on_button(&self.button());
                Action::None
            }
            WM_LBUTTONUP => {
                let clicked = std::mem::take(&mut self.pressed) && on_button(&self.button());
                if clicked { Action::Close } else { Action::None }
            }
            WM_KEYDOWN if wparam.0 as u16 == VK_ESCAPE.0 || wparam.0 as u16 == VK_RETURN.0 => Action::Close,
            WM_DPICHANGED => {
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

    // ---- drawing ----------------------------------------------------------

    fn draw_to_canvas(&mut self) -> Option<Result<()>> {
        if !self.canvas.as_ref().is_some_and(|c| c.dib.size == self.size) {
            self.canvas =
                Canvas::new(&self.factory, self.size).map_err(|e| crate::app::log(&format!("guide canvas failed: {e}"))).ok();
        }
        if self.fonts.is_none() {
            self.fonts = Fonts::new(self.scale).map_err(|e| crate::app::log(&format!("guide fonts failed: {e}"))).ok();
        }
        let target = self.canvas.as_ref()?.target.target.clone();
        unsafe {
            target.BeginDraw();
            let drawn = self.draw(&target);
            Some(target.EndDraw(None, None).and(drawn))
        }
    }

    fn paint(&mut self) {
        let mut inside = RECT::default();
        if unsafe { GetClientRect(self.hwnd, &mut inside) }.is_ok() && inside.right > 0 {
            self.size = (inside.right, inside.bottom);
        }
        match self.draw_to_canvas() {
            Some(Ok(())) => {}
            Some(Err(e)) => {
                crate::app::log(&format!("guide draw failed: {e}"));
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
        let r = |x: f32, y: f32, w: f32, h: f32| rect(x * s, y * s, w * s, h * s);
        let words = self.words;
        let clock = self.frozen.unwrap_or_else(|| self.started.elapsed().as_secs_f32());
        let scene = (clock / SCENE_SECONDS) as usize % 4;
        let t = clock % SCENE_SECONDS;

        p.text(words.title, &fonts.title, &r(24.0, 14.0, 472.0, 30.0), white(0.95));

        // The stage: the screen, the desktops as a map, and the mouse.
        let stage = r(24.0, 52.0, 472.0, 192.0);
        p.fill(&stage, 10.0 * s, rgba(0.0, 0.0, 0.0, 0.35));
        let (col, row) = match scene {
            0 => (along(t, &[(0.0, 0.0), (0.7, 1.0), (1.5, 2.0)], 0.45), 1.0),
            1 => (1.0, along(t, &[(0.0, 0.0), (0.7, 1.0), (1.5, 2.0)], 0.45)),
            _ => (1.0, 1.0),
        };
        let current = (row.round() as usize) * 3 + col.round() as usize;

        // The screen shows the windows of the desktop the highlight is on.
        let screen = r(44.0, 72.0, 132.0, 88.0);
        p.fill(&screen, 6.0 * s, rgba(0.17, 0.23, 0.33, 1.0));
        for &(c, v) in DESKTOPS[current] {
            p.fill(&r(56.0 + v * 12.0, 80.0 + v * 7.0, 72.0, 48.0), 3.0 * s, window_colour(c, 0.95));
        }
        p.fill_square(&r(44.0, 150.0, 132.0, 10.0), rgba(0.12, 0.12, 0.12, 1.0));
        p.stroke(&screen, 6.0 * s, 2.0 * s, white(0.25));
        p.text(words.screen, &fonts.small_centred, &r(44.0, 164.0, 132.0, 18.0), white(0.45));

        let (cw, ch, gap, mx, my) = (58.0, 36.0, 7.0, 200.0, 72.0);
        for i in 0..9 {
            let (x, y) = (mx + (i % 3) as f32 * (cw + gap), my + (i / 3) as f32 * (ch + gap));
            p.fill(&r(x, y, cw, ch), 5.0 * s, white(0.08));
            for &(c, v) in DESKTOPS[i] {
                p.fill(&r(x + 5.0 + v * 5.0, y + 4.0 + v * 2.5, cw * 0.55, ch * 0.52), 2.0 * s, window_colour(c, 0.85));
            }
        }
        let board_shown = match scene {
            2 => ((t - 0.95) / 0.3).clamp(0.0, 1.0),
            3 => 1.0,
            _ => 0.0,
        };
        if board_shown < 1.0 {
            let (x, y) = (mx + col * (cw + gap), my + row * (ch + gap));
            p.stroke(&r(x - 2.0, y - 2.0, cw + 4.0, ch + 4.0), 6.0 * s, 3.0 * s, accent(1.0 - board_shown));
        }

        // The mouse: the side button lit while held, moving the way the hand does.
        let (dx, dy) = match scene {
            0 => (along(t, &[(0.0, 0.0), (0.4, 26.0)], 1.3), 0.0),
            1 => (0.0, along(t, &[(0.0, -12.0), (0.4, 16.0)], 1.3)),
            _ => (0.0, 0.0),
        };
        let pressed = match scene {
            0 | 1 => t < 2.4,
            2 => (0.4..0.65).contains(&t),
            _ => false,
        };
        let yellow = |a: f32| rgba(1.0, 0.8, 0.3, a);
        let (ox, oy) = (412.0 + dx, 104.0 + dy);
        let body = r(ox, oy, 36.0, 54.0);
        p.fill(&body, 18.0 * s, white(0.06));
        p.stroke(&body, 18.0 * s, 2.0 * s, white(0.65));
        p.line(((ox + 18.0) * s, oy * s), ((ox + 18.0) * s, (oy + 20.0) * s), 2.0 * s, white(0.55));
        p.fill(&r(ox - 8.0, oy + 14.0, 8.0, 11.0), 2.5 * s, if pressed { yellow(1.0) } else { white(0.35) });
        p.fill(&r(ox - 8.0, oy + 28.0, 8.0, 11.0), 2.5 * s, white(0.35));
        if pressed {
            p.stroke(&r(ox - 11.0, oy + 11.0, 14.0, 17.0), 4.0 * s, 1.5 * s, yellow(0.6));
        }
        if scene == 2 && (0.4..1.0).contains(&t) {
            let k = (t - 0.4) / 0.6;
            p.stroke(&r(ox - 4.0 - 14.0 * k, oy + 19.5 - 14.0 * k, 28.0 * k, 28.0 * k), 40.0 * s, 2.0 * s, yellow(1.0 - k));
        }
        let note = r(372.0, 214.0, 116.0, 20.0);
        match scene {
            0 => {
                p.text("⟶", &fonts.arrow, &r(406.0, 62.0, 70.0, 34.0), yellow(1.0));
                p.text(words.hold, &fonts.small_centred, &note, yellow(0.95));
            }
            1 => {
                p.text("↓", &fonts.arrow, &r(458.0, 104.0, 30.0, 40.0), yellow(1.0));
                p.text(words.hold, &fonts.small_centred, &note, yellow(0.95));
            }
            2 => p.text(words.click, &fonts.small_centred, &note, yellow(0.95)),
            _ => {}
        }

        // The board, over the stage, with two windows pinned.
        if board_shown > 0.0 {
            let a = board_shown;
            let inner = r(34.0, 62.0, 452.0, 172.0);
            p.fill(&inner, 8.0 * s, rgba(0.10, 0.11, 0.14, a));
            for i in 0..8 {
                let tile = r(46.0 + (i % 4) as f32 * 109.0, 74.0 + (i / 4) as f32 * 78.0, 99.0, 68.0);
                p.fill(&tile, 5.0 * s, white(0.08 * a));
                p.fill(&r(tile.left / s + 8.0, tile.top / s + 16.0, 62.0, 40.0), 3.0 * s, window_colour(i % 6, 0.8 * a));
                if i % 4 == 0 {
                    let pulse = if scene == 3 { 0.6 + 0.4 * (t * 4.0).sin().abs() } else { 1.0 };
                    p.stroke(&tile, 5.0 * s, 2.5 * s, rgba(0.62, 0.45, 1.0, a * pulse));
                }
            }
        }

        p.text(words.scenes[scene], &fonts.text, &r(24.0, 252.0, 472.0, 26.0), white(0.85));

        // Where Flick is: by the clock, perhaps hidden under ^, and how it
        // is dragged out. The drawing has room of its own under the words.
        p.text(words.tray[0], &fonts.small, &r(24.0, 300.0, 300.0, 20.0), white(0.6));
        p.text(words.tray[1], &fonts.small, &r(24.0, 320.0, 300.0, 20.0), white(0.6));
        let bar = r(336.0, 326.0, 160.0, 28.0);
        p.fill(&bar, 6.0 * s, rgba(0.13, 0.13, 0.13, 1.0));
        let chevron = r(342.0, 330.0, 20.0, 20.0);
        p.fill(&chevron, 4.0 * s, yellow(0.25));
        p.text("^", &fonts.small_centred, &chevron, white(0.9));
        p.text("A  가   12:00", &fonts.small, &r(392.0, 326.0, 104.0, 28.0), white(0.75));
        let k = along(clock % 3.0, &[(0.0, 0.0), (0.7, 1.0)], 1.1);
        p.fill(&r(338.0, 290.0, 28.0, 28.0), 5.0 * s, rgba(0.17, 0.17, 0.17, 1.0 - k));
        let (ix, iy) = (344.0 + 22.0 * k, 296.0 + 38.0 * k);
        p.fill(&r(ix, iy, 16.0, 16.0), 4.0 * s, accent(1.0));

        let button = self.button();
        p.fill(&button, 7.0 * s, accent(if self.hover { 1.0 } else { 0.85 }));
        p.text(words.button, &fonts.button, &button, white(1.0));
        Ok(())
    }

    /// Draws the window `seconds` into the playing, at `scale`, into a bitmap
    /// and returns its size and BGRA pixels, for reviewing it unshown.
    pub fn render_to_pixels(&mut self, seconds: f32, scale: f32) -> Option<((i32, i32), Vec<u8>)> {
        self.frozen = Some(seconds);
        self.set_scale(scale);
        let _ = self.draw_to_canvas()?;
        Some((self.size, self.canvas.as_ref()?.dib.pixels().to_vec()))
    }
}
