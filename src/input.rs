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

use crate::grid::Dir;
use std::{
    cell::{Cell, RefCell},
    sync::{
        atomic::{AtomicU32, Ordering},
        Mutex,
    },
};
use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, WPARAM},
    System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId},
    UI::{
        Input::{
            GetRawInputData,
            KeyboardAndMouse::{
                MapVirtualKeyW, SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS,
                KEYEVENTF_KEYUP, MAPVK_VK_TO_VSC, VIRTUAL_KEY,
            },
            RegisterRawInputDevices, HRAWINPUT, RAWINPUT, RAWINPUTDEVICE, RAWINPUTHEADER, RIDEV_INPUTSINK, RIDEV_REMOVE,
            RID_INPUT, RIM_TYPEMOUSE,
        },
        WindowsAndMessaging::{
            CallNextHookEx, CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, GetSystemMetrics, KillTimer,
            PostMessageW, PostThreadMessageW, RegisterClassW, SetTimer, SetWindowsHookExW, UnhookWindowsHookEx,
            HC_ACTION, HWND_MESSAGE, KBDLLHOOKSTRUCT, MSG, MSLLHOOKSTRUCT, SM_CXSCREEN, SM_CXVIRTUALSCREEN, SM_CYSCREEN,
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

/// Thread message telling the hook thread to pick up new settings.
const WM_RELOAD: u32 = WM_APP + 20;
/// Marks key events we replay ourselves, so the hook lets them through.
const OWN_INPUT: usize = 0x4B41_4E4B;
/// All keys of a chord must go down within this long to count as one press.
const CHORD_WINDOW_MS: u32 = 60;

#[derive(Clone, Debug, PartialEq)]
pub enum Trigger {
    XButton1,
    XButton2,
    Middle,
    Key(u32),
    /// Several keys pressed together, e.g. "e+r+t".
    Chord(Vec<u32>),
}

fn key_code(name: &str) -> Option<u32> {
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
}

impl Settings {
    fn chord(&self) -> Option<&[u32]> {
        self.triggers.iter().find_map(|t| match t {
            Trigger::Chord(keys) => Some(keys.as_slice()),
            _ => None,
        })
    }
}

/// Settings handed from the app to the hook thread.
static SHARED: Mutex<Option<Settings>> = Mutex::new(None);
static THREAD: AtomicU32 = AtomicU32::new(0);

// State of the hook thread.
thread_local! {
    static SETTINGS: RefCell<Option<Settings>> = const { RefCell::new(None) };
    static HELD: Cell<bool> = const { Cell::new(false) };
    static STEPPED: Cell<bool> = const { Cell::new(false) };
    /// Mouse travel not yet converted into steps.
    static ACCUM: Cell<(i32, i32)> = const { Cell::new((0, 0)) };
    /// The hook thread's hidden window, which receives raw mouse input.
    static RAW_WINDOW: Cell<isize> = const { Cell::new(0) };
    /// Last position reported by an absolute pointer (remote desktop, pen).
    static LAST_ABSOLUTE: Cell<Option<(i32, i32)>> = const { Cell::new(None) };
    /// Chord keys held back while waiting to see whether the chord completes.
    static PENDING: RefCell<Vec<u32>> = const { RefCell::new(Vec::new()) };
    /// Chord keys still down after the chord fired; their events are dropped.
    static SWALLOW: RefCell<Vec<u32>> = const { RefCell::new(Vec::new()) };
    static CHORD_TIMER: Cell<usize> = const { Cell::new(0) };
}

/// Starts the hook thread (once) and gives it these settings.
pub fn install(target: HWND, triggers: Vec<Trigger>, step_x: i32, step_y: i32) {
    *SHARED.lock().unwrap() = Some(Settings { target: target.0, triggers, step_x: step_x.max(20), step_y: step_y.max(20) });
    match THREAD.load(Ordering::SeqCst) {
        0 => {
            std::thread::spawn(hook_thread);
        }
        thread => unsafe {
            let _ = PostThreadMessageW(thread, WM_RELOAD, WPARAM(0), LPARAM(0));
        },
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
    SETTINGS.with(|slot| *slot.borrow_mut() = settings);
    flush_pending(None);
    SWALLOW.with(|keys| keys.borrow_mut().clear());
    if HELD.replace(false) {
        listen_raw(false);
    }
}

fn hook_thread() {
    unsafe {
        THREAD.store(GetCurrentThreadId(), Ordering::SeqCst);
        reload();
        let module = GetModuleHandleW(None).unwrap_or_default();
        let class = windows::core::w!("kankan.input");
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(raw_window_proc),
            hInstance: module.into(),
            lpszClassName: class,
            ..Default::default()
        });
        let window =
            CreateWindowExW(WINDOW_EX_STYLE(0), class, None, WINDOW_STYLE(0), 0, 0, 0, 0, HWND_MESSAGE, None, module, None);
        RAW_WINDOW.set(window.0);
        let mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), module, 0);
        let keyboard = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), module, 0);
        if mouse.is_err() || keyboard.is_err() {
            crate::app::log("installing the input hooks failed");
        }
        let mut message = MSG::default();
        while GetMessageW(&mut message, None, 0, 0).as_bool() {
            if message.hwnd.0 == 0 && message.message == WM_RELOAD {
                reload();
            } else {
                // Timer callbacks arrive here.
                DispatchMessageW(&message);
            }
        }
        for hook in [mouse, keyboard].into_iter().flatten() {
            let _ = UnhookWindowsHookEx(hook);
        }
    }
}

fn post(s: &Settings, message: u32, wparam: usize) {
    unsafe {
        let _ = PostMessageW(HWND(s.target), message, WPARAM(wparam), LPARAM(0));
    }
}

/// Starts or stops receiving raw mouse input (only wanted while held).
fn listen_raw(on: bool) {
    let window = HWND(RAW_WINDOW.get());
    let device = RAWINPUTDEVICE {
        usUsagePage: 1, // generic desktop
        usUsage: 2,     // mouse
        dwFlags: if on { RIDEV_INPUTSINK } else { RIDEV_REMOVE },
        hwndTarget: if on { window } else { HWND(0) },
    };
    unsafe {
        let _ = RegisterRawInputDevices(&[device], std::mem::size_of::<RAWINPUTDEVICE>() as u32);
    }
}

fn press() {
    if !HELD.replace(true) {
        STEPPED.set(false);
        ACCUM.set((0, 0));
        LAST_ABSOLUTE.set(None);
        listen_raw(true);
    }
}

fn release(s: &Settings) {
    if HELD.replace(false) {
        listen_raw(false);
        post(s, if STEPPED.get() { WM_RELEASE } else { WM_CLICK }, 0);
    }
}

/// Raw mouse input while the trigger is held: the device's own movement,
/// unaffected by the cursor being frozen.
fn on_raw_input(handle: HRAWINPUT) {
    const MOUSE_MOVE_ABSOLUTE: u16 = 0x01;
    const MOUSE_VIRTUAL_DESKTOP: u16 = 0x02;
    if !HELD.get() {
        return;
    }
    let mut raw = RAWINPUT::default();
    let mut size = std::mem::size_of::<RAWINPUT>() as u32;
    let header = std::mem::size_of::<RAWINPUTHEADER>() as u32;
    let read = unsafe { GetRawInputData(handle, RID_INPUT, Some(&mut raw as *mut _ as *mut _), &mut size, header) };
    if read == u32::MAX || raw.header.dwType != RIM_TYPEMOUSE.0 {
        return;
    }
    let mouse = unsafe { raw.data.mouse };
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

/// Adds mouse travel and emits one step per threshold crossed. A step is taken
/// along whichever axis is further through its own threshold, and the other
/// axis is cleared so a diagonal drift does not leak into the next step.
fn travel(s: &Settings, dx: i32, dy: i32) {
    let (mut ax, mut ay) = ACCUM.get();
    ax += dx;
    ay += dy;
    loop {
        let nx = ax.abs() as f32 / s.step_x as f32;
        let ny = ay.abs() as f32 / s.step_y as f32;
        if nx < 1.0 && ny < 1.0 {
            break;
        }
        let dir = if nx >= ny {
            let dir = if ax > 0 { Dir::Right } else { Dir::Left };
            ax -= ax.signum() * s.step_x;
            ay = 0;
            dir
        } else {
            let dir = if ay > 0 { Dir::Down } else { Dir::Up };
            ay -= ay.signum() * s.step_y;
            ax = 0;
            dir
        };
        STEPPED.set(true);
        post(s, WM_STEP, dir_index(dir));
    }
    ACCUM.set((ax, ay));
}

/// Returns true when the event belongs to the gesture and must not reach apps.
fn on_mouse(s: &Settings, message: u32, info: &MSLLHOOKSTRUCT) -> bool {
    let xbutton = (info.mouseData >> 16) as u16;
    let button = match message {
        WM_XBUTTONDOWN | WM_XBUTTONUP if xbutton == 1 => Some(Trigger::XButton1),
        WM_XBUTTONDOWN | WM_XBUTTONUP if xbutton == 2 => Some(Trigger::XButton2),
        WM_MBUTTONDOWN | WM_MBUTTONUP => Some(Trigger::Middle),
        _ => None,
    };
    if button.is_some_and(|button| s.triggers.contains(&button)) {
        match message {
            WM_XBUTTONDOWN | WM_MBUTTONDOWN => press(),
            _ => release(s),
        }
        return true;
    }
    // The cursor stays put while the trigger is held; the movement itself is
    // read from raw input (`on_raw_input`).
    message == WM_MOUSEMOVE && HELD.get()
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

/// Gives the held-back chord keys to the apps after all (they were ordinary
/// typing), followed by `then`, the event that showed it was not a chord.
fn flush_pending(then: Option<(u32, bool)>) {
    let timer = CHORD_TIMER.replace(0);
    if timer != 0 {
        unsafe {
            let _ = KillTimer(None, timer);
        }
    }
    let mut events: Vec<INPUT> = PENDING.with(|keys| keys.borrow_mut().drain(..).map(|key| key_event(key, false)).collect());
    events.extend(then.map(|(key, up)| key_event(key, up)));
    if !events.is_empty() {
        unsafe {
            SendInput(&events, std::mem::size_of::<INPUT>() as i32);
        }
    }
}

unsafe extern "system" fn chord_timeout(_: HWND, _: u32, _: usize, _: u32) {
    flush_pending(None);
}

/// Handles a key that is part of the chord. Returns true to swallow the event.
///
/// The first keys of a chord cannot be told from typing, so they are held
/// back briefly. If the rest follow in time the chord acts as the trigger and
/// nothing is typed; otherwise the keys are replayed in order.
fn on_chord_key(s: &Settings, chord: &[u32], key: u32, up: bool) -> bool {
    let swallowed = SWALLOW.with(|keys| {
        let mut keys = keys.borrow_mut();
        let found = keys.contains(&key);
        if found && up {
            keys.retain(|k| *k != key);
        }
        found
    });
    if swallowed {
        // The chord is over as soon as one of its keys comes up.
        if up {
            release(s);
        }
        return true;
    }
    let pending = PENDING.with(|keys| keys.borrow().contains(&key));
    if up {
        if pending {
            flush_pending(Some((key, true)));
            return true;
        }
        return false;
    }
    if pending {
        return true; // auto-repeat while waiting
    }
    let complete = PENDING.with(|keys| {
        let mut keys = keys.borrow_mut();
        keys.push(key);
        chord.iter().all(|k| keys.contains(k))
    });
    if complete {
        let timer = CHORD_TIMER.replace(0);
        if timer != 0 {
            let _ = unsafe { KillTimer(None, timer) };
        }
        let keys = PENDING.with(|keys| std::mem::take(&mut *keys.borrow_mut()));
        SWALLOW.with(|slot| *slot.borrow_mut() = keys);
        press();
    } else if CHORD_TIMER.get() == 0 {
        CHORD_TIMER.set(unsafe { SetTimer(None, 0, CHORD_WINDOW_MS, Some(chord_timeout)) });
    }
    true
}

fn on_key(s: &Settings, key: u32, up: bool) -> bool {
    if s.triggers.contains(&Trigger::Key(key)) {
        if up {
            release(s);
        } else {
            press();
        }
        return true;
    }
    if let Some(chord) = s.chord() {
        if chord.contains(&key) {
            return on_chord_key(s, chord, key, up);
        }
        // Another key in between: what was held back was typing. Replay it
        // together with this key so the order is kept.
        if PENDING.with(|keys| !keys.borrow().is_empty()) {
            flush_pending(Some((key, up)));
            return true;
        }
    }
    false
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
