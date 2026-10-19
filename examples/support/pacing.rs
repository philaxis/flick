//! Repeatable six-second holds; run with navigation-e2e.ps1 -Mode pacing.
use std::{path::Path, process::{Command, Stdio}, time::{Duration, Instant}};
use windows::{core::w, Win32::{Foundation::{HWND, LPARAM, WPARAM}, Graphics::Dwm::*, System::Performance::QueryPerformanceFrequency, UI::{Input::KeyboardAndMouse::*, WindowsAndMessaging::*}}};

pub unsafe fn run(out: &Path, flick: &str) {
    let wait = |ms| std::thread::sleep(Duration::from_millis(ms));
    let cfg = std::path::PathBuf::from(std::env::var_os("APPDATA").unwrap()).join("Flick");
    std::fs::write(cfg.join("config.toml"), "config_version = 3\ntrigger = \"xbutton2, f8\"\nminimap = false\nhotkeys = false\n[mouse]\nstep_x = 220\nstep_y = 220\nfollow_x = true\nfollow_y = true\nreverse = false\n").unwrap();
    let ids = std::fs::read_to_string(out.join("made.txt")).unwrap();
    let ids: Vec<_> = ids.lines().collect();
    let all = super::vdapi::get_desktops().unwrap();
    let home = *all.iter().find(|d| d.id() == ids[0]).unwrap();
    super::vdapi::switch_desktop(home).unwrap(); wait(1000);
    let me = std::env::current_exe().unwrap();
    let mut windows = Vec::new();
    for (title, dest, width, height) in [("F wide", 0, "800", "300"), ("F tall", 1, "300", "700")] {
        windows.push(Command::new(&me).args(["hold", "60", "130", "200", title, "100", "100", width, height, "180"])
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap());
        wait(400);
        let hwnd = FindWindowW(w!("flick.test.hold"), &windows::core::HSTRING::from(title));
        assert!(hwnd.0 != 0);
        super::vdapi::move_window_to_desktop(*all.iter().find(|d| d.id() == ids[dest]).unwrap(), hwnd).unwrap();
    }
    let timing = || {
        let mut t = DWM_TIMING_INFO { cbSize: std::mem::size_of::<DWM_TIMING_INFO>() as u32, ..Default::default() };
        DwmGetCompositionTimingInfo(HWND(0), &mut t).unwrap(); t
    };
    let mut frequency = 0; QueryPerformanceFrequency(&mut frequency).unwrap();
    let mut report = String::new();
    let mut gaps = Vec::new();
    for _ in 0..120 { let at = Instant::now(); DwmFlush().unwrap(); gaps.push(at.elapsed().as_secs_f64() * 1000.0); }
    gaps.sort_by(f64::total_cmp);
    let t = timing();
    let hz = frequency as f64 / t.qpcRefreshPeriod as f64;
    report += &format!("refresh_hz={hz:.4} DwmFlush_ms mean={:.3} p50={:.3} p95={:.3} max={:.3}\n", gaps.iter().sum::<f64>() / gaps.len() as f64, gaps[60], gaps[114], gaps[119]);
    let mouse = |flags, dx| {
        let input = INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, mouseData: 2, dwFlags: flags, ..Default::default() } } };
        assert_eq!(SendInput(&[input], std::mem::size_of::<INPUT>() as i32), 1);
    };
    for mode in ["board", "follow"] {
        super::vdapi::switch_desktop(home).unwrap(); wait(1000);
        let mut child = Command::new(flick).arg("--portable").env("FLICK_TRACE", out.join(format!("{mode}.csv"))).env("FLICK_GLIDE_STATS", "1")
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
        wait(2500);
        let app = FindWindowW(w!("flick.main"), None); assert!(app.0 != 0);
        if mode == "board" {
            PostMessageW(app, WM_APP + 2, WPARAM(0), LPARAM(0)).unwrap(); wait(1000);
            assert!(IsWindowVisible(FindWindowW(w!("flick.board"), None)).as_bool());
        }
        let before = timing();
        let began = Instant::now();
        mouse(MOUSEEVENTF_XDOWN, 0); wait(80);
        for leg in 0..12 {
            let dx = if leg % 2 == 0 { 340 } else { -340 };
            let start = Instant::now();
            for i in 1..=100 {
                mouse(MOUSEEVENTF_MOVE, dx * i / 100 - dx * (i - 1) / 100);
                if let Some(left) = Duration::from_millis(i as u64 * 5).checked_sub(start.elapsed()) { std::thread::sleep(left); }
            }
        }
        let after = timing();
        let elapsed = began.elapsed().as_secs_f64();
        // Snapshot only after the measured interval: screen capture itself blocks DWM.
        wait(500);
        let status = Command::new(&me).args(["shot", out.join(format!("{mode}.raw")).to_str().unwrap()]).status().unwrap();
        assert!(status.success());
        mouse(MOUSEEVENTF_XUP, 0); wait(1600);
        if mode == "board" { PostMessageW(FindWindowW(w!("flick.board"), None), WM_KEYDOWN, WPARAM(0x1b), LPARAM(0)).unwrap(); wait(600); }
        let displayed = after.cFramesDisplayed.saturating_sub(before.cFramesDisplayed);
        let missed = after.cFramesMissed.saturating_sub(before.cFramesMissed);
        let refreshes = after.cRefresh.saturating_sub(before.cRefresh);
        report += &format!("{mode}: seconds={elapsed:.3} cRefresh={refreshes} cFramesDisplayed={displayed} cFramesMissed={missed} alive={}\n", child.try_wait().unwrap().is_none());
        let _ = child.kill(); let _ = child.wait();
        std::fs::write(out.join("report.txt"), &report).unwrap();
    }
    for window in &mut windows { let _ = window.kill(); let _ = window.wait(); }
    println!("{report}");
}
