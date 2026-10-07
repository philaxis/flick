//! Thin layer over `winvd` plus the window bookkeeping the grid needs.

use std::collections::{HashMap, HashSet};
use windows::core::{w, HSTRING, PWSTR};
use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
use windows::Win32::{
    Foundation::{CloseHandle, BOOL, HWND, LPARAM, RECT, TRUE},
    Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS},
    Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST},
    System::{
        Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
        },
        ProcessStatus::{K32EmptyWorkingSet, K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS},
        Threading::{
            AttachThreadInput, GetCurrentThreadId, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
            PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SET_QUOTA,
        },
    },
    UI::{
        Input::KeyboardAndMouse::SetFocus,
        WindowsAndMessaging::{
            EnumWindows, GetForegroundWindow, GetShellWindow, GetWindow, GetWindowPlacement, SetWindowPlacement,
            ShowWindow, SW_SHOWMAXIMIZED, SW_SHOWNOACTIVATE, WINDOWPLACEMENT, GetWindowLongW,
            GetWindowRect, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsIconic,
            IsWindowVisible, SetForegroundWindow, GWL_EXSTYLE, GW_OWNER, WS_EX_TOOLWINDOW,
        },
    },
};
use winvd::Desktop;

pub fn id_of(desktop: &Desktop) -> Option<String> {
    desktop.get_id().ok().map(|guid| format!("{guid:?}"))
}

/// Snapshot of the desktops that exist right now, in Windows' own order.
pub struct Desktops {
    pub list: Vec<(String, Desktop)>,
    pub current: String,
}

impl Desktops {
    pub fn read() -> Option<Self> {
        // Keyed by GUID only: a handle that also carries the index goes stale
        // as soon as a desktop is moved or removed.
        let list = winvd::get_desktops()
            .ok()?
            .into_iter()
            .filter_map(|d| {
                let guid = d.get_id().ok()?;
                Some((format!("{guid:?}"), Desktop::from(guid)))
            })
            .collect();
        let current = id_of(&winvd::get_current_desktop().ok()?)?;
        Some(Desktops { list, current })
    }

    pub fn ids(&self) -> Vec<String> {
        self.list.iter().map(|(id, _)| id.clone()).collect()
    }

    pub fn get(&self, id: &str) -> Option<Desktop> {
        self.list.iter().find(|(i, _)| i == id).map(|(_, d)| *d)
    }
}

/// A window the user would think of as "a window": visible, unowned, titled,
/// and not a tool window (which also excludes our own overlay).
pub fn is_app_window(hwnd: HWND) -> bool {
    unsafe {
        hwnd.0 != 0
            && IsWindowVisible(hwnd).as_bool()
            && GetWindow(hwnd, GW_OWNER).0 == 0
            && GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 == 0
            && GetWindowTextLengthW(hwnd) > 0
            && hwnd != GetShellWindow()
    }
}

/// App windows in Z order, topmost first.
pub fn app_windows() -> Vec<HWND> {
    unsafe extern "system" fn each(hwnd: HWND, out: LPARAM) -> BOOL {
        if is_app_window(hwnd) {
            (*(out.0 as *mut Vec<HWND>)).push(hwnd);
        }
        TRUE
    }
    let mut out: Vec<HWND> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&mut out as *mut _ as isize));
    }
    out
}

pub fn count_on(desktop: Desktop) -> usize {
    app_windows()
        .into_iter()
        .filter(|hwnd| winvd::is_window_on_desktop(desktop, *hwnd).unwrap_or(false))
        .count()
}

/// `SetForegroundWindow` is refused for background processes unless we share
/// input state with the thread that currently owns the foreground.
pub fn force_foreground(hwnd: HWND) {
    unsafe {
        let me = GetCurrentThreadId();
        let other = GetWindowThreadProcessId(GetForegroundWindow(), None);
        let attach = other != 0 && other != me;
        if attach {
            let _ = AttachThreadInput(me, other, true);
        }
        let _ = SetForegroundWindow(hwnd);
        if attach {
            let _ = SetFocus(hwnd);
            let _ = AttachThreadInput(me, other, false);
        }
    }
}

/// A programmatic desktop switch leaves keyboard focus on a window of the
/// desktop that was just left. Hand it to the top window of the new desktop,
/// or to the shell when that desktop is empty.
pub fn focus_top_window() {
    let top = app_windows().into_iter().find(|hwnd| unsafe {
        !IsIconic(*hwnd).as_bool() && winvd::is_window_on_current_desktop(*hwnd).unwrap_or(false)
    });
    force_foreground(top.unwrap_or_else(|| unsafe { GetShellWindow() }));
}

fn pinned_everywhere(hwnd: HWND) -> bool {
    winvd::is_pinned_window(hwnd).unwrap_or(false) || winvd::is_pinned_app(hwnd).unwrap_or(false)
}

fn describe(hwnd: HWND, desktop: String) -> WindowInfo {
    let minimized = unsafe { IsIconic(hwnd) }.as_bool();
    let rect = if minimized {
        let mut placement = WINDOWPLACEMENT { length: std::mem::size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
        let _ = unsafe { GetWindowPlacement(hwnd, &mut placement) };
        placement.rcNormalPosition
    } else {
        dwm_attribute::<RECT>(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS).unwrap_or_else(|| {
            let mut rect = RECT::default();
            let _ = unsafe { GetWindowRect(hwnd, &mut rect) };
            rect
        })
    };
    let mut buffer = [0u16; 256];
    let len = unsafe { GetWindowTextW(hwnd, &mut buffer) }.max(0) as usize;
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    WindowInfo { hwnd, desktop, rect, minimized, title: String::from_utf16_lossy(&buffer[..len]), pid }
}

/// App windows shown on every desktop (pinned themselves or through their app).
pub fn pinned_windows() -> Vec<WindowInfo> {
    app_windows()
        .into_iter()
        .filter(|hwnd| pinned_everywhere(*hwnd) && dwm_attribute::<u32>(*hwnd, DWMWA_CLOAKED).unwrap_or(0) == 0)
        .map(|hwnd| describe(hwnd, String::new()))
        .collect()
}

/// Full path of a process's executable.
fn exe_path(pid: u32) -> Option<String> {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buffer = [0u16; 520];
        let mut len = buffer.len() as u32;
        let found = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buffer.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(process);
        found.then(|| String::from_utf16_lossy(&buffer[..len as usize]))
    }
}

/// File name of a process's executable, lower case ("chrome.exe").
pub fn exe_name(pid: u32) -> Option<String> {
    Some(exe_path(pid)?.rsplit(['\\', '/']).next()?.to_lowercase())
}

/// The description stored in an executable's version information, which is
/// the app's name as people know it ("Windows Terminal", "Google Chrome").
fn file_description(path: &str) -> Option<String> {
    unsafe {
        let path = HSTRING::from(path);
        let size = GetFileVersionInfoSizeW(&path, None);
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        GetFileVersionInfoW(&path, 0, size, data.as_mut_ptr().cast()).ok()?;
        let mut value = std::ptr::null_mut();
        let mut len = 0u32;
        // The strings are stored per language; take the first one listed.
        if !VerQueryValueW(data.as_ptr().cast(), w!("\\VarFileInfo\\Translation"), &mut value, &mut len).as_bool() || len < 4 {
            return None;
        }
        let pair = std::slice::from_raw_parts(value as *const u16, 2);
        let key = HSTRING::from(format!("\\StringFileInfo\\{:04x}{:04x}\\FileDescription", pair[0], pair[1]));
        if !VerQueryValueW(data.as_ptr().cast(), &key, &mut value, &mut len).as_bool() || len == 0 {
            return None;
        }
        let text = std::slice::from_raw_parts(value as *const u16, len as usize);
        let text = String::from_utf16_lossy(text).trim_end_matches('\0').trim().to_owned();
        (!text.is_empty()).then_some(text)
    }
}

/// The app's display name for a process, or an empty string when the process
/// is only a host for other apps' windows.
pub fn app_label(pid: u32) -> String {
    thread_local! {
        static CACHE: std::cell::RefCell<HashMap<String, String>> = std::cell::RefCell::new(HashMap::new());
    }
    let Some(path) = exe_path(pid) else { return String::new() };
    CACHE.with(|cache| {
        cache
            .borrow_mut()
            .entry(path.clone())
            .or_insert_with(|| {
                let file = path.rsplit(['\\', '/']).next().unwrap_or_default();
                if file.eq_ignore_ascii_case("ApplicationFrameHost.exe") {
                    return String::new();
                }
                file_description(&path).unwrap_or_else(|| file.trim_end_matches(".exe").to_owned())
            })
            .clone()
    })
}

pub fn exe_of_window(hwnd: HWND) -> Option<String> {
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    exe_name(pid)
}

pub struct WindowInfo {
    pub hwnd: HWND,
    /// Id of the desktop the window lives on.
    pub desktop: String,
    /// Screen rectangle; for a minimized window, where it will be restored to.
    pub rect: RECT,
    pub minimized: bool,
    pub title: String,
    pub pid: u32,
}

fn dwm_attribute<T: Default>(hwnd: HWND, attribute: windows::Win32::Graphics::Dwm::DWMWINDOWATTRIBUTE) -> Option<T> {
    let mut value = T::default();
    unsafe {
        DwmGetWindowAttribute(hwnd, attribute, &mut value as *mut T as *mut _, std::mem::size_of::<T>() as u32)
            .ok()
            .map(|_| value)
    }
}

/// Every app window with the desktop it belongs to, topmost first. Windows
/// pinned to all desktops belong to none and are left out.
pub fn windows(current: &str) -> Vec<WindowInfo> {
    app_windows()
        .into_iter()
        .filter_map(|hwnd| {
            if pinned_everywhere(hwnd) {
                return None;
            }
            let desktop = id_of(&winvd::get_desktop_by_window(hwnd).ok()?)?;
            // On the desktop being shown nothing is hidden by the shell, so a
            // cloaked window there is a suspended store app, not a real window.
            if desktop == current && dwm_attribute::<u32>(hwnd, DWMWA_CLOAKED).unwrap_or(0) != 0 {
                return None;
            }
            Some(describe(hwnd, desktop))
        })
        .collect()
}

/// Parent pid of every running process.
pub fn process_parents() -> HashMap<u32, u32> {
    let mut parents = HashMap::new();
    unsafe {
        let Ok(snapshot) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return parents };
        let mut entry = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut more = Process32FirstW(snapshot, &mut entry).is_ok();
        while more {
            parents.insert(entry.th32ProcessID, entry.th32ParentProcessID);
            more = Process32NextW(snapshot, &mut entry).is_ok();
        }
        let _ = CloseHandle(snapshot);
    }
    parents
}

/// The given processes plus all their descendants (a browser window's
/// memory is mostly in its child processes).
pub fn with_descendants(roots: &HashSet<u32>, parents: &HashMap<u32, u32>) -> Vec<u32> {
    let belongs = |mut pid: u32| {
        for _ in 0..32 {
            if roots.contains(&pid) {
                return true;
            }
            match parents.get(&pid) {
                Some(&parent) if parent != 0 && parent != pid => pid = parent,
                _ => return false,
            }
        }
        false
    };
    parents.keys().copied().filter(|pid| belongs(*pid)).collect()
}

/// Working-set bytes of the given processes and their descendants. Shared
/// pages are counted more than once, so this is an estimate.
pub fn memory_of(roots: &HashSet<u32>, parents: &HashMap<u32, u32>) -> u64 {
    let mut total = 0u64;
    for pid in with_descendants(roots, parents) {
        unsafe {
            let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { continue };
            let mut counters = PROCESS_MEMORY_COUNTERS::default();
            if K32GetProcessMemoryInfo(process, &mut counters, std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32).as_bool() {
                total += counters.WorkingSetSize as u64;
            }
            let _ = CloseHandle(process);
        }
    }
    total
}

/// Asks Windows to move these processes' memory out of RAM (it is paged back
/// in on demand). Processes we may not touch are skipped.
pub fn trim(pids: &[u32]) {
    for &pid in pids {
        unsafe {
            let Ok(process) = OpenProcess(PROCESS_SET_QUOTA | PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { continue };
            let _ = K32EmptyWorkingSet(process);
            let _ = CloseHandle(process);
        }
    }
}

/// Moves a window to another monitor, keeping its place and size relative to
/// the monitor. A maximized window stays maximized there; a minimized one
/// will be restored there.
pub fn move_to_monitor(hwnd: HWND, to: RECT) {
    unsafe {
        let mut from = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &mut from).as_bool() {
            return;
        }
        let from = from.rcMonitor;
        let mut placement = WINDOWPLACEMENT { length: std::mem::size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
        if GetWindowPlacement(hwnd, &mut placement).is_err() {
            return;
        }
        let (fw, fh) = ((from.right - from.left).max(1) as f32, (from.bottom - from.top).max(1) as f32);
        let (tw, th) = ((to.right - to.left) as f32, (to.bottom - to.top) as f32);
        let r = placement.rcNormalPosition;
        let w = (((r.right - r.left) as f32 * tw / fw) as i32).min(tw as i32);
        let h = (((r.bottom - r.top) as f32 * th / fh) as i32).min(th as i32);
        let x = (to.left + ((r.left - from.left) as f32 * tw / fw) as i32).clamp(to.left, to.right - w);
        let y = (to.top + ((r.top - from.top) as f32 * th / fh) as i32).clamp(to.top, to.bottom - h);
        let maximized = placement.showCmd == SW_SHOWMAXIMIZED.0 as u32;
        if maximized {
            // A maximized window only changes monitor by way of its normal size.
            ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
        placement.rcNormalPosition = RECT { left: x, top: y, right: x + w, bottom: y + h };
        if maximized {
            placement.showCmd = SW_SHOWMAXIMIZED.0 as u32;
        }
        let _ = SetWindowPlacement(hwnd, &placement);
    }
}
