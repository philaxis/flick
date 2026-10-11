#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(windows)]
#[macro_use]
mod debuglog;

mod config;
mod grid;

#[cfg(windows)]
mod admin;
#[cfg(windows)]
mod app;
#[cfg(windows)]
mod board;
#[cfg(any(windows, test))]
mod hold;
#[cfg(windows)]
mod input;
#[cfg(windows)]
mod install;
#[cfg(windows)]
mod overlay;
#[cfg(windows)]
mod paint;
#[cfg(windows)]
mod settings;
#[cfg(any(windows, test))]
mod slide;
#[cfg(any(windows, test))]
mod stack;
#[cfg(any(windows, test))]
mod tiles;
#[cfg(windows)]
mod tray;
#[cfg(windows)]
mod vd;
#[cfg(any(windows, test))]
mod vdapi;

#[cfg(windows)]
fn main() {
    use windows::{
        core::w,
        Win32::{
            Foundation::{GetLastError, ERROR_ALREADY_EXISTS},
            System::Threading::CreateMutexW,
        },
    };

    let args: Vec<String> = std::env::args().collect();
    // `--render-board <file> 1920x1040,400x300` draws made-up windows of
    // those sizes in place of the current cell's.
    if let [_, flag, path, sizes] = args.as_slice() {
        if flag == "--render-board" {
            return app::render_board(path, Some(sizes));
        }
    }
    if let [_, flag, path] = args.as_slice() {
        if flag == "--render" {
            return app::render_sample(path);
        }
        if flag == "--render-board" {
            return app::render_board(path, None);
        }
        if flag == "--render-settings" {
            return app::render_settings(path);
        }
    }
    let flag = args.get(1).map(String::as_str);
    admin::set_portable(args.iter().any(|arg| arg == "--portable"));
    // What the settings run, as administrator, to turn that setting on or off.
    if flag == Some("--admin") {
        let arguments: Vec<String> = args[2..].iter().filter(|arg| *arg != "--portable").cloned().collect();
        std::process::exit(admin::helper(&arguments));
    }
    if flag == Some("--uninstall") {
        return install::uninstall();
    }
    // Refuse to run where the Windows internals are not ones we know, before
    // installing or touching anything.
    if vdapi::select(flag == Some("--force")).is_none() {
        let build = vdapi::windows_build().map_or("unknown".to_owned(), |(b, r)| format!("{b}.{r}"));
        return app::warn(&format!(
            "Flick does not run on this version of Windows.\n\nIt supports Windows 11 23H2 (build 22631.3085 or later), 24H2 (build 26100.2605 or later) and 25H2.\nThis PC: build {build}"
        ));
    }
    // Started from a download folder or the like: install, then let the
    // installed copy take over. `--portable` and debug builds run in place.
    if flag != Some("--portable") && !cfg!(debug_assertions) && !install::running_installed() {
        match install::install() {
            Ok(()) => return,
            Err(e) => app::log(&format!("install failed, running in place: {e}")),
        }
    }
    // Set to run as administrator: the scheduled task that does so takes over.
    if admin::hand_over() {
        return;
    }
    // One instance only: two sets of hooks would step twice per gesture. One
    // that is just quitting to let this one take over (the setting to run
    // as administrator was changed) is waited for a moment.
    let mut tries = 0;
    let _mutex = loop {
        let mutex = unsafe { CreateMutexW(None, true, w!("flick.single-instance")) };
        if unsafe { GetLastError() } != Err(ERROR_ALREADY_EXISTS.into()) {
            break mutex;
        }
        tries += 1;
        if tries > 30 {
            return;
        }
        if let Ok(mutex) = mutex {
            unsafe {
                let _ = windows::Win32::Foundation::CloseHandle(mutex);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    };
    app::run(flag == Some("--first-run"));
}

#[cfg(not(windows))]
fn main() {
    eprintln!("{} runs on Windows only.", config::APP_NAME);
}
