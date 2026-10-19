//! Test driver: posts the app's own messages to the running instance, sends
//! input and captures the screen, so behaviour can be checked without a person
//! at the mouse.
//!   drive step <0..3> | click | release | settings
//!   drive clickprobe
//!   drive boardstress <cycles> <ms> <post|xbutton|chord|winpair> <still|move|hover>
//!   drive xbutton <down|up> | mouse <x> <y> [down|up|rdown|rup] | move <dx> <dy>
//!   drive stroke <dx> <dy> <ms> | keys <down|up> <letters> | typetest
//!   drive shot <path> | pixel <x> <y> | wins [all] | cursor | info | idle
//!   drive post <class> <message> <wparam> <lparam> | client <class>
//!   drive key <down|up> <vk hex>... [unseen] | keystate <vk hex>... | levels | front <class|0xHWND> | mem
//!   drive desk | mkdesk | rmdesk <desktop id prefix>
//!   drive hold <r> <g> <b> <title> <x> <y> <w> <h> <seconds>
//!   drive holds <hold arguments>...  (several windows in one process)
//!   drive navigation shelves <out dir> <flick exe>
//!   drive pinprobe <flick exe> <out dir>
//!   drive boardshot <flick exe> <out dir>
//! The message numbers are the app's `WM_STEP`, `WM_CLICK`, `WM_RELEASE`
//! (src/input.rs) and `WM_OPEN_SETTINGS` (src/app.rs).
// The app is a binary only; its door to the virtual desktops is compiled in
// here as it is, and this driver uses a small part of it.
#[allow(dead_code)]
#[path = "../src/vdapi.rs"]
mod vdapi;

#[cfg(windows)]
#[path = "support/navigation.rs"]
mod drive_navigation;

#[cfg(windows)]
#[path = "support/pacing.rs"]
mod drive_pacing;

#[cfg(windows)]
#[path = "support/shelves.rs"]
mod drive_shelves;

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
            Some("navigation") => drive_navigation::navigation(&args[2], std::path::Path::new(&args[3]), args.get(4).map(String::as_str)),
            Some("hold") => hold(&args[2..]),
            Some("holds") => {
                assert!(!args[2..].is_empty() && args[2..].len() % 9 == 0);
                std::thread::scope(|threads| {
                    for window in args[2..].chunks_exact(9) { threads.spawn(move || hold(window)); }
                });
            },
            Some("pinprobe") => pinprobe(&args[2], &args[3]),
            Some("boardshot") => boardshot(&args[2], &args[3]),
            Some("clickprobe") => boardstress(0, 0, "post", "still"),
            Some("boardstress") => boardstress(args[2].parse().unwrap(), args[3].parse().unwrap(), &args[4], &args[5]),
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
                let _ = windows::Win32::Graphics::Dwm::DwmFlush();
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
            Some("under") if args.len() > 2 => {
                // under <x> <y>...: which window takes a click at each point, and whose it is.
                use windows::Win32::Foundation::{POINT, RECT};
                use windows::Win32::UI::WindowsAndMessaging::{
                    GetAncestor, GetClassNameW, GetWindowRect, GetWindowThreadProcessId, WindowFromPoint, GA_ROOT,
                };
                for pair in args[2..].chunks(2) {
                    let at = POINT { x: pair[0].parse().unwrap(), y: pair[1].parse().unwrap() };
                    let window = WindowFromPoint(at);
                    let root = GetAncestor(window, GA_ROOT);
                    let mut class = [0u16; 128];
                    let n = GetClassNameW(root, &mut class).max(0) as usize;
                    let mut pid = 0;
                    GetWindowThreadProcessId(root, Some(&mut pid));
                    let mut rect = RECT::default();
                    let _ = GetWindowRect(root, &mut rect);
                    println!(
                        "{},{}: 0x{:x} {} pid {pid} ({},{})-({},{})",
                        at.x, at.y, root.0, String::from_utf16_lossy(&class[..n]),
                        rect.left, rect.top, rect.right, rect.bottom
                    );
                }
            }
            Some("shell") => {
                // shell [show]: the shell's notice windows Flick minimizes on a switch, and their
                // state. "show" first shows them with no notice in them, as the shell leaves them stuck.
                use windows::Win32::Foundation::{HWND, RECT};
                use windows::Win32::UI::WindowsAndMessaging::{
                    FindWindowExW, GetWindowLongW, GetWindowRect, IsIconic, IsWindowVisible, ShowWindow, GWL_EXSTYLE,
                    SW_SHOWNOACTIVATE,
                };
                let mut host = HWND(0);
                loop {
                    host = FindWindowExW(None, host, w!("XamlExplorerHostIslandWindow"), w!(""));
                    if host.0 == 0 {
                        break;
                    }
                    let bridge = FindWindowExW(
                        host, None, w!("Windows.UI.Composition.DesktopWindowContentBridge"), w!("DesktopWindowXamlSource"),
                    );
                    if bridge.0 != 0 && args.get(2).is_some_and(|word| word == "show") {
                        ShowWindow(host, SW_SHOWNOACTIVATE);
                    }
                    let mut rect = RECT::default();
                    let _ = GetWindowRect(host, &mut rect);
                    println!(
                        "0x{:x} bridge {} visible {} minimized {} ex 0x{:x} ({},{})-({},{})",
                        host.0, bridge.0 != 0, IsWindowVisible(host).as_bool(), IsIconic(host).as_bool(),
                        GetWindowLongW(host, GWL_EXSTYLE), rect.left, rect.top, rect.right, rect.bottom
                    );
                }
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
            Some("under") => {
                // under: the window under the cursor, and who has the keyboard and the mouse capture.
                use windows::Win32::Foundation::{HWND, POINT};
                use windows::Win32::UI::WindowsAndMessaging::{GetClassNameW, GetCursorPos, GetGUIThreadInfo, WindowFromPoint, GUITHREADINFO};
                let class = |h: HWND| {
                    let mut c = [0u16; 64];
                    let n = GetClassNameW(h, &mut c).max(0) as usize;
                    format!("{:x}:{}", h.0, String::from_utf16_lossy(&c[..n]))
                };
                let mut p = POINT::default();
                let _ = GetCursorPos(&mut p);
                let mut g = GUITHREADINFO { cbSize: std::mem::size_of::<GUITHREADINFO>() as u32, ..Default::default() };
                let _ = GetGUIThreadInfo(0, &mut g);
                println!("at {},{} under {} active {} focus {} capture {} flags {:x}", p.x, p.y, class(WindowFromPoint(p)), class(g.hwndActive), class(g.hwndFocus), class(g.hwndCapture), g.flags.0);
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
            Some("desk") => {
                // desk: prints the current desktop's id.
                vdapi::select(false);
                println!("desk {:?}", vdapi::get_current_desktop().map(|d| d.id()));
            }
            Some("mkdesk") => {
                // mkdesk: creates a desktop for a test and prints its id.
                vdapi::select(false);
                println!("{:?}", vdapi::create_desktop().map(|d| d.id()));
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

/// Exercises the live windows and hooks; the click probe verifies delivery,
/// rather than assuming a window found under the cursor receives the click.
#[cfg(windows)]
fn boardstress(cycles: usize, millis: u64, mode: &str, cursor: &str) {
    use std::{sync::atomic::{AtomicUsize, Ordering}, time::{Duration, Instant}};
    use windows::{core::w, Win32::{
        Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM},
        System::{Com::{CoInitializeEx, COINIT_MULTITHREADED}, LibraryLoader::GetModuleHandleW},
        UI::{Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
    }};
    static CLICKS: AtomicUsize = AtomicUsize::new(0);
    unsafe extern "system" fn probe(hwnd: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        if message == WM_LBUTTONDOWN { CLICKS.fetch_add(1, Ordering::Relaxed); }
        DefWindowProcW(hwnd, message, w, l)
    }
    assert!(cycles <= 10_000 && millis <= 1000);
    assert!(["post", "xbutton", "chord", "winpair"].contains(&mode));
    assert!(["still", "move", "hover"].contains(&cursor));
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        assert!(vdapi::select(false).is_some());
        let app = FindWindowW(w!("flick.main"), None);
        assert!(cycles == 0 || app.0 != 0, "Flick is not running");
        let class = w!("flick.test.clicks");
        let instance = GetModuleHandleW(None).unwrap();
        RegisterClassW(&WNDCLASSW { lpfnWndProc: Some(probe), hInstance: instance.into(), lpszClassName: class, ..Default::default() });
        let (width, height) = (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN));
        let make = || CreateWindowExW(WINDOW_EX_STYLE(0), class, w!("Flick click probe"), WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            width / 2 - 200, height / 2 - 150, 400, 300, None, None, instance, None);
        let target = make();
        let other = make();
        assert!(target.0 != 0 && other.0 != 0);
        let current = vdapi::get_current_desktop().unwrap();
        let desktop = (cycles > 0).then(|| vdapi::create_desktop().unwrap());
        if let Some(desktop) = desktop {
            let moved = vdapi::move_window_to_desktop(desktop, other);
            println!("temporary desktop {} thumbnail source {:x}: {moved:?}", desktop.id(), other.0);
        }
        let pump = |millis| {
            let until = Instant::now() + Duration::from_millis(millis);
            let mut message = MSG::default();
            while Instant::now() < until {
                while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        };
        SetForegroundWindow(target);
        pump(500);
        let board = FindWindowW(w!("flick.board"), None);
        let popup = FindWindowW(w!("flick.board.preview"), None);
        let send = |events: &[INPUT]| assert_eq!(SendInput(events, std::mem::size_of::<INPUT>() as i32), events.len() as u32);
        let tap = || match mode {
            "post" => { PostMessageW(app, WM_APP + 2, WPARAM(0), LPARAM(0)).unwrap(); }
            "chord" | "winpair" => {
                let codes: &[u32] = if mode == "winpair" { &[0x5B, 0x7C] } else { &[0x57, 0x45, 0x52] };
                let events: Vec<_> = [false, true].into_iter().flat_map(|up| codes.iter().map(move |vk| key_vk(*vk, up))).collect();
                send(&events);
            }
            _ => {
                let events: Vec<_> = [MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP].into_iter().map(|flags| INPUT {
                    r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { mouseData: 2, dwFlags: flags, ..Default::default() } },
                }).collect();
                send(&events);
            }
        };
        let (mut open, mut closed, mut popups, mut completed) = (0, 0, 0, 0);
        for cycle in 0..cycles {
            if !IsWindow(app).as_bool() { println!("CRASH before cycle {cycle}"); break; }
            for half in 0..2 {
                tap();
                if cursor != "still" {
                    let x = width / 3 + ((cycle * 2 + half) % 9) as i32 * width / 24;
                    let y = if cursor == "hover" { height - 70 - (cycle % 5) as i32 * 35 } else { height / 3 + (cycle % 5) as i32 * 50 };
                    let _ = SetCursorPos(x, y);
                }
                pump(millis);
                if IsWindowVisible(board).as_bool() { open += 1; } else { closed += 1; }
                if IsWindowVisible(popup).as_bool() { popups += 1; }
            }
            completed += 1;
            if completed % 25 == 0 { println!("cycle {completed}/{cycles}: open {open} closed {closed} popup {popups}"); }
        }
        let alive = cycles == 0 || IsWindow(app).as_bool();
        if cycles > 0 {
            let _ = PostMessageW(board, WM_KEYDOWN, WPARAM(VK_ESCAPE.0 as usize), LPARAM(0));
        }
        pump(350);
        SetForegroundWindow(target);
        pump(150);
        for (x, y) in [(width / 2, height / 2), (width / 2, height - 180), (150, 150)] {
            let _ = SetWindowPos(target, HWND_TOP, x - 100, y - 80, 300, 200, SWP_SHOWWINDOW);
            pump(100);
            let _ = SetCursorPos(x, y);
            let under = WindowFromPoint(POINT { x, y });
            let mut name = [0u16; 128];
            let n = GetClassNameW(under, &mut name).max(0) as usize;
            let before = CLICKS.load(Ordering::Relaxed);
            let buttons = if GetSystemMetrics(SM_SWAPBUTTON) != 0 { [MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP] }
                else { [MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP] };
            let events: Vec<_> = buttons.into_iter().map(|flags| INPUT {
                r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { dwFlags: flags, ..Default::default() } },
            }).collect();
            send(&events);
            pump(150);
            println!("click probe {x},{y}: under {:x}:{} delivered {}", under.0, String::from_utf16_lossy(&name[..n]), CLICKS.load(Ordering::Relaxed) - before);
        }
        println!("RESULT cycles {completed}/{cycles} mode {mode} interval {millis} cursor {cursor} alive {alive} board_visible {} popup_visible {} clicks {}/3",
            IsWindowVisible(board).as_bool(), IsWindowVisible(popup).as_bool(), CLICKS.load(Ordering::Relaxed));
        let _ = DestroyWindow(other);
        let _ = DestroyWindow(target);
        if let Some(desktop) = desktop {
            println!("remove temporary desktop: {:?}", vdapi::remove_desktop(desktop, current));
        }
        assert!(alive && completed == cycles && CLICKS.load(Ordering::Relaxed) == 3, "stress or click delivery failed");
    }
}

/// A plain window of one colour, kept up for a while: something whose live
/// preview can be found on the screen by its colour.
#[cfg(windows)]
fn hold(args: &[String]) {
    use windows::core::{w, HSTRING};
    use windows::Win32::{Foundation::COLORREF, Graphics::Gdi::CreateSolidBrush, System::LibraryLoader::GetModuleHandleW, UI::WindowsAndMessaging::*};
    let n = |i: usize| args[i].parse::<i32>().unwrap();
    unsafe extern "system" fn plain(hwnd: windows::Win32::Foundation::HWND, message: u32, w: windows::Win32::Foundation::WPARAM, l: windows::Win32::Foundation::LPARAM) -> windows::Win32::Foundation::LRESULT {
        DefWindowProcW(hwnd, message, w, l)
    }
    unsafe {
        let instance = GetModuleHandleW(None).unwrap();
        let class = w!("flick.test.hold");
        RegisterClassW(&WNDCLASSW {
            lpfnWndProc: Some(plain),
            hInstance: instance.into(),
            lpszClassName: class,
            hbrBackground: CreateSolidBrush(COLORREF((n(0) | n(1) << 8 | n(2) << 16) as u32)),
            ..Default::default()
        });
        let window = CreateWindowExW(WINDOW_EX_STYLE(0), class, &HSTRING::from(args[3].as_str()), WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            n(4), n(5), n(6), n(7), None, None, instance, None);
        assert!(window.0 != 0);
        let until = std::time::Instant::now() + std::time::Duration::from_secs(n(8) as u64);
        let mut message = MSG::default();
        while std::time::Instant::now() < until && IsWindow(window).as_bool() {
            while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}

/// Opens the board over windows pinned to every desktop and to their row,
/// moves the selection about, and records, for every screen capture it can
/// take meanwhile, where the preview of each kind of window is (found by its
/// colour): `pinprobe.csv` in `out`, with a few full captures and the
/// app's debug log. The grid, the desktops and the pins are as they were
/// afterwards.
#[cfg(windows)]
fn pinprobe(flick: &str, out: &str) {
    use std::time::{Duration, Instant};
    use windows::core::{w, HSTRING};
    use windows::Win32::{
        Foundation::{HWND, LPARAM, WPARAM},
        Graphics::Gdi::*,
        System::Com::{CoInitializeEx, COINIT_MULTITHREADED},
        UI::WindowsAndMessaging::*,
    };
    let out = std::path::PathBuf::from(out);
    std::fs::create_dir_all(&out).unwrap();
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        assert!(vdapi::select(false).is_some());
        if FindWindowW(w!("flick.main"), None).0 != 0 {
            println!("RESULT another Flick is running; not probing");
            return;
        }
        let home = vdapi::get_current_desktop().unwrap();
        let made: Vec<_> = (0..4).map(|_| vdapi::create_desktop().unwrap()).collect();
        let me = std::env::current_exe().unwrap();
        let row_exe = out.join("probe-row.exe");
        std::fs::copy(&me, &row_exe).unwrap();
        let (sw, sh) = (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN));
        let spawn = |exe: &std::path::Path, rgb: &str, title: &str, x: i32, y: i32, w: i32, h: i32| {
            let mut args: Vec<String> = vec!["hold".into()];
            args.extend(rgb.split(' ').map(String::from));
            args.extend([title.to_owned(), x.to_string(), y.to_string(), w.to_string(), h.to_string(), "150".into()]);
            std::process::Command::new(exe).args(args).spawn().unwrap()
        };
        let mut holds = vec![
            spawn(&me, "255 0 255", "AllProbe", sw / 10, sh / 10, sw * 2 / 5, sh * 2 / 5),
            spawn(&row_exe, "0 255 255", "RowProbe", sw / 5, sh / 5, sw * 2 / 5, sh * 2 / 5),
            spawn(&me, "255 255 0", "CellProbe", sw / 3, sh / 4, sw / 2, sh / 2),
            spawn(&me, "0 255 0", "NextProbe", sw / 4, sh / 4, sw / 3, sh / 3),
        ];
        let find = |title: &str| {
            for _ in 0..200 {
                let window = FindWindowW(w!("flick.test.hold"), &HSTRING::from(title));
                if window.0 != 0 {
                    return window;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            panic!("{title} did not come up");
        };
        let (all, next) = (find("AllProbe"), find("NextProbe"));
        let _ = (find("RowProbe"), find("CellProbe"));
        println!("pin AllProbe: {:?}, NextProbe to the next cell: {:?}", vdapi::pin_window(all), vdapi::move_window_to_desktop(made[0], next));

        // The grid: this desktop and two made ones in a row, two more below.
        let dir = std::path::PathBuf::from(std::env::var_os("APPDATA").unwrap()).join("Flick");
        std::fs::create_dir_all(&dir).unwrap();
        let state = dir.join("state.json");
        let saved = std::fs::read(&state).ok();
        let others: Vec<String> = vdapi::get_desktops().unwrap().into_iter().map(|d| d.id())
            .filter(|id| *id != home.id() && !made.iter().any(|d| d.id() == *id)).collect();
        let mut first = vec![home.id(), made[0].id(), made[1].id()];
        first.extend(others);
        let grid = serde_json::json!({
            "rows": [{"name": "Probe", "cells": first, "last": null}, {"name": "Below", "cells": [made[2].id(), made[3].id()], "last": null}],
            "follow_apps": ["probe-row.exe"],
            "guide_seen": true,
        });
        std::fs::write(&state, serde_json::to_string_pretty(&grid).unwrap()).unwrap();
        let _ = std::fs::remove_file(dir.join("debug-log.txt"));

        let mut app = std::process::Command::new(flick).arg("--portable").spawn().unwrap();
        let mut main = HWND(0);
        for _ in 0..200 {
            main = FindWindowW(w!("flick.main"), None);
            if main.0 != 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        std::thread::sleep(Duration::from_millis(2500));
        let _ = PostMessageW(main, WM_APP + 2, WPARAM(0), LPARAM(0));
        std::thread::sleep(Duration::from_millis(1500));
        let board = FindWindowW(w!("flick.board"), None);
        println!("flick {:x} board {:x} visible {}", main.0, board.0, IsWindowVisible(board).as_bool());

        // Captures, and the box of each colour in them.
        let (x0, y0) = (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN));
        let (w, h) = (GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN));
        let screen = GetDC(None);
        let dc = CreateCompatibleDC(screen);
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: -h, biPlanes: 1, biBitCount: 32, ..Default::default() },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let bitmap = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0).unwrap();
        SelectObject(dc, bitmap);
        let pixels = std::slice::from_raw_parts(bits as *const u8, (w * h * 4) as usize);
        let colours: [(&str, fn(u8, u8, u8) -> bool); 4] = [
            ("all", |r, g, b| r > 235 && g < 20 && b > 235),
            ("row", |r, g, b| r < 20 && g > 235 && b > 235),
            ("cell", |r, g, b| r > 235 && g > 235 && b < 20),
            ("next", |r, g, b| r < 20 && g > 235 && b < 20),
        ];
        let mut csv = String::from("label,frame,t_ms,capture_ms");
        for (name, _) in &colours {
            csv += &format!(",{name}_l,{name}_t,{name}_r,{name}_b,{name}_n");
        }
        csv += "\n";
        let plan: [(&str, &[(u64, usize)], u64); 7] = [
            ("right", &[(0, 1)], 700),
            ("right2", &[(0, 1)], 700),
            ("left", &[(0, 0)], 700),
            ("left2", &[(0, 0)], 700),
            ("down", &[(0, 3)], 700),
            ("up", &[(0, 2)], 700),
            ("rapid", &[(0, 1), (120, 1), (240, 0)], 900),
        ];
        for (label, steps, ms) in plan {
            let started = Instant::now();
            let mut posted = 0;
            let mut frame = 0;
            let mut saved_shot = false;
            while started.elapsed() < Duration::from_millis(ms) {
                while posted < steps.len() && started.elapsed() >= Duration::from_millis(steps[posted].0) {
                    let _ = PostMessageW(main, WM_APP + 1, WPARAM(steps[posted].1), LPARAM(0));
                    posted += 1;
                }
                let t = started.elapsed();
                let _ = BitBlt(dc, 0, 0, w, h, screen, x0, y0, SRCCOPY);
                let capture = started.elapsed() - t;
                let mut boxes = [(i32::MAX, i32::MAX, -1, -1, 0u32); 4];
                for y in 0..h {
                    let line = &pixels[(y * w * 4) as usize..((y + 1) * w * 4) as usize];
                    for x in 0..w {
                        let p = &line[(x * 4) as usize..(x * 4 + 3) as usize];
                        for (i, (_, is)) in colours.iter().enumerate() {
                            if is(p[2], p[1], p[0]) {
                                let b = &mut boxes[i];
                                *b = (b.0.min(x), b.1.min(y), b.2.max(x + 1), b.3.max(y + 1), b.4 + 1);
                            }
                        }
                    }
                }
                csv += &format!("{label},{frame},{:.1},{:.1}", t.as_secs_f64() * 1000.0, capture.as_secs_f64() * 1000.0);
                for b in boxes {
                    csv += &if b.4 == 0 { ",,,,,0".to_owned() } else { format!(",{},{},{},{},{}", b.0, b.1, b.2, b.3, b.4) };
                }
                csv += "\n";
                if !saved_shot && (label == "right" || label == "down") && t >= Duration::from_millis(150) || frame == 0 && label == "right" {
                    saved_shot = frame != 0;
                    let mut raw = Vec::with_capacity(pixels.len() + 8);
                    raw.extend_from_slice(&w.to_le_bytes());
                    raw.extend_from_slice(&h.to_le_bytes());
                    raw.extend_from_slice(pixels);
                    std::fs::write(out.join(format!("{label}-{frame}-{}ms.raw", t.as_millis())), raw).unwrap();
                }
                frame += 1;
            }
            println!("{label}: {frame} captures");
        }
        std::fs::write(out.join("pinprobe.csv"), csv).unwrap();

        // Everything back as it was.
        let _ = app.kill();
        let _ = app.wait();
        std::thread::sleep(Duration::from_millis(300));
        let _ = vdapi::switch_desktop(home);
        println!("unpin: {:?}", vdapi::unpin_window(all));
        for hold in &mut holds {
            let _ = hold.kill();
        }
        for desktop in made {
            let _ = vdapi::remove_desktop(desktop, home);
        }
        let _ = std::fs::copy(dir.join("debug-log.txt"), out.join("debug-log.txt"));
        match saved {
            Some(saved) => std::fs::write(&state, saved).unwrap(),
            None => { let _ = std::fs::remove_file(&state); }
        }
        println!("RESULT probe done");
    }
}

/// Opens the board over a window pinned to every desktop, one pinned to
/// its row and two of the cell's own, and captures the screen: as it comes
/// up (its hints shown, the first time), resting on a preview, and pointing
/// at a cell of the map. Puts everything back as it was.
#[cfg(windows)]
fn boardshot(flick: &str, out: &str) {
    use std::process::{Command, Stdio};
    use std::time::Duration;
    use windows::core::{w, HSTRING};
    use windows::Win32::{
        Foundation::{HWND, LPARAM, WPARAM},
        Graphics::Gdi::*,
        System::Com::{CoInitializeEx, COINIT_MULTITHREADED},
        UI::WindowsAndMessaging::*,
    };
    let out = std::path::PathBuf::from(out);
    std::fs::create_dir_all(&out).unwrap();
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        assert!(vdapi::select(false).is_some());
        if FindWindowW(w!("flick.main"), None).0 != 0 {
            println!("RESULT another Flick is running; not capturing");
            return;
        }
        let home = vdapi::get_current_desktop().unwrap();
        let made: Vec<_> = (0..3).map(|_| vdapi::create_desktop().unwrap()).collect();
        let me = std::env::current_exe().unwrap();
        let row_exe = out.join("shot-row.exe");
        std::fs::copy(&me, &row_exe).unwrap();
        let (sw, sh) = (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN));
        let spawn = |exe: &std::path::Path, rgb: &str, title: &str, x: i32, y: i32, w: i32, h: i32| {
            let mut args: Vec<String> = vec!["hold".into()];
            args.extend(rgb.split(' ').map(String::from));
            args.extend([title.to_owned(), x.to_string(), y.to_string(), w.to_string(), h.to_string(), "120".into()]);
            Command::new(exe).args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap()
        };
        let mut holds = vec![
            spawn(&me, "70 90 140", "Notes everywhere", sw / 10, sh / 10, sw * 2 / 5, sh * 2 / 5),
            spawn(&row_exe, "40 120 110", "Shelf chat", sw / 5, sh / 5, sw * 2 / 5, sh * 2 / 5),
            spawn(&me, "150 110 60", "Report draft", sw / 3, sh / 4, sw / 2, sh / 2),
            spawn(&me, "90 60 120", "Reference", sw / 4, sh / 3, sw / 3, sh / 2),
            spawn(&me, "60 130 70", "Next door", sw / 4, sh / 4, sw / 3, sh / 3),
        ];
        let find = |title: &str| {
            for _ in 0..200 {
                let window = FindWindowW(w!("flick.test.hold"), &HSTRING::from(title));
                if window.0 != 0 {
                    return window;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            panic!("{title} did not come up");
        };
        let (all, next) = (find("Notes everywhere"), find("Next door"));
        let _ = (find("Shelf chat"), find("Report draft"), find("Reference"));
        println!("pin: {:?}, move: {:?}", vdapi::pin_window(all), vdapi::move_window_to_desktop(made[0], next));

        let dir = std::path::PathBuf::from(std::env::var_os("APPDATA").unwrap()).join("Flick");
        std::fs::create_dir_all(&dir).unwrap();
        let state = dir.join("state.json");
        let saved = std::fs::read(&state).ok();
        let others: Vec<String> = vdapi::get_desktops().unwrap().into_iter().map(|d| d.id())
            .filter(|id| *id != home.id() && !made.iter().any(|d| d.id() == *id)).collect();
        let mut first = vec![home.id(), made[0].id()];
        first.extend(others);
        let grid = serde_json::json!({
            "rows": [{"name": "Work", "cells": first, "last": null}, {"name": "Later", "cells": [made[1].id(), made[2].id()], "last": null}],
            "follow_apps": ["shot-row.exe"],
            "guide_seen": true,
        });
        std::fs::write(&state, serde_json::to_string_pretty(&grid).unwrap()).unwrap();

        let mut app = Command::new(flick).arg("--portable").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
        let mut main = HWND(0);
        for _ in 0..200 {
            main = FindWindowW(w!("flick.main"), None);
            if main.0 != 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        std::thread::sleep(Duration::from_millis(2500));
        let _ = SetCursorPos(sw - 4, sh / 2);
        let _ = PostMessageW(main, WM_APP + 2, WPARAM(0), LPARAM(0));
        std::thread::sleep(Duration::from_millis(1500));
        let board = FindWindowW(w!("flick.board"), None);
        println!("flick {:x} board {:x} visible {}", main.0, board.0, IsWindowVisible(board).as_bool());

        let (x0, y0) = (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN));
        let (w, h) = (GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN));
        let screen = GetDC(None);
        let dc = CreateCompatibleDC(screen);
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: -h, biPlanes: 1, biBitCount: 32, ..Default::default() },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let bitmap = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0).unwrap();
        SelectObject(dc, bitmap);
        let pixels = std::slice::from_raw_parts(bits as *const u8, (w * h * 4) as usize);
        let shot = |name: &str| {
            let _ = BitBlt(dc, 0, 0, w, h, screen, x0, y0, SRCCOPY);
            let mut raw = Vec::with_capacity(pixels.len() + 8);
            raw.extend_from_slice(&w.to_le_bytes());
            raw.extend_from_slice(&h.to_le_bytes());
            raw.extend_from_slice(pixels);
            std::fs::write(out.join(format!("{name}.raw")), raw).unwrap();
        };
        shot("board");
        // Resting on the cell's own windows, right of the pinned places.
        let _ = SetCursorPos(sw * 3 / 5, sh / 4);
        std::thread::sleep(Duration::from_millis(1500));
        shot("resting");
        // Across the bottom of the map, where its cells are.
        for x in (sw / 3..sw * 2 / 3).step_by((sw / 40).max(1) as usize) {
            let _ = SetCursorPos(x, sh * 4 / 5);
            std::thread::sleep(Duration::from_millis(40));
        }
        // Where the cells are depends on the screen: a few places on it.
        for (i, (fx, fy)) in [(0.47, 0.80), (0.56, 0.80), (0.47, 0.87), (0.56, 0.87)].into_iter().enumerate() {
            let _ = SetCursorPos((sw as f32 * fx) as i32, (sh as f32 * fy) as i32);
            std::thread::sleep(Duration::from_millis(500));
            shot(&format!("map-{i}"));
        }

        // Everything back as it was.
        let _ = app.kill();
        let _ = app.wait();
        std::thread::sleep(Duration::from_millis(300));
        let _ = vdapi::switch_desktop(home);
        println!("unpin: {:?}", vdapi::unpin_window(all));
        for hold in &mut holds {
            let _ = hold.kill();
            let _ = hold.wait();
        }
        for desktop in made {
            let _ = vdapi::remove_desktop(desktop, home);
        }
        let _ = std::fs::remove_file(&row_exe);
        match saved {
            Some(saved) => std::fs::write(&state, saved).unwrap(),
            None => { let _ = std::fs::remove_file(&state); }
        }
        println!("RESULT shots done");
    }
}

#[cfg(not(windows))]
fn main() {}
