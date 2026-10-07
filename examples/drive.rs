//! Test driver: posts the app's own messages to the running instance and
//! captures the screen, so behaviour can be checked without a person at the mouse.
//!   drive step <0..3> | click | release | shot <path>
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
        let app = FindWindowW(w!("kankan.main"), None);
        let post = |message: u32, wparam: usize| {
            let _ = PostMessageW(app, message, WPARAM(wparam), LPARAM(0));
        };
        match args.get(1).map(String::as_str) {
            Some("step") => post(WM_APP + 1, args[2].parse().unwrap()),
            Some("click") => post(WM_APP + 2, 0),
            Some("release") => post(WM_APP + 3, 0),
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
            Some("info") => {
                use windows::Win32::Foundation::RECT;
                use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
                use windows::Win32::UI::WindowsAndMessaging::{GetWindowLongW, GetWindowRect, IsWindowVisible, GWL_EXSTYLE, GWL_STYLE, GetForegroundWindow};
                let board = FindWindowW(w!("kankan.board"), None);
                let mut r = RECT::default();
                let _ = GetWindowRect(board, &mut r);
                let mut cloaked = 0u32;
                let _ = DwmGetWindowAttribute(board, DWMWA_CLOAKED, &mut cloaked as *mut _ as *mut _, 4);
                let fg = GetForegroundWindow();
                let mut title = [0u16; 128];
                let n = windows::Win32::UI::WindowsAndMessaging::GetWindowTextW(fg, &mut title).max(0) as usize;
                println!("foreground on current desktop: {:?} title: {}", winvd::is_window_on_current_desktop(fg), String::from_utf16_lossy(&title[..n]).chars().take(30).collect::<String>());
                println!("board {:?} rect {:?} visible {:?} cloaked {} style {:x} ex {:x} fg {:?}", board, r, IsWindowVisible(board), cloaked, GetWindowLongW(board, GWL_STYLE), GetWindowLongW(board, GWL_EXSTYLE), GetForegroundWindow());
            }
            _ => println!("app window: {:?}", app),
        }
    }
}

#[cfg(not(windows))]
fn main() {}
