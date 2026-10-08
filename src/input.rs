//! Global hooks that turn "hold the trigger and move the mouse" into grid steps.
//!
//! The hooks live on their own thread. Windows delivers every mouse and
//! keyboard event through that thread, so it must never wait on anything: it
//! only does arithmetic and posts messages to the app window, and the app can
//! take as long as it likes without the cursor stuttering.
//!
//! While the trigger is held the cursor does not move: the hook swallows
//! every mouse move, and the movement is read from raw input instead, which
//! reports what the device did regardless of where the cursor is.

use crate::{
    config::Config,
    grid::Dir,
    hold::{Chord, Held, Source},
};
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Mutex,
    },
    time::Instant,
};
use windows::Win32::{
    Foundation::{CloseHandle, HANDLE, HWND, LPARAM, LRESULT, POINT, WPARAM},
    Security::{GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TokenIntegrityLevel, TOKEN_MANDATORY_LABEL, TOKEN_QUERY},
    System::{
        LibraryLoader::GetModuleHandleW,
        StationsAndDesktops::{CloseDesktop, OpenInputDesktop, DESKTOP_CONTROL_FLAGS, DESKTOP_READOBJECTS},
        Threading::{GetCurrentProcess, GetCurrentThreadId, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION},
    },
    UI::{
        Input::{
            GetRawInputData,
            KeyboardAndMouse::{
                MapVirtualKeyW, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS,
                KEYEVENTF_KEYUP, MAPVK_VK_TO_VSC, VIRTUAL_KEY,
            },
            RegisterRawInputDevices, HRAWINPUT, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER, RIDEV_INPUTSINK, RIDEV_REMOVE,
            RID_INPUT, RIM_TYPEKEYBOARD, RIM_TYPEMOUSE,
        },
        WindowsAndMessaging::{
            CallNextHookEx, CreateWindowExW, DefWindowProcW, DispatchMessageW, GetCursorPos, GetForegroundWindow,
            GetMessageW, GetSystemMetrics, GetWindowThreadProcessId, KillTimer, PostMessageW, PostThreadMessageW,
            RegisterClassW, SetTimer, SetWindowsHookExW, UnhookWindowsHookEx, WindowFromPoint, HC_ACTION, HHOOK,
            HWND_MESSAGE, KBDLLHOOKSTRUCT, MSG, MSLLHOOKSTRUCT, SM_CXSCREEN, SM_CXVIRTUALSCREEN, SM_CYSCREEN,
            SM_CYVIRTUALSCREEN, WH_KEYBOARD_LL, WH_MOUSE_LL, WINDOW_EX_STYLE, WINDOW_STYLE, WM_APP, WM_INPUT,
            WM_KEYDOWN, WM_KEYUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEMOVE, WM_QUIT, WM_SYSKEYDOWN, WM_SYSKEYUP,
            WM_XBUTTONDOWN, WM_XBUTTONUP, WNDCLASSW,
        },
    },
};

/// wparam: direction index (see `dir_from_index`).
pub const WM_STEP: u32 = WM_APP + 1;
/// The trigger was pressed and released without travelling a step.
pub const WM_CLICK: u32 = WM_APP + 2;
/// The trigger was released after at least one step.
pub const WM_RELEASE: u32 = WM_APP + 3;

/// The user finished choosing a new trigger; wparam is 1 when one was chosen
/// (fetch it with `take_captured`) and 0 when it was cancelled.
pub const WM_CAPTURED: u32 = WM_APP + 4;

/// Thread message telling the hook thread to pick up new settings.
const WM_RELOAD: u32 = WM_APP + 20;
/// Thread message telling the hook thread that the PC woke up: whatever was
/// held when it went to sleep is not any more, and the hooks may be gone.
const WM_WOKE: u32 = WM_APP + 21;
/// Two clicks of the trigger closer together than this are one.
const CLICK_APART_MS: u128 = 300;
/// While something is held, this often it is checked that letting go of it
/// could still be seen (`blind`).
const WATCH_MS: u32 = 100;
/// The hooks are put in afresh this often while nothing is going on: Windows
/// drops a hook that was slow to answer, without a word.
const REHOOK_MS: u32 = 60_000;
/// Marks key events we replay ourselves, so the hook lets them through. (An
/// arbitrary number: "KANK" in ASCII.)
const OWN_INPUT: usize = 0x4B41_4E4B;
/// All keys of a chord must go down within this long to count as one press.
/// Two keys next to each other are also typed in quick succession ("ty" in
/// "type"), so a two-key chord has to be much more simultaneous than that.
const CHORD_WINDOW_MS: u32 = 100;
const PAIR_WINDOW_MS: u32 = 40;

#[derive(Clone, Debug, PartialEq)]
pub enum Trigger {
    XButton1,
    XButton2,
    Middle,
    Key(u32),
    /// Several keys pressed together, e.g. "e+r+t".
    Chord(Vec<u32>),
}

/// The config-file name of a key (the inverse of `key_code`).
fn key_name(key: u32) -> String {
    match key {
        0x13 => "pause".into(),
        0x14 => "capslock".into(),
        0x20 => "space".into(),
        0x5D => "apps".into(),
        0x91 => "scrolllock".into(),
        0xA3 => "rctrl".into(),
        0xA5 => "ralt".into(),
        0x30..=0x39 | 0x41..=0x5A => (key as u8 as char).to_ascii_lowercase().to_string(),
        0x70..=0x87 => format!("f{}", key - 0x70 + 1),
        _ => format!("vk{key:02x}"),
    }
}

fn key_code(name: &str) -> Option<u32> {
    if let Some(hex) = name.strip_prefix("vk") {
        return u32::from_str_radix(hex, 16).ok().filter(|code| (1..=0xFE).contains(code));
    }
    Some(match name {
        "pause" => 0x13,
        "capslock" => 0x14,
        "space" => 0x20,
        "apps" => 0x5D,
        "scrolllock" => 0x91,
        "rctrl" => 0xA3,
        "ralt" => 0xA5,
        _ => {
            let mut chars = name.chars();
            match (chars.next(), chars.next()) {
                // Letters and digits share their virtual-key code with ASCII.
                (Some(c), None) if c.is_ascii_alphanumeric() => c.to_ascii_uppercase() as u32,
                _ => {
                    let n: u32 = name.strip_prefix('f')?.parse().ok()?;
                    (1..=24).contains(&n).then(|| 0x70 + n - 1)?
                }
            }
        }
    })
}

impl Trigger {
    /// How this trigger is written in the config file.
    pub fn to_config(&self) -> String {
        match self {
            Trigger::XButton1 => "xbutton1".into(),
            Trigger::XButton2 => "xbutton2".into(),
            Trigger::Middle => "middle".into(),
            Trigger::Key(key) => key_name(*key),
            Trigger::Chord(keys) => keys.iter().map(|key| key_name(*key)).collect::<Vec<_>>().join("+"),
        }
    }

    /// How this trigger is called when talking to the user.
    pub fn describe(&self) -> String {
        match self {
            Trigger::XButton1 => "마우스 뒤로 버튼".into(),
            Trigger::XButton2 => "마우스 앞으로 버튼".into(),
            Trigger::Middle => "마우스 휠 버튼".into(),
            _ => self.to_config().to_uppercase(),
        }
    }

    fn parse_one(name: &str) -> Option<Trigger> {
        Some(match name {
            "xbutton1" => Trigger::XButton1,
            "xbutton2" => Trigger::XButton2,
            "middle" => Trigger::Middle,
            _ => {
                let keys: Option<Vec<u32>> = name.split('+').map(|key| key_code(key.trim())).collect();
                match keys?.as_slice() {
                    [key] => Trigger::Key(*key),
                    keys => Trigger::Chord(keys.to_vec()),
                }
            }
        })
    }

    /// Parses a comma-separated list such as "xbutton2, e+r+t". Returns the
    /// triggers understood and the entries that were not.
    pub fn parse_list(text: &str) -> (Vec<Trigger>, Vec<String>) {
        let mut triggers = Vec::new();
        let mut unknown = Vec::new();
        for name in text.split(',').map(|name| name.trim().to_ascii_lowercase()).filter(|name| !name.is_empty()) {
            match Trigger::parse_one(&name) {
                Some(trigger) => triggers.push(trigger),
                None => unknown.push(name),
            }
        }
        (triggers, unknown)
    }
}

pub fn dir_index(dir: Dir) -> usize {
    match dir {
        Dir::Left => 0,
        Dir::Right => 1,
        Dir::Up => 2,
        Dir::Down => 3,
    }
}

pub fn dir_from_index(index: usize) -> Option<Dir> {
    [Dir::Left, Dir::Right, Dir::Up, Dir::Down].get(index).copied()
}

#[derive(Clone)]
struct Settings {
    /// The app window, as a plain number so it can cross threads.
    target: isize,
    triggers: Vec<Trigger>,
    step_x: i32,
    step_y: i32,
    /// Rows change on a quick flick rather than by distance (see `travel`).
    vertical_sticky: bool,
    /// A movement counts as vertical only when it is this many times more
    /// vertical than horizontal. Everything else is sideways.
    vertical_bias: f32,
}

impl Settings {
    /// The keys of the chord among the triggers (only one is used).
    fn chord(&self) -> Vec<u32> {
        self.triggers
            .iter()
            .find_map(|t| match t {
                Trigger::Chord(keys) => Some(keys.clone()),
                _ => None,
            })
            .unwrap_or_default()
    }

    fn is_vertical(&self, dx: i32, dy: i32) -> bool {
        dy.abs() as f32 >= self.vertical_bias * dx.abs() as f32
    }
}

/// While set, the next button or key combination pressed is reported as the
/// new trigger instead of being acted on.
static CAPTURING: AtomicBool = AtomicBool::new(false);
static CAPTURED: Mutex<Option<String>> = Mutex::new(None);

pub fn begin_capture() {
    CAPTURING.store(true, Ordering::SeqCst);
}

pub fn cancel_capture() {
    CAPTURING.store(false, Ordering::SeqCst);
}

pub fn take_captured() -> Option<String> {
    CAPTURED.lock().unwrap().take()
}

fn finish_capture(s: &Settings, result: Option<String>) {
    CAPTURING.store(false, Ordering::SeqCst);
    CAPTURE_KEYS.with(|keys| keys.borrow_mut().clear());
    let chosen = result.is_some();
    *CAPTURED.lock().unwrap() = result;
    post(s, WM_CAPTURED, chosen as usize);
}

/// Keys count as one combination from the first press to the first release.
fn capture_key(s: &Settings, key: u32, up: bool) -> bool {
    const ESCAPE: u32 = 0x1B;
    if !up {
        if key == ESCAPE {
            finish_capture(s, None);
        } else {
            CAPTURE_KEYS.with(|keys| {
                let mut keys = keys.borrow_mut();
                if !keys.contains(&key) {
                    keys.push(key);
                }
            });
        }
        return true;
    }
    let keys = CAPTURE_KEYS.with(|keys| std::mem::take(&mut *keys.borrow_mut()));
    // A single letter or digit as a hold-trigger would make it untypable.
    let typable = |key: u32| (0x30..=0x5A).contains(&key) || key == 0x20;
    match keys.as_slice() {
        [] => {}
        [only] if typable(*only) => {}
        keys => finish_capture(s, Some(keys.iter().map(|key| key_name(*key)).collect::<Vec<_>>().join("+"))),
    }
    true
}

/// Settings handed from the app to the hook thread.
static SHARED: Mutex<Option<Settings>> = Mutex::new(None);
static THREAD: AtomicU32 = AtomicU32::new(0);

// State of the hook thread.
thread_local! {
    static SETTINGS: RefCell<Option<Settings>> = const { RefCell::new(None) };
    /// Which triggers are down right now.
    static HELD: RefCell<Held> = RefCell::new(Held::default());
    static STEPPED: Cell<bool> = const { Cell::new(false) };
    /// Mouse travel not yet converted into steps.
    static ACCUM: Cell<(i32, i32)> = const { Cell::new((0, 0)) };
    /// Direction (-1 up, 1 down) of the flick the hand is still carrying
    /// through, 0 when a new flick may be taken.
    static FLICKED: Cell<i32> = const { Cell::new(0) };
    static LAST_MOVE: Cell<Option<Instant>> = const { Cell::new(None) };
    /// The last moments of travel: (when, dx, dy).
    static RECENT: RefCell<VecDeque<(Instant, i32, i32)>> = const { RefCell::new(VecDeque::new()) };
    /// The hook thread's hidden window, which receives raw mouse input.
    static RAW_WINDOW: Cell<isize> = const { Cell::new(0) };
    /// Last position reported by an absolute pointer (remote desktop, pen).
    static LAST_ABSOLUTE: Cell<Option<(i32, i32)>> = const { Cell::new(None) };
    /// The chord among the triggers (one without keys when there is none).
    static CHORD: RefCell<Chord> = RefCell::new(Chord::default());
    /// The timer that gives up waiting for the rest of the chord, 0 for none.
    static CHORD_TIMER: Cell<usize> = const { Cell::new(0) };
    /// Keys pressed so far while a new trigger is being chosen.
    static CAPTURE_KEYS: RefCell<Vec<u32>> = const { RefCell::new(Vec::new()) };
    /// The mouse and the keyboard hook.
    static HOOKS: RefCell<Vec<HHOOK>> = const { RefCell::new(Vec::new()) };
    /// The timer that watches over a hold (`watch`), 0 for none.
    static WATCH_TIMER: Cell<usize> = const { Cell::new(0) };
    /// How far the mouse got from where it was when the hold began: where it
    /// is now, and the farthest it was.
    static STRAYED: Cell<((i32, i32), i32)> = const { Cell::new(((0, 0), 0)) };
    /// When the trigger was last clicked.
    static LAST_CLICK: Cell<Option<Instant>> = const { Cell::new(None) };
    /// When the first key of the chord was held back.
    static CHORD_BEGAN: Cell<Option<Instant>> = const { Cell::new(None) };
    /// This process' integrity level, and those found of other processes.
    static LEVELS: RefCell<(Option<u32>, Vec<(u32, Option<u32>)>)> = const { RefCell::new((None, Vec::new())) };
}

/// Starts the hook thread (once) and gives it these settings.
pub fn install(target: HWND, triggers: Vec<Trigger>, config: &Config) {
    *SHARED.lock().unwrap() = Some(Settings {
        target: target.0,
        triggers,
        step_x: config.step_x.max(20),
        step_y: config.step_y.max(20),
        vertical_sticky: config.vertical_sticky,
        vertical_bias: config.vertical_bias(),
    });
    match THREAD.load(Ordering::SeqCst) {
        0 => {
            std::thread::spawn(hook_thread);
        }
        thread => unsafe {
            let _ = PostThreadMessageW(thread, WM_RELOAD, WPARAM(0), LPARAM(0));
        },
    }
}

/// The PC woke from sleep.
pub fn woke() {
    let thread = THREAD.load(Ordering::SeqCst);
    if thread != 0 {
        unsafe {
            let _ = PostThreadMessageW(thread, WM_WOKE, WPARAM(0), LPARAM(0));
        }
    }
}

pub fn uninstall() {
    let thread = THREAD.swap(0, Ordering::SeqCst);
    if thread != 0 {
        unsafe {
            let _ = PostThreadMessageW(thread, WM_QUIT, WPARAM(0), LPARAM(0));
        }
    }
}

fn reload() {
    let settings = SHARED.lock().unwrap().clone();
    // Keys held back for the old chord are typing now.
    let keys = settings.as_ref().map(Settings::chord).unwrap_or_default();
    let held_back = CHORD.with(|chord| {
        let mut old = chord.replace(Chord::new(keys));
        old.timeout()
    });
    replay(&held_back);
    watch_chord();
    #[cfg(feature = "debug-log")]
    if let Some(s) = &settings {
        debug_log!(
            "settings: triggers {:?}, step_x {}, step_y {}, flick {}, vertical when dy >= {:.2} dx",
            s.triggers, s.step_x, s.step_y, s.vertical_sticky, s.vertical_bias
        );
    }
    // What was held under the old settings may not be a trigger any more.
    end_hold("the settings changed");
    SETTINGS.with(|slot| *slot.borrow_mut() = settings);
}

/// Puts the hooks in, taking out the ones there were.
fn hook() {
    unsafe {
        let module = GetModuleHandleW(None).unwrap_or_default();
        let mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), module, 0);
        let keyboard = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), module, 0);
        if mouse.is_err() || keyboard.is_err() {
            crate::app::log("installing the input hooks failed");
        }
        let old = HOOKS.replace([mouse, keyboard].into_iter().flatten().collect());
        for hook in old {
            let _ = UnhookWindowsHookEx(hook);
        }
    }
}

/// Now and then, while nothing is held or held back.
unsafe extern "system" fn rehook(_: HWND, _: u32, _: usize, _: u32) {
    if !held() && !CHORD.with(|chord| chord.borrow().waiting()) {
        hook();
    }
}

fn hook_thread() {
    unsafe {
        THREAD.store(GetCurrentThreadId(), Ordering::SeqCst);
        reload();
        let module = GetModuleHandleW(None).unwrap_or_default();
        let class = windows::core::w!("flick.input");
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(raw_window_proc),
            hInstance: module.into(),
            lpszClassName: class,
            ..Default::default()
        });
        let window =
            CreateWindowExW(WINDOW_EX_STYLE(0), class, None, WINDOW_STYLE(0), 0, 0, 0, 0, HWND_MESSAGE, None, module, None);
        RAW_WINDOW.set(window.0);
        hook();
        SetTimer(None, 0, REHOOK_MS, Some(rehook));
        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            if message.hwnd.0 == 0 && message.message == WM_RELOAD {
                reload();
            } else if message.hwnd.0 == 0 && message.message == WM_WOKE {
                end_hold("the PC had gone to sleep");
                hook();
            } else {
                // Timer callbacks arrive here.
                DispatchMessageW(&message);
            }
        }
        // Whoever holds a trigger down as the app goes must not find the
        // cursor still kept in place by it.
        end_hold("the app is closing");
        for hook in HOOKS.take() {
            let _ = UnhookWindowsHookEx(hook);
        }
    }
}

fn post(s: &Settings, message: u32, wparam: usize) {
    unsafe {
        let _ = PostMessageW(HWND(s.target), message, WPARAM(wparam), LPARAM(0));
    }
}

/// Starts or stops receiving raw input (only wanted while held): the mouse
/// for its movement, and both it and the keyboard to see the trigger let go
/// of even if the hooks do not.
fn listen_raw(on: bool) {
    let window = HWND(RAW_WINDOW.get());
    // Generic desktop devices: 2 is the mouse, 6 the keyboard.
    let devices = [2, 6].map(|usage| RAWINPUTDEVICE {
        usUsagePage: 1,
        usUsage: usage,
        dwFlags: if on { RIDEV_INPUTSINK } else { RIDEV_REMOVE },
        hwndTarget: if on { window } else { HWND(0) },
    });
    let registered = unsafe { RegisterRawInputDevices(&devices, std::mem::size_of::<RAWINPUTDEVICE>() as u32) };
    if let (true, Err(e)) = (on, registered) {
        // Without it the held trigger sees no movement at all.
        crate::app::log(&format!("raw mouse input could not be registered: {e}"));
    }
}

fn held() -> bool {
    HELD.with(|held| held.borrow().any())
}

fn press(source: Source) {
    let began = HELD.with(|held| held.borrow_mut().press(source));
    debug_log!("down {source:?}{}", if began { ": hold begins" } else { "" });
    if began {
        STEPPED.set(false);
        ACCUM.set((0, 0));
        FLICKED.set(0);
        LAST_MOVE.set(None);
        RECENT.with(|recent| recent.borrow_mut().clear());
        LAST_ABSOLUTE.set(None);
        STRAYED.set(((0, 0), 0));
        listen_raw(true);
        WATCH_TIMER.set(unsafe { SetTimer(None, 0, WATCH_MS, Some(watch)) });
    }
}

/// Stops what a hold keeps going: the raw input and the watching over it.
fn stop_listening() {
    listen_raw(false);
    let timer = WATCH_TIMER.replace(0);
    if timer != 0 {
        unsafe {
            let _ = KillTimer(None, timer);
        }
    }
}

/// Ends the hold, whatever holds it, without waiting for that to be let go
/// of: for when letting go could not be seen, or does not matter any more.
fn end_hold(why: &str) {
    if HELD.with(|held| held.borrow_mut().clear()) {
        debug_log!("hold ended by the app: {why}");
        let _ = why;
        CHORD.with(|chord| chord.borrow_mut().let_go());
        stop_listening();
        // Never as a click: the user did not ask for the board.
        with_settings(|s| {
            post(s, WM_RELEASE, 0);
            false
        });
    }
}

/// The integrity level of a process (whether it runs as administrator).
unsafe fn level_of(process: HANDLE) -> Option<u32> {
    let mut token = HANDLE::default();
    OpenProcessToken(process, TOKEN_QUERY, &mut token).ok()?;
    let mut label = [0u8; 128];
    let mut len = 0u32;
    let got = GetTokenInformation(token, TokenIntegrityLevel, Some(label.as_mut_ptr().cast()), label.len() as u32, &mut len);
    let _ = CloseHandle(token);
    got.ok()?;
    let sid = (*(label.as_ptr() as *const TOKEN_MANDATORY_LABEL)).Label.Sid;
    Some(*GetSidSubAuthority(sid, (*GetSidSubAuthorityCount(sid)).saturating_sub(1) as u32))
}

/// Whether a window belongs to a process above this one (run as
/// administrator while this one is not). Windows keeps what is typed and
/// clicked there from the hooks and the raw input of a process below.
fn above_us(window: HWND) -> bool {
    if window.0 == 0 {
        return false;
    }
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(window, Some(&mut pid));
    }
    LEVELS.with(|levels| {
        let mut levels = levels.borrow_mut();
        let ours = *levels.0.get_or_insert_with(|| unsafe { level_of(GetCurrentProcess()) }.unwrap_or(u32::MAX));
        let theirs = match levels.1.iter().find(|(known, _)| *known == pid) {
            Some((_, level)) => *level,
            None => {
                let level = unsafe {
                    OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok().and_then(|process| {
                        let level = level_of(process);
                        let _ = CloseHandle(process);
                        level
                    })
                };
                // Process ids come round again; a short memory is enough.
                if levels.1.len() >= 32 {
                    levels.1.clear();
                }
                levels.1.push((pid, level));
                level
            }
        };
        // A process that cannot even be asked is taken to be above.
        theirs.map_or(true, |theirs| theirs > ours)
    })
}

/// Why letting go of the trigger could not be seen right now, if so.
fn blind() -> Option<&'static str> {
    unsafe {
        // Locked, or asked for permission on the secure desktop.
        match OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, DESKTOP_READOBJECTS) {
            Ok(desktop) => {
                let _ = CloseDesktop(desktop);
            }
            Err(_) => return Some("the screen is locked or asking for permission"),
        }
        if above_us(GetForegroundWindow()) {
            return Some("a window run as administrator has the keyboard");
        }
        let mut cursor = POINT::default();
        if GetCursorPos(&mut cursor).is_ok() && above_us(WindowFromPoint(cursor)) {
            return Some("the cursor is on a window run as administrator");
        }
    }
    None
}

/// Watches over a hold: where its end could not be seen it is ended at
/// once, or it would go on after the trigger was let go of.
unsafe extern "system" fn watch(_: HWND, _: u32, _: usize, _: u32) {
    if !held() {
        return stop_listening();
    }
    if let Some(why) = blind() {
        end_hold(why);
    }
}

fn release(s: &Settings, source: Source) {
    let ended = HELD.with(|held| held.borrow_mut().release(source));
    debug_log!("up {source:?}{}", if ended { ": hold ends" } else { "" });
    if ended {
        debug_log!(
            "hold ended as a {}; cursor movements held back meanwhile: {}",
            if STEPPED.get() { "move" } else { "click" },
            crate::debuglog::take_held_back()
        );
        stop_listening();
        // A click is the trigger pressed and let go of where it was. One
        // that travelled, only not far enough for a step, is a move given
        // up: it must not open the board.
        let slack = s.step_x.min(s.step_y) / 3;
        let stayed = !STEPPED.get() && STRAYED.get().1 < slack;
        if !stayed && !STEPPED.get() {
            debug_log!("no click: the mouse had strayed {} (a click stays within {slack})", STRAYED.get().1);
        }
        // Nor is a click right upon a click one: nobody opens the board and
        // answers it that fast, but a button set up to send a key combination
        // may send it over and over while it is held.
        let now = Instant::now();
        let again = stayed && LAST_CLICK.replace(stayed.then_some(now)).is_some_and(|last| now.duration_since(last).as_millis() < CLICK_APART_MS);
        if again {
            debug_log!("no click: one was made less than {CLICK_APART_MS} ms ago");
        }
        let click = stayed && !again;
        post(s, if click { WM_CLICK } else { WM_RELEASE }, 0);
    }
}

/// A trigger seen let go of by raw input, which tells of it as well as the
/// hook does and often first. Whichever comes second finds nothing held;
/// this one is what ends the hold when the hook never hears of it.
fn raw_release(source: Source) {
    if HELD.with(|held| held.borrow().has(source)) {
        debug_log!("up {source:?} (told by raw input)");
        with_settings(|s| {
            release(s, source);
            false
        });
    }
}

/// A key seen coming up by raw input.
fn raw_key_up(key: u32) {
    raw_release(Source::Key(key));
    if CHORD.with(|chord| chord.borrow().holds(key)) {
        CHORD.with(|chord| chord.borrow_mut().let_go());
        raw_release(Source::Chord);
    }
}

/// Raw mouse input while the trigger is held: the device's own movement,
/// unaffected by the cursor being frozen.
fn on_raw_input(handle: HRAWINPUT) {
    const MOUSE_MOVE_ABSOLUTE: u16 = 0x01;
    const MOUSE_VIRTUAL_DESKTOP: u16 = 0x02;
    // Buttons coming up, as raw input flags them.
    const BUTTONS_UP: [(u16, Source); 3] = [(0x0020, Source::Middle), (0x0080, Source::XButton1), (0x0200, Source::XButton2)];
    const KEY_UP: u16 = 0x01;
    const KEY_E0: u16 = 0x02;
    if !held() {
        return;
    }
    let mut raw = RAWINPUT::default();
    let mut size = std::mem::size_of::<RAWINPUT>() as u32;
    let header = std::mem::size_of::<RAWINPUTHEADER>() as u32;
    let read = unsafe { GetRawInputData(handle, RID_INPUT, Some(&mut raw as *mut _ as *mut _), &mut size, header) };
    if read == u32::MAX {
        return;
    }
    if raw.header.dwType == RIM_TYPEKEYBOARD.0 {
        let keyboard = unsafe { raw.data.keyboard };
        if keyboard.Flags & KEY_UP != 0 {
            // Raw input does not tell right from left Ctrl and Alt by the key.
            let right = keyboard.Flags & KEY_E0 != 0;
            raw_key_up(match keyboard.VKey {
                0x11 => if right { 0xA3 } else { 0xA2 },
                0x12 => if right { 0xA5 } else { 0xA4 },
                key => key as u32,
            });
        }
        return;
    }
    if raw.header.dwType != RIM_TYPEMOUSE.0 {
        return;
    }
    let mouse = unsafe { raw.data.mouse };
    let buttons = unsafe { mouse.Anonymous.Anonymous.usButtonFlags };
    for (flag, source) in BUTTONS_UP {
        if buttons & flag != 0 {
            raw_release(source);
        }
    }
    if !held() {
        return;
    }
    #[cfg(feature = "debug-log")]
    crate::debuglog::device(raw.header.hDevice);
    let (dx, dy) = if mouse.usFlags & MOUSE_MOVE_ABSOLUTE != 0 {
        // Absolute pointers report a position in 0..65535 across the screen;
        // the movement is the difference from the previous report.
        let (w, h) = unsafe {
            if mouse.usFlags & MOUSE_VIRTUAL_DESKTOP != 0 {
                (GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN))
            } else {
                (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN))
            }
        };
        let now = ((mouse.lLastX as i64 * w as i64 / 65535) as i32, (mouse.lLastY as i64 * h as i64 / 65535) as i32);
        match LAST_ABSOLUTE.replace(Some(now)) {
            Some(before) => (now.0 - before.0, now.1 - before.1),
            None => (0, 0),
        }
    } else {
        (mouse.lLastX, mouse.lLastY)
    };
    if dx != 0 || dy != 0 {
        with_settings(|s| {
            travel(s, dx, dy);
            false
        });
    }
}

unsafe extern "system" fn raw_window_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if message == WM_INPUT {
        on_raw_input(HRAWINPUT(lparam.0));
    }
    DefWindowProcW(hwnd, message, wparam, lparam)
}

/// Vertical flicks are judged over the last this-many milliseconds of travel.
const FLICK_WINDOW_MS: u128 = 150;
/// With no movement for this long the hand has stopped.
const FLICK_PAUSE_MS: u128 = 70;

/// Adds mouse travel and emits steps.
///
/// Sideways it is linear: one step per threshold of travel, as far as the
/// hand goes. Vertically (unless the user asked for linear there too) a row
/// changes only on a flick: half the vertical threshold covered within
/// `FLICK_WINDOW_MS`. One flick is one row however far it carries on; a flick
/// the other way goes back at once; and the slow movement of bringing the
/// hand back for the next flick is too slow to count, so flick, flick is two
/// rows.
fn travel(s: &Settings, dx: i32, dy: i32) {
    let ((x, y), farthest) = STRAYED.get();
    let (x, y) = (x + dx, y + dy);
    STRAYED.set(((x, y), farthest.max(x.abs()).max(y.abs())));
    if !s.vertical_sticky {
        return travel_linear(s, dx, dy);
    }
    let now = Instant::now();
    let paused = LAST_MOVE.replace(Some(now)).is_some_and(|before| now.duration_since(before).as_millis() > FLICK_PAUSE_MS);
    if paused {
        debug_log!("(a pause)");
    }
    let (wx, wy) = RECENT.with(|recent| {
        let mut recent = recent.borrow_mut();
        // A stop, or turning round after a flick, starts a fresh stroke.
        let turned = FLICKED.get() != 0 && dy != 0 && dy.signum() != FLICKED.get();
        if paused || turned {
            recent.clear();
            FLICKED.set(0);
        }
        recent.push_back((now, dx, dy));
        while recent.front().is_some_and(|(at, _, _)| now.duration_since(*at).as_millis() > FLICK_WINDOW_MS) {
            recent.pop_front();
        }
        recent.iter().fold((0, 0), |(x, y), (_, dx, dy)| (x + dx, y + dy))
    });
    let (mut ax, _) = ACCUM.get();
    if s.is_vertical(wx, wy) {
        // Moving up or down: nothing of it counts sideways. What was
        // travelled sideways before is kept, though: the hand wavers up and
        // down on its way, and only a flick is a change of mind.
        if FLICKED.get() == 0 && wy.abs() >= (s.step_y / 2).max(40) {
            ax = 0;
            FLICKED.set(wy.signum());
            RECENT.with(|recent| recent.borrow_mut().clear());
            STEPPED.set(true);
            debug_log!("step {}", if wy > 0 { "down" } else { "up" });
            post(s, WM_STEP, dir_index(if wy > 0 { Dir::Down } else { Dir::Up }));
        }
    } else {
        ax += dx;
        while ax.abs() >= s.step_x {
            let dir = if ax > 0 { Dir::Right } else { Dir::Left };
            ax -= ax.signum() * s.step_x;
            FLICKED.set(0);
            STEPPED.set(true);
            debug_log!("step {dir:?}");
            post(s, WM_STEP, dir_index(dir));
        }
    }
    debug_log!(
        "move {dx} {dy}: last moments {wx} {wy} ({}), sideways so far {ax} of {}",
        if s.is_vertical(wx, wy) { "vertical" } else { "sideways" },
        s.step_x
    );
    ACCUM.set((ax, 0));
}

/// Both axes by distance alone: one step per threshold of travel. The other
/// axis is cleared on a step so that drift does not leak into the next one.
fn travel_linear(s: &Settings, dx: i32, dy: i32) {
    let (mut ax, mut ay) = ACCUM.get();
    ax += dx;
    ay += dy;
    loop {
        let vertical = s.is_vertical(ax, ay);
        let dir = if vertical && ay.abs() >= s.step_y {
            let dir = if ay > 0 { Dir::Down } else { Dir::Up };
            ay -= ay.signum() * s.step_y;
            ax = 0;
            dir
        } else if !vertical && ax.abs() >= s.step_x {
            let dir = if ax > 0 { Dir::Right } else { Dir::Left };
            ax -= ax.signum() * s.step_x;
            ay = 0;
            dir
        } else {
            break;
        };
        STEPPED.set(true);
        debug_log!("step {dir:?}");
        post(s, WM_STEP, dir_index(dir));
    }
    debug_log!("move {dx} {dy}: so far {ax} of {}, {ay} of {}", s.step_x, s.step_y);
    ACCUM.set((ax, ay));
}

/// Returns true when the event belongs to the gesture and must not reach apps.
fn on_mouse(s: &Settings, message: u32, info: &MSLLHOOKSTRUCT) -> bool {
    let xbutton = (info.mouseData >> 16) as u16;
    let button = match message {
        WM_XBUTTONDOWN | WM_XBUTTONUP if xbutton == 1 => Some((Trigger::XButton1, Source::XButton1)),
        WM_XBUTTONDOWN | WM_XBUTTONUP if xbutton == 2 => Some((Trigger::XButton2, Source::XButton2)),
        WM_MBUTTONDOWN | WM_MBUTTONUP => Some((Trigger::Middle, Source::Middle)),
        _ => None,
    };
    let down = matches!(message, WM_XBUTTONDOWN | WM_MBUTTONDOWN);
    if CAPTURING.load(Ordering::SeqCst) {
        // Choosing a new trigger: the first button pressed is it.
        return match button {
            Some((trigger, _)) if down => {
                finish_capture(s, Some(trigger.to_config()));
                true
            }
            _ => false,
        };
    }
    if let Some((trigger, source)) = button {
        if s.triggers.contains(&trigger) {
            if down {
                press(source);
            } else {
                release(s, source);
            }
            return true;
        }
    }
    // The cursor stays put while the trigger is held; the movement itself is
    // read from raw input (`on_raw_input`).
    let held_back = message == WM_MOUSEMOVE && held();
    #[cfg(feature = "debug-log")]
    if held_back {
        crate::debuglog::held_back();
    }
    held_back
}

fn key_event(key: u32, up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(key as u16),
                wScan: unsafe { MapVirtualKeyW(key, MAPVK_VK_TO_VSC) } as u16,
                dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
                time: 0,
                dwExtraInfo: OWN_INPUT,
            },
        },
    }
}

/// Gives key events to the apps: (key, is a release).
fn replay(events: &[(u32, bool)]) {
    if events.is_empty() {
        return;
    }
    let events: Vec<INPUT> = events.iter().map(|&(key, up)| key_event(key, up)).collect();
    unsafe {
        SendInput(&events, std::mem::size_of::<INPUT>() as i32);
    }
}

/// Keeps the timer that ends the wait for the rest of the chord in step with
/// the chord: running, from the first key held back, while any is.
fn watch_chord() {
    let (waiting, keys) = CHORD.with(|chord| (chord.borrow().waiting(), chord.borrow().len()));
    let timer = CHORD_TIMER.get();
    if waiting && timer == 0 {
        let window = if keys <= 2 { PAIR_WINDOW_MS } else { CHORD_WINDOW_MS };
        CHORD_BEGAN.set(Some(Instant::now()));
        CHORD_TIMER.set(unsafe { SetTimer(None, 0, window, Some(chord_timeout)) });
    } else if !waiting && timer != 0 {
        CHORD_TIMER.set(0);
        unsafe {
            let _ = KillTimer(None, timer);
        }
    }
}

/// The rest of the chord did not follow in time.
unsafe extern "system" fn chord_timeout(_: HWND, _: u32, _: usize, _: u32) {
    let held_back = CHORD.with(|chord| chord.borrow_mut().timeout());
    debug_log!("the chord's keys did not all go down in time: typing");
    replay(&held_back);
    watch_chord();
}

fn on_key(s: &Settings, key: u32, up: bool) -> bool {
    if CAPTURING.load(Ordering::SeqCst) {
        return capture_key(s, key, up);
    }
    // Keys of a choice that was given up must not count towards the next.
    CAPTURE_KEYS.with(|keys| keys.borrow_mut().clear());
    if s.triggers.contains(&Trigger::Key(key)) {
        if up {
            release(s, Source::Key(key));
        } else {
            press(Source::Key(key));
        }
        return true;
    }
    let verdict = CHORD.with(|chord| {
        let mut chord = chord.borrow_mut();
        if chord.has(key) {
            chord.key(key, up)
        } else {
            chord.other_key(key, up)
        }
    });
    #[cfg(feature = "debug-log")]
    if !up && verdict == crate::hold::Verdict::default() && CHORD.with(|chord| chord.borrow().has(key)) {
        // How late the rest of the chord came says whether the wait is too short.
        if let Some(late) = CHORD_BEGAN.get().map(|began| began.elapsed().as_millis()).filter(|late| *late < 1000) {
            debug_log!("another key of the chord went down {late} ms after the first: too late, typing");
        }
    }
    replay(&verdict.replay);
    watch_chord();
    match verdict.trigger {
        Some(true) => press(Source::Chord),
        Some(false) => release(s, Source::Chord),
        None => {}
    }
    verdict.swallow
}

fn with_settings(f: impl FnOnce(&Settings) -> bool) -> bool {
    SETTINGS.with(|slot| slot.try_borrow().ok().and_then(|s| s.as_ref().map(f)).unwrap_or(false))
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let info = &*(lparam.0 as *const MSLLHOOKSTRUCT);
        if with_settings(|s| on_mouse(s, wparam.0 as u32, info)) {
            return LRESULT(1);
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let info = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
        let up = matches!(wparam.0 as u32, WM_KEYUP | WM_SYSKEYUP);
        let down = matches!(wparam.0 as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
        // Our own replayed keys pass; input injected by others (remote
        // desktop tools) is treated like a real keyboard.
        if (up || down) && info.dwExtraInfo != OWN_INPUT && with_settings(|s| on_key(s, info.vkCode, up)) {
            return LRESULT(1);
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}
