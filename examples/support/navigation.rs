//! Navigation regression probe used by drive and scripts/navigation-e2e.ps1.
use super::vdapi;
use std::{path::Path, process::{Command, Stdio}, time::{Duration, Instant}};
use windows::{core::w, Win32::{Foundation::{LPARAM, WPARAM}, System::Com::{CoInitializeEx, COINIT_MULTITHREADED}, UI::{Input::KeyboardAndMouse::*, WindowsAndMessaging::*}}};

pub fn navigation(mode: &str, out: &Path, flick: Option<&str>) {
    unsafe { let _ = CoInitializeEx(None, COINIT_MULTITHREADED); }
    assert!(vdapi::select(false).is_some());
    let cfg = std::path::PathBuf::from(std::env::var_os("APPDATA").unwrap()).join("Flick");
    std::fs::create_dir_all(out).unwrap();
    let wait = |ms| std::thread::sleep(Duration::from_millis(ms));
    match mode {
        "stage" => {
            std::fs::write(out.join("home.txt"), vdapi::get_current_desktop().unwrap().id()).unwrap();
            let original: Vec<_> = vdapi::get_desktops().unwrap().into_iter().map(|d| d.id()).collect();
            std::fs::write(out.join("original.txt"), original.join("\n")).unwrap();
            let mut cursor = windows::Win32::Foundation::POINT::default();
            unsafe { GetCursorPos(&mut cursor).unwrap(); }
            std::fs::write(out.join("cursor.txt"), format!("{} {}", cursor.x, cursor.y)).unwrap();
            let mut ids = Vec::new();
            for _ in 0..6 {
                ids.push(vdapi::create_desktop().unwrap().id());
                std::fs::write(out.join("made.txt"), ids.join("\n")).unwrap();
            }
            let others: Vec<_> = vdapi::get_desktops().unwrap().into_iter().map(|d| d.id()).filter(|id| !ids.contains(id)).collect();
            let mut rows: Vec<_> = ids.chunks(2).enumerate().map(|(i, cells)| serde_json::json!({"name": format!("Probe {}", i + 1), "cells": cells, "last": cells[0]})).collect();
            rows.push(serde_json::json!({"name": "Existing", "cells": others}));
            std::fs::create_dir_all(&cfg).unwrap();
            std::fs::write(cfg.join("state.json"), serde_json::json!({"rows": rows, "guide_seen": true}).to_string()).unwrap();
        }
        "unstage" => {
            let all = vdapi::get_desktops().unwrap();
            let home = std::fs::read_to_string(out.join("home.txt")).unwrap();
            let home = *all.iter().find(|d| d.id() == home).unwrap();
            vdapi::switch_desktop(home).unwrap();
            wait(500);
            for id in std::fs::read_to_string(out.join("made.txt")).unwrap_or_default().lines() {
                if let Some(desktop) = all.iter().find(|d| d.id() == id) {
                    vdapi::remove_desktop(*desktop, home).unwrap();
                }
            }
            let restored: Vec<_> = vdapi::get_desktops().unwrap().into_iter().map(|d| d.id()).collect();
            assert_eq!(restored.join("\n"), std::fs::read_to_string(out.join("original.txt")).unwrap());
            assert_eq!(vdapi::get_current_desktop().unwrap().id(), home.id());
            let cursor = std::fs::read_to_string(out.join("cursor.txt")).unwrap();
            let xy: Vec<i32> = cursor.split_whitespace().map(|n| n.parse().unwrap()).collect();
            unsafe { SetCursorPos(xy[0], xy[1]).unwrap(); }
            std::fs::write(out.join("restored.txt"), "Original desktop, desktop order and cursor restored\n").unwrap();
        }
        "shelves" => unsafe { super::drive_shelves::run(out, flick.unwrap()); },
        "display" => unsafe { super::drive_display::run(out, flick.unwrap()); },
        "recede" => unsafe { super::drive_display::recede(out, flick.unwrap()); },
        "dock" => unsafe { super::drive_display::dock(out, flick.unwrap()); },
        // A shelf turned in one hold stays turned: shown and arrived at there in the next.
        "cube" => unsafe {
            let ids = std::fs::read_to_string(out.join("made.txt")).unwrap();
            let ids: Vec<_> = ids.lines().collect();
            let all = vdapi::get_desktops().unwrap();
            let current = || vdapi::get_current_desktop().unwrap().id();
            let mut state: serde_json::Value = serde_json::from_slice(&std::fs::read(cfg.join("state.json")).unwrap()).unwrap();
            let rows = state["rows"].as_array_mut().unwrap();
            rows[0] = serde_json::json!({"name": "Cube 1", "cells": [ids[0]], "last": ids[0]});
            rows[1] = serde_json::json!({"name": "Cube 2", "cells": [ids[1], ids[2], ids[3]], "last": ids[1]});
            rows[2] = serde_json::json!({"name": "Cube 3", "cells": [ids[4], ids[5]], "last": ids[4]});
            std::fs::write(cfg.join("state.json"), state.to_string()).unwrap();
            std::fs::write(cfg.join("config.toml"), "config_version = 3\ntrigger = \"xbutton2, f8\"\nminimap = true\nhotkeys = false\n[mouse]\nstep_x = 220\nstep_y = 220\nnotch_x = 40\nnotch_y = 100\nfollow_x = true\nfollow_y = true\nreverse = false\n").unwrap();
            vdapi::switch_desktop(*all.iter().find(|d| d.id() == ids[4]).unwrap()).unwrap();
            wait(1200);
            let mouse = |flags, dx, dy| {
                let input = INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, dy, mouseData: 2, dwFlags: flags, ..Default::default() } } };
                assert_eq!(SendInput(&[input], std::mem::size_of::<INPUT>() as i32), 1);
            };
            let stroke = |dx: i32, dy: i32| {
                for i in 1..=30 { mouse(MOUSEEVENTF_MOVE, dx * i / 30 - dx * (i - 1) / 30, dy * i / 30 - dy * (i - 1) / 30); wait(15); }
                wait(250);
            };
            let mut child = Command::new(flick.unwrap()).arg("--portable").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
            wait(3000);
            assert!(FindWindowW(w!("flick.main"), None).0 != 0, "Flick did not start");
            let mut report = String::new();
            // Shelf 3 up to shelf 2, two cells along it, back down to shelf 3, in one hold.
            mouse(MOUSEEVENTF_XDOWN, 0, 0); wait(80);
            for (dx, dy) in [(4, -340), (600, 4), (4, 340)] { stroke(dx, dy); }
            mouse(MOUSEEVENTF_XUP, 0, 0); wait(2400);
            let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(cfg.join("state.json")).unwrap()).unwrap();
            let back = current() == ids[4];
            let kept = saved["rows"][1]["last"] == ids[3];
            report += &format!("back-on-shelf-3: {back}\nshelf-2-saved-turned: {kept} last={}\n", saved["rows"][1]["last"]);
            // The next hold shows shelf 2 at that cell and arrives there.
            mouse(MOUSEEVENTF_XDOWN, 0, 0); wait(80);
            stroke(4, -340); wait(700);
            let _ = Command::new(std::env::current_exe().unwrap()).args(["shot", out.join("cube-held-up.raw").to_str().unwrap()]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
            mouse(MOUSEEVENTF_XUP, 0, 0); wait(2400);
            let got = current();
            let arrived = got == ids[3];
            report += &format!("next-hold-arrives-turned: {arrived} expected={} actual={got}\n", ids[3]);
            let _ = child.kill();
            let _ = child.wait();
            let failures = [back, kept, arrived].iter().filter(|ok| !**ok).count();
            report += &format!("navigation failures: {failures}\n");
            std::fs::write(out.join("report.txt"), &report).unwrap();
            print!("{report}");
            assert_eq!(failures, 0, "cube probe failed; see report.txt");
        },
        // A quick, sure move goes to the next desktop without the screen
        // receding; a slow one recedes as before. Both switch on letting go.
        "quick" => unsafe {
            let ids = std::fs::read_to_string(out.join("made.txt")).unwrap();
            let ids: Vec<_> = ids.lines().collect();
            let all = vdapi::get_desktops().unwrap();
            let current = || vdapi::get_current_desktop().unwrap().id();
            let goto = |i: usize| {
                let desktop = *all.iter().find(|d| d.id() == ids[i]).unwrap();
                for _ in 0..3 {
                    vdapi::switch_desktop(desktop).unwrap(); wait(900);
                    if current() == ids[i] { return; }
                }
                panic!("probe could not establish starting desktop {i}");
            };
            let mouse = |flags, dx, dy| {
                let input = INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, dy, mouseData: 2, dwFlags: flags, ..Default::default() } } };
                assert_eq!(SendInput(&[input], std::mem::size_of::<INPUT>() as i32), 1);
            };
            let stroke = |dx: i32, ms: u64| {
                let n = (ms / 10).max(1) as i32;
                for i in 1..=n { mouse(MOUSEEVENTF_MOVE, dx * i / n - dx * (i - 1) / n, 0); wait(ms / n as u64); }
            };
            std::fs::write(cfg.join("config.toml"), "config_version = 3\ntrigger = \"xbutton2, f8\"\nminimap = true\nhotkeys = false\n[mouse]\nstep_x = 220\nstep_y = 220\nnotch_x = 40\nnotch_y = 100\nfollow_x = true\nfollow_y = true\nreverse = false\n[experimental]\nquick_flick = \"slide\"\n").unwrap();
            goto(0);
            let trace = out.join("trace-quick.csv");
            let _ = std::fs::remove_file(&trace);
            let mut child = Command::new(flick.unwrap()).arg("--portable").env("FLICK_TRACE", &trace)
                .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
            wait(3000);
            assert!(FindWindowW(w!("flick.main"), None).0 != 0, "Flick did not start");
            let mut report = String::new();
            let mut arrived = Vec::new();
            // 1.5 cells in 100 ms and let go; then 1.5 cells in 900 ms, held a moment.
            for (label, ms, rest) in [("fast", 100, 40), ("slow", 900, 250)] {
                goto(0);
                mouse(MOUSEEVENTF_XDOWN, 0, 0); wait(80);
                stroke(340, ms); wait(rest);
                mouse(MOUSEEVENTF_XUP, 0, 0); wait(2400);
                let ok = current() == ids[1];
                report += &format!("{label}-arrives: {ok}\n");
                arrived.push(ok);
            }
            let _ = child.kill();
            let _ = child.wait();
            // The smallest the screen got in each hold (trace column `scale`).
            let text = std::fs::read_to_string(&trace).unwrap_or_default();
            let mut smallest: Vec<(String, f64)> = Vec::new();
            for line in text.lines().skip(1) {
                let cols: Vec<_> = line.split(',').collect();
                let (Some(hold), Some(scale)) = (cols.first(), cols.get(11).and_then(|s| s.parse::<f64>().ok())) else { continue };
                match smallest.iter_mut().find(|(h, _)| h == hold) {
                    Some((_, least)) => *least = least.min(scale),
                    None => smallest.push((hold.to_string(), scale)),
                }
            }
            report += &format!("smallest scale per hold: {smallest:?}\n");
            let fast_whole = smallest.first().is_some_and(|(_, s)| *s > 0.999);
            let slow_receded = smallest.last().is_some_and(|(_, s)| *s < 0.9) && smallest.len() >= 2;
            report += &format!("fast-never-recedes: {fast_whole}\nslow-recedes: {slow_receded}\n");
            let failures = arrived.iter().chain([&fast_whole, &slow_receded]).filter(|ok| !**ok).count();
            report += &format!("navigation failures: {failures}\n");
            std::fs::write(out.join("report.txt"), &report).unwrap();
            print!("{report}");
            assert_eq!(failures, 0, "quick probe failed; see report.txt");
        },
        "pacing" => unsafe { super::drive_pacing::run(out, flick.unwrap()); },
        "run" | "motion" | "geometry" => unsafe {
            let geometry = mode == "geometry";
            let motion = mode != "run";
            let ids = std::fs::read_to_string(out.join("made.txt")).unwrap();
            let ids: Vec<_> = ids.lines().collect();
            let all = vdapi::get_desktops().unwrap();
            let goto = |i: usize| {
                let desktop = *all.iter().find(|d| d.id() == ids[i]).unwrap();
                for _ in 0..3 {
                    vdapi::switch_desktop(desktop).unwrap(); wait(900);
                    if vdapi::get_current_desktop().unwrap().id() == ids[i] { return; }
                }
                panic!("probe could not establish starting desktop {i}");
            };
            let mouse = |flags, dx, dy| {
                let input = INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, dy, mouseData: 2, dwFlags: flags, ..Default::default() } } };
                assert_eq!(SendInput(&[input], std::mem::size_of::<INPUT>() as i32), 1);
            };
            let stroke = |dx: i32, dy: i32, ms: u64| {
                let n = (ms / 10).max(1) as i32;
                for i in 1..=n {
                    mouse(MOUSEEVENTF_MOVE, dx * i / n - dx * (i - 1) / n, dy * i / n - dy * (i - 1) / n);
                    wait(ms / n as u64);
                }
            };
            let me = std::env::current_exe().unwrap();
            let mut report = String::new();
            let mut failures = 0;
            for (follow, minimap) in [(true, false), (true, true), (false, true)] {
                if geometry && (!follow || minimap) { continue; }
                let config = format!("config_version = 3\ntrigger = \"xbutton2, f8\"\nminimap = {minimap}\nhotkeys = false\n[mouse]\nstep_x = 220\nstep_y = 220\nnotch_x = 40\nnotch_y = 100\nfollow_x = {follow}\nfollow_y = {follow}\nreverse = false\n");
                std::fs::write(cfg.join("config.toml"), config).unwrap();
                goto(0);
                let mut child = Command::new(flick.unwrap()).arg("--portable").env("FLICK_GLIDE_STATS", "1").env("FLICK_TRACE", out.join(format!("trace-{follow}-{minimap}.csv")))
                    .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
                wait(3000);
                let app = FindWindowW(w!("flick.main"), None);
                assert!(app.0 != 0, "Flick did not start");
                if !geometry {
                for (label, start, moves, target) in [
                    ("mouse-right", 0, vec![(340, 8)], 1),
                    ("mouse-up-then-right", 4, vec![(4, -340), (340, 4)], 3),
                    ("mouse-right-then-up", 4, vec![(340, 4), (4, -340)], 3),
                    ("mouse-down-then-right", 2, vec![(4, 340), (340, 4)], 5),
                    ("edge-then-inward", 1, vec![(900, 0), (-340, 0)], 0),
                ] {
                    goto(start);
                    mouse(MOUSEEVENTF_XDOWN, 0, 0);
                    wait(80);
                    for (dx, dy) in moves { stroke(dx, dy, if motion && follow { 240 } else { 450 }); if !motion || !follow { wait(250); } }
                    if follow {
                        let still = vdapi::get_current_desktop().unwrap().id() == ids[start];
                        failures += usize::from(!still);
                        report += &format!("follow={follow} minimap={minimap} {label} Windows-stays-until-release: {still}\n");
                    }
                    if !motion { let _ = Command::new(&me).args(["shot", out.join(format!("{follow}-{minimap}-{label}.raw")).to_str().unwrap()]).status(); }
                    mouse(MOUSEEVENTF_XUP, 0, 0);
                    wait(if motion { 2400 } else { 1800 });
                    let got = vdapi::get_current_desktop().unwrap().id();
                    let ok = got == ids[target];
                    failures += usize::from(!ok);
                    report += &format!("follow={follow} minimap={minimap} {label}: {ok} start={} expected={} actual={got}\n", ids[start], ids[target]);
                }
                goto(0);
                for up in [false, true] {
                    let input = super::key_vk(0x77, up);
                    SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
                    if !up { wait(80); stroke(340, 8, 450); wait(250); }
                }
                wait(if motion { 2400 } else { 1800 });
                let got = vdapi::get_current_desktop().unwrap().id();
                report += &format!("follow={follow} minimap={minimap} keyboard-trigger: {} actual={got}\n", got == ids[1]);
                failures += usize::from(got != ids[1]);
                // The second press arrives before the first picture can settle.
                if follow {
                    goto(4);
                    for (dx, dy) in [(4, -340), (340, 4)] {
                        mouse(MOUSEEVENTF_XDOWN, 0, 0);
                        stroke(dx, dy, 50);
                        wait(200); // Remove fling velocity without giving the released hold time to settle.
                        mouse(MOUSEEVENTF_XUP, 0, 0);
                        wait(5);
                    }
                    wait(2200);
                    let got = vdapi::get_current_desktop().unwrap().id();
                    report += &format!("follow={follow} minimap={minimap} rapid-up-then-right: {} actual={got}\n", got == ids[3]);
                    failures += usize::from(got != ids[3]);
                }
                }
                if motion {
                    goto(0);
                    if follow && !geometry {
                        mouse(MOUSEEVENTF_XDOWN, 0, 0); wait(80); stroke(180, 0, 100); mouse(MOUSEEVENTF_XUP, 0, 0); wait(if motion { 2400 } else { 1800 });
                        let ok = vdapi::get_current_desktop().unwrap().id() == ids[0];
                        failures += usize::from(!ok);
                        report += &format!("follow={follow} minimap={minimap} subthreshold-no-fling: {ok}\n");
                        goto(2);
                        mouse(MOUSEEVENTF_XDOWN, 0, 0); wait(80); stroke(340, 0, 240);
                        mouse(MOUSEEVENTF_XUP, 0, 0); wait(2400);
                        let ok = vdapi::get_current_desktop().unwrap().id() == ids[3];
                        failures += usize::from(!ok);
                        report += &format!("follow={follow} minimap={minimap} next-hold-after-subthreshold: {ok}\n");
                    }
                    // Colored, differently proportioned native windows expose source-frame fitting errors.
                    let mut windows = Vec::new();
                    for (title, rgb, width, height, dest) in [("F wide", [255, 255, 0], 800, 300, 0), ("F tall", [0, 255, 255], 300, 700, 1)] {
                        let args = ["hold".to_owned(), rgb[0].to_string(), rgb[1].to_string(), rgb[2].to_string(), title.into(), "200".into(), "120".into(), width.to_string(), height.to_string(), "90".into()];
                        windows.push(Command::new(&me).args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap());
                        wait(400);
                        let hwnd = FindWindowW(w!("flick.test.hold"), &windows::core::HSTRING::from(title));
                        vdapi::move_window_to_desktop(*all.iter().find(|d| d.id() == ids[dest]).unwrap(), hwnd).unwrap();
                    }
                    goto(0);
                    let _ = PostMessageW(app, WM_APP + 2, WPARAM(0), LPARAM(0)); wait(900);
                    let board = FindWindowW(w!("flick.board"), None);
                    let opened = IsWindowVisible(board).as_bool();
                    failures += usize::from(!opened);
                    report += &format!("follow={follow} minimap={minimap} board-open: {opened}\n");
                    std::fs::write(out.join("report.txt"), &report).unwrap();
                    if follow && !minimap {
                        let geometry_window = CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), w!("F geometry probe"), WS_POPUP, 0, 0, 1, 1, None, None, None, None);
                        assert!(geometry_window.0 != 0);
                        for title in ["F wide", "F tall"] {
                            use windows::Win32::{Foundation::{POINT, RECT}, Graphics::{Dwm::*, Gdi::ClientToScreen}};
                            let hwnd = FindWindowW(w!("flick.test.hold"), &windows::core::HSTRING::from(title));
                            let mut outer = RECT::default(); let mut frame = RECT::default();
                            GetWindowRect(hwnd, &mut outer).unwrap();
                            DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, &mut frame as *mut _ as _, std::mem::size_of::<RECT>() as u32).unwrap();
                            let thumbnail = DwmRegisterThumbnail(geometry_window, hwnd).unwrap();
                            let source = DwmQueryThumbnailSourceSize(thumbnail).unwrap();
                            DwmUnregisterThumbnail(thumbnail).unwrap();
                            let mut client = RECT::default(); let mut client_origin = POINT::default();
                            GetClientRect(hwnd, &mut client).unwrap(); ClientToScreen(hwnd, &mut client_origin);
                            report += &format!("{title} hwnd={:x} source={}x{} outer={outer:?} visible={frame:?} client={client:?} client_origin={client_origin:?}\n", hwnd.0, source.cx, source.cy);
                        }
                        let _ = DestroyWindow(geometry_window);
                    }
                    for (label, vk) in [("right", 0x27), ("left", 0x25), ("down", 0x28), ("up", 0x26)] {
                        let _ = label;
                        let _ = PostMessageW(board, WM_KEYDOWN, WPARAM(vk), LPARAM(0)); wait(1400);
                    }
                    if follow && !minimap {
                        // Capture only after the held springs settle: BitBlt stalls DWM on this laptop.
                        mouse(MOUSEEVENTF_XDOWN, 0, 0); wait(80); stroke(110, 0, 250); wait(900);
                        let _ = Command::new(&me).args(["shot", out.join("board-held-wide.raw").to_str().unwrap()]).status();
                        mouse(MOUSEEVENTF_XUP, 0, 0); wait(1500);
                        let _ = PostMessageW(board, WM_KEYDOWN, WPARAM(0x27), LPARAM(0)); wait(1500);
                        mouse(MOUSEEVENTF_XDOWN, 0, 0); wait(80); stroke(-110, 0, 250); wait(900);
                        let _ = Command::new(&me).args(["shot", out.join("board-held-tall.raw").to_str().unwrap()]).status();
                        mouse(MOUSEEVENTF_XUP, 0, 0); wait(1500);
                        let _ = PostMessageW(board, WM_KEYDOWN, WPARAM(0x25), LPARAM(0)); wait(150);
                        let _ = Command::new(&me).args(["shot", out.join("board-moving-left.raw").to_str().unwrap()]).status();
                        wait(1500);
                    }
                    let _ = PostMessageW(board, WM_KEYDOWN, WPARAM(0x1b), LPARAM(0)); wait(1500);
                    goto(2);
                    let _ = PostMessageW(app, WM_APP + 2, WPARAM(0), LPARAM(0)); wait(700);
                    mouse(MOUSEEVENTF_XDOWN, 0, 0); wait(80);
                    stroke(4, 340, if follow { 240 } else { 450 });
                    if !follow { wait(250); }
                    stroke(340, 4, if follow { 240 } else { 450 });
                    mouse(MOUSEEVENTF_XUP, 0, 0); wait(1500);
                    let _ = PostMessageW(board, WM_KEYDOWN, WPARAM(0x1b), LPARAM(0)); wait(1200);
                    let got = vdapi::get_current_desktop().unwrap().id();
                    let ok = got == ids[5]; failures += usize::from(!ok);
                    report += &format!("follow={follow} minimap={minimap} board-down-then-right: {ok} actual={got}\n");
                    for window in &mut windows { let _ = window.kill(); let _ = window.wait(); }
                } else {
                // Fifty physical button taps interleaved with holds, 100 ms apart.
                let measuring = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
                let running = measuring.clone();
                let window = app.0;
                let sampler = std::thread::spawn(move || {
                    let mut latencies = Vec::new();
                    let mut timeouts = 0;
                    while running.load(std::sync::atomic::Ordering::Relaxed) {
                        let before = Instant::now();
                        let answered = SendMessageTimeoutW(windows::Win32::Foundation::HWND(window), WM_NULL, WPARAM(0), LPARAM(0), SMTO_ABORTIFHUNG | SMTO_BLOCK, 2500, None).0 != 0;
                        latencies.push(before.elapsed().as_millis());
                        timeouts += usize::from(!answered);
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    (latencies, timeouts)
                });
                let started = Instant::now();
                for i in 0..50 {
                    mouse(MOUSEEVENTF_XDOWN, 0, 0);
                    wait(10);
                    if i % 3 == 0 { stroke(if i % 2 == 0 { 180 } else { -180 }, 3, 20); }
                    mouse(MOUSEEVENTF_XUP, 0, 0);
                    if let Some(left) = Duration::from_millis((i + 1) * 100).checked_sub(started.elapsed()) { std::thread::sleep(left); }
                }
                let elapsed = started.elapsed().as_millis();
                wait(2000);
                measuring.store(false, std::sync::atomic::Ordering::Relaxed);
                let (mut latencies, timeouts) = sampler.join().unwrap();
                let board = FindWindowW(w!("flick.board"), None);
                let _ = PostMessageW(board, WM_KEYDOWN, WPARAM(0x1b), LPARAM(0));
                wait(2000);
                latencies.sort();
                report += &format!("follow={follow} minimap={minimap} 50 taps in {elapsed}ms: UI p95={}ms max={}ms timeouts={timeouts} alive={}\n", latencies[latencies.len() * 95 / 100], latencies.last().unwrap(), child.try_wait().unwrap().is_none());
                failures += usize::from(timeouts != 0 || child.try_wait().unwrap().is_some());
                // Real keyboard input must reach the board without joining foreign input queues.
                goto(0);
                mouse(MOUSEEVENTF_XDOWN, 0, 0); wait(30); mouse(MOUSEEVENTF_XUP, 0, 0);
                wait(700);
                let board = FindWindowW(w!("flick.board"), None);
                let focused = GetForegroundWindow() == board && IsWindowVisible(board).as_bool();
                for vk in [0x27, 0x1b] {
                    for up in [false, true] {
                        SendInput(&[super::key_vk(vk, up)], std::mem::size_of::<INPUT>() as i32);
                        wait(20);
                    }
                    wait(400);
                }
                wait(1500);
                let got = vdapi::get_current_desktop().unwrap().id();
                let selected = got == ids[1] && !IsWindowVisible(board).as_bool();
                failures += usize::from(!focused || !selected);
                report += &format!("follow={follow} minimap={minimap} board keyboard: focused={focused} Esc-goes-to-selection={selected} actual={got}\n");
                }
                let _ = child.kill();
                let _ = child.wait();
                wait(500);
                std::fs::write(out.join("report.txt"), &report).unwrap();
            }
            report += &format!("navigation failures: {failures}\n");
            std::fs::write(out.join("report.txt"), &report).unwrap();
            print!("{report}");
            assert_eq!(failures, 0, "navigation probe failed; see report.txt");
        },
        _ => panic!("navigation stage|run|motion|geometry|unstage <out> [flick]"),
    }
}
