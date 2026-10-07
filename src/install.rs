//! Self-installation. Running the exe from anywhere copies it to
//! `%LOCALAPPDATA%\<app>`, adds a Start menu shortcut and an entry in
//! Windows' installed-apps list, and starts the installed copy.
//! `--uninstall` (what that entry runs) undoes it.

use crate::{config::APP_NAME, tray::RUN_KEY};
use std::{
    fs,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};
use windows::{
    core::{w, ComInterface, HSTRING, PCWSTR},
    Win32::{
        Foundation::{LPARAM, WPARAM},
        System::{
            Com::{CoCreateInstance, CoInitializeEx, IPersistFile, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED},
            Registry::{
                RegDeleteKeyValueW, RegDeleteTreeW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_DWORD, REG_SZ,
            },
        },
        UI::{
            Shell::{IShellLinkW, ShellLink},
            WindowsAndMessaging::{FindWindowW, MessageBoxW, PostMessageW, MB_ICONINFORMATION, MB_OK, WM_CLOSE},
        },
    },
};

/// Window class of the running app's message window.
pub const MAIN_CLASS: PCWSTR = w!("flick.main");
/// Earlier names of the app, cleaned up on install.
const OLD_CLASSES: [PCWSTR; 2] = [w!("kankan.main"), w!("desk2d.main")];
const OLD_NAMES: [&str; 2] = ["KanKan", "desk2d"];
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn env_dir(variable: &str) -> PathBuf {
    std::env::var_os(variable).map(PathBuf::from).unwrap_or_default()
}

// Where an install under the name `app` keeps its files, its Start menu
// shortcut and its entry in the installed-apps list.

fn install_dir(app: &str) -> PathBuf {
    env_dir("LOCALAPPDATA").join(app)
}

fn installed_exe() -> PathBuf {
    install_dir(APP_NAME).join(format!("{APP_NAME}.exe"))
}

fn shortcut(app: &str) -> PathBuf {
    env_dir("APPDATA").join("Microsoft\\Windows\\Start Menu\\Programs").join(format!("{app}.lnk"))
}

fn uninstall_key(app: &str) -> HSTRING {
    HSTRING::from(format!("Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\{app}"))
}

/// Whether this process is the installed copy.
pub fn running_installed() -> bool {
    let same = |a: &Path, b: &Path| a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy());
    std::env::current_exe().is_ok_and(|exe| same(&exe, &installed_exe()))
}

/// Asks a running instance to quit and waits for it to go.
fn stop_running() {
    for class in std::iter::once(MAIN_CLASS).chain(OLD_CLASSES) {
        for _ in 0..50 {
            let window = unsafe { FindWindowW(class, None) };
            if window.0 == 0 {
                break;
            }
            unsafe {
                let _ = PostMessageW(window, WM_CLOSE, WPARAM(0), LPARAM(0));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

fn set_string(key: &HSTRING, name: PCWSTR, value: &str) {
    let value = HSTRING::from(value);
    unsafe {
        let bytes = ((value.len() + 1) * 2) as u32;
        let _ = RegSetKeyValueW(HKEY_CURRENT_USER, key, name, REG_SZ.0, Some(value.as_ptr().cast()), bytes);
    }
}

fn set_flag(key: &HSTRING, name: PCWSTR) {
    let one = 1u32;
    unsafe {
        let _ = RegSetKeyValueW(HKEY_CURRENT_USER, key, name, REG_DWORD.0, Some(&one as *const u32 as *const _), 4);
    }
}

fn create_shortcut(exe: &Path) -> windows::core::Result<()> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        link.SetPath(&HSTRING::from(exe.as_os_str()))?;
        link.SetDescription(w!("가상 데스크톱을 2D 격자로 이동"))?;
        link.cast::<IPersistFile>()?.Save(&HSTRING::from(shortcut(APP_NAME).as_os_str()), true)
    }
}

/// Installs this exe and starts the installed copy.
pub fn install() -> std::io::Result<()> {
    stop_running();
    let exe = installed_exe();
    fs::create_dir_all(install_dir(APP_NAME))?;
    // The old copy stays locked for a moment after its process has gone.
    let mut copied = fs::copy(std::env::current_exe()?, &exe);
    for _ in 0..30 {
        if copied.is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
        copied = fs::copy(std::env::current_exe()?, &exe);
    }
    copied?;

    if let Err(e) = create_shortcut(&exe) {
        crate::app::log(&format!("shortcut failed: {e}"));
    }
    let key = uninstall_key(APP_NAME);
    let path = exe.to_string_lossy();
    set_string(&key, w!("DisplayName"), APP_NAME);
    set_string(&key, w!("DisplayVersion"), env!("CARGO_PKG_VERSION"));
    set_string(&key, w!("DisplayIcon"), &path);
    set_string(&key, w!("InstallLocation"), &install_dir(APP_NAME).to_string_lossy());
    set_string(&key, w!("UninstallString"), &format!("\"{path}\" --uninstall"));
    set_flag(&key, w!("NoModify"));
    set_flag(&key, w!("NoRepair"));

    // Leftovers of installs under earlier names.
    for old in OLD_NAMES {
        unsafe {
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, &HSTRING::from(old));
            let _ = RegDeleteTreeW(HKEY_CURRENT_USER, &uninstall_key(old));
        }
        let _ = fs::remove_file(shortcut(old));
        let _ = fs::remove_dir_all(install_dir(old));
    }

    Command::new(&exe).arg("--first-run").spawn()?;
    Ok(())
}

pub fn uninstall() {
    stop_running();
    crate::tray::set_autostart(false);
    unsafe {
        let _ = RegDeleteTreeW(HKEY_CURRENT_USER, &uninstall_key(APP_NAME));
    }
    let _ = fs::remove_file(shortcut(APP_NAME));
    let text = format!("{APP_NAME}을(를) 제거했습니다.\n설정과 격자 배치는 %APPDATA%\\{APP_NAME} 에 남아 있습니다.");
    unsafe {
        MessageBoxW(None, &HSTRING::from(text), &HSTRING::from(APP_NAME), MB_OK | MB_ICONINFORMATION);
    }
    // A running exe cannot delete itself; a detached shell removes the folder
    // once this process has exited.
    if running_installed() {
        let command = format!("ping -n 3 127.0.0.1 >nul & rmdir /s /q \"{}\"", install_dir(APP_NAME).display());
        let _ = Command::new("cmd").args(["/c", &command]).creation_flags(CREATE_NO_WINDOW).spawn();
    }
}
