//! The window shown on the first run, and again from the tray menu or when
//! Flick is started while it is already running: a short setup wizard, then
//! how it is used, played over and over, and where its icon is.
//!
//! Drawn like the settings window. It changes nothing itself; what the user
//! does is an `Action` for the app, which answers with a new `View`.

use crate::{
    paint::{self, accent, rect, rgba, white, Canvas, Painter, Rect},
    touchpad::Status,
    vd,
};
use std::time::{Duration, Instant};
use windows::{
    core::{w, Error, Result},
    Win32::{
        Foundation::{BOOL, COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM},
        Globalization::GetUserDefaultUILanguage,
        Graphics::{
            Direct2D::{Common::D2D1_COLOR_F, ID2D1Factory, ID2D1RenderTarget},
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
                GetKeyState, ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VK_ESCAPE,
                VK_RETURN, VK_TAB,
            },
            WindowsAndMessaging::{
                CreateWindowExW, GetClientRect, IsWindowVisible, KillTimer, LoadCursorW, RegisterClassW, SetTimer,
                SetWindowPos, ShowWindow, HICON, HWND_TOPMOST, IDC_ARROW, SET_WINDOW_POS_FLAGS, SWP_NOACTIVATE,
                SWP_NOZORDER, SWP_SHOWWINDOW, SW_HIDE, WM_DPICHANGED, WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONDOWN,
                WM_ACTIVATE, WM_LBUTTONUP, WM_MOUSEMOVE, WM_PAINT, WM_TIMER, WNDCLASSW, WS_CAPTION, WS_EX_TOPMOST, WS_POPUP,
                WS_SYSMENU,
            },
        },
    },
};

/// Sent when the cursor leaves a window that asked to be told
/// (`TrackMouseEvent`).
const WM_MOUSELEAVE: u32 = 0x02A3;

/// Size of the window's inside, in unscaled units.
const SIZE: (f32, f32) = (560.0, 460.0);
const MARGIN: f32 = 28.0;
const WIDTH: f32 = SIZE.0 - 2.0 * MARGIN;
const RIGHT: f32 = SIZE.0 - MARGIN;
const FOOTER_TOP: f32 = SIZE.1 - 18.0 - 30.0;
const PRIMARY_WIDTH: f32 = 104.0;
const BACK_WIDTH: f32 = 88.0;
const BUTTON_GAP: f32 = 10.0;
/// How long each way of using it is played, and how often it is redrawn.
const SCENE_SECONDS: f32 = 3.2;
const FRAME_MS: u32 = 33;
const TIMER_ID: usize = 1;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Page {
    Trigger,
    Touchpad,
    Try,
    Tour,
}

/// Which pages show: Touchpad only when a precision touchpad exists.
fn pages(touchpad: bool) -> &'static [Page] {
    const WITH_TOUCHPAD: &[Page] = &[Page::Trigger, Page::Touchpad, Page::Try, Page::Tour];
    const WITHOUT_TOUCHPAD: &[Page] = &[Page::Trigger, Page::Try, Page::Tour];
    if touchpad {
        WITH_TOUCHPAD
    } else {
        WITHOUT_TOUCHPAD
    }
}

fn neighbour(list: &[Page], page: Page, forward: bool) -> Option<Page> {
    let i = list.iter().position(|p| *p == page)?;
    if forward {
        list.get(i + 1).copied()
    } else {
        i.checked_sub(1).map(|before| list[before])
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub enum Listen {
    #[default]
    Off,
    Mouse,
    Keys,
}

/// What the app tells the wizard.
#[derive(Clone, Default)]
pub struct View {
    pub trigger: String,
    pub listen: Listen,
    pub heard: bool,
    pub touchpad: Status,
    pub three_finger: bool,
    pub desktops: usize,
    pub moved: bool,
    pub board_opened: bool,
}

/// What the user asked for; carried out by the app.
pub enum Action {
    None,
    Close,
    Shown(Page),
    Listen(Listen),
    ThreeFinger(bool),
    OpenTouchpadSettings,
    MakeDesktops,
}

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
    pub page_trigger: &'static str,
    pub trigger_body: &'static str,
    pub now: &'static str,
    pub listening: &'static str,
    pub heard: &'static str,
    pub use_key: &'static str,
    pub keys_listening: &'static str,
    pub hint: &'static str,
    pub touchpad_note: &'static str,
    pub touchpad_title: &'static str,
    pub touchpad_body: &'static str,
    pub three_fingers: &'static str,
    pub open_touchpad: &'static str,
    pub try_title: &'static str,
    pub one_desktop: &'static str,
    pub make_desktops: &'static str,
    pub desktop_count: &'static str,
    pub check_move: &'static str,
    pub check_click: &'static str,
    pub or_three: &'static str,
    pub overview_note: &'static str,
    pub skip: &'static str,
    pub back: &'static str,
    pub next: &'static str,
    /// On the Trigger page while another window has the keyboard: it listens only while in front.
    pub click_first: &'static str,
}

const ENGLISH: Words = Words {
    title: "Flick is running",
    scenes: [
        "Hold {trigger} + move sideways: next desktop",
        "Up and down goes to another shelf (row)",
        "Click it once: every desktop and window",
        "Pin: Off → This shelf → All shelves",
    ],
    hold: "hold + move",
    click: "click once",
    screen: "your screen",
    tray: ["Flick lives by the clock, maybe inside ^.", "Drag it out to keep it in sight."],
    button: "Got it",
    balloon: ("Flick is running here", "Right-click this icon for Settings and the setup guide."),
    menu: "Setup and how to use",
    admin_front: ("Flick can't hear its button now", "A window run as administrator is in front. Click another window, or enable Run as administrator in Settings."),
    page_trigger: "Choose your Flick button",
    trigger_body: "Flick works while you hold one button and move the mouse. Press the mouse button you want to use now.",
    now: "Now: {trigger}",
    listening: "Listening… side buttons (back/forward) work best",
    heard: "✓ Heard it. Flick button: {trigger}",
    use_key: "Use a key…",
    keys_listening: "Press the key or keys… (Esc: cancel)",
    hint: "Nothing showing up? No side buttons: press the wheel button, or use a key. Mouse software (Logitech Options+, G HUB, Razer Synapse…) can take over side buttons: set them back to Back/Forward there, or have them send a key combination and choose it with Use a key.",
    touchpad_note: "On a laptop? Three fingers on the touchpad can work too: next page.",
    touchpad_title: "Three fingers on the touchpad",
    touchpad_body: "Swipe with three fingers to move as if holding the button; tap with three fingers to see every desktop.",
    three_fingers: "Use three fingers",
    open_touchpad: "Open Windows touchpad settings",
    try_title: "Try it",
    one_desktop: "This PC has only one desktop, so there is nowhere to go yet.",
    make_desktops: "Make 3 desktops",
    desktop_count: "{n} desktops in your grid",
    check_move: "Hold {trigger} and move the mouse sideways: the next desktop",
    check_click: "Click {trigger} once: every desktop and window",
    or_three: " (or three fingers)",
    overview_note: "To close the overview: Esc, or click an empty spot.",
    skip: "Skip",
    back: "Back",
    next: "Next",
    click_first: "Click this window, then press the button",
};

const KOREAN: Words = Words {
    title: "Flick 이 켜졌어요",
    scenes: [
        "{trigger} 누른 채 옆으로: 옆 데스크톱",
        "위아래로: 다른 셸프(행)",
        "한 번 누르면: 모든 데스크톱과 창",
        "고정: 끄기 → 이 셸프 → 모든 셸프",
    ],
    hold: "누른 채 움직이기",
    click: "한 번 누르기",
    screen: "지금 화면",
    tray: ["Flick 은 시계 옆에 있어요. ^ 안에 있을 수도 있어요.", "끌어서 꺼내 두면 늘 보여요."],
    button: "알겠어요",
    balloon: ("Flick 이 여기서 돌고 있어요", "이 아이콘을 오른쪽 클릭하면 설정과 설정 마법사가 있어요."),
    menu: "설정 마법사와 사용법",
    admin_front: ("지금은 Flick 버튼이 안 들려요", "관리자 권한 창이 앞에 있어요. 다른 창을 누르거나, 설정에서 관리자 권한 실행을 켜세요."),
    page_trigger: "Flick 버튼 정하기",
    trigger_body: "Flick은 버튼 하나를 누른 채 마우스를 움직여서 씁니다. 쓰고 싶은 마우스 버튼을 지금 눌러 보세요.",
    now: "지금: {trigger}",
    listening: "기다리는 중… 옆 버튼(뒤로/앞으로)이 가장 편해요",
    heard: "✓ 들렸어요. Flick 버튼: {trigger}",
    use_key: "키로 정하기…",
    keys_listening: "키나 키 조합을 누르세요… (Esc: 취소)",
    hint: "눌러도 반응이 없나요? 옆 버튼이 없으면 휠 버튼을 누르거나 '키로 정하기'를 쓰세요. 로지텍 Options+, G HUB, Razer Synapse 같은 마우스 프로그램이 옆 버튼을 가로챌 수 있어요. 거기서 버튼을 기본(뒤로/앞으로)으로 돌리거나, 키 조합을 보내게 한 뒤 '키로 정하기'로 그 버튼을 누르세요.",
    touchpad_note: "노트북이라면 터치패드 세 손가락도 쓸 수 있어요. 다음 장에서 켜세요.",
    touchpad_title: "터치패드 세 손가락",
    touchpad_body: "세 손가락으로 밀면 버튼을 누른 채 움직이는 것과 같고, 세 손가락으로 톡 치면 모든 데스크톱이 보여요.",
    three_fingers: "세 손가락 쓰기",
    open_touchpad: "Windows 터치패드 설정 열기",
    try_title: "해 보기",
    one_desktop: "지금 데스크톱이 하나뿐이라 아직 갈 곳이 없어요.",
    make_desktops: "데스크톱 3개 만들기",
    desktop_count: "데스크톱 {n}개가 있어요",
    check_move: "{trigger} 누른 채 마우스를 옆으로 → 옆 데스크톱",
    check_click: "{trigger} 한 번 클릭 → 모든 데스크톱과 창",
    or_three: " (또는 세 손가락)",
    overview_note: "전체 보기를 닫으려면 Esc나 빈 곳 클릭.",
    skip: "건너뛰기",
    back: "이전",
    next: "다음",
    click_first: "이 창을 한 번 누른 뒤 버튼을 눌러 보세요",
};

const CHINESE: Words = Words {
    title: "Flick 已在运行",
    scenes: ["按住 {trigger} 左右移动：下一个桌面", "上下移动：切换架子（行）", "按一下：所有桌面和窗口", "固定：关闭 → 本架子 → 所有架子"],
    hold: "按住并移动",
    click: "按一下",
    screen: "你的屏幕",
    tray: ["Flick 在时钟旁边，可能在 ^ 里。", "把它拖出来，随时可见。"],
    button: "知道了",
    balloon: ("Flick 正在这里运行", "右键点击此图标可打开设置和设置向导。"),
    menu: "设置向导和用法",
    admin_front: ("Flick 现在听不到按键", "前面是以管理员身份运行的窗口。点一下别的窗口，或在设置中开启以管理员身份运行。"),
    page_trigger: "选择 Flick 按键",
    trigger_body: "按住一个按键并移动鼠标即可使用 Flick。现在按一下你想用的鼠标按键。",
    now: "当前：{trigger}",
    listening: "正在等待…侧键（后退/前进）最好用",
    heard: "✓ 收到。Flick 按键：{trigger}",
    use_key: "改用按键…",
    keys_listening: "请按下按键或组合键…（Esc：取消）",
    hint: "按了没反应？没有侧键：按滚轮键，或用“改用按键”。罗技 Options+、G HUB、雷蛇 Synapse 等鼠标软件可能占用侧键：在其中恢复为后退/前进，或让它发送组合键，再用“改用按键”选择。",
    touchpad_note: "笔记本？也可以用触摸板三指，下一页开启。",
    touchpad_title: "触摸板三指",
    touchpad_body: "三指滑动相当于按住按键移动，三指轻点查看所有桌面。",
    three_fingers: "使用三指",
    open_touchpad: "打开 Windows 触摸板设置",
    try_title: "试一试",
    one_desktop: "目前只有一个桌面，还没有地方可去。",
    make_desktops: "建立 3 个桌面",
    desktop_count: "共有 {n} 个桌面",
    check_move: "按住{trigger}横向移动鼠标 → 下一个桌面",
    check_click: "单击{trigger}一次 → 所有桌面和窗口",
    or_three: "（或三指）",
    overview_note: "关闭总览：按 Esc 或点击空白处。",
    skip: "跳过",
    back: "上一步",
    next: "下一步",
    click_first: "先点一下此窗口，再按按键",
};

/// The words for the language Windows is shown in: Korean, Chinese, or
/// English for any other.
pub fn words() -> &'static Words {
    match crate::i18n::language(unsafe { GetUserDefaultUILanguage() }) {
        1 => &KOREAN,
        2 => &CHINESE,
        _ => &ENGLISH,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
enum Hit {
    #[default]
    Nothing,
    Skip,
    Back,
    Next,
    UseKey,
    ThreeFinger,
    OpenSettings,
    MakeDesktops,
}

struct Fonts {
    title: IDWriteTextFormat,
    big: IDWriteTextFormat,
    text: IDWriteTextFormat,
    small: IDWriteTextFormat,
    small_centred: IDWriteTextFormat,
    small_right: IDWriteTextFormat,
    check: IDWriteTextFormat,
    button: IDWriteTextFormat,
    arrow: IDWriteTextFormat,
}

impl Fonts {
    fn new(s: f32) -> Result<Fonts> {
        let factory = paint::dwrite_factory()?;
        let left = DWRITE_TEXT_ALIGNMENT_LEADING;
        let centre = DWRITE_TEXT_ALIGNMENT_CENTER;
        let text = paint::text_format(&factory, 14.0 * s, 400, left)?;
        let big = paint::text_format(&factory, 22.0 * s, 600, left)?;
        let small = paint::text_format(&factory, 12.0 * s, 400, left)?;
        unsafe {
            text.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP)?;
            big.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP)?;
            small.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP)?;
        }
        Ok(Fonts {
            title: paint::text_format(&factory, 18.0 * s, 600, left)?,
            big,
            text,
            small,
            small_centred: paint::text_format(&factory, 11.5 * s, 400, centre)?,
            small_right: paint::text_format(&factory, 11.5 * s, 400, DWRITE_TEXT_ALIGNMENT_TRAILING)?,
            check: paint::text_format(&factory, 15.0 * s, 700, centre)?,
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
    view: View,
    page: Page,
    size: (i32, i32),
    scale: f32,
    started: Instant,
    shown_at: Instant,
    /// Seconds into the playing, when drawn for a picture instead.
    frozen: Option<f32>,
    hover: Hit,
    press: Option<Hit>,
    focus: Hit,
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
                view: View::default(),
                page: Page::Trigger,
                size: (0, 0),
                scale: 1.0,
                started: Instant::now(),
                shown_at: Instant::now(),
                frozen: None,
                hover: Hit::Nothing,
                press: None,
                focus: Hit::Next,
            })
        }
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    pub fn page(&self) -> Page {
        self.page
    }

    pub fn set_view(&mut self, view: View) {
        self.view = view;
        self.invalidate();
    }

    pub fn words(&self) -> &'static Words {
        self.words
    }

    pub fn is_open(&self) -> bool {
        unsafe { IsWindowVisible(self.hwnd) }.as_bool()
    }

    /// Shows the window in the middle of the primary monitor, on its first
    /// page. Not `activate`d it leaves the keyboard where it is, so that
    /// keys meant for another window (Enter, Esc) do not put it away unseen.
    pub fn open(&mut self, view: View, activate: bool) -> Action {
        self.set_view(view);
        self.page = Page::Trigger;
        self.started = Instant::now();
        self.shown_at = Instant::now();
        self.frozen = None;
        self.hover = Hit::Nothing;
        self.press = None;
        self.focus = Hit::Next;
        if self.is_open() {
            self.invalidate();
            return Action::Shown(self.page);
        }
        let monitors = vd::monitors();
        let Some(home) = monitors.iter().find(|m| m.primary) else { return Action::None };
        self.set_scale(fitting_scale(home.scale, home.work.right - home.work.left));
        let mut frame = RECT { left: 0, top: 0, right: self.size.0, bottom: self.size.1 };
        unsafe {
            let dpi = (home.scale * 96.0).round() as u32;
            let _ = AdjustWindowRectExForDpi(&mut frame, WS_POPUP | WS_CAPTION | WS_SYSMENU, false, WS_EX_TOPMOST, dpi);
            let (w, h) = (frame.right - frame.left, frame.bottom - frame.top);
            let quiet = if activate { SET_WINDOW_POS_FLAGS(0) } else { SWP_NOACTIVATE };
            let fitted = fit_frame((home.work.left + home.work.right - w) / 2, (home.work.top + home.work.bottom - h) / 2, w, h, &home.work);
            let _ = SetWindowPos(self.hwnd, HWND_TOPMOST, fitted.left, fitted.top, fitted.right - fitted.left, fitted.bottom - fitted.top, SWP_SHOWWINDOW | quiet);
            SetTimer(self.hwnd, TIMER_ID, FRAME_MS, None);
        }
        // On every desktop, so that trying a move on the Try page does not
        // leave it behind on the desktop just left.
        let _ = crate::vdapi::pin_window(self.hwnd);
        Action::Shown(self.page)
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

    fn use_key_button(&self) -> Rect {
        let s = self.scale;
        rect(MARGIN * s, 186.0 * s, 132.0 * s, 30.0 * s)
    }

    fn three_finger_switch(&self) -> Rect {
        let s = self.scale;
        rect((RIGHT - 44.0) * s, 130.0 * s, 44.0 * s, 24.0 * s)
    }

    fn open_settings_button(&self) -> Rect {
        let s = self.scale;
        rect(MARGIN * s, 300.0 * s, 300.0 * s, 30.0 * s)
    }

    fn make_desktops_button(&self) -> Rect {
        let s = self.scale;
        rect(MARGIN * s, 94.0 * s, 210.0 * s, 30.0 * s)
    }

    fn footer_controls(&self) -> Vec<(Rect, Hit)> {
        let s = self.scale;
        let list = pages(self.view.touchpad != Status::Absent);
        let index = list.iter().position(|p| *p == self.page);
        let first = index == Some(0);
        let last = index.is_some_and(|i| i + 1 == list.len());
        let mut controls = Vec::new();
        if !last {
            controls.push((rect(MARGIN * s, FOOTER_TOP * s, 84.0 * s, 30.0 * s), Hit::Skip));
        }
        let next_left = RIGHT - PRIMARY_WIDTH;
        if !first {
            controls.push((rect((next_left - BUTTON_GAP - BACK_WIDTH) * s, FOOTER_TOP * s, BACK_WIDTH * s, 30.0 * s), Hit::Back));
        }
        controls.push((rect(next_left * s, FOOTER_TOP * s, PRIMARY_WIDTH * s, 30.0 * s), Hit::Next));
        controls
    }

    fn controls(&self) -> Vec<(Rect, Hit)> {
        let mut controls = self.footer_controls();
        match self.page {
            Page::Trigger => controls.push((self.use_key_button(), Hit::UseKey)),
            Page::Touchpad => {
                controls.push((self.three_finger_switch(), Hit::ThreeFinger));
                if self.view.touchpad == Status::Conflict {
                    controls.push((self.open_settings_button(), Hit::OpenSettings));
                }
            }
            Page::Try => {
                if self.view.desktops < 2 {
                    controls.push((self.make_desktops_button(), Hit::MakeDesktops));
                }
            }
            Page::Tour => {}
        }
        controls
    }

    fn hit_at(&self, x: f32, y: f32) -> Hit {
        self.controls()
            .into_iter()
            .find(|(r, _)| x >= r.left && x <= r.right && y >= r.top && y <= r.bottom)
            .map_or(Hit::Nothing, |(_, hit)| hit)
    }

    /// Handles a window message. `None` means "not ours, use the default".
    pub fn handle(&mut self, message: u32, wparam: WPARAM, lparam: LPARAM) -> Option<Action> {
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
            WM_TIMER if wparam.0 == TIMER_ID => {
                self.invalidate();
                Action::None
            }
            WM_MOUSEMOVE => {
                let hover = self.hit_at(x, y);
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
            WM_MOUSELEAVE => {
                self.hover = Hit::Nothing;
                self.invalidate();
                Action::None
            }
            WM_LBUTTONDOWN => {
                let hit = self.hit_at(x, y);
                if hit != Hit::Nothing {
                    self.focus = hit;
                }
                self.press = Some(hit);
                unsafe {
                    SetCapture(self.hwnd);
                }
                self.invalidate();
                Action::None
            }
            WM_LBUTTONUP => {
                unsafe {
                    let _ = ReleaseCapture();
                }
                let pressed = self.press.take();
                self.invalidate();
                match pressed {
                    Some(hit) if hit != Hit::Nothing && self.hit_at(x, y) == hit => self.activate(hit),
                    _ => Action::None,
                }
            }
            WM_KEYDOWN if self.view.listen != Listen::Keys => {
                let key = wparam.0 as u16;
                if key == VK_ESCAPE.0 {
                    Action::Close
                } else if key == VK_RETURN.0 {
                    self.advance()
                } else if key == VK_TAB.0 {
                    self.move_focus(unsafe { GetKeyState(0x10) } < 0);
                    Action::None
                } else {
                    return None;
                }
            }
            WM_KEYDOWN => Action::None,
            // Listens only while in front: a side button pressed for another
            // window (a browser's Back) must not quietly become the trigger.
            WM_ACTIVATE if self.page == Page::Trigger => {
                let active = wparam.0 & 0xFFFF != 0;
                match (active, self.view.listen) {
                    (true, Listen::Off) => Action::Listen(Listen::Mouse),
                    (false, Listen::Mouse | Listen::Keys) => Action::Listen(Listen::Off),
                    _ => Action::None,
                }
            }
            WM_DPICHANGED => {
                unsafe {
                    let to = *(lparam.0 as *const RECT);
                    let monitors = vd::monitors();
                    if let Some(home) = monitors.iter().max_by_key(|m| overlap(&to, &m.bounds)) {
                        self.set_scale(fitting_scale((wparam.0 & 0xffff) as f32 / 96.0, home.work.right - home.work.left));
                        let mut frame = RECT { right: self.size.0, bottom: self.size.1, ..Default::default() };
                        let _ = AdjustWindowRectExForDpi(&mut frame, WS_POPUP | WS_CAPTION | WS_SYSMENU, false, WS_EX_TOPMOST, (wparam.0 & 0xffff) as u32);
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

    fn move_focus(&mut self, backwards: bool) {
        let controls = self.controls();
        if controls.is_empty() {
            return;
        }
        let index = controls.iter().position(|(_, hit)| *hit == self.focus);
        let next = match index {
            Some(i) if backwards => (i + controls.len() - 1) % controls.len(),
            Some(i) => (i + 1) % controls.len(),
            None if backwards => controls.len() - 1,
            None => 0,
        };
        self.focus = controls[next].1;
        self.invalidate();
    }

    fn activate(&mut self, hit: Hit) -> Action {
        match hit {
            Hit::Skip => Action::Close,
            Hit::Back => self.back(),
            Hit::Next => self.advance(),
            Hit::UseKey => Action::Listen(Listen::Keys),
            Hit::ThreeFinger => Action::ThreeFinger(!self.view.three_finger),
            Hit::OpenSettings => Action::OpenTouchpadSettings,
            Hit::MakeDesktops => Action::MakeDesktops,
            Hit::Nothing => Action::None,
        }
    }

    fn advance(&mut self) -> Action {
        match neighbour(pages(self.view.touchpad != Status::Absent), self.page, true) {
            Some(page) => self.goto(page),
            None => Action::Close,
        }
    }

    fn back(&mut self) -> Action {
        match neighbour(pages(self.view.touchpad != Status::Absent), self.page, false) {
            Some(page) => self.goto(page),
            None => Action::None,
        }
    }

    fn goto(&mut self, page: Page) -> Action {
        self.page = page;
        self.shown_at = Instant::now();
        self.focus = Hit::Next;
        self.hover = Hit::Nothing;
        self.press = None;
        self.invalidate();
        Action::Shown(page)
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
        let clock = self.frozen.unwrap_or_else(|| self.started.elapsed().as_secs_f32());
        self.draw_header(&p, fonts);
        match self.page {
            Page::Trigger => self.draw_trigger(&p, fonts),
            Page::Touchpad => self.draw_touchpad(&p, fonts),
            Page::Try => self.draw_try(&p, fonts),
            Page::Tour => self.draw_tour(&p, fonts, clock, s),
        }
        self.draw_footer(&p, fonts);
        self.draw_focus(&p);
        Ok(())
    }

    fn draw_header(&self, p: &Painter, fonts: &Fonts) {
        let s = self.scale;
        let title = match self.page {
            Page::Trigger => self.words.page_trigger,
            Page::Touchpad => self.words.touchpad_title,
            Page::Try => self.words.try_title,
            Page::Tour => self.words.title,
        };
        p.text(title, &fonts.title, &rect(MARGIN * s, 14.0 * s, 360.0 * s, 30.0 * s), white(0.95));
        let list = pages(self.view.touchpad != Status::Absent);
        if let Some(i) = list.iter().position(|p| *p == self.page) {
            let step = format!("{} / {}", i + 1, list.len());
            p.text(&step, &fonts.small_right, &rect(400.0 * s, 18.0 * s, 132.0 * s, 20.0 * s), white(0.45));
        }
    }

    fn draw_footer(&self, p: &Painter, fonts: &Fonts) {
        let s = self.scale;
        let list = pages(self.view.touchpad != Status::Absent);
        let last = list.iter().position(|p| *p == self.page).is_some_and(|i| i + 1 == list.len());
        for (area, hit) in self.footer_controls() {
            match hit {
                Hit::Skip => {
                    let hot = self.hover == Hit::Skip;
                    if hot {
                        p.fill(&area, 7.0 * s, white(0.10));
                    }
                    p.text(self.words.skip, &fonts.button, &area, white(if hot { 0.95 } else { 0.55 }));
                }
                Hit::Back => self.draw_button(p, fonts, &area, self.words.back, Hit::Back, false),
                Hit::Next => {
                    let label = if last { self.words.button } else { self.words.next };
                    self.draw_button(p, fonts, &area, label, Hit::Next, true);
                }
                _ => {}
            }
        }
    }

    fn draw_focus(&self, p: &Painter) {
        let s = self.scale;
        if let Some((area, _)) = self.controls().into_iter().find(|(_, hit)| *hit == self.focus) {
            let ring = Rect { left: area.left - 3.0 * s, top: area.top - 3.0 * s, right: area.right + 3.0 * s, bottom: area.bottom + 3.0 * s };
            p.stroke(&ring, 5.0 * s, 2.0 * s, accent(1.0));
        }
    }

    fn draw_button(&self, p: &Painter, fonts: &Fonts, area: &Rect, text: &str, hit: Hit, primary: bool) {
        let s = self.scale;
        let hot = self.hover == hit;
        if primary {
            p.fill(area, 7.0 * s, accent(if hot { 1.0 } else { 0.85 }));
            p.text(text, &fonts.button, area, white(1.0));
        } else {
            p.fill(area, 7.0 * s, white(if hot { 0.20 } else { 0.10 }));
            p.text(text, &fonts.button, area, white(if hot { 1.0 } else { 0.85 }));
        }
    }

    fn draw_switch(&self, p: &Painter, switch: &Rect, on: bool, hit: Hit) {
        let h = switch.bottom - switch.top;
        let hot = self.hover == hit;
        p.fill(switch, h / 2.0, if on { accent(0.95) } else { white(if hot { 0.22 } else { 0.14 }) });
        let knob = if on { switch.right - h / 2.0 } else { switch.left + h / 2.0 };
        p.disc((knob, switch.top + h / 2.0), h / 2.0 - 3.0 * self.scale, white(0.95));
    }

    fn draw_trigger(&self, p: &Painter, fonts: &Fonts) {
        let s = self.scale;
        let words = self.words;
        let trigger = &self.view.trigger;
        p.text(words.trigger_body, &fonts.text, &rect(MARGIN * s, 58.0 * s, WIDTH * s, 52.0 * s), white(0.75));
        p.text(&filled(words.now, trigger, 0), &fonts.big, &rect(MARGIN * s, 116.0 * s, WIDTH * s, 34.0 * s), white(0.95));
        let (line, colour) = if self.view.heard {
            (filled(words.heard, trigger, 0), accent(1.0))
        } else if self.view.listen == Listen::Keys {
            (words.keys_listening.to_owned(), white(0.75))
        } else if self.view.listen == Listen::Mouse {
            (words.listening.to_owned(), white(0.75))
        } else {
            (words.click_first.to_owned(), white(0.75))
        };
        if !line.is_empty() {
            p.text(&line, &fonts.text, &rect(MARGIN * s, 156.0 * s, WIDTH * s, 24.0 * s), colour);
        }
        self.draw_button(p, fonts, &self.use_key_button(), words.use_key, Hit::UseKey, false);
        let mut y = 226.0;
        if self.view.touchpad != Status::Absent {
            p.text(words.touchpad_note, &fonts.small, &rect(MARGIN * s, y * s, WIDTH * s, 28.0 * s), white(0.55));
            y += 34.0;
        }
        let elapsed = self.frozen.unwrap_or_else(|| self.shown_at.elapsed().as_secs_f32());
        if !self.view.heard && elapsed >= 8.0 {
            p.text(words.hint, &fonts.small, &rect(MARGIN * s, y * s, WIDTH * s, 140.0 * s), white(0.55));
        }
    }

    fn draw_touchpad(&self, p: &Painter, fonts: &Fonts) {
        let s = self.scale;
        let words = self.words;
        p.text(words.touchpad_body, &fonts.text, &rect(MARGIN * s, 58.0 * s, WIDTH * s, 52.0 * s), white(0.75));
        p.text(words.three_fingers, &fonts.text, &rect(MARGIN * s, 130.0 * s, 300.0 * s, 30.0 * s), white(0.85));
        self.draw_switch(p, &self.three_finger_switch(), self.view.three_finger, Hit::ThreeFinger);
        let colour = if self.view.touchpad == Status::Active { accent(1.0) } else { white(0.70) };
        p.text(self.view.touchpad.words(), &fonts.text, &rect(MARGIN * s, 174.0 * s, WIDTH * s, 110.0 * s), colour);
        if self.view.touchpad == Status::Conflict {
            self.draw_button(p, fonts, &self.open_settings_button(), words.open_touchpad, Hit::OpenSettings, false);
        }
    }

    fn draw_try(&self, p: &Painter, fonts: &Fonts) {
        let s = self.scale;
        let words = self.words;
        let trigger = &self.view.trigger;
        if self.view.desktops < 2 {
            p.text(words.one_desktop, &fonts.text, &rect(MARGIN * s, 58.0 * s, WIDTH * s, 28.0 * s), white(0.80));
            self.draw_button(p, fonts, &self.make_desktops_button(), words.make_desktops, Hit::MakeDesktops, false);
        } else {
            let line = filled(words.desktop_count, trigger, self.view.desktops);
            p.text(&line, &fonts.text, &rect(MARGIN * s, 58.0 * s, WIDTH * s, 28.0 * s), white(0.80));
        }
        let start = if self.view.desktops < 2 { 140.0 } else { 100.0 };
        let touch = self.view.touchpad == Status::Active;
        let items = [(self.view.moved, filled(words.check_move, trigger, 0)), (self.view.board_opened, filled(words.check_click, trigger, 0))];
        for (i, (done, mut text)) in items.into_iter().enumerate() {
            let y = start + i as f32 * 52.0;
            let circle = rect(MARGIN * s, (y + 3.0) * s, 18.0 * s, 18.0 * s);
            if done {
                p.disc((circle.left + 9.0 * s, circle.top + 9.0 * s), 9.0 * s, accent(0.95));
                p.text("✓", &fonts.check, &rect(circle.left, circle.top - 1.0 * s, 18.0 * s, 18.0 * s), white(1.0));
            } else {
                p.stroke(&circle, 9.0 * s, 1.5 * s, white(0.35));
            }
            if touch {
                text.push_str(words.or_three);
            }
            p.text(&text, &fonts.text, &rect((MARGIN + 28.0) * s, y * s, (WIDTH - 28.0) * s, 44.0 * s), white(if done { 0.95 } else { 0.75 }));
        }
        p.text(words.overview_note, &fonts.small, &rect(MARGIN * s, 300.0 * s, WIDTH * s, 24.0 * s), white(0.55));
    }

    fn draw_tour(&self, p: &Painter, fonts: &Fonts, clock: f32, s: f32) {
        let r = |x: f32, y: f32, w: f32, h: f32| rect(x * s, y * s, w * s, h * s);
        let words = self.words;
        let scene = (clock / SCENE_SECONDS) as usize % 4;
        let t = clock % SCENE_SECONDS;

        // The stage: the screen, the desktops as a map, and the mouse.
        let stage = r(24.0, 52.0, 512.0, 192.0);
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
        let (ox, oy) = (452.0 + dx, 104.0 + dy);
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
        let note = r(412.0, 214.0, 116.0, 20.0);
        match scene {
            0 => {
                p.text("⟶", &fonts.arrow, &r(446.0, 62.0, 70.0, 34.0), yellow(1.0));
                p.text(words.hold, &fonts.small_centred, &note, yellow(0.95));
            }
            1 => {
                p.text("↓", &fonts.arrow, &r(498.0, 104.0, 30.0, 40.0), yellow(1.0));
                p.text(words.hold, &fonts.small_centred, &note, yellow(0.95));
            }
            2 => p.text(words.click, &fonts.small_centred, &note, yellow(0.95)),
            _ => {}
        }

        // The board, over the stage, with two windows pinned.
        if board_shown > 0.0 {
            let a = board_shown;
            let inner = r(34.0, 62.0, 492.0, 172.0);
            p.fill(&inner, 8.0 * s, rgba(0.10, 0.11, 0.14, a));
            for i in 0..8 {
                let tile = r(46.0 + (i % 4) as f32 * 119.0, 74.0 + (i / 4) as f32 * 78.0, 109.0, 68.0);
                p.fill(&tile, 5.0 * s, white(0.08 * a));
                p.fill(&r(tile.left / s + 8.0, tile.top / s + 16.0, 62.0, 40.0), 3.0 * s, window_colour(i % 6, 0.8 * a));
                if i % 4 == 0 {
                    let pulse = if scene == 3 { 0.6 + 0.4 * (t * 4.0).sin().abs() } else { 1.0 };
                    p.stroke(&tile, 5.0 * s, 2.5 * s, rgba(0.62, 0.45, 1.0, a * pulse));
                }
            }
        }

        p.text(&scene_text(words.scenes[scene], &self.view.trigger), &fonts.text, &r(24.0, 248.0, 512.0, 48.0), white(0.85));

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
    }

    /// Draws the page `page` showing `view` into a bitmap and returns its
    /// size and BGRA pixels, for reviewing it without showing it.
    pub fn render_to_pixels(&mut self, view: View, page: Page, seconds: f32, scale: f32) -> Option<((i32, i32), Vec<u8>)> {
        self.view = view;
        self.page = page;
        self.frozen = Some(seconds);
        // The hint is reviewed as it looks after the eight seconds it waits.
        self.shown_at = Instant::now() - Duration::from_secs(9);
        self.set_scale(scale);
        let _ = self.draw_to_canvas()?;
        Some((self.size, self.canvas.as_ref()?.dib.pixels().to_vec()))
    }
}

fn filled(template: &str, trigger: &str, n: usize) -> String {
    template.replace("{trigger}", trigger).replace("{n}", &n.to_string())
}

fn scene_text(template: &str, trigger: &str) -> String {
    template.replace("{trigger}", trigger)
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

#[cfg(test)]
mod wording_tests {
    use super::*;

    fn all(words: &Words) -> Vec<&'static str> {
        let mut all = vec![
            words.title,
            words.hold,
            words.click,
            words.screen,
            words.tray[0],
            words.tray[1],
            words.button,
            words.balloon.0,
            words.balloon.1,
            words.menu,
            words.admin_front.0,
            words.admin_front.1,
            words.page_trigger,
            words.trigger_body,
            words.now,
            words.listening,
            words.heard,
            words.use_key,
            words.keys_listening,
            words.hint,
            words.touchpad_note,
            words.touchpad_title,
            words.touchpad_body,
            words.three_fingers,
            words.open_touchpad,
            words.try_title,
            words.one_desktop,
            words.make_desktops,
            words.desktop_count,
            words.check_move,
            words.check_click,
            words.or_three,
            words.overview_note,
            words.skip,
            words.back,
            words.next,
            words.click_first,
        ];
        all.extend(words.scenes);
        all
    }

    #[test]
    fn guide_names_the_current_trigger_in_every_language() {
        for words in [&ENGLISH, &KOREAN, &CHINESE] {
            for text in all(words) {
                assert!(!text.is_empty());
                let text = filled(text, "F13", 3);
                assert!(!text.contains('{'), "{text}");
            }
            let text = scene_text(words.scenes[0], "F13");
            assert!(text.contains("F13"));
        }
    }

    #[test]
    fn pages_follow_the_touchpad_and_stop_at_both_ends() {
        assert_eq!(pages(true), &[Page::Trigger, Page::Touchpad, Page::Try, Page::Tour]);
        assert_eq!(pages(false), &[Page::Trigger, Page::Try, Page::Tour]);
        assert_eq!(neighbour(pages(true), Page::Trigger, false), None);
        assert_eq!(neighbour(pages(true), Page::Tour, true), None);
        assert_eq!(neighbour(pages(true), Page::Trigger, true), Some(Page::Touchpad));
        assert_eq!(neighbour(pages(true), Page::Touchpad, true), Some(Page::Try));
        assert_eq!(neighbour(pages(false), Page::Trigger, true), Some(Page::Try));
        assert_eq!(neighbour(pages(false), Page::Try, false), Some(Page::Trigger));
        assert_eq!(neighbour(pages(false), Page::Tour, false), Some(Page::Try));
    }
}
