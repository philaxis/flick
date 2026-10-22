//! Bringing a shelf back: a cell remembers a probe app, Flick is asked to
//! bring its shelf back, and the probe's window has to land in that cell.
//! The desktops, the probe and state.json are put back as they were.
use super::vdapi;
use std::{path::Path, process::{Command, Stdio}, time::{Duration, Instant}};
use windows::{core::{w, HSTRING}, Win32::{Foundation::{HWND, LPARAM, WPARAM}, UI::WindowsAndMessaging::*}};

/// The app's `WM_RESTORE_ROW` (src/app.rs).
const WM_RESTORE_ROW: u32 = WM_APP + 15;

pub unsafe fn run(out: &Path, flick: &str) {
    let wait = |ms| std::thread::sleep(Duration::from_millis(ms));
    if FindWindowW(w!("flick.main"), None).0 != 0 {
        println!("RESULT another Flick is running; not probing");
        return;
    }
    let cfg = std::path::PathBuf::from(std::env::var_os("APPDATA").unwrap()).join("Flick");
    std::fs::create_dir_all(&cfg).unwrap();
    let home = vdapi::get_current_desktop().unwrap();
    let made: Vec<_> = (0..2).map(|_| vdapi::create_desktop().unwrap()).collect();
    let probe = out.join("probe-ghost.exe");
    std::fs::copy(std::env::current_exe().unwrap(), &probe).unwrap();
    let state = cfg.join("state.json");
    let saved = std::fs::read(&state).ok();
    let log = cfg.join("log.txt");
    let logged = std::fs::read_to_string(&log).map_or(0, |text| text.lines().count());
    let memory = serde_json::json!({ made[0].id(): { "taken": 1, "apps": [
        { "exe": probe.to_str().unwrap(), "args": "", "title": "probe-ghost", "name": "probe-ghost" }
    ] } });
    let rows = serde_json::json!([
        { "name": "Ghost", "cells": [home.id(), made[0].id()], "last": home.id() },
        { "name": "Other", "cells": [made[1].id()] },
    ]);
    std::fs::write(&state, serde_json::json!({ "rows": rows, "memory": memory, "guide_seen": true, "board_hints_seen": true }).to_string()).unwrap();

    let mut app = Command::new(flick).arg("--portable").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
    let mut main = HWND(0);
    for _ in 0..100 {
        main = FindWindowW(w!("flick.main"), None);
        if main.0 != 0 { break; }
        wait(100);
    }
    let mut landed = false;
    let mut window = HWND(0);
    if main.0 == 0 {
        println!("RESULT Flick did not start");
    } else {
        // Its config read and its timers set.
        wait(1500);
        let _ = PostMessageW(main, WM_RESTORE_ROW, WPARAM(0), LPARAM(0));
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(20) {
            window = FindWindowW(w!("flick.test.hold"), &HSTRING::from("probe-ghost"));
            if window.0 != 0 {
                landed = vdapi::get_desktop_by_window(window).is_ok_and(|desktop| desktop.id() == made[0].id());
                if landed { break; }
            }
            wait(100);
        }
        println!("RESULT ghost window came up: {}", window.0 != 0);
        println!("RESULT ghost window landed in its cell: {landed}");
    }

    if window.0 != 0 {
        let _ = PostMessageW(window, WM_CLOSE, WPARAM(0), LPARAM(0));
    }
    let _ = app.kill();
    let _ = app.wait();
    let errors = std::fs::read_to_string(&log)
        .map_or(0, |text| text.lines().skip(logged).filter(|line| line.contains("restore")).count());
    println!("RESULT restore lines in log.txt: {errors}");
    let _ = vdapi::switch_desktop(home);
    wait(500);
    for desktop in made {
        if let Err(e) = vdapi::remove_desktop(desktop, home) {
            println!("desktop not removed: {e:?}");
        }
    }
    match saved {
        Some(saved) => std::fs::write(&state, saved).unwrap(),
        None => { let _ = std::fs::remove_file(&state); }
    }
    let _ = std::fs::remove_file(&probe);
}
