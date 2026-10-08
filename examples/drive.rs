//! Test driver: posts the app's own messages to the running instance, sends
//! input and captures the screen, so behaviour can be checked without a person
//! at the mouse.
//!   drive step <0..3> | click | release | settings
//!   drive xbutton <down|up> | mouse <x> <y> [down|up|rdown|rup] | move <dx> <dy>
//!   drive stroke <dx> <dy> <ms> | keys <down|up> <letters> | typetest
//!   drive shot <path> | pixel <x> <y> | wins [all] | cursor | info | idle
//!   drive post <class> <message> <wparam> <lparam> | client <class>
//!   drive key <down|up> <vk hex>... [unseen] | keystate <vk hex>... | levels | front <class|0xHWND> | mem
//!   drive rmdesk <desktop id prefix>
//! The message numbers are the app's `WM_STEP`, `WM_CLICK`, `WM_RELEASE`
//! (src/input.rs) and `WM_OPEN_SETTINGS` (src/app.rs).
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
            Some("settings") => post(WM_APP + 12, 0),
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
            Some("post") => {
                // post <class> <message> <wparam> <lparam>: any message to the first window of a class.
                let class = windows::core::HSTRING::from(args[2].as_str());
                let number = |i: usize| args[i].parse::<isize>().unwrap();
                let window = FindWindowW(&class, None);
                let posted = PostMessageW(window, number(3) as u32, WPARAM(number(4) as usize), LPARAM(number(5)));
                println!("window {:x} {:?}", window.0, posted.map_err(|e| e.message().to_string()));
            }
            Some("client") => {
                // client <class>: where the inside of that window is on the screen, and its size.
                use windows::Win32::Foundation::{POINT, RECT};
                use windows::Win32::Graphics::Gdi::ClientToScreen;
                use windows::Win32::UI::WindowsAndMessaging::{GetClientRect, IsWindowVisible};
                let window = FindWindowW(&windows::core::HSTRING::from(args[2].as_str()), None);
                let (mut origin, mut inside) = (POINT::default(), RECT::default());
                let _ = ClientToScreen(window, &mut origin);
                let _ = GetClientRect(window, &mut inside);
                println!("{} {} {} {} visible {}", origin.x, origin.y, inside.right, inside.bottom, IsWindowVisible(window).as_bool() as u8);
            }
            Some("key") => {
                // key <down|up> <vk hex>...: presses or releases keys by virtual-key code, all at once.
                // A last word "unseen" marks them as the app's own replayed keys, which its
                // keyboard hook lets by: a key event the hook misses.
                let up = args[2] == "up";
                let unseen = args.last().is_some_and(|word| word == "unseen");
                let codes = &args[3..args.len() - unseen as usize];
                let mut events: Vec<_> = codes.iter().map(|vk| key_vk(u32::from_str_radix(vk, 16).unwrap(), up)).collect();
                for event in events.iter_mut().filter(|_| unseen) {
                    event.Anonymous.ki.dwExtraInfo = 0x4B41_4E4B;
                }
                windows::Win32::UI::Input::KeyboardAndMouse::SendInput(&events, std::mem::size_of::<windows::Win32::UI::Input::KeyboardAndMouse::INPUT>() as i32);
            }
            Some("keystate") => {
                // keystate <vk hex>...: whether Windows holds each key or button to be down right now.
                for vk in &args[2..] {
                    let down = windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(i32::from_str_radix(vk, 16).unwrap()) < 0;
                    print!("{vk}:{} ", if down { "down" } else { "up" });
                }
                println!();
            }
            Some("levels") => {
                // Lists the visible windows whose process runs at a higher integrity level than
                // this one (as administrator): input meant for them never reaches the app's hooks.
                use windows::Win32::Foundation::{BOOL, CloseHandle, HANDLE, HWND};
                use windows::Win32::Security::{GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TokenIntegrityLevel, TOKEN_MANDATORY_LABEL, TOKEN_QUERY};
                use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION};
                use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetClassNameW, GetForegroundWindow, GetWindowThreadProcessId, IsWindowVisible};
                unsafe fn level(process: HANDLE) -> Option<u32> {
                    let mut token = HANDLE::default();
                    OpenProcessToken(process, TOKEN_QUERY, &mut token).ok()?;
                    let mut buffer = [0u8; 128];
                    let mut len = 0u32;
                    let got = GetTokenInformation(token, TokenIntegrityLevel, Some(buffer.as_mut_ptr().cast()), buffer.len() as u32, &mut len);
                    let _ = CloseHandle(token);
                    got.ok()?;
                    let sid = (*(buffer.as_ptr() as *const TOKEN_MANDATORY_LABEL)).Label.Sid;
                    Some(*GetSidSubAuthority(sid, (*GetSidSubAuthorityCount(sid) - 1) as u32))
                }
                unsafe fn of_window(hwnd: HWND) -> Option<u32> {
                    let mut pid = 0u32;
                    GetWindowThreadProcessId(hwnd, Some(&mut pid));
                    let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
                    let found = level(process);
                    let _ = CloseHandle(process);
                    found
                }
                unsafe extern "system" fn each(hwnd: HWND, _: LPARAM) -> BOOL {
                    if IsWindowVisible(hwnd).as_bool() {
                        let mut class = [0u16; 64];
                        let n = GetClassNameW(hwnd, &mut class).max(0) as usize;
                        println!("{:x} {:?} {}", hwnd.0, of_window(hwnd), String::from_utf16_lossy(&class[..n]));
                    }
                    true.into()
                }
                println!("this process: {:?}; foreground: {:?}", level(GetCurrentProcess()), of_window(GetForegroundWindow()));
                let _ = EnumWindows(Some(each), LPARAM(0));
            }
            Some("front") => {
                // front <class | 0xHWND>: brings a window to the front; prints the one that was.
                use windows::Win32::Foundation::HWND;
                use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, SetForegroundWindow};
                let window = match args[2].strip_prefix("0x") {
                    Some(hex) => HWND(isize::from_str_radix(hex, 16).unwrap()),
                    None => FindWindowW(&windows::core::HSTRING::from(args[2].as_str()), None),
                };
                let before = GetForegroundWindow();
                // Input of our own just before makes Windows grant the request.
                let nudge = key_vk(0x87, true);
                windows::Win32::UI::Input::KeyboardAndMouse::SendInput(&[nudge], std::mem::size_of_val(&nudge) as i32);
                let done = SetForegroundWindow(window).as_bool();
                println!("0x{:x} {done}", before.0);
            }
            Some("mem") => {
                // mem: what the running app holds. Working set and private bytes in MB, GDI and
                // USER objects, handles and threads.
                use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32};
                use windows::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX};
                use windows::Win32::System::Threading::{GetGuiResources, GetProcessHandleCount, OpenProcess, GR_GDIOBJECTS, GR_USEROBJECTS, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ};
                use windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId;
                let mut pid = 0u32;
                GetWindowThreadProcessId(app, Some(&mut pid));
                let Ok(process) = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, false, pid) else { return println!("no app") };
                let mut memory = PROCESS_MEMORY_COUNTERS_EX::default();
                let _ = K32GetProcessMemoryInfo(process, &mut memory as *mut _ as *mut PROCESS_MEMORY_COUNTERS, std::mem::size_of_val(&memory) as u32);
                let mut handles = 0u32;
                let _ = GetProcessHandleCount(process, &mut handles);
                let mut threads = 0;
                if let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) {
                    let mut entry = THREADENTRY32 { dwSize: std::mem::size_of::<THREADENTRY32>() as u32, ..Default::default() };
                    let mut more = Thread32First(snapshot, &mut entry).is_ok();
                    while more {
                        threads += (entry.th32OwnerProcessID == pid) as u32;
                        more = Thread32Next(snapshot, &mut entry).is_ok();
                    }
                }
                let mb = |bytes: usize| bytes as f64 / 1048576.0;
                println!(
                    "ws {:.1} private {:.1} peak-ws {:.1} gdi {} user {} handles {handles} threads {threads}",
                    mb(memory.WorkingSetSize), mb(memory.PrivateUsage), mb(memory.PeakWorkingSetSize),
                    GetGuiResources(process, GR_GDIOBJECTS), GetGuiResources(process, GR_USEROBJECTS)
                );
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
            Some("rmdesk") => {
                // rmdesk <id prefix>: removes a desktop a test made; its windows go to the current one.
                vdapi::select(false);
                let wanted = args[2].to_ascii_uppercase();
                let (all, current) = (vdapi::get_desktops().unwrap_or_default(), vdapi::get_current_desktop());
                match (all.into_iter().find(|d| d.id().starts_with(&wanted)), current) {
                    (Some(desktop), Ok(current)) if desktop != current => println!("{:?}", vdapi::remove_desktop(desktop, current)),
                    _ => println!("not removed: no such desktop, or it is the current one"),
                }
            }
            _ => println!("app window: {:?}", app),
        }
    }
}

#[cfg(windows)]
fn key(c: char, up: bool) -> windows::Win32::UI::Input::KeyboardAndMouse::INPUT {
    key_vk(c.to_ascii_uppercase() as u32, up)
}

#[cfg(windows)]
fn key_vk(vk: u32, up: bool) -> windows::Win32::UI::Input::KeyboardAndMouse::INPUT {
    use windows::Win32::UI::Input::KeyboardAndMouse::*;
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
        // One chord key held alone for long, then released: it is typing, and
        // must not be left down.
        send(&[key('w', false)]);
        pump(400);
        send(&[key('w', true)]);
        pump(300);
        let stuck = windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState('W' as i32) < 0;
        println!("after w held alone: {:?} (expected \"werthewererew\"), w still down: {stuck}", text(edit));
        // The chord: all three at once, held, released.
        send(&[key('w', false), key('e', false), key('r', false)]);
        pump(400);
        send(&[key('w', true), key('e', true), key('r', true)]);
        pump(400);
        // (Tapping the chord opens the board, which takes the focus.)
        println!("after chord: {:?} (expected unchanged)", text(edit));
        let _ = DestroyWindow(edit);
    }
}

#[cfg(not(windows))]
fn main() {}
