#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod config;
mod grid;

#[cfg(windows)]
mod app;
#[cfg(windows)]
mod board;
#[cfg(windows)]
mod input;
#[cfg(windows)]
mod install;
#[cfg(windows)]
mod overlay;
#[cfg(windows)]
mod tray;
#[cfg(windows)]
mod vd;

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
    if let [_, flag, path] = args.as_slice() {
        if flag == "--render" {
            return app::render_sample(path);
        }
        if flag == "--render-board" {
            return app::render_board(path);
        }
    }
    let flag = args.get(1).map(String::as_str);
    if flag == Some("--uninstall") {
        return install::uninstall();
    }
    // Started from a download folder or the like: install, then let the
    // installed copy take over. `--portable` and debug builds run in place.
    if flag != Some("--portable") && !cfg!(debug_assertions) && !install::running_installed() {
        match install::install() {
            Ok(()) => return,
            Err(e) => app::log(&format!("install failed, running in place: {e}")),
        }
    }
    // One instance only: two sets of hooks would step twice per gesture.
    let _mutex = unsafe { CreateMutexW(None, true, w!("kankan.single-instance")) };
    if unsafe { GetLastError() } == Err(ERROR_ALREADY_EXISTS.into()) {
        return;
    }
    app::run(flag == Some("--first-run"));
}

#[cfg(not(windows))]
fn main() {
    eprintln!("{} runs on Windows only.", config::APP_NAME);
}
