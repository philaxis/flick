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
        }
        "run" => unsafe {
            let ids = std::fs::read_to_string(out.join("made.txt")).unwrap();
            let ids: Vec<_> = ids.lines().collect();
            let all = vdapi::get_desktops().unwrap();
            let goto = |i: usize| { vdapi::switch_desktop(*all.iter().find(|d| d.id() == ids[i]).unwrap()).unwrap(); wait(900); };
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
            for follow in [true, false] {
                let config = format!("config_version = 2\ntrigger = \"xbutton2, f8\"\nminimap = true\nhotkeys = false\n[mouse]\nstep_x = 260\nstep_y = 400\nnotch_x = 40\nnotch_y = 100\nfollow_x = {follow}\nfollow_y = {follow}\nreverse = false\n");
                std::fs::write(cfg.join("config.toml"), config).unwrap();
                goto(0);
                let mut child = Command::new(flick.unwrap()).arg("--portable").env("FLICK_GLIDE_STATS", "1").env("FLICK_TRACE", out.join(format!("trace-{follow}.csv")))
                    .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
                wait(3000);
                let app = FindWindowW(w!("flick.main"), None);
                assert!(app.0 != 0, "Flick did not start");
                for (label, start, moves, target) in [
                    ("mouse-right", 0, vec![(300, 12)], 1),
                    ("mouse-up-then-right", 4, vec![(8, -440), (330, 5)], 3),
                    ("mouse-right-then-up", 4, vec![(300, 10), (5, -600)], 2),
                ] {
                    goto(start);
                    mouse(MOUSEEVENTF_XDOWN, 0, 0);
                    wait(80);
                    for (dx, dy) in moves { stroke(dx, dy, 450); wait(250); }
                    let _ = Command::new(&me).args(["shot", out.join(format!("{follow}-{label}.raw")).to_str().unwrap()]).status();
                    mouse(MOUSEEVENTF_XUP, 0, 0);
                    wait(1800);
                    let got = vdapi::get_current_desktop().unwrap().id();
                    let ok = got == ids[target];
                    failures += usize::from(!ok);
                    report += &format!("follow={follow} {label}: {ok} start={} expected={} actual={got}\n", ids[start], ids[target]);
                }
                goto(0);
                for up in [false, true] {
                    let input = super::key_vk(0x77, up);
                    SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
                    if !up { wait(80); stroke(300, 8, 450); wait(250); }
                }
                wait(1800);
                let got = vdapi::get_current_desktop().unwrap().id();
                report += &format!("follow={follow} keyboard-trigger: {} actual={got}\n", got == ids[1]);
                failures += usize::from(got != ids[1]);
                // Fifty physical button taps interleaved with holds, 100 ms apart.
                let mut latencies = Vec::new();
                for i in 0..50 {
                    mouse(MOUSEEVENTF_XDOWN, 0, 0);
                    wait(10);
                    if i % 3 == 0 { stroke(if i % 2 == 0 { 90 } else { -90 }, 3, 20); }
                    mouse(MOUSEEVENTF_XUP, 0, 0);
                    wait(70);
                    let before = Instant::now();
                    let answered = SendMessageTimeoutW(app, WM_NULL, WPARAM(0), LPARAM(0), SMTO_ABORTIFHUNG | SMTO_BLOCK, 2500, None).0 != 0;
                    latencies.push(before.elapsed().as_millis());
                    if !answered { report += &format!("follow={follow} UI timeout at tap {i}\n"); }
                }
                let board = FindWindowW(w!("flick.board"), None);
                let _ = PostMessageW(board, WM_KEYDOWN, WPARAM(0x1b), LPARAM(0));
                wait(2000);
                latencies.sort();
                report += &format!("follow={follow} 50 taps: UI p95={}ms max={}ms alive={}\n", latencies[47], latencies[49], child.try_wait().unwrap().is_none());
                let _ = child.kill();
                let _ = child.wait();
                wait(500);
                std::fs::write(out.join("report.txt"), &report).unwrap();
            }
            report += &format!("navigation failures: {failures}\n");
            std::fs::write(out.join("report.txt"), &report).unwrap();
            print!("{report}");
        },
        _ => panic!("navigation stage|run|unstage <out> [flick]"),
    }
}
