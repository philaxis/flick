//! rawprobe <seconds>: counts raw mouse button events (any window in front),
//! to see whether they still come while a window run as administrator has
//! the keyboard, where the low-level hook sees nothing.
use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::*;
use windows::Win32::UI::WindowsAndMessaging::*;

static mut DOWN: u32 = 0;
static mut UP: u32 = 0;

unsafe extern "system" fn proc_(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_INPUT {
        let mut raw = RAWINPUT::default();
        let mut size = std::mem::size_of::<RAWINPUT>() as u32;
        if GetRawInputData(HRAWINPUT(lparam.0), RID_INPUT, Some(&mut raw as *mut _ as *mut _), &mut size, std::mem::size_of::<RAWINPUTHEADER>() as u32) != u32::MAX
            && raw.header.dwType == RIM_TYPEMOUSE.0
        {
            let flags = raw.data.mouse.Anonymous.Anonymous.usButtonFlags as u32;
            // 0x100 / 0x200: the fifth button (forward) down / up.
            if flags & 0x100 != 0 { DOWN += 1; }
            if flags & 0x200 != 0 { UP += 1; }
        }
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

fn main() {
    let secs: u64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(10);
    unsafe {
        let class = WNDCLASSW { lpfnWndProc: Some(proc_), lpszClassName: w!("rawprobe"), ..Default::default() };
        RegisterClassW(&class);
        let hwnd = CreateWindowExW(Default::default(), w!("rawprobe"), w!(""), Default::default(), 0, 0, 0, 0, HWND_MESSAGE, None, None, None);
        let dev = [RAWINPUTDEVICE { usUsagePage: 1, usUsage: 2, dwFlags: RIDEV_INPUTSINK, hwndTarget: hwnd }];
        RegisterRawInputDevices(&dev, std::mem::size_of::<RAWINPUTDEVICE>() as u32).unwrap();
        SetTimer(hwnd, 1, (secs * 1000) as u32, None);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if msg.message == WM_TIMER { break; }
            DispatchMessageW(&msg);
        }
        println!("forward button down {} up {}", DOWN, UP);
    }
}
