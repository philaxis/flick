//! The settings window: the trigger and whether to start with Windows, then
//! a tab for each hand, the mouse and the touchpad, with how it goes
//! sideways along a row and up and down from row to row.
//!
//! It owns layout, hit-testing and drawing, in the look of the board. It
//! changes nothing itself; what the user does is turned into an `Action` for
//! the app, which answers with a new `View`.

use crate::{
    config::{CommitSpeed, Mouse, QuickFlick, Touchpad},
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
            Controls::SetScrollInfo,
            Input::KeyboardAndMouse::{
                GetKeyState, ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VK_ESCAPE, VK_TAB,
            },
            WindowsAndMessaging::{
                CreateWindowExW, GetClientRect, IsWindowVisible, LoadCursorW, RegisterClassW, SetWindowPos,
                ShowWindow, HWND_TOPMOST, IDC_ARROW, SWP_NOACTIVATE, SWP_NOZORDER, SWP_SHOWWINDOW, SW_HIDE,
                WM_MOUSEWHEEL, WM_VSCROLL, GetScrollInfo, SCROLLINFO, SIF_ALL, SB_VERT, WS_VSCROLL,
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
const SIZE: (f32, f32) = (656.0, ROWS.0 + FILE_ROW * ROWS.1 + 52.0);
/// Top of the first row and the height of each.
const ROWS: (f32, f32) = (14.0, 44.0);
/// Left edge of the labels and right edge of the controls.
const EDGES: (f32, f32) = (28.0, 628.0);
/// The tab's rows, the first with the tabs themselves above it, and the
/// link to the file below them.
const TAB_ROWS: (f32, f32) = (5.0, 13.0);
const FILE_ROW: f32 = 20.0;
/// The experiments' heading; their switch and choices follow, then a note.
const EXPERIMENT_ROW: f32 = 14.0;
/// The commit springs offered, as `config::CommitSpeed` names them.
const SPEEDS: [CommitSpeed; 3] = [CommitSpeed::Default, CommitSpeed::Gentle, CommitSpeed::Slow];
/// The quick flicks offered, as `config::QuickFlick` names them.
const QUICKS: [QuickFlick; 3] = [QuickFlick::Off, QuickFlick::Slide, QuickFlick::Instant];
/// How far the tab's controls stand in from its edges, and the room
/// between its two ways.
const INSET: f32 = 16.0;
const GAP: f32 = 24.0;
/// The width of a way's labels and of the value after its sliders.
const LABEL: f32 = 76.0;
const VALUE: f32 = 90.0;
/// What the sliders span, and the step they move in: the mouse's distances
/// in raw counts, the touchpad's in tenths of a mm.
const COUNTS: (i32, i32, i32) = (20, 600, 10);
const TENTHS_MM: (i32, i32, i32) = (10, 300, 5);

/// What the window shows.
#[derive(Clone, Default)]
pub struct View {
    /// The trigger in words.
    pub trigger: String,
    /// Waiting for the user to press what the trigger should become.
    pub capturing: bool,
    pub autostart: bool,
    pub admin: bool,
    /// See `Config::minimap`, `mouse`, `touchpad`.
    pub minimap: bool,
    pub mouse: Mouse,
    pub touchpad: Touchpad,
    pub touchpad_status: crate::touchpad::Status,
    /// See `config::Experimental`.
    pub motion_blur: bool,
    pub commit_speed: CommitSpeed,
    pub quick_flick: QuickFlick,
}

/// What the user asked for; carried out by the app.
pub enum Action {
    None,
    /// Use these for the gesture right away; a control is being dragged.
    Try { mouse: Mouse, touchpad: Touchpad },
    /// Write a value to the config file: its key and the value as TOML.
    Set(&'static str, String),
    /// Start (or give up) waiting for a new trigger to be pressed.
    Capture(bool),
    Autostart(bool),
    Admin(bool),
    /// Open the config file, which has the settings not shown here.
    OpenFile,
    TouchpadSettings,
    Close,
}

/// Which hand's settings are shown.
#[derive(Clone, Copy, PartialEq, Default, Debug)]
pub enum Tab {
    #[default]
    Mouse,
    Touchpad,
}

/// The ways a hand goes: sideways along a row (0) and up and down from row
/// to row (1), as `_x` and `_y` in the config.
const WAYS: [usize; 2] = [0, 1];

#[derive(Clone, Copy, Debug, PartialEq, Default)]
enum Hit {
    #[default]
    Nothing,
    Trigger,
    Autostart,
    Admin,
    Minimap,
    Tab(Tab),
    /// A way's distance and following the hand.
    Step(usize),
    Follow(usize),
    Reverse,
    Touchpad,
    TouchpadSettings,
    Blur,
    Speed(usize),
    Quick(usize),
    File,
}

/// Where the controls are, in pixels.
struct Layout {
    trigger: Rect,
    autostart: Rect,
    admin: Rect,
    minimap: Rect,
    tabs: [Rect; 2],
    /// The tab's area under the tabs.
    panel: Rect,
    /// Each way's column, its sliders' tracks and its switch.
    columns: [Rect; 2],
    steps: [Rect; 2],
    follows: [Rect; 2],
    reverse: Rect,
    touchpad: Rect,
    touchpad_settings: Rect,
    blur: Rect,
    speeds: [Rect; 3],
    quicks: [Rect; 3],
    file: Rect,
}

impl Layout {
    fn new(s: f32) -> Layout {
        let row = |i: f32| ROWS.0 + i * ROWS.1;
        // A control `w` wide and `h` high at the right of row `i`, ending at `right`.
        let at = |right: f32, i: f32, w: f32, h: f32| rect((right - w) * s, (row(i) + (ROWS.1 - h) / 2.0) * s, w * s, h * s);
        let right = |i: f32, w: f32, h: f32| at(EDGES.1, i, w, h);
        let inner = (EDGES.0 + INSET, EDGES.1 - INSET);
        let width = (inner.1 - inner.0 - GAP) / 2.0;
        let column = |way: usize| (inner.0 + way as f32 * (width + GAP), inner.0 + way as f32 * (width + GAP) + width);
        let slider = |way: usize, i: f32| {
            let (left, right) = column(way);
            rect((left + LABEL) * s, row(i) * s, (right - left - LABEL - VALUE) * s, ROWS.1 * s)
        };
        let tab = |i: f32| rect((EDGES.0 + i * 130.0) * s, (row(TAB_ROWS.0 - 1.0) + 10.0) * s, 130.0 * s, 34.0 * s);
        let (top, bottom) = (row(TAB_ROWS.0) - 2.0, row(TAB_ROWS.1) + 6.0);
        Layout {
            trigger: right(0.0, 64.0, 30.0),
            autostart: right(1.0, 44.0, 24.0),
            minimap: right(2.0, 44.0, 24.0),
            admin: right(3.0, 44.0, 24.0),
            tabs: [tab(0.0), tab(1.0)],
            panel: rect(EDGES.0 * s, top * s, (EDGES.1 - EDGES.0) * s, (bottom - top) * s),
            columns: WAYS.map(|way| {
                let (left, right) = column(way);
                rect(left * s, row(TAB_ROWS.0) * s, (right - left) * s, ROWS.1 * s)
            }),
            steps: WAYS.map(|way| slider(way, TAB_ROWS.0 + 1.0)),
            follows: WAYS.map(|way| at(column(way).1, TAB_ROWS.0 + 3.0, 44.0, 24.0)),
            reverse: at(inner.1, TAB_ROWS.0 + 4.0, 44.0, 24.0),
            touchpad: at(inner.1, TAB_ROWS.0 + 5.0, 44.0, 24.0),
            touchpad_settings: rect(inner.0 * s, (row(TAB_ROWS.0 + 7.0) + 6.0) * s, 260.0 * s, 30.0 * s),
            blur: right(EXPERIMENT_ROW + 1.0, 44.0, 24.0),
            speeds: [0, 1, 2].map(|i| at(EDGES.1 - (2 - i) as f32 * 96.0, EXPERIMENT_ROW + 2.0, 90.0, 30.0)),
            quicks: [0, 1, 2].map(|i| at(EDGES.1 - (2 - i) as f32 * 96.0, EXPERIMENT_ROW + 3.0, 90.0, 30.0)),
            file: rect(EDGES.0 * s, (row(FILE_ROW) + 8.0) * s, 190.0 * s, 26.0 * s),
        }
    }

    fn controls(&self, tab: Tab, conflict: bool) -> Vec<(Rect, Hit)> {
        let mut controls = vec![
            (self.trigger, Hit::Trigger), (self.autostart, Hit::Autostart),
            (self.minimap, Hit::Minimap), (self.admin, Hit::Admin),
            (self.tabs[0], Hit::Tab(Tab::Mouse)), (self.tabs[1], Hit::Tab(Tab::Touchpad)),
            (self.steps[0], Hit::Step(0)), (self.follows[0], Hit::Follow(0)),
            (self.steps[1], Hit::Step(1)), (self.follows[1], Hit::Follow(1)),
            (self.reverse, Hit::Reverse),
        ];
        if tab == Tab::Touchpad {
            controls.push((self.touchpad, Hit::Touchpad));
            if conflict { controls.push((self.touchpad_settings, Hit::TouchpadSettings)); }
        }
        controls.push((self.blur, Hit::Blur));
        controls.extend(self.speeds.iter().enumerate().map(|(i, r)| (*r, Hit::Speed(i))));
        controls.extend(self.quicks.iter().enumerate().map(|(i, r)| (*r, Hit::Quick(i))));
        controls.push((self.file, Hit::File));
        controls
    }
    fn hit(&self, tab: Tab, conflict: bool, x: f32, y: f32) -> Hit {
        self.controls(tab, conflict).into_iter()
            .find(|(r, _)| x >= r.left && x < r.right && y >= r.top && y < r.bottom)
            .map_or(Hit::Nothing, |(_, hit)| hit)
    }

}

/// One hand's settings as the tab shows them, sliders in their own units
/// (`COUNTS`, `TENTHS_MM`).
#[derive(Clone, Copy)]
struct Hand {
    steps: [i32; 2],
    follows: [bool; 2],
    reverse: bool,
}

impl View {
    fn hand(&self, tab: Tab) -> Hand {
        match tab {
            Tab::Mouse => {
                let m = &self.mouse;
                Hand {
                    steps: [m.step_x, m.step_y],
                    follows: [m.follow_x, m.follow_y],
                    reverse: m.reverse,
                }
            }
            Tab::Touchpad => {
                let t = &self.touchpad;
                let tenths = |mm: f64| (mm * 10.0).round() as i32;
                Hand {
                    steps: [tenths(t.step_x), tenths(t.step_y)],
                    follows: [t.follow_x, t.follow_y],
                    reverse: t.reverse,
                }
            }
        }
    }

    fn set_hand(&mut self, tab: Tab, hand: Hand) {
        match tab {
            Tab::Mouse => {
                let m = &mut self.mouse;
                [m.step_x, m.step_y] = hand.steps;
                [m.follow_x, m.follow_y] = hand.follows;
                m.reverse = hand.reverse;
            }
            Tab::Touchpad => {
                let t = &mut self.touchpad;
                [t.step_x, t.step_y] = hand.steps.map(|tenths| tenths as f64 / 10.0);
                [t.follow_x, t.follow_y] = hand.follows;
                t.reverse = hand.reverse;
            }
        }
    }
}

/// The config key of a control on `tab` (`config::with_value`).
fn key(tab: Tab, hit: Hit) -> Option<&'static str> {
    let mouse = tab == Tab::Mouse;
    Some(match hit {
        Hit::Step(0) => if mouse { "mouse.step_x" } else { "touchpad.step_x" },
        Hit::Step(_) => if mouse { "mouse.step_y" } else { "touchpad.step_y" },
        Hit::Follow(0) => if mouse { "mouse.follow_x" } else { "touchpad.follow_x" },
        Hit::Follow(_) => if mouse { "mouse.follow_y" } else { "touchpad.follow_y" },
        Hit::Reverse => if mouse { "mouse.reverse" } else { "touchpad.reverse" },
        Hit::Touchpad => "touchpad.three_finger",
        Hit::Minimap => "minimap",
        Hit::Blur => "experimental.motion_blur",
        Hit::Speed(_) => "experimental.commit_speed",
        Hit::Quick(_) => "experimental.quick_flick",
        _ => return None,
    })
}

/// A distance on `tab` in words, from the slider's units.
fn distance_words(tab: Tab, step: i32) -> String {
    match tab {
        Tab::Mouse => format!("{step} {}", tr("counts", "카운트", "计数")),
        Tab::Touchpad => format!("{}.{} mm", step / 10, step % 10),
    }
}



struct Fonts {
    label: IDWriteTextFormat,
    value: IDWriteTextFormat,
    button: IDWriteTextFormat,
    notice: IDWriteTextFormat,
    title: IDWriteTextFormat,
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
            title: paint::text_format(&factory, 13.5 * s, 600, DWRITE_TEXT_ALIGNMENT_LEADING)?,
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
    tab: Tab,
    focus: Hit,
    scroll: i32,
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
                &windows::core::HSTRING::from(tr("Flick Settings", "Flick 설정", "Flick 设置")),
                WS_POPUP | WS_CAPTION | WS_SYSMENU | WS_VSCROLL,
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
                tab: Tab::Mouse,
                focus: Hit::Trigger,
                scroll: 0,
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
        // Opened for the touchpad's sake: on its tab.
        if view.touchpad_status == crate::touchpad::Status::Conflict {
            self.tab = Tab::Touchpad;
        }
        self.set_view(view);
        if self.is_open() {
            return;
        }
        let monitors = vd::monitors();
        let Some(home) = monitors.iter().find(|m| m.primary) else { return };
        self.set_scale(fitting_scale(home.scale, home.work.right - home.work.left));
        self.scroll = 0;
        self.focus = Hit::Trigger;
        self.hover = Hit::Nothing;
        self.press = None;
        let mut frame = RECT { left: 0, top: 0, right: self.size.0, bottom: self.size.1 };
        unsafe {
            let dpi = (home.scale * 96.0).round() as u32;
            let _ = AdjustWindowRectExForDpi(&mut frame, WS_POPUP | WS_CAPTION | WS_SYSMENU | WS_VSCROLL, false, WS_EX_TOOLWINDOW, dpi);
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
        if !self.controls().iter().any(|(_, hit)| *hit == self.focus) { self.focus = Hit::Tab(self.tab); }
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
        let y = y + self.scroll as f32;
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
                if unsafe { GetKeyState(0x11) } < 0 {
                    self.focus = Hit::Tab(if self.tab == Tab::Mouse { Tab::Touchpad } else { Tab::Mouse });
                    self.show(if self.tab == Tab::Mouse { Tab::Touchpad } else { Tab::Mouse })
                } else {
                    self.move_focus(unsafe { GetKeyState(0x10) } < 0);
                    Action::None
                }
            }
            _ if pressed(0x20) || pressed(0x0d) => self.activate(self.focus),
            _ if pressed(0x25) || pressed(0x26) => self.adjust_slider(-1),
            _ if pressed(0x27) || pressed(0x28) => self.adjust_slider(1),
            _ if pressed(0x21) || pressed(0x22) => {
                self.scroll_by(if wparam.0 == 0x21 { -self.size.1 } else { self.size.1 });
                Action::None
            }
            WM_MOUSEWHEEL => {
                self.scroll_by(-((wparam.0 >> 16) as i16 as i32) * 44 / 120);
                Action::None
            }
            WM_VSCROLL => {
                let mut info = SCROLLINFO { cbSize: std::mem::size_of::<SCROLLINFO>() as u32, fMask: SIF_ALL, ..Default::default() };
                let _ = unsafe { GetScrollInfo(self.hwnd, SB_VERT, &mut info) };
                let to = match wparam.0 & 0xffff {
                    0 => self.scroll - 44, 1 => self.scroll + 44,
                    2 => self.scroll - self.size.1, 3 => self.scroll + self.size.1,
                    4 | 5 => info.nTrackPos, 6 => 0, 7 => self.content_height(), _ => self.scroll,
                };
                self.scroll_by(to - self.scroll);
                Action::None
            }
            _ if pressed(b'1') => self.show(Tab::Mouse),
            _ if pressed(b'2') => self.show(Tab::Touchpad),
            _ if pressed(b'R') => self.toggle(Hit::Reverse),
            _ if pressed(b'T') => {
                self.tab = Tab::Touchpad;
                self.toggle(Hit::Touchpad)
            }
            WM_KEYDOWN if wparam.0 == 0x57 && self.view.touchpad_status == crate::touchpad::Status::Conflict =>
                Action::TouchpadSettings,
            WM_DPICHANGED => {
                // Moved to a monitor with another scaling: Windows says where
                // the window should be now.
                unsafe {
                    let to = *(lparam.0 as *const RECT);
                    let monitors = vd::monitors();
                    if let Some(home) = monitors.iter().max_by_key(|m| overlap(&to, &m.bounds)) {
                        self.set_scale(fitting_scale((wparam.0 & 0xffff) as f32 / 96.0, home.work.right - home.work.left));
                        let mut frame = RECT { right: self.size.0, bottom: self.size.1, ..Default::default() };
                        let _ = AdjustWindowRectExForDpi(&mut frame, WS_POPUP | WS_CAPTION | WS_SYSMENU | WS_VSCROLL, false, WS_EX_TOOLWINDOW, (wparam.0 & 0xffff) as u32);
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

    fn show(&mut self, tab: Tab) -> Action {
        self.tab = tab;
        if !self.controls().iter().any(|(_, hit)| *hit == self.focus) { self.focus = Hit::Tab(tab); }
        self.reveal_focus();
        self.invalidate();
        Action::None
    }

    /// Turns the switch `hit` stands for on the tab shown the other way.
    fn toggle(&self, hit: Hit) -> Action {
        let hand = self.view.hand(self.tab);
        let on = match hit {
            Hit::Follow(way) => hand.follows[way],
            Hit::Reverse => hand.reverse,
            Hit::Touchpad => self.view.touchpad.three_finger,
            Hit::Minimap => self.view.minimap,
            Hit::Blur => self.view.motion_blur,
            _ => return Action::None,
        };
        match key(self.tab, hit) {
            Some(key) => Action::Set(key, (!on).to_string()),
            None => Action::None,
        }
    }

    /// The value of the slider `hit` stands for on the tab shown, and what
    /// it spans.
    fn slider(&self, hit: Hit) -> Option<(i32, (i32, i32, i32))> {
        let hand = self.view.hand(self.tab);
        let distances = if self.tab == Tab::Mouse { COUNTS } else { TENTHS_MM };
        match hit {
            Hit::Step(way) => Some((hand.steps[way], distances)),
            _ => None,
        }
    }

    /// Moves the control that is being dragged along with the cursor.
    fn drag(&mut self, x: f32) -> Action {
        let Some(Press { hit, .. }) = self.press else { return Action::None };
        let Some((_, span)) = self.slider(hit) else { return Action::None };
        let layout = Layout::new(self.scale);
        let along = |track: &Rect| {
            let (from, to) = slider_ends(track, self.scale);
            let at = ((x - from) / (to - from)).clamp(0.0, 1.0);
            let (low, high, step) = span;
            ((low as f32 + at * (high - low) as f32) / step as f32).round() as i32 * step
        };
        let mut hand = self.view.hand(self.tab);
        match hit {
            Hit::Step(way) => hand.steps[way] = along(&layout.steps[way]),
            _ => return Action::None,
        }
        self.view.set_hand(self.tab, hand);
        self.invalidate();
        Action::Try { mouse: self.view.mouse, touchpad: self.view.touchpad }
    }

    fn mouse_down(&mut self, x: f32, y: f32) -> Action {
        let hit = Layout::new(self.scale).hit(self.tab, self.conflict(), x, y);
        let before = self.slider(hit).map_or(0.0, |(value, _)| value as f32);
        self.focus = hit;
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
        let hover = Layout::new(self.scale).hit(self.tab, self.conflict(), x, y);
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
        if let Some((now, _)) = self.slider(hit) {
            // A drag ended: what it arrived at is kept.
            let Some(key) = key(self.tab, hit) else { return Action::None };
            let value = slider_value(self.tab, hit, now);
            return if now as f32 == before { Action::None } else { Action::Set(key, value) };
        }
        if Layout::new(self.scale).hit(self.tab, self.conflict(), x, y) != hit {
            return Action::None;
        }
        self.activate(hit)
    }

    fn activate(&mut self, hit: Hit) -> Action {
        match hit {
            Hit::Trigger => Action::Capture(!self.view.capturing),
            Hit::Autostart => Action::Autostart(!self.view.autostart),
            Hit::Admin => Action::Admin(!self.view.admin),
            Hit::Tab(tab) => self.show(tab),
            Hit::Follow(_) | Hit::Reverse | Hit::Touchpad | Hit::Minimap | Hit::Blur => self.toggle(hit),
            Hit::Speed(i) => Action::Set("experimental.commit_speed", format!("\"{}\"", SPEEDS[i].name())),
            Hit::Quick(i) => Action::Set("experimental.quick_flick", format!("\"{}\"", QUICKS[i].name())),
            Hit::TouchpadSettings if self.view.touchpad_status == crate::touchpad::Status::Conflict => Action::TouchpadSettings,
            Hit::File => Action::OpenFile,
            _ => Action::None,
        }
    }

    fn conflict(&self) -> bool { self.view.touchpad_status == crate::touchpad::Status::Conflict }
    fn controls(&self) -> Vec<(Rect, Hit)> { Layout::new(self.scale).controls(self.tab, self.conflict()) }
    fn content_height(&self) -> i32 { (SIZE.1 * self.scale).round() as i32 }
    fn scroll_by(&mut self, by: i32) {
        let next = (self.scroll + by).clamp(0, (self.content_height() - self.size.1).max(0));
        if self.scroll != next { self.scroll = next; self.invalidate(); }
        let info = SCROLLINFO { cbSize: std::mem::size_of::<SCROLLINFO>() as u32, fMask: SIF_ALL,
            nMin: 0, nMax: self.content_height() - 1, nPage: self.size.1.max(0) as u32, nPos: self.scroll, ..Default::default() };
        unsafe { SetScrollInfo(self.hwnd, SB_VERT, &info, true); }
    }
    fn reveal_focus(&mut self) {
        if let Some((area, _)) = self.controls().into_iter().find(|(_, hit)| *hit == self.focus) {
            let next = reveal(self.scroll, self.size.1, area.top as i32 - 6, area.bottom as i32 + 6);
            self.scroll_by(next - self.scroll);
        }
    }
    fn move_focus(&mut self, backwards: bool) {
        let controls = self.controls();
        self.focus = next_focus(&controls, self.focus, backwards);
        self.reveal_focus();
        self.invalidate();
    }
    fn adjust_slider(&mut self, direction: i32) -> Action {
        let Some((value, span)) = self.slider(self.focus) else { return Action::None };
        let Some(key) = key(self.tab, self.focus) else { return Action::None };
        let next = slider_step(value, span, direction);
        Action::Set(key, slider_value(self.tab, self.focus, next))
    }

    // ---- drawing ----------------------------------------------------------

    fn draw_to_canvas(&mut self) -> Option<Result<()>> {
        if !self.canvas.as_ref().is_some_and(|c| c.dib.size == (self.size.0, self.content_height().max(self.size.1))) {
            self.canvas = Canvas::new(&self.factory, (self.size.0, self.content_height().max(self.size.1)))
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
        self.scroll_by(0);
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
                let _ = BitBlt(window, 0, 0, w, h, canvas.dib.dc, 0, self.scroll, SRCCOPY);
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
        let tab = self.tab;
        let hand = view.hand(tab);

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

        label(2.0, tr("Minimap while moving", "이동 중 미니맵 표시", "移动时显示小地图"));
        self.draw_switch(&p, &layout.minimap, view.minimap, Hit::Minimap);

        label(3.0, tr("Run as administrator (off takes effect after restart)", "관리자 권한 실행 (끄기는 재시작 후 적용)", "以管理员身份运行（关闭后重启生效）"));
        self.draw_switch(&p, &layout.admin, view.admin, Hit::Admin);

        // The tabs, the one shown lit and joined to its area below.
        p.fill(&layout.panel, 10.0 * s, white(0.045));
        for (area, this, text) in [(&layout.tabs[0], Tab::Mouse, tr("Mouse (1)", "마우스 (1)", "鼠标 (1)")), (&layout.tabs[1], Tab::Touchpad, tr("Touchpad (2)", "터치패드 (2)", "触摸板 (2)"))] {
            let on = tab == this;
            let hot = self.hover == Hit::Tab(this);
            if on {
                p.fill(area, 7.0 * s, accent(0.95));
            } else if hot {
                p.fill(area, 7.0 * s, white(0.10));
            }
            p.text(text, &fonts.button, area, white(if on || hot { 1.0 } else { 0.55 }));
        }

        let inner = |i: f32| Rect { left: (EDGES.0 + INSET) * s, right: (EDGES.1 - INSET) * s, ..row(i) };
        let distances = if tab == Tab::Mouse { COUNTS } else { TENTHS_MM };
        for way in WAYS {
            let column = layout.columns[way];
            let line = |i: f32| Rect { top: row(i).top, bottom: row(i).bottom, ..column };
            let title = [tr("Sideways · along a shelf", "좌우 · 같은 셸프(행)", "左右 · 同一架子（行）"), tr("Up and down · to another shelf", "위아래 · 다른 셸프(행)", "上下 · 切换架子（行）")][way];
            p.text(title, &fonts.title, &line(TAB_ROWS.0), white(0.92));
            p.text(tr("Distance", "거리", "距离"), &fonts.label, &line(TAB_ROWS.0 + 1.0), white(0.62));
            let words = distance_words(tab, hand.steps[way]);
            self.draw_slider(&p, fonts, &layout.steps[way], column.right, hand.steps[way], distances, &words, Hit::Step(way));
            p.text(tr("Movement needed to switch", "다음 칸으로 넘어가는 데 필요한 이동 거리", "切换到下一格所需的移动距离"), &fonts.notice, &line(TAB_ROWS.0 + 2.0), white(0.62));
            p.text(tr("Follows the hand", "손 따라가기", "跟随手"), &fonts.label, &line(TAB_ROWS.0 + 3.0), white(0.62));
            self.draw_switch(&p, &layout.follows[way], hand.follows[way], Hit::Follow(way));
        }

        p.text(tr("Reverse direction (R)", "방향 반전 (R)", "反向 (R)"), &fonts.label, &inner(TAB_ROWS.0 + 4.0), white(0.62));
        self.draw_switch(&p, &layout.reverse, hand.reverse, Hit::Reverse);

        let notice = match tab {
            Tab::Mouse => {
                tr("Hold the trigger and move between shelves (rows). Distance is raw mouse counts, not pixels; travel depends on mouse DPI. Follows the hand also animates keyboard shortcuts.",
                    "트리거를 누르고 셸프(행) 사이로 움직이세요. 거리는 픽셀이 아닌 원시 마우스 카운트이며 이동량은 마우스 DPI에 따라 달라집니다. 손 따라가기는 키보드 전환 애니메이션에도 적용됩니다.",
                    "按住触发键在架子（行）间移动。距离是原始鼠标计数，并非像素；实际距离取决于鼠标 DPI。跟随手也控制键盘切换动画。")
            }
            Tab::Touchpad => {
                p.text(tr("Three-finger touchpad (T)", "세 손가락 터치패드 (T)", "三指触摸板 (T)"), &fonts.label, &inner(TAB_ROWS.0 + 5.0), white(0.62));
                self.draw_switch(&p, &layout.touchpad, view.touchpad.three_finger, Hit::Touchpad);
                match view.touchpad_status {
                    crate::touchpad::Status::Absent => tr("No precision touchpad detected", "정밀 터치패드를 찾지 못했습니다", "未检测到精密触摸板"),
                    crate::touchpad::Status::Off => tr("Off. Enable to use three fingers as the trigger.", "끄기 상태입니다. 켜면 세 손가락이 트리거가 됩니다.", "已关闭。开启后三指可作为触发键。"),
                    crate::touchpad::Status::Active => tr("Ready. Tap for the board; move the distances above to go from desktop to desktop.", "준비됐습니다. 탭하면 전체 보기, 움직이면 데스크톱 전환입니다.", "已就绪。轻点打开总览，移动可切换桌面。"),
                    crate::touchpad::Status::Conflict => tr(crate::touchpad::NOTICE, "Windows의 세 손가락 살짝 밀기와 탭을 모두 ‘아무것도 안 함’으로 설정하세요.", "请将 Windows 的三指轻扫和点击均设为‘无’。"),
                    crate::touchpad::Status::Unavailable => tr("Touchpad reports could not be read on this device.", "이 장치의 터치패드 입력을 읽지 못했습니다.", "无法读取此设备的触摸板输入。"),
                }
            }
        };
        // A row of its own under the switches, the button in the next.
        let area = inner(TAB_ROWS.0 + if tab == Tab::Mouse { 5.0 } else { 6.0 });
        let area = Rect { bottom: area.top + if tab == Tab::Mouse { 3.0 } else { 1.0 } * ROWS.1 * s, ..area };
        p.text(notice, &fonts.notice, &area, white(0.62));
        if tab == Tab::Touchpad && view.touchpad_status == crate::touchpad::Status::Conflict {
            self.draw_button(&p, fonts, &layout.touchpad_settings, tr("Open Windows touchpad settings (W)", "Windows 터치패드 설정 열기 (W)", "打开 Windows 触摸板设置 (W)"), Hit::TouchpadSettings);
        }

        // Ways to make motion look smoother; both start off.
        p.text(tr("Experimental · smoother-looking motion", "실험 · 더 부드럽게 보이는 움직임", "实验 · 看起来更顺滑的运动"), &fonts.title, &row(EXPERIMENT_ROW), white(0.92));
        label(EXPERIMENT_ROW + 1.0, tr("Motion blur trails on fast moves", "빠른 이동에 모션 블러 잔상", "快速移动时的动态模糊残影"));
        self.draw_switch(&p, &layout.blur, view.motion_blur, Hit::Blur);
        label(EXPERIMENT_ROW + 2.0, tr("Commit spring", "커밋 스프링", "切换弹簧"));
        let names = [tr("Default", "기본", "默认"), tr("Gentle", "부드럽게", "柔和"), tr("Slow", "느리게", "缓慢")];
        for (i, area) in layout.speeds.iter().enumerate() {
            if view.commit_speed == SPEEDS[i] { p.fill(area, 7.0 * s, accent(0.95)); }
            else { self.draw_button(&p, fonts, area, names[i], Hit::Speed(i)); continue; }
            p.text(names[i], &fonts.button, area, white(1.0));
        }
        label(EXPERIMENT_ROW + 3.0, tr("Quick flick", "빠른 넘김", "快速翻页"));
        let names = [tr("Off", "끔", "关闭"), tr("Slide", "슬라이드", "滑动"), tr("Instant", "즉시", "即时")];
        for (i, area) in layout.quicks.iter().enumerate() {
            if view.quick_flick == QUICKS[i] { p.fill(area, 7.0 * s, accent(0.95)); }
            else { self.draw_button(&p, fonts, area, names[i], Hit::Quick(i)); continue; }
            p.text(names[i], &fonts.button, area, white(1.0));
        }
        p.text(tr("Applied from the next move. Trails show only on fast moves and use the public window pictures; slower springs take longer to settle. A quick, sure move goes straight to the next desktop without zooming out; slow down or pause and the zoomed-out view comes in.",
            "다음 이동부터 적용됩니다. 잔상은 빠른 이동에만 생기고 공개 창 썸네일을 씁니다. 느린 스프링은 정착이 더 오래 걸립니다. 빠르고 확실하게 움직이면 축소 없이 바로 옆 데스크톱으로 넘어가고, 느려지거나 멈추면 축소 화면이 나타납니다.",
            "从下一次移动开始生效。残影只在快速移动时出现，并使用公开的窗口缩略图；较慢的弹簧需要更久才停稳。快速而果断地移动会直接切到相邻桌面而不缩小；放慢或停顿时会出现缩小视图。"),
            &fonts.notice, &Rect { bottom: row(EXPERIMENT_ROW + 4.0).top + 2.0 * ROWS.1 * s, ..row(EXPERIMENT_ROW + 4.0) }, white(0.62));

        let hot = self.hover == Hit::File;
        p.text(tr("More settings: config.toml", "추가 설정: config.toml", "更多设置：config.toml"), &fonts.label, &layout.file, white(if hot { 0.85 } else { 0.38 }));
        if let Some((area, _)) = self.controls().into_iter().find(|(_, hit)| *hit == self.focus) {
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
    pub fn render_to_pixels(&mut self, view: View, tab: Tab, scale: f32) -> Option<((i32, i32), Vec<u8>)> {
        self.view = view;
        self.tab = tab;
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
fn reveal(scroll: i32, height: i32, top: i32, bottom: i32) -> i32 {
    if top < scroll { top.max(0) } else if bottom > scroll + height { (bottom - height).max(0) } else { scroll }
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
fn slider_value(tab: Tab, hit: Hit, value: i32) -> String {
    match (tab, hit) { (Tab::Touchpad, Hit::Step(_)) => format!("{}.{}", value / 10, value % 10), _ => value.to_string() }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn focus_visits_all_visible_controls_and_wraps_both_ways() {
        for tab in [Tab::Mouse, Tab::Touchpad] {
            for conflict in [false, true] {
                let controls = Layout::new(1.0).controls(tab, conflict);
                let mut focus = controls[0].1;
                for (_, expected) in controls.iter().skip(1).chain(controls.iter().take(1)) {
                    focus = next_focus(&controls, focus, false);
                    assert_eq!(focus, *expected);
                }
                assert_eq!(next_focus(&controls, focus, true), controls.last().unwrap().1);
                assert_eq!(controls.iter().any(|(_, h)| *h == Hit::TouchpadSettings), tab == Tab::Touchpad && conflict);
            }
        }
    }
    #[test]
    fn keyboard_sliders_clamp_and_keep_touchpad_units() {
        assert_eq!(slider_step(600, COUNTS, 1), 600);
        assert_eq!(slider_step(20, COUNTS, -1), 20);
        assert_eq!(slider_step(260, COUNTS, 1), 270);
        let saved = slider_value(Tab::Touchpad, Hit::Step(0), 105);
        assert_eq!(saved, "10.5");
        assert!((saved.parse::<f64>().unwrap() - 10.5).abs() < 1e-9);
    }
    #[test]
    fn high_dpi_content_stays_accessible_in_a_short_work_area() {
        assert!((fitting_scale(2.0, 1920) - 2.0).abs() < 1e-6);
        assert!(SIZE.0 * fitting_scale(2.0, 1000) <= 960.01);
        let work = RECT { left: -1920, top: 0, right: 0, bottom: 1040 };
        let fitted = fit_frame(-1600, -100, 1312, 1400, &work);
        assert!(fitted.left >= work.left && fitted.right <= work.right);
        assert_eq!((fitted.top, fitted.bottom), (0, 1040));
        assert!(overlap(&fitted, &work) > overlap(&fitted, &RECT { left: 0, top: 0, right: 1920, bottom: 1080 }));
        let bottom = (SIZE.1 * 2.0) as i32;
        let scroll = reveal(0, 1000, bottom - 60, bottom);
        assert_eq!(scroll + 1000, bottom);
        assert_eq!(reveal(scroll, 1000, 10, 50), 10);
    }
}
