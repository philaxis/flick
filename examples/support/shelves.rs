//! Shelf-pin regression: real menus, held mouse input, board keys and desktop notifications.
use super::vdapi;
use std::{path::Path, process::{Command, Stdio}, time::Duration};
use windows::{core::{w, HSTRING}, Win32::{Foundation::{HWND, LPARAM, RECT, WPARAM}, Graphics::{Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED}, Gdi::{GetDC, GetPixel, ReleaseDC}}, UI::{Input::KeyboardAndMouse::*, WindowsAndMessaging::*}}};

pub unsafe fn run(out: &Path, flick: &str) {
    let wait = |ms| std::thread::sleep(Duration::from_millis(ms));
    let cfg = std::path::PathBuf::from(std::env::var_os("APPDATA").unwrap()).join("Flick");
    let ids = std::fs::read_to_string(out.join("made.txt")).unwrap();
    let ids: Vec<_> = ids.lines().collect();
    let all = vdapi::get_desktops().unwrap();
    let desktop = |i: usize| *all.iter().find(|d| d.id() == ids[i]).unwrap();
    let goto = |i| {
        let mut switched = vdapi::switch_desktop(desktop(i));
        for _ in 0..50 {
            if switched.is_ok() { break; }
            wait(100); switched = vdapi::switch_desktop(desktop(i));
        }
        switched.unwrap(); wait(1600);
    };
    let key = |vk| {
        let events = [super::key_vk(vk, false), super::key_vk(vk, true)];
        assert_eq!(SendInput(&events, std::mem::size_of::<INPUT>() as i32), 2);
    };
    let mouse = |flags, dx, dy| {
        let input = INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, dy, mouseData: 2, dwFlags: flags, ..Default::default() } } };
        assert_eq!(SendInput(&[input], std::mem::size_of::<INPUT>() as i32), 1);
    };
    let click = |rect: RECT| {
        SetCursorPos((rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2).unwrap();
        let buttons = if GetSystemMetrics(SM_SWAPBUTTON) == 0 { [MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP] }
            else { [MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP] };
        for button in buttons { mouse(button, 0, 0); }
        wait(300);
    };
    let gesture = |dx, dy| {
        mouse(MOUSEEVENTF_XDOWN, 0, 0); wait(80);
        for _ in 0..34 { mouse(MOUSEEVENTF_MOVE, dx, dy); wait(10); }
        wait(250); mouse(MOUSEEVENTF_XUP, 0, 0); wait(2200);
    };
    let me = std::env::current_exe().unwrap();
    let app_exe = out.join("shelf-app.exe");
    std::fs::copy(&me, &app_exe).unwrap();
    let shot = |label: &str| {
        assert!(Command::new(&me).args(["shot", out.join(format!("{label}.raw")).to_str().unwrap()])
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().unwrap().success());
    };
    let mut report = String::new();
    let mut failures = 0;
    for (follow_x, follow) in [(true, true), (false, true), (false, false)] {
        std::fs::write(cfg.join("config.toml"), format!("config_version = 3\ntrigger = \"xbutton2, f8\"\nminimap = false\nhotkeys = true\n[mouse]\nstep_x = 220\nstep_y = 220\nnotch_x = 40\nnotch_y = 100\nfollow_x = {follow_x}\nfollow_y = {follow}\nreverse = false\n")).unwrap();
        // Clear the preceding run's app pin, retaining the staged grid.
        let mut state: serde_json::Value = serde_json::from_slice(&std::fs::read(cfg.join("state.json")).unwrap()).unwrap();
        state["follow_apps"] = serde_json::json!([]);
        for row in state["rows"].as_array_mut().unwrap() { row["last"] = row["cells"][0].clone(); }
        std::fs::write(cfg.join("state.json"), state.to_string()).unwrap();
        goto(4);
        let mut children = Vec::new();
        let mut windows = Vec::new();
        let titles = ["Shelf window", "Shelf app A", "Shelf app B", "Shelf other row", "Shelf minimized"];
        for (exe, indices) in [(&me, 0..1), (&app_exe, 1..5)] {
            let mut args = vec!["holds".to_owned()];
            for i in indices {
                args.extend(["0".into(), "255".into(), "255".into(), titles[i].into(), (120 + i * 190).to_string(), "200".into(), "180".into(), "300".into(), "600".into()]);
            }
            children.push(Command::new(exe).args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap());
        }
        for (i, title) in titles.iter().enumerate() {
            let mut hwnd = HWND(0);
            for _ in 0..100 {
                hwnd = FindWindowW(w!("flick.test.hold"), &HSTRING::from(*title));
                if hwnd.0 != 0 { break; } wait(50);
            }
            assert!(hwnd.0 != 0, "probe window missing");
            // The HWND can exist before Explorer has registered its application view.
            let target = desktop(if i == 3 { 2 } else { 4 });
            let mut moved = vdapi::move_window_to_desktop(target, hwnd);
            for _ in 0..100 {
                if moved.is_ok() { break; }
                wait(50); moved = vdapi::move_window_to_desktop(target, hwnd);
            }
            moved.unwrap();
            if i == 4 { ShowWindow(hwnd, SW_MINIMIZE); }
            windows.push(hwnd);
        }
        let mut app = Command::new(flick).arg("--portable").env("FLICK_GLIDE_STATS", "1")
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
        wait(3500);
        let main = FindWindowW(w!("flick.main"), None);
        assert!(main.0 != 0);
        for (hwnd, choice) in [(windows[0], 0), (windows[1], 2)] {
            goto(4);
            let mut rect = RECT::default();
            GetWindowRect(hwnd, &mut rect).unwrap();
            click(rect);
            assert_eq!(GetForegroundWindow(), hwnd);
            PostMessageW(main, WM_HOTKEY, WPARAM(8), LPARAM(0)).unwrap(); wait(400);
            let popup = FindWindowW(w!("#32768"), None);
            let mut menu = MENUBARINFO { cbSize: std::mem::size_of::<MENUBARINFO>() as u32, ..Default::default() };
            GetMenuBarInfo(popup, OBJID_CLIENT, 0, &mut menu).unwrap();
            GetMenuItemRect(None, menu.hMenu, choice, &mut rect).unwrap();
            click(rect); wait(600);
        }
        let pinned = GetPropW(windows[0], w!("Flick.ShelfPin.Instance")).0 != 0;
        let state: serde_json::Value = serde_json::from_slice(&std::fs::read(cfg.join("state.json")).unwrap()).unwrap();
        let app_pinned = state["follow_apps"].as_array().is_some_and(|a| a.iter().any(|v| v == "shelf-app.exe"));
        report += &format!("follow_x={follow_x} follow_y={follow} pins window={pinned} app={app_pinned}\n");
        std::fs::write(out.join("report.txt"), &report).unwrap();
        assert!(pinned && app_pinned, "probe did not set the shelf pins");
        let mut incoming = None;
        let mut check = |label: &str, expected: usize| {
            let current = vdapi::get_current_desktop().unwrap().id();
            failures += usize::from(current != ids[expected]);
            report += &format!("follow_x={follow_x} follow_y={follow} {label} current={} expected={}\n", current, ids[expected]);
            for (i, hwnd) in windows.iter().enumerate() {
                let on = vdapi::get_desktop_by_window(*hwnd).unwrap().id();
                let visible = IsWindowVisible(*hwnd).as_bool();
                let minimized = IsIconic(*hwnd).as_bool();
                let mut cloak = 0u32;
                DwmGetWindowAttribute(*hwnd, DWMWA_CLOAKED, &mut cloak as *mut _ as _, 4).unwrap();
                let same_row = (i == 3) == (expected / 2 == 1);
                let mut rect = RECT::default();
                GetWindowRect(*hwnd, &mut rect).unwrap();
                let dc = GetDC(None);
                let pixel = GetPixel(dc, (rect.left + rect.right) / 2, (rect.top + rect.bottom) / 2).0;
                ReleaseDC(None, dc);
                let painted = pixel & 0xff < 30 && (pixel >> 8) & 0xff > 220 && (pixel >> 16) & 0xff > 220;
                let ok = visible && minimized == (i == 4) && if same_row { on == current && (i == 4 || (cloak == 0 && painted)) } else { on != current };
                failures += usize::from(!ok);
                report += &format!("  window={i} ok={ok} visible={visible} minimized={minimized} cloak={cloak} painted={painted} pixel={pixel:06x} desktop={on}\n");
            }
            std::fs::write(out.join("report.txt"), &report).unwrap();
        };
        check("initial-third-row", 4);
        if follow_x && follow {
            mouse(MOUSEEVENTF_XDOWN, 0, 0); wait(80);
            shot("leaving-start");
            mouse(MOUSEEVENTF_MOVE, 0, -10); wait(200); shot("leaving-early");
            for step in 0..4 {
                for _ in 0..8 { mouse(MOUSEEVENTF_MOVE, 0, -10); wait(10); }
                wait(200); shot(&format!("leaving-{step}"));
            }
            mouse(MOUSEEVENTF_XUP, 0, 0); wait(2200);
        } else { gesture(0, -10); }
        check("leave-third-row", 2);
        gesture(10, 0); check("second-row-right", 3);
        gesture(-10, 0); check("second-row-left", 2);
        gesture(10, 0); check("second-row-right-again", 3);
        // Change the third shelf's anchor in the board without visiting it: its pins
        // still live in cell 4, but the next vertical arrival must show them in cell 5.
        PostMessageW(main, WM_APP + 2, WPARAM(0), LPARAM(0)).unwrap(); wait(700);
        for vk in [0x28, 0x27, 0x26, 0x0D] { key(vk); wait(500); }
        wait(1200); check("anchor-change-stays-second-row", 3);
        mouse(MOUSEEVENTF_XDOWN, 0, 0); wait(80);
        if follow_x && follow { mouse(MOUSEEVENTF_MOVE, 0, 10); wait(200); shot("returning-early"); }
        for step in 0..4 {
            for _ in 0..8 { mouse(MOUSEEVENTF_MOVE, 0, 10); wait(10); }
            if follow_x && follow { wait(200); shot(&format!("returning-{step}")); }
        }
        if follow {
            wait(1000);
            let label = format!("incoming-{follow_x}");
            shot(&label);
            let raw = std::fs::read(out.join(format!("{label}.raw"))).unwrap();
            let width = i32::from_le_bytes(raw[..4].try_into().unwrap()) as usize;
            let height = i32::from_le_bytes(raw[4..8].try_into().unwrap()) as usize;
            assert_eq!(raw.len(), 8 + width * height * 4);
            // The first cyan probe enters left of centre; the other shelf's window
            // is further right. This also rejects a blank overlay over a visible HWND.
            let cyan = raw[8..].chunks_exact(4).enumerate().filter(|(i, p)| {
                i % width < width / 3 && i / width > height / 4 && i / width < height * 3 / 4
                    && p[0] > 100 && p[0] == p[1] && p[2] < 20
            }).count();
            incoming = Some(cyan);
        }
        mouse(MOUSEEVENTF_XUP, 0, 0); wait(2400);
        check("return-other-cell", 5);
        gesture(-10, 0); check("third-row-left", 4);
        // Board selection returns to a cell different from the windows' last home.
        PostMessageW(main, WM_APP + 2, WPARAM(0), LPARAM(0)).unwrap(); wait(700);
        for vk in [0x26, 0x27, 0x28, 0x27, 0x0D] { key(vk); wait(500); }
        wait(2000); check("board-return-other-cell", 5);
        // OS switches exercise desktop_changed without Flick's pre-switch path.
        goto(2); check("external-leave", 2);
        goto(3); check("external-sideways", 3);
        goto(4); check("external-return-other-cell", 4);
        // Registered navigation hotkeys (Ctrl+Alt+Win+arrows).
        for vk in [0x26, 0x27, 0x28] {
            for up in [false, true] {
                let events: Vec<_> = [0x11, 0x12, 0x5B, vk].into_iter().map(|k| super::key_vk(k, up)).collect();
                assert_eq!(SendInput(&events, std::mem::size_of::<INPUT>() as i32), 4);
            }
            wait(2200);
        }
        check("hotkey-return", 4);
        if let Some(cyan) = incoming {
            report += &format!("follow_x={follow_x} incoming-preview={} cyan_pixels={cyan}\n", cyan > 1000);
            failures += usize::from(cyan <= 1000);
        }
        std::fs::write(out.join("report.txt"), &report).unwrap();
        app.kill().unwrap(); app.wait().unwrap();
        for child in &mut children { let _ = child.kill(); let _ = child.wait(); }
        wait(500);
    }
    report += &format!("RESULT failures={failures}\n");
    std::fs::write(out.join("report.txt"), &report).unwrap();
    assert_eq!(failures, 0, "shelf regression; see report.txt");
}
