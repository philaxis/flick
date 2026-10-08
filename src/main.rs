#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod config;
mod grid;

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
    if flag == Some("--uninstall") {
        return install::uninstall();
    }
    // Refuse to run where the Windows internals are not ones we know, before
    // installing or touching anything.
    if vdapi::select(flag == Some("--force")).is_none() {
        let build = vdapi::windows_build().map_or("알 수 없음".to_owned(), |(b, r)| format!("{b}.{r}"));
        return app::warn(&format!(
            "이 윈도우에서는 Flick을 실행하지 않습니다.\n\n윈도우 11 23H2(빌드 22631.3085 이상), 24H2(빌드 26100.2605 이상), 25H2를 지원합니다.\n이 PC의 빌드: {build}\n\nFlick does not run on this version of Windows. It supports Windows 11 23H2 (build 22631.3085 or later), 24H2 (build 26100.2605 or later) and 25H2."
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
    // One instance only: two sets of hooks would step twice per gesture.
    let _mutex = unsafe { CreateMutexW(None, true, w!("flick.single-instance")) };
    if unsafe { GetLastError() } == Err(ERROR_ALREADY_EXISTS.into()) {
        return;
    }
    app::run(flag == Some("--first-run"));
}

#[cfg(not(windows))]
fn main() {
    eprintln!("{} runs on Windows only.", config::APP_NAME);
}
