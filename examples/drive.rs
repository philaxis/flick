//! Test driver: posts the app's own messages to the running instance, sends
//! input and captures the screen, so behaviour can be checked without a person
//! at the mouse.
//!   drive step <0..3> | click | release | change-trigger
//!   drive xbutton <down|up> | mouse <x> <y> [down|up|rdown|rup] | move <dx> <dy>
//!   drive stroke <dx> <dy> <ms> | keys <down|up> <letters> | typetest
//!   drive shot <path> | pixel <x> <y> | wins [all] | cursor | info | idle
//! The message numbers are the app's `WM_STEP`, `WM_CLICK`, `WM_RELEASE`
//! (src/input.rs) and `WM_CHANGE_TRIGGER` (src/app.rs).
// The app is a binary only; its door to the virtual desktops is compiled in
// here as it is, and this driver uses a small part of it.
#[allow(dead_code)]
#[path = "../src/vdapi.rs"]
mod vdapi;

#[cfg(windows)]
fn main() {
    use windows::{
        core::w,
        Win32::{
            Foundation::{LPARAM, WPARAM},
            Graphics::Gdi::{
                BitBlt, CreateCompatibleDC, CreateDIBSection, GetDC, SelectObject, BITMAPINFO, BITMAPINFOHEADER,
                DIB_RGB_COLORS, SRCCOPY,
            },
            UI::{
                HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2},
                WindowsAndMessaging::{
                    FindWindowW, GetSystemMetrics, PostMessageW, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
                    SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, WM_APP,
                },
            },
        },
    };
    let args: Vec<String> = std::env::args().collect();
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let app = FindWindowW(w!("flick.main"), None);
        let post = |message: u32, wparam: usize| {
            let _ = PostMessageW(app, message, WPARAM(wparam), LPARAM(0));
        };
        match args.get(1).map(String::as_str) {
            Some("step") => post(WM_APP + 1, args[2].parse().unwrap()),
            Some("click") => post(WM_APP + 2, 0),
            Some("release") => post(WM_APP + 3, 0),
            Some("change-trigger") => post(WM_APP + 12, 0),
            Some("xbutton") => {
                // xbutton <down|up>: the mouse "forward" side button.
                use windows::Win32::UI::Input::KeyboardAndMouse::{SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, MOUSEINPUT};
                let flags = if args[2] == "down" { MOUSEEVENTF_XDOWN } else { MOUSEEVENTF_XUP };
                let input = INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { mouseData: 2, dwFlags: flags, ..Default::default() } } };
                SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
            }
            Some("shot") => {
                let (x, y) = (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN));
                let (w, h) = (GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN));
                let screen = GetDC(None);
                let dc = CreateCompatibleDC(screen);
                let info = BITMAPINFO {
                    bmiHeader: BITMAPINFOHEADER {
                        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: w,
                        biHeight: -h,
                        biPlanes: 1,
                        biBitCount: 32,
                        ..Default::default()
                    },
                    ..Default::default()
                };
                let mut bits = std::ptr::null_mut();
                let bitmap = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0).unwrap();
                SelectObject(dc, bitmap);
                let _ = BitBlt(dc, 0, 0, w, h, screen, x, y, SRCCOPY);
                let pixels = std::slice::from_raw_parts(bits as *const u8, (w * h * 4) as usize);
                let mut out = Vec::with_capacity(pixels.len() + 8);
                out.extend_from_slice(&w.to_le_bytes());
                out.extend_from_slice(&h.to_le_bytes());
                out.extend_from_slice(pixels);
                std::fs::write(&args[2], out).unwrap();
                println!("{w}x{h} at {x},{y}");
            }
            Some("mouse") => {
                // mouse <x> <y> [down|up|rdown|rup]: moves the real cursor, optionally pressing a button.
                use windows::Win32::UI::Input::KeyboardAndMouse::{
                    SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MOVE,
                    MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEINPUT,
                };
                use windows::Win32::UI::WindowsAndMessaging::SetCursorPos;
                let _ = SetCursorPos(args[2].parse().unwrap(), args[3].parse().unwrap());
                // Injected buttons are physical ones; honour a left-handed mouse setting.
                let swapped = GetSystemMetrics(windows::Win32::UI::WindowsAndMessaging::SM_SWAPBUTTON) != 0;
                let (primary_down, primary_up, other_down, other_up) = if swapped {
                    (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP)
                } else {
                    (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP)
                };
                let flags = match args.get(4).map(String::as_str) {
                    Some("down") => primary_down,
                    Some("up") => primary_up,
                    Some("rdown") => other_down,
                    Some("rup") => other_up,
                    _ => MOUSEEVENTF_MOVE,
                };
                let input = INPUT {
                    r#type: INPUT_MOUSE,
                    Anonymous: INPUT_0 { mi: MOUSEINPUT { dwFlags: flags, ..Default::default() } },
                };
                SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
            }
            Some("move") => {
                // move <dx> <dy>: relative mouse movement, like a real mouse.
                use windows::Win32::UI::Input::KeyboardAndMouse::{SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_MOVE, MOUSEINPUT};
                let input = INPUT {
                    r#type: INPUT_MOUSE,
                    Anonymous: INPUT_0 {
                        mi: MOUSEINPUT { dx: args[2].parse().unwrap(), dy: args[3].parse().unwrap(), dwFlags: MOUSEEVENTF_MOVE, ..Default::default() },
                    },
                };
                SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
            }
            Some("stroke") => {
                // stroke <dx> <dy> <ms>: one continuous movement, as a stream of small reports.
                use windows::Win32::UI::Input::KeyboardAndMouse::{SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_MOVE, MOUSEINPUT};
                let (dx, dy, ms): (i32, i32, u64) = (args[2].parse().unwrap(), args[3].parse().unwrap(), args[4].parse().unwrap());
                let reports = (ms / 5).max(1) as i32;
                let started = std::time::Instant::now();
                for i in 1..=reports {
                    let part = |total: i32| total * i / reports - total * (i - 1) / reports;
                    let input = INPUT {
                        r#type: INPUT_MOUSE,
                        Anonymous: INPUT_0 { mi: MOUSEINPUT { dx: part(dx), dy: part(dy), dwFlags: MOUSEEVENTF_MOVE, ..Default::default() } },
                    };
                    SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
                    let due = std::time::Duration::from_millis(ms * i as u64 / reports as u64);
                    if let Some(wait) = due.checked_sub(started.elapsed()) {
                        std::thread::sleep(wait);
                    }
                }
            }
            Some("keys") => {
                // keys <down|up> <letters>: presses or releases all the letters at once.
                let up = args[2] == "up";
                let events: Vec<_> = args[3].chars().map(|c| key(c, up)).collect();
                windows::Win32::UI::Input::KeyboardAndMouse::SendInput(&events, std::mem::size_of::<windows::Win32::UI::Input::KeyboardAndMouse::INPUT>() as i32);
            }
            Some("typetest") => typetest(),
            Some("idle") => {
                // Milliseconds since anyone (or anything) last produced input.
                use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
                let mut info = LASTINPUTINFO { cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
                let _ = GetLastInputInfo(&mut info);
                println!("{}", windows::Win32::System::SystemInformation::GetTickCount().wrapping_sub(info.dwTime));
            }
            Some("pixel") => {
                // pixel <x> <y>: colour of the board window's own surface there, and of the screen.
                use windows::Win32::Graphics::Gdi::{GetPixel, ReleaseDC};
                let board = FindWindowW(w!("flick.board"), None);
                let (x, y): (i32, i32) = (args[2].parse().unwrap(), args[3].parse().unwrap());
                let window = GetDC(board);
                let screen = GetDC(None);
                println!("window {:06x} screen {:06x}", GetPixel(window, x, y).0, GetPixel(screen, x, y).0);
                ReleaseDC(board, window);
                ReleaseDC(None, screen);
            }
            Some("wins") => {
                // Lists every visible top-level window: handle, class, title, rectangle, ex-style.
                use windows::Win32::Foundation::{BOOL, HWND, RECT};
                use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetClassNameW, GetWindowLongW, GetWindowRect, GetWindowTextW, IsWindowVisible, GWL_EXSTYLE};
                unsafe extern "system" fn each(hwnd: HWND, _: LPARAM) -> BOOL {
                    let all = std::env::args().nth(2).as_deref() == Some("all");
                    if all || IsWindowVisible(hwnd).as_bool() {
                        let mut cloaked = 0u32;
                        let _ = windows::Win32::Graphics::Dwm::DwmGetWindowAttribute(hwnd, windows::Win32::Graphics::Dwm::DWMWA_CLOAKED, &mut cloaked as *mut _ as *mut _, 4);
                        if all {
                            print!("v{}c{}|", IsWindowVisible(hwnd).as_bool() as u8, cloaked);
                        }
                        let mut class = [0u16; 128];
                        let n = GetClassNameW(hwnd, &mut class).max(0) as usize;
                        let mut title = [0u16; 128];
                        let t = GetWindowTextW(hwnd, &mut title).max(0) as usize;
                        let mut r = RECT::default();
                        let _ = GetWindowRect(hwnd, &mut r);
                        println!("{:x}|{}|{}|{},{},{},{}|{:x}", hwnd.0, String::from_utf16_lossy(&class[..n]), String::from_utf16_lossy(&title[..t]), r.left, r.top, r.right, r.bottom, GetWindowLongW(hwnd, GWL_EXSTYLE));
                    }
                    true.into()
                }
                let _ = EnumWindows(Some(each), LPARAM(0));
            }
            Some("cursor") => {
                let mut p = windows::Win32::Foundation::POINT::default();
                let _ = windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut p);
                println!("cursor {},{}", p.x, p.y);
            }
            Some("info") => {
                use windows::Win32::Foundation::RECT;
                use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
                use windows::Win32::UI::WindowsAndMessaging::{GetWindowLongW, GetWindowRect, IsWindowVisible, GWL_EXSTYLE, GWL_STYLE, GetForegroundWindow};
                let board = FindWindowW(w!("flick.board"), None);
                let mut r = RECT::default();
                let _ = GetWindowRect(board, &mut r);
                let mut cloaked = 0u32;
                let _ = DwmGetWindowAttribute(board, DWMWA_CLOAKED, &mut cloaked as *mut _ as *mut _, 4);
                let fg = GetForegroundWindow();
                let mut title = [0u16; 128];
                let n = windows::Win32::UI::WindowsAndMessaging::GetWindowTextW(fg, &mut title).max(0) as usize;
                vdapi::select(false);
                println!("foreground on current desktop: {:?} title: {}", vdapi::is_window_on_current_desktop(fg), String::from_utf16_lossy(&title[..n]).chars().take(30).collect::<String>());
                println!("board {:?} rect {:?} visible {:?} cloaked {} style {:x} ex {:x} fg {:?}", board, r, IsWindowVisible(board), cloaked, GetWindowLongW(board, GWL_STYLE), GetWindowLongW(board, GWL_EXSTYLE), GetForegroundWindow());
            }
            _ => println!("app window: {:?}", app),
        }
    }
}

#[cfg(windows)]
fn key(c: char, up: bool) -> windows::Win32::UI::Input::KeyboardAndMouse::INPUT {
    use windows::Win32::UI::Input::KeyboardAndMouse::*;
    let vk = c.to_ascii_uppercase() as u32;
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk as u16),
                wScan: unsafe { MapVirtualKeyW(vk, MAPVK_VK_TO_VSC) } as u16,
                dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
                ..Default::default()
            },
        },
    }
}

/// Types into a text box of its own and prints what arrived, to check that a
/// key chord used as trigger neither eats nor reorders ordinary typing.
#[cfg(windows)]
fn typetest() {
    use windows::core::w;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows::Win32::UI::Input::KeyboardAndMouse::{SendInput, SetFocus, INPUT};
    use windows::Win32::UI::WindowsAndMessaging::*;
    unsafe {
        let edit = CreateWindowExW(
            WS_EX_TOPMOST, w!("EDIT"), w!(""), WS_OVERLAPPEDWINDOW | WS_VISIBLE, 200, 200, 420, 120, None, None, None, None,
        );
        let other = GetWindowThreadProcessId(GetForegroundWindow(), None);
        let _ = AttachThreadInput(GetCurrentThreadId(), other, true);
        let _ = SetForegroundWindow(edit);
        let _ = SetFocus(edit);
        let _ = AttachThreadInput(GetCurrentThreadId(), other, false);
        let pump = |ms: u64| {
            let end = std::time::Instant::now() + std::time::Duration::from_millis(ms);
            let mut message = MSG::default();
            while std::time::Instant::now() < end {
                while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
        };
        let send = |events: &[INPUT]| {
            SendInput(events, std::mem::size_of::<INPUT>() as i32);
        };
        let text = |edit: HWND| {
            let mut buffer = [0u16; 128];
            let n = GetWindowTextW(edit, &mut buffer).max(0) as usize;
            String::from_utf16_lossy(&buffer[..n])
        };
        pump(300);
        // Ordinary typing, one key at a time.
        for c in "werthewere".chars() {
            send(&[key(c, false)]);
            pump(35);
            send(&[key(c, true)]);
            pump(35);
        }
        pump(200);
        println!("typed one by one: {:?} (expected \"werthewere\")", text(edit));
        // Fast rollover: next key goes down before the previous one is up.
        send(&[key('r', false)]);
        pump(20);
        send(&[key('e', false)]);
        pump(20);
        send(&[key('r', true)]);
        pump(20);
        send(&[key('e', true)]);
        pump(200);
        println!("after rollover re: {:?} (expected \"werthewerere\")", text(edit));
        // The chord: all three at once, held, released.
        send(&[key('w', false), key('e', false), key('r', false)]);
        pump(400);
        send(&[key('w', true), key('e', true), key('r', true)]);
        pump(400);
        println!("after chord: {:?} (expected unchanged)", text(edit));
        let _ = DestroyWindow(edit);
    }
}

#[cfg(not(windows))]
fn main() {}
