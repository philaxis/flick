//! The settings window: the trigger and whether to start with Windows, then
//! a tab for each hand, the mouse and the touchpad, with how it goes
//! sideways along a row and up and down from row to row.
//!
//! It owns layout, hit-testing and drawing, in the look of the board. It
//! changes nothing itself; what the user does is turned into an `Action` for
//! the app, which answers with a new `View`.

use crate::{
    config::{Mouse, Touchpad},
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
                DWRITE_TEXT_ALIGNMENT_TRAILING, DWRITE_WORD_WRAPPING_WRAP,
            },
            Dwm::{DwmSetWindowAttribute, DWMWA_CAPTION_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE},
            Gdi::{BitBlt, GetDC, InvalidateRect, ReleaseDC, ValidateRect, SRCCOPY},
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            HiDpi::AdjustWindowRectExForDpi,
            Input::KeyboardAndMouse::{
                ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VK_ESCAPE, VK_TAB,
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
const SIZE: (f32, f32) = (656.0, ROWS.0 + FILE_ROW * ROWS.1 + 52.0);
/// Top of the first row and the height of each.
const ROWS: (f32, f32) = (14.0, 44.0);
/// Left edge of the labels and right edge of the controls.
const EDGES: (f32, f32) = (28.0, 628.0);
/// The tab's rows, the first with the tabs themselves above it, and the
/// link to the file below them.
const TAB_ROWS: (f32, f32) = (3.0, 11.0);
const FILE_ROW: f32 = 12.0;
/// How far the tab's controls stand in from its edges, and the room
/// between its two ways.
const INSET: f32 = 16.0;
const GAP: f32 = 24.0;
/// The width of a way's labels and of the value after its sliders.
const LABEL: f32 = 76.0;
const VALUE: f32 = 64.0;
/// What the sliders span, and the step they move in: the mouse's distances
/// in pixels, the touchpad's in tenths of a mm, notches in percent.
const PIXELS: (i32, i32, i32) = (80, 600, 10);
const TENTHS_MM: (i32, i32, i32) = (40, 300, 5);
const NOTCH: (i32, i32, i32) = (0, crate::slide::FIRMEST as i32, 10);

/// What the window shows.
#[derive(Clone, Default)]
pub struct View {
    /// The trigger in words.
    pub trigger: String,
    /// Waiting for the user to press what the trigger should become.
    pub capturing: bool,
    pub autostart: bool,
    /// See `Config::mouse`, `touchpad`.
    pub mouse: Mouse,
    pub touchpad: Touchpad,
    pub touchpad_status: crate::touchpad::Status,
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

#[derive(Clone, Copy, PartialEq, Default)]
enum Hit {
    #[default]
    Nothing,
    Trigger,
    Autostart,
    Tab(Tab),
    /// A way's distance, notches and following the hand.
    Step(usize),
    Notch(usize),
    Follow(usize),
    Reverse,
    Touchpad,
    TouchpadSettings,
    File,
}

/// Where the controls are, in pixels.
struct Layout {
    trigger: Rect,
    autostart: Rect,
    tabs: [Rect; 2],
    /// The tab's area under the tabs.
    panel: Rect,
    /// Each way's column, its sliders' tracks and its switch.
    columns: [Rect; 2],
    steps: [Rect; 2],
    notches: [Rect; 2],
    follows: [Rect; 2],
    reverse: Rect,
    touchpad: Rect,
    touchpad_settings: Rect,
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
            tabs: [tab(0.0), tab(1.0)],
            panel: rect(EDGES.0 * s, top * s, (EDGES.1 - EDGES.0) * s, (bottom - top) * s),
            columns: WAYS.map(|way| {
                let (left, right) = column(way);
                rect(left * s, row(TAB_ROWS.0) * s, (right - left) * s, ROWS.1 * s)
            }),
            steps: WAYS.map(|way| slider(way, TAB_ROWS.0 + 1.0)),
            notches: WAYS.map(|way| slider(way, TAB_ROWS.0 + 2.0)),
            follows: WAYS.map(|way| at(column(way).1, TAB_ROWS.0 + 3.0, 44.0, 24.0)),
            reverse: at(inner.1, TAB_ROWS.0 + 4.0, 44.0, 24.0),
            touchpad: at(inner.1, TAB_ROWS.0 + 5.0, 44.0, 24.0),
            touchpad_settings: rect(inner.0 * s, (row(TAB_ROWS.0 + 7.0) + 6.0) * s, 260.0 * s, 30.0 * s),
            file: rect(EDGES.0 * s, (row(FILE_ROW) + 8.0) * s, 190.0 * s, 26.0 * s),
        }
    }

    /// What is at (`x`, `y`) with `tab` shown.
    fn hit(&self, tab: Tab, x: f32, y: f32) -> Hit {
        let inside = |r: &Rect| x >= r.left && x < r.right && y >= r.top && y < r.bottom;
        let touchpad = tab == Tab::Touchpad;
        let none = Rect::default();
        [
            (self.trigger, Hit::Trigger),
            (self.autostart, Hit::Autostart),
            (self.tabs[0], Hit::Tab(Tab::Mouse)),
            (self.tabs[1], Hit::Tab(Tab::Touchpad)),
            (self.steps[0], Hit::Step(0)),
            (self.steps[1], Hit::Step(1)),
            (self.notches[0], Hit::Notch(0)),
            (self.notches[1], Hit::Notch(1)),
            (self.follows[0], Hit::Follow(0)),
            (self.follows[1], Hit::Follow(1)),
            (self.reverse, Hit::Reverse),
            (if touchpad { self.touchpad } else { none }, Hit::Touchpad),
            (if touchpad { self.touchpad_settings } else { none }, Hit::TouchpadSettings),
            (self.file, Hit::File),
        ]
        .into_iter()
        .find(|(area, _)| inside(area))
        .map_or(Hit::Nothing, |(_, hit)| hit)
    }
}

/// One hand's settings as the tab shows them, sliders in their own units
/// (`PIXELS`, `TENTHS_MM`, `NOTCH`).
#[derive(Clone, Copy)]
struct Hand {
    steps: [i32; 2],
    notches: [i32; 2],
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
                    notches: [m.notch_x as i32, m.notch_y as i32],
                    follows: [m.follow_x, m.follow_y],
                    reverse: m.reverse,
                }
            }
            Tab::Touchpad => {
                let t = &self.touchpad;
                let tenths = |mm: f64| (mm * 10.0).round() as i32;
                Hand {
                    steps: [tenths(t.step_x), tenths(t.step_y)],
                    notches: [t.notch_x as i32, t.notch_y as i32],
                    follows: [t.follow_x, t.follow_y],
                    reverse: t.reverse,
                }
            }
        }
    }

    fn set_hand(&mut self, tab: Tab, hand: Hand) {
        let notch = |n: i32| n.max(0) as u32;
        match tab {
            Tab::Mouse => {
                let m = &mut self.mouse;
                [m.step_x, m.step_y] = hand.steps;
                [m.notch_x, m.notch_y] = hand.notches.map(notch);
                [m.follow_x, m.follow_y] = hand.follows;
                m.reverse = hand.reverse;
            }
            Tab::Touchpad => {
                let t = &mut self.touchpad;
                [t.step_x, t.step_y] = hand.steps.map(|tenths| tenths as f64 / 10.0);
                [t.notch_x, t.notch_y] = hand.notches.map(notch);
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
        Hit::Notch(0) => if mouse { "mouse.notch_x" } else { "touchpad.notch_x" },
        Hit::Notch(_) => if mouse { "mouse.notch_y" } else { "touchpad.notch_y" },
        Hit::Follow(0) => if mouse { "mouse.follow_x" } else { "touchpad.follow_x" },
        Hit::Follow(_) => if mouse { "mouse.follow_y" } else { "touchpad.follow_y" },
        Hit::Reverse => if mouse { "mouse.reverse" } else { "touchpad.reverse" },
        Hit::Touchpad => "touchpad.three_finger",
        _ => return None,
    })
}

/// A distance on `tab` in words, from the slider's units.
fn distance_words(tab: Tab, step: i32) -> String {
    match tab {
        Tab::Mouse => format!("{step} px"),
        Tab::Touchpad => format!("{}.{} mm", step / 10, step % 10),
    }
}

fn notch_words(notch: i32) -> String {
    if notch == 0 { "Off".into() } else { format!("{notch}%") }
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
                tab: Tab::Mouse,
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
            _ if pressed(VK_TAB.0 as u8) => self.show(if self.tab == Tab::Mouse { Tab::Touchpad } else { Tab::Mouse }),
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

    fn show(&mut self, tab: Tab) -> Action {
        self.tab = tab;
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
        let distances = if self.tab == Tab::Mouse { PIXELS } else { TENTHS_MM };
        match hit {
            Hit::Step(way) => Some((hand.steps[way], distances)),
            Hit::Notch(way) => Some((hand.notches[way], NOTCH)),
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
            Hit::Notch(way) => hand.notches[way] = along(&layout.notches[way]),
            _ => return Action::None,
        }
        self.view.set_hand(self.tab, hand);
        self.invalidate();
        Action::Try { mouse: self.view.mouse, touchpad: self.view.touchpad }
    }

    fn mouse_down(&mut self, x: f32, y: f32) -> Action {
        let hit = Layout::new(self.scale).hit(self.tab, x, y);
        let before = self.slider(hit).map_or(0.0, |(value, _)| value as f32);
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
        let hover = Layout::new(self.scale).hit(self.tab, x, y);
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
            let value = match (self.tab, hit) {
                (Tab::Touchpad, Hit::Step(_)) => format!("{}.{}", now / 10, now % 10),
                _ => now.to_string(),
            };
            return if now as f32 == before { Action::None } else { Action::Set(key, value) };
        }
        if Layout::new(self.scale).hit(self.tab, x, y) != hit {
            return Action::None;
        }
        match hit {
            Hit::Trigger => Action::Capture(!self.view.capturing),
            Hit::Autostart => Action::Autostart(!self.view.autostart),
            Hit::Tab(tab) => self.show(tab),
            Hit::Follow(_) | Hit::Reverse | Hit::Touchpad => self.toggle(hit),
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
        let tab = self.tab;
        let hand = view.hand(tab);

        let row = |i: f32| rect(EDGES.0 * s, (ROWS.0 + i * ROWS.1) * s, (EDGES.1 - EDGES.0) * s, ROWS.1 * s);
        let label = |i: f32, text: &str| p.text(text, &fonts.label, &row(i), white(0.62));

        label(0.0, "Trigger");
        let (words, colour) = match view.capturing {
            true => ("Press the button or key to use", accent(1.0)),
            false => (view.trigger.as_str(), white(0.92)),
        };
        p.text(words, &fonts.value, &Rect { right: layout.trigger.left - 12.0 * s, ..row(0.0) }, colour);
        self.draw_button(&p, fonts, &layout.trigger, if view.capturing { "Cancel" } else { "Change" }, Hit::Trigger);

        label(1.0, "Start with Windows");
        self.draw_switch(&p, &layout.autostart, view.autostart, Hit::Autostart);

        // The tabs, the one shown lit and joined to its area below.
        p.fill(&layout.panel, 10.0 * s, white(0.045));
        for (area, this, text) in [(&layout.tabs[0], Tab::Mouse, "Mouse (1)"), (&layout.tabs[1], Tab::Touchpad, "Touchpad (2)")] {
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
        let distances = if tab == Tab::Mouse { PIXELS } else { TENTHS_MM };
        for way in WAYS {
            let column = layout.columns[way];
            let line = |i: f32| Rect { top: row(i).top, bottom: row(i).bottom, ..column };
            let title = ["Sideways · along a row", "Up and down · to another row"][way];
            p.text(title, &fonts.title, &line(TAB_ROWS.0), white(0.92));
            p.text("Distance", &fonts.label, &line(TAB_ROWS.0 + 1.0), white(0.62));
            let words = distance_words(tab, hand.steps[way]);
            self.draw_slider(&p, fonts, &layout.steps[way], column.right, hand.steps[way], distances, &words, Hit::Step(way));
            p.text("Notch", &fonts.label, &line(TAB_ROWS.0 + 2.0), white(0.62));
            let words = notch_words(hand.notches[way]);
            self.draw_slider(&p, fonts, &layout.notches[way], column.right, hand.notches[way], NOTCH, &words, Hit::Notch(way));
            p.text("Follows the hand", &fonts.label, &line(TAB_ROWS.0 + 3.0), white(0.62));
            self.draw_switch(&p, &layout.follows[way], hand.follows[way], Hit::Follow(way));
        }

        p.text("Reverse direction (R)", &fonts.label, &inner(TAB_ROWS.0 + 4.0), white(0.62));
        self.draw_switch(&p, &layout.reverse, hand.reverse, Hit::Reverse);

        let notice = match tab {
            Tab::Mouse => {
                "Hold the trigger and move the mouse: sideways along a row of desktops, up and down from row to row. \
                 Notches hold each desktop until the whole distance is moved."
            }
            Tab::Touchpad => {
                p.text("Three-finger touchpad (T)", &fonts.label, &inner(TAB_ROWS.0 + 5.0), white(0.62));
                self.draw_switch(&p, &layout.touchpad, view.touchpad.three_finger, Hit::Touchpad);
                match view.touchpad_status {
                    crate::touchpad::Status::Absent => "No precision touchpad detected",
                    crate::touchpad::Status::Off => "Off. Enable to use three fingers as the trigger.",
                    crate::touchpad::Status::Active => "Ready. Tap for the board; move the distances above to go from desktop to desktop.",
                    crate::touchpad::Status::Conflict => crate::touchpad::NOTICE,
                    crate::touchpad::Status::Unavailable => "Touchpad reports could not be read on this device.",
                }
            }
        };
        // A row of its own under the switches, the button in the next.
        let area = inner(TAB_ROWS.0 + if tab == Tab::Mouse { 5.0 } else { 6.0 });
        p.text(notice, &fonts.notice, &area, white(0.62));
        if tab == Tab::Touchpad && view.touchpad_status == crate::touchpad::Status::Conflict {
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
