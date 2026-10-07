//! Global hooks that turn "hold the trigger and move the mouse" into grid steps.
//!
//! The hook callbacks only do arithmetic and post messages; the desktop switch
//! itself happens later on the message loop so the hooks never stall input.

use crate::grid::Dir;
use std::cell::Cell;
use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM},
    System::LibraryLoader::GetModuleHandleW,
    UI::WindowsAndMessaging::{
        CallNextHookEx, GetCursorPos, PostMessageW, SetWindowsHookExW, UnhookWindowsHookEx, HC_ACTION,
        HHOOK, KBDLLHOOKSTRUCT, LLKHF_INJECTED, LLMHF_INJECTED, MSLLHOOKSTRUCT, WH_KEYBOARD_LL,
        WH_MOUSE_LL, WM_APP, WM_KEYDOWN, WM_KEYUP, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEMOVE,
        WM_SYSKEYDOWN, WM_SYSKEYUP, WM_XBUTTONDOWN, WM_XBUTTONUP,
    },
};

/// wparam: direction index (see `dir_from_index`).
pub const WM_STEP: u32 = WM_APP + 1;
/// The trigger was pressed and released without travelling a step.
pub const WM_CLICK: u32 = WM_APP + 2;
/// The trigger was released after at least one step.
pub const WM_RELEASE: u32 = WM_APP + 3;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Trigger {
    XButton1,
    XButton2,
    Middle,
    Key(u32),
}

impl Trigger {
    pub fn parse(name: &str) -> Option<Trigger> {
        let name = name.trim().to_ascii_lowercase();
        Some(match name.as_str() {
            "xbutton1" => Trigger::XButton1,
            "xbutton2" => Trigger::XButton2,
            "middle" => Trigger::Middle,
            "pause" => Trigger::Key(0x13),
            "capslock" => Trigger::Key(0x14),
            "apps" => Trigger::Key(0x5D),
            "scrolllock" => Trigger::Key(0x91),
            "rctrl" => Trigger::Key(0xA3),
            "ralt" => Trigger::Key(0xA5),
            _ => {
                let n: u32 = name.strip_prefix('f')?.parse().ok()?;
                (13..=24).contains(&n).then(|| Trigger::Key(0x7C + n - 13))?
            }
        })
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

#[derive(Clone, Copy)]
struct Settings {
    target: HWND,
    trigger: Trigger,
    step_x: i32,
    step_y: i32,
}

thread_local! {
    static SETTINGS: Cell<Option<Settings>> = const { Cell::new(None) };
    static HOOKS: Cell<(HHOOK, HHOOK)> = const { Cell::new((HHOOK(0), HHOOK(0))) };
    static HELD: Cell<bool> = const { Cell::new(false) };
    static STEPPED: Cell<bool> = const { Cell::new(false) };
    /// Mouse travel not yet converted into steps.
    static ACCUM: Cell<(i32, i32)> = const { Cell::new((0, 0)) };
}

pub fn install(target: HWND, trigger: Trigger, step_x: i32, step_y: i32) {
    uninstall();
    SETTINGS.set(Some(Settings { target, trigger, step_x: step_x.max(20), step_y: step_y.max(20) }));
    unsafe {
        let module = GetModuleHandleW(None).unwrap_or_default();
        let mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), module, 0).unwrap_or_default();
        let keyboard = match trigger {
            Trigger::Key(_) => SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), module, 0).unwrap_or_default(),
            _ => HHOOK(0),
        };
        HOOKS.set((mouse, keyboard));
    }
}

pub fn uninstall() {
    let (mouse, keyboard) = HOOKS.replace((HHOOK(0), HHOOK(0)));
    for hook in [mouse, keyboard] {
        if hook.0 != 0 {
            unsafe {
                let _ = UnhookWindowsHookEx(hook);
            }
        }
    }
    HELD.set(false);
}

fn post(s: &Settings, message: u32, wparam: usize) {
    unsafe {
        let _ = PostMessageW(s.target, message, WPARAM(wparam), LPARAM(0));
    }
}

fn press(_s: &Settings) {
    if !HELD.replace(true) {
        STEPPED.set(false);
        ACCUM.set((0, 0));
    }
}

fn release(s: &Settings) {
    if HELD.replace(false) {
        post(s, if STEPPED.get() { WM_RELEASE } else { WM_CLICK }, 0);
    }
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
    let is_trigger = match (s.trigger, message) {
        (Trigger::XButton1, WM_XBUTTONDOWN | WM_XBUTTONUP) => xbutton == 1,
        (Trigger::XButton2, WM_XBUTTONDOWN | WM_XBUTTONUP) => xbutton == 2,
        (Trigger::Middle, WM_MBUTTONDOWN | WM_MBUTTONUP) => true,
        _ => false,
    };
    if is_trigger {
        match message {
            WM_XBUTTONDOWN | WM_MBUTTONDOWN => press(s),
            _ => release(s),
        }
        return true;
    }
    if message == WM_MOUSEMOVE && HELD.get() {
        // Swallowing the move keeps the cursor where it is, so each event's
        // point is "cursor + this movement" and the gesture never runs into a
        // screen edge.
        let mut cursor = POINT::default();
        if unsafe { GetCursorPos(&mut cursor) }.is_ok() {
            travel(s, info.pt.x - cursor.x, info.pt.y - cursor.y);
        }
        return true;
    }
    false
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let info = &*(lparam.0 as *const MSLLHOOKSTRUCT);
        if info.flags & LLMHF_INJECTED == 0 {
            if let Some(s) = SETTINGS.get() {
                if on_mouse(&s, wparam.0 as u32, info) {
                    return LRESULT(1);
                }
            }
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        let info = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
        if let Some(s) = SETTINGS.get() {
            if s.trigger == Trigger::Key(info.vkCode) && (info.flags & LLKHF_INJECTED).0 == 0 {
                match wparam.0 as u32 {
                    WM_KEYDOWN | WM_SYSKEYDOWN => press(&s),
                    WM_KEYUP | WM_SYSKEYUP => release(&s),
                    _ => {}
                }
                return LRESULT(1);
            }
        }
    }
    CallNextHookEx(None, code, wparam, lparam)
}
