//! Running as administrator, which the installed Flick always asks for.
//!
//! Windows keeps what is typed and clicked on a window run as administrator
//! from the hooks of a process that is not, and lets such a process neither
//! close that window nor move it. So Flick is started by a scheduled task
//! that runs it with the highest privileges: Windows asks for permission
//! once, when the task is made (`turn_on`, at each start until it is).
//!
//! Flick runs as administrator while there is a task. Started in any other
//! way it hands over to the task (`hand_over`).

use crate::config::APP_NAME;
use std::{
    os::windows::process::CommandExt,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use windows::{
    core::{w, HSTRING, PCWSTR},
    Win32::{
        Foundation::{CloseHandle, HANDLE, HWND},
        Security::{GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY},
        System::Threading::{GetCurrentProcess, GetExitCodeProcess, OpenProcessToken, WaitForSingleObject, INFINITE},
        UI::{
            Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW},
            WindowsAndMessaging::{FindWindowW, SW_HIDE},
        },
    },
};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// This process was started with `--portable`: it is not the installed copy
/// and must be started that way again.
static PORTABLE: AtomicBool = AtomicBool::new(false);

pub fn set_portable(portable: bool) {
    PORTABLE.store(portable, Ordering::SeqCst);
}

pub fn portable() -> bool {
    PORTABLE.load(Ordering::SeqCst)
}

/// The name of the scheduled task, and of the entry in the registry that
/// starts the app with Windows. A copy that is not the installed one has
/// its own, so that trying one out leaves the installed one alone.
pub fn task_name() -> String {
    if PORTABLE.load(Ordering::SeqCst) {
        format!("{APP_NAME} (portable)")
    } else {
        APP_NAME.to_owned()
    }
}

/// Runs Windows' schtasks with these arguments; `None` when it failed.
fn schtasks(arguments: &[&str]) -> Option<String> {
    let system = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| "C:\\Windows".into());
    let output = Command::new(system.join("System32\\schtasks.exe"))
        .args(arguments)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The task as Windows has it, when there is one.
fn task() -> Option<String> {
    schtasks(&["/Query", "/TN", &task_name(), "/XML"])
}

/// Whether the setting is on.
pub fn enabled() -> bool {
    task().is_some()
}

/// Whether the task also starts Flick when the user signs in.
pub fn at_sign_in() -> bool {
    task().is_some_and(|task| task.contains("<LogonTrigger>"))
}

/// Whether this process runs as administrator.
pub fn elevated() -> bool {
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION::default();
        let mut len = 0u32;
        let size = std::mem::size_of::<TOKEN_ELEVATION>() as u32;
        let got = GetTokenInformation(token, TokenElevation, Some(&mut elevation as *mut _ as *mut _), size, &mut len);
        let _ = CloseHandle(token);
        got.is_ok() && elevation.TokenIsElevated != 0
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Who is signed in, the way the task scheduler names users.
fn user() -> String {
    let var = |name: &str| std::env::var(name).unwrap_or_default();
    format!("{}\\{}", var("USERDOMAIN"), var("USERNAME"))
}

/// Makes the task (or makes it anew): it runs this exe with the highest
/// privileges for `user`, on demand and, with `at_sign_in`, when they sign
/// in. Only an administrator can.
fn write_task(user: &str, at_sign_in: bool) -> bool {
    let Ok(exe) = std::env::current_exe() else { return false };
    let user = escape(user);
    let trigger = if at_sign_in {
        format!("<LogonTrigger><Enabled>true</Enabled><UserId>{user}</UserId></LogonTrigger>")
    } else {
        String::new()
    };
    let arguments = if PORTABLE.load(Ordering::SeqCst) { "<Arguments>--portable</Arguments>" } else { "" };
    // What schtasks would choose by itself does not do for an app that is to
    // stay: not started on battery, stopped after three days, and run at a
    // priority below normal, which makes the hooks slow to answer.
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo><Description>{APP_NAME}: run as administrator</Description></RegistrationInfo>
  <Triggers>{trigger}</Triggers>
  <Principals>
    <Principal id="Author"><UserId>{user}</UserId><LogonType>InteractiveToken</LogonType><RunLevel>HighestAvailable</RunLevel></Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>false</StartWhenAvailable>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>4</Priority>
  </Settings>
  <Actions Context="Author"><Exec><Command>"{}"</Command>{arguments}</Exec></Actions>
</Task>
"#,
        escape(&exe.to_string_lossy())
    );
    let file = std::env::temp_dir().join(format!("{APP_NAME}-task-{}.xml", std::process::id()));
    let bytes: Vec<u8> = std::iter::once(0xFEFFu16).chain(xml.encode_utf16()).flat_map(u16::to_le_bytes).collect();
    if std::fs::write(&file, bytes).is_err() {
        return false;
    }
    let made = schtasks(&["/Create", "/TN", &task_name(), "/XML", &file.to_string_lossy(), "/F"]).is_some();
    let _ = std::fs::remove_file(file);
    made
}

/// Runs this exe as administrator with `arguments` (Windows asks the user
/// for permission) and waits for it. True when it ran and reported success.
fn run_elevated(arguments: &str) -> bool {
    let Ok(exe) = std::env::current_exe() else { return false };
    let (exe, arguments) = (HSTRING::from(exe.as_os_str()), HSTRING::from(arguments));
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: w!("runas"),
        lpFile: PCWSTR(exe.as_ptr()),
        lpParameters: PCWSTR(arguments.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    unsafe {
        // Fails when the user says no.
        if ShellExecuteExW(&mut info).is_err() || info.hProcess.is_invalid() {
            return false;
        }
        WaitForSingleObject(info.hProcess, INFINITE);
        let mut code = 1u32;
        let _ = GetExitCodeProcess(info.hProcess, &mut code);
        let _ = CloseHandle(info.hProcess);
        code == 0
    }
}

/// What `run_elevated` runs: `--admin on <at sign-in: 0 or 1> <user>` makes
/// the task. Returns the exit code.
pub fn helper(arguments: &[String]) -> i32 {
    let done = match arguments {
        [on, at_sign_in, user] if on == "on" => write_task(user, at_sign_in == "1"),
        _ => false,
    };
    !done as i32
}

pub fn portable_flag() -> &'static str {
    if PORTABLE.load(Ordering::SeqCst) {
        " --portable"
    } else {
        ""
    }
}

fn remove_task() -> bool {
    schtasks(&["/Delete", "/TN", &task_name(), "/F"]).is_some()
}

/// Starts the task. True once Flick is seen running from it.
fn run_task() -> bool {
    if schtasks(&["/Run", "/TN", &task_name()]).is_none() {
        return false;
    }
    (0..50).any(|_| {
        std::thread::sleep(Duration::from_millis(100));
        unsafe { FindWindowW(crate::install::MAIN_CLASS, None) }.0 != 0
    })
}

/// Turns running as administrator on. Windows asks for permission once. `at_sign_in`:
/// whether Flick is to start with Windows, which the task does from now on.
/// Returns true when the caller must quit now, another Flick, run as
/// administrator, being about to take over.
pub fn turn_on(at_sign_in: bool) -> bool {
    if elevated() {
        // Already run as administrator by the user's own doing: only the
        // task is missing.
        write_task(&user(), at_sign_in);
        return false;
    }
    let made = run_elevated(&format!("--admin on {} \"{}\"{}", at_sign_in as u8, user(), portable_flag()));
    if !made || !enabled() {
        crate::app::log("the task that runs the app as administrator was not made");
        return false;
    }
    // The other one waits for this one to go (`main`).
    schtasks(&["/Run", "/TN", &task_name()]).is_some()
}

/// Sets whether the task starts Flick at sign-in, keeping whom it is for.
pub fn set_at_sign_in(on: bool) {
    let user = task()
        .and_then(|task| Some(task.split_once("<UserId>")?.1.split_once("</UserId>")?.0.to_owned()))
        .unwrap_or_else(user);
    if !write_task(&user, on) {
        crate::app::log("the task that runs the app as administrator could not be changed");
    }
}

/// At the start: with the setting on and this process not run as
/// administrator, the task is started instead. True when it was and this
/// process has nothing left to do.
pub fn hand_over() -> bool {
    !elevated() && enabled() && run_task()
}

/// Removes the task as part of uninstalling. Not run as administrator this
/// takes permission; true when the uninstalling was handed to a process that
/// has it.
pub fn uninstall() -> bool {
    if !enabled() {
        return false;
    }
    if elevated() {
        remove_task();
        return false;
    }
    run_elevated("--uninstall")
}

/// Lets processes not run as administrator send `messages` to `window` of
/// this one even when it is: closing it to install a newer version, for one.
pub fn let_in(window: HWND, messages: &[u32]) {
    use windows::Win32::UI::WindowsAndMessaging::{ChangeWindowMessageFilterEx, MSGFLT_ALLOW};
    for message in messages {
        unsafe {
            let _ = ChangeWindowMessageFilterEx(window, *message, MSGFLT_ALLOW, None);
        }
    }
}
