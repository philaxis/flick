//! Notification-area icon, the menus, and the "run at startup" registry entry.

use crate::config::APP_NAME;
use windows::{
    core::{w, HSTRING, PCWSTR},
    Win32::{
        Foundation::{HWND, POINT},
        System::LibraryLoader::GetModuleHandleW,
        System::Registry::{RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ},
        UI::{
            Shell::{
                Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE, NIM_MODIFY,
                NOTIFYICONDATAW,
            },
            WindowsAndMessaging::{
                AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, GetSystemMetrics, LoadIconW, LoadImageW,
                SetForegroundWindow, TrackPopupMenu, HICON, HMENU, IDI_APPLICATION, IMAGE_ICON, LR_DEFAULTCOLOR,
                MF_CHECKED, MF_SEPARATOR, MF_STRING, SM_CXSMICON, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTBUTTON,
            },
        },
    },
};

/// Where Windows keeps what to start at sign-in; the app's entry there is
/// named after the app.
pub const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");

#[derive(Clone, Copy, PartialEq)]
pub enum Command {
    Peek = 1,
    Settings,
    Exit,
}

/// The app icon embedded in the exe (resource id 1), at tray size.
pub fn app_icon() -> HICON {
    unsafe {
        let size = GetSystemMetrics(SM_CXSMICON);
        GetModuleHandleW(None)
            .and_then(|module| LoadImageW(module, PCWSTR(1 as *const u16), IMAGE_ICON, size, size, LR_DEFAULTCOLOR))
            .map(|handle| HICON(handle.0))
            .or_else(|_| LoadIconW(None, IDI_APPLICATION))
            .unwrap_or_default()
    }
}

fn icon_data(hwnd: HWND) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW { cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32, hWnd: hwnd, uID: 1, ..Default::default() }
}

pub fn add(hwnd: HWND, callback_message: u32) {
    let mut data = icon_data(hwnd);
    data.uFlags = NIF_ICON | NIF_MESSAGE | NIF_TIP;
    data.uCallbackMessage = callback_message;
    data.hIcon = app_icon();
    for (slot, unit) in data.szTip.iter_mut().zip(APP_NAME.encode_utf16()) {
        *slot = unit;
    }
    unsafe {
        Shell_NotifyIconW(NIM_ADD, &data);
    }
}

/// Shows a balloon notification from the tray icon.
pub fn notify(hwnd: HWND, title: &str, text: &str) {
    let mut data = icon_data(hwnd);
    data.uFlags = NIF_INFO;
    data.dwInfoFlags = NIIF_INFO;
    for (slot, unit) in data.szInfoTitle.iter_mut().zip(title.encode_utf16().take(63)) {
        *slot = unit;
    }
    for (slot, unit) in data.szInfo.iter_mut().zip(text.encode_utf16().take(255)) {
        *slot = unit;
    }
    unsafe {
        Shell_NotifyIconW(NIM_MODIFY, &data);
    }
}

pub fn remove(hwnd: HWND) {
    unsafe {
        Shell_NotifyIconW(NIM_DELETE, &icon_data(hwnd));
    }
}

fn add_item(menu: HMENU, id: usize, text: PCWSTR, checked: bool) {
    let flags = if checked { MF_STRING | MF_CHECKED } else { MF_STRING };
    unsafe {
        let _ = AppendMenuW(menu, flags, id, text);
    }
}

fn add_separator(menu: HMENU) {
    unsafe {
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
    }
}

/// Shows a menu at the cursor, waits for the user and returns the id of the
/// item picked (0 for none). The menu is destroyed. `owner` must be the
/// foreground window, or the menu does not close on a click elsewhere.
fn pick(menu: HMENU, owner: HWND) -> usize {
    unsafe {
        let mut cursor = POINT::default();
        let _ = GetCursorPos(&mut cursor);
        let picked =
            TrackPopupMenu(menu, TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON, cursor.x, cursor.y, 0, owner, None);
        let _ = DestroyMenu(menu);
        picked.0 as usize
    }
}

/// Shows the tray icon's menu at the cursor and returns what was picked.
pub fn menu(hwnd: HWND) -> Option<Command> {
    let menu = unsafe { CreatePopupMenu() }.ok()?;
    add_item(menu, Command::Peek as usize, w!("전체 보기"), false);
    add_item(menu, Command::Settings as usize, w!("설정…"), false);
    add_separator(menu);
    add_item(menu, Command::Exit as usize, w!("종료"), false);
    unsafe {
        let _ = SetForegroundWindow(hwnd);
    }
    let picked = pick(menu, hwnd);
    [Command::Peek, Command::Settings, Command::Exit].into_iter().find(|c| *c as usize == picked)
}

pub fn autostart_enabled() -> bool {
    let name = HSTRING::from(APP_NAME);
    unsafe { RegGetValueW(HKEY_CURRENT_USER, RUN_KEY, &name, RRF_RT_REG_SZ, None, None, None).is_ok() }
}

pub fn set_autostart(enabled: bool) {
    let name = HSTRING::from(APP_NAME);
    unsafe {
        if !enabled {
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, &name);
            return;
        }
        let Ok(exe) = std::env::current_exe() else { return };
        let command = HSTRING::from(format!("\"{}\"", exe.display()));
        let bytes = (command.len() + 1) * 2;
        let set = RegSetKeyValueW(HKEY_CURRENT_USER, RUN_KEY, &name, REG_SZ.0, Some(command.as_ptr().cast()), bytes as u32);
        if let Err(e) = set {
            crate::app::log(&format!("the run-at-startup entry could not be written: {e}"));
        }
    }
}

/// Which kinds of pinning a window currently has.
#[derive(Clone, Copy, Default)]
pub struct PinState {
    pub row_window: bool,
    pub row_app: bool,
    pub all_window: bool,
    pub all_app: bool,
}

#[derive(Clone, Copy, PartialEq)]
pub enum PinCommand {
    RowWindow = 1,
    RowApp,
    AllWindow,
    AllApp,
}

/// Shows the pin menu for one window at the cursor. The caller must make
/// `owner` the foreground window first (see `pick`).
pub fn pin_menu(owner: HWND, state: PinState) -> Option<PinCommand> {
    let menu = unsafe { CreatePopupMenu() }.ok()?;
    add_item(menu, PinCommand::RowWindow as usize, w!("이 창을 행 안에서 따라오게"), state.row_window);
    add_item(menu, PinCommand::RowApp as usize, w!("이 앱의 창을 행 안에서 따라오게"), state.row_app);
    add_separator(menu);
    add_item(menu, PinCommand::AllWindow as usize, w!("이 창을 모든 칸에 고정"), state.all_window);
    add_item(menu, PinCommand::AllApp as usize, w!("이 앱을 모든 칸에 고정"), state.all_app);
    let picked = pick(menu, owner);
    [PinCommand::RowWindow, PinCommand::RowApp, PinCommand::AllWindow, PinCommand::AllApp]
        .into_iter()
        .find(|c| *c as usize == picked)
}

#[derive(Clone, Copy, PartialEq)]
pub enum RowCommand {
    CloseWindows = 1,
    Remove,
}

/// The menu of a workspace in the board, at the cursor. `summary` says what
/// closing would close; `removable` is false for the only workspace.
pub fn row_menu(owner: HWND, summary: &str, removable: bool) -> Option<RowCommand> {
    let menu = unsafe { CreatePopupMenu() }.ok()?;
    let close = HSTRING::from(format!("열린 창 닫기  ({summary})"));
    add_item(menu, RowCommand::CloseWindows as usize, PCWSTR(close.as_ptr()), false);
    if removable {
        add_item(menu, RowCommand::Remove as usize, w!("워크스페이스 없애기  (창은 가장 가까운 칸으로)"), false);
    }
    let picked = pick(menu, owner);
    [RowCommand::CloseWindows, RowCommand::Remove].into_iter().find(|c| *c as usize == picked)
}
