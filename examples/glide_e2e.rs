//! End-to-end check of the screen following the hand (src/glide.rs) on a
//! real Windows session, with Flick running `--portable`:
//!   glide_e2e prepare <dir>    desktops A, B (a row) and C (below A); state.json and config.toml
//!   glide_e2e window <rrggbb> <x> <y> <w> <h> <desktop id|pin> <secs>   (w and h 0: maximized)
//!   glide_e2e run <dir> <exe for the row window>
//!   glide_e2e cleanup <dir>
//!   glide_e2e cliplift <dir>   a window half off the screen and a pinned one beneath another:
//!                              one quick flick and one unsure hold, the screen read back in each
//! A full-screen window of its own colour on each desktop (B's maximized), one pinned to all
//! desktops and one of an app that goes along its row; the trigger is held
//! and the mouse moved with SendInput, and the screen read back in between.
#[allow(dead_code)]
#[path = "../src/vdapi.rs"]
mod vdapi;

#[cfg(windows)]
fn main() {
    imp::main();
}

#[cfg(not(windows))]
fn main() {}

#[cfg(windows)]
mod imp {
    use super::vdapi;
    use std::{
        fmt::Write as _,
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    use windows::{
        core::w,
        Win32::{
            Foundation::{COLORREF, HWND, LPARAM, LRESULT, RECT, WPARAM},
            Graphics::Gdi::{
                BitBlt, CreateCompatibleDC, CreateDIBSection, CreateSolidBrush, GetDC, SelectObject, BITMAPINFO,
                BITMAPINFOHEADER, CAPTUREBLT, DIB_RGB_COLORS, SRCCOPY,
            },
            System::LibraryLoader::GetModuleHandleW,
            UI::{
                HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2},
                Input::KeyboardAndMouse::{
                    SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_MOVE, MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, MOUSEINPUT,
                },
                WindowsAndMessaging::{
                    CreateWindowExW, DefWindowProcW, DispatchMessageW, PeekMessageW, RegisterClassW, ShowWindow,
                    SystemParametersInfoW, TranslateMessage, CW_USEDEFAULT, MSG, PM_REMOVE, SPI_GETWORKAREA, SW_SHOWMAXIMIZED, SW_SHOWNOACTIVATE,
                    SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, WNDCLASSW, WS_EX_APPWINDOW, WS_OVERLAPPEDWINDOW, WS_POPUP,
                },
            },
        },
    };

    const RED: u32 = 0xD02020;
    const BLUE: u32 = 0x2040D0;
    const YELLOW: u32 = 0xE0C020;
    const GREEN: u32 = 0x20B040;
    const MAGENTA: u32 = 0xC020C0;

    pub fn main() {
        unsafe {
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }
        vdapi::select(false);
        let args: Vec<String> = std::env::args().collect();
        match args.get(1).map(String::as_str) {
            Some("prepare") => prepare(&args[2]),
            Some("window") => window(&args[2..]),
            Some("run") => run(&args[2], &args[3]),
            Some("cleanup") => cleanup(&args[2]),
            Some("cliplift") => cliplift(&args[2]),
            _ => eprintln!("usage: see the top of examples/glide_e2e.rs"),
        }
    }

    fn work_area() -> RECT {
        let mut area = RECT::default();
        unsafe {
            let _ = SystemParametersInfoW(SPI_GETWORKAREA, 0, Some(&mut area as *mut _ as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
        }
        area
    }

    fn appdata() -> std::path::PathBuf {
        std::path::PathBuf::from(std::env::var("APPDATA").unwrap()).join("Flick")
    }

    /// Desktops A, B, C: the first three, made where there are fewer.
    fn prepare(dir: &str) {
        std::fs::create_dir_all(dir).unwrap();
        let mut made = Vec::new();
        while vdapi::get_desktops().unwrap().len() < 3 {
            made.push(vdapi::create_desktop().unwrap().id());
        }
        std::fs::write(format!("{dir}/made.txt"), made.join("\n")).unwrap();
        let ids: Vec<String> = vdapi::get_desktops().unwrap().into_iter().map(|d| d.id()).collect();
        let (a, b, rest) = (&ids[0], &ids[1], &ids[2..]);
        let quoted = |ids: &[String]| ids.iter().map(|id| format!("\"{id}\"")).collect::<Vec<_>>().join(",");
        let state = format!(
            "{{\"rows\":[{{\"name\":\"Shelf 1\",\"cells\":[\"{a}\",\"{b}\"],\"last\":\"{a}\"}},\
             {{\"name\":\"Shelf 2\",\"cells\":[{}],\"last\":\"{}\"}}],\"follow_apps\":[\"glide_row.exe\"],\"guide_seen\":true}}",
            quoted(rest),
            rest[0]
        );
        std::fs::create_dir_all(appdata()).unwrap();
        std::fs::write(appdata().join("state.json"), state).unwrap();
        write_config(true);
        std::fs::write(format!("{dir}/ids.txt"), ids.join("\n")).unwrap();
        println!("A {a} B {b} C {}; made {}", rest[0], made.len());
    }

    fn write_config(follow: bool) {
        let config = format!(
            "trigger = \"xbutton2\"\nhotkeys = false\n[mouse]\nstep_x = 260\nstep_y = 400\nnotch_x = 30\nnotch_y = 100\n\
             follow_x = {follow}\nfollow_y = {follow}\nreverse = false\n[touchpad]\nfollow_x = {follow}\nfollow_y = {follow}\n"
        );
        std::fs::write(appdata().join("config.toml"), config).unwrap();
    }

    fn cleanup(dir: &str) {
        let made = std::fs::read_to_string(format!("{dir}/made.txt")).unwrap_or_default();
        let all = vdapi::get_desktops().unwrap_or_default();
        if let Some(home) = all.first().copied() {
            let _ = vdapi::switch_desktop(home);
            for id in made.lines().filter(|id| !id.is_empty()) {
                if let Some(desktop) = all.iter().find(|d| d.id() == id) {
                    println!("remove {id}: {:?}", vdapi::remove_desktop(*desktop, home));
                }
            }
        }
    }

    unsafe extern "system" fn window_proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        DefWindowProcW(hwnd, message, wparam, lparam)
    }

    fn pump(ms: u64) {
        let until = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < until {
            unsafe {
                let mut message = MSG::default();
                while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// A window of one colour, on a desktop or pinned to all, for `secs`.
    fn window(args: &[String]) {
        let color = u32::from_str_radix(&args[0], 16).unwrap();
        let n = |i: usize| args[i].parse::<i32>().unwrap();
        let (r, g, b) = (color >> 16 & 0xFF, color >> 8 & 0xFF, color & 0xFF);
        unsafe {
            let instance = GetModuleHandleW(None).unwrap();
            RegisterClassW(&WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                hInstance: instance.into(),
                lpszClassName: w!("glide.e2e"),
                hbrBackground: CreateSolidBrush(COLORREF(b << 16 | g << 8 | r)),
                ..Default::default()
            });
            let title = windows::core::HSTRING::from(format!("glide {}", args[0]));
            let maximized = n(3) == 0 && n(4) == 0;
            let hwnd = if maximized {
                let d = CW_USEDEFAULT;
                CreateWindowExW(WS_EX_APPWINDOW, w!("glide.e2e"), &title, WS_OVERLAPPEDWINDOW, d, d, d, d, None, None, instance, None)
            } else {
                CreateWindowExW(WS_EX_APPWINDOW, w!("glide.e2e"), &title, WS_POPUP, n(1), n(2), n(3), n(4), None, None, instance, None)
            };
            ShowWindow(hwnd, if maximized { SW_SHOWMAXIMIZED } else { SW_SHOWNOACTIVATE });
            pump(300);
            if args[5] == "pin" {
                println!("pin {:?}", vdapi::pin_window(hwnd));
            } else if let Some(desktop) = vdapi::get_desktops().unwrap().into_iter().find(|d| d.id() == args[5]) {
                println!("move {:?}", vdapi::move_window_to_desktop(desktop, hwnd));
            }
            pump(args[6].parse::<u64>().unwrap() * 1000);
        }
    }

    struct Shot {
        w: i32,
        h: i32,
        pixels: Vec<u8>,
    }

    impl Shot {
        fn take(area: RECT) -> Shot {
            let (w, h) = (area.right - area.left, area.bottom - area.top);
            unsafe {
                let screen = GetDC(None);
                let dc = CreateCompatibleDC(screen);
                let info = BITMAPINFO {
                    bmiHeader: BITMAPINFOHEADER {
                        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: w,
                        biHeight: -h,
                        biPlanes: 1,
                        biBitCount: 32,
                        ..Default::default()
                    },
                    ..Default::default()
                };
                let mut bits = std::ptr::null_mut();
                let bitmap = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0).unwrap();
                SelectObject(dc, bitmap);
                let _ = BitBlt(dc, 0, 0, w, h, screen, area.left, area.top, SRCCOPY | CAPTUREBLT);
                let pixels = std::slice::from_raw_parts(bits as *const u8, (w * h * 4) as usize).to_vec();
                Shot { w, h, pixels }
            }
        }

        /// The colour at a fraction of the area.
        fn at(&self, fx: f64, fy: f64) -> u32 {
            let (x, y) = (((fx * self.w as f64) as i32).clamp(0, self.w - 1), ((fy * self.h as f64) as i32).clamp(0, self.h - 1));
            let i = ((y * self.w + x) * 4) as usize;
            (self.pixels[i + 2] as u32) << 16 | (self.pixels[i + 1] as u32) << 8 | self.pixels[i] as u32
        }

        /// Half size, as a 24-bit BMP.
        fn save(&self, path: &str) {
            let (w, h) = (self.w / 2, self.h / 2);
            let row = ((w * 3 + 3) / 4 * 4) as usize;
            let mut out = Vec::new();
            out.extend_from_slice(b"BM");
            out.extend_from_slice(&((54 + row * h as usize) as u32).to_le_bytes());
            out.extend_from_slice(&[0, 0, 0, 0, 54, 0, 0, 0, 40, 0, 0, 0]);
            out.extend_from_slice(&w.to_le_bytes());
            out.extend_from_slice(&(-h).to_le_bytes());
            out.extend_from_slice(&[1, 0, 24, 0]);
            out.extend_from_slice(&[0u8; 24]);
            for y in 0..h {
                let mut line = Vec::with_capacity(row);
                for x in 0..w {
                    let i = (((y * 2) * self.w + x * 2) * 4) as usize;
                    line.extend_from_slice(&self.pixels[i..i + 3]);
                }
                line.resize(row, 0);
                out.extend_from_slice(&line);
            }
            let _ = std::fs::write(path, out);
        }
    }

    fn name(color: u32) -> &'static str {
        let (r, g, b) = ((color >> 16 & 0xFF) as i32, (color >> 8 & 0xFF) as i32, (color & 0xFF) as i32);
        let near = |c: u32| {
            let (cr, cg, cb) = ((c >> 16 & 0xFF) as i32, (c >> 8 & 0xFF) as i32, (c & 0xFF) as i32);
            (r - cr).abs() + (g - cg).abs() + (b - cb).abs() < 60
        };
        [(RED, "red"), (BLUE, "blue"), (YELLOW, "yellow"), (GREEN, "green"), (MAGENTA, "magenta"), (0, "black")]
            .into_iter()
            .find(|(c, _)| near(*c))
            .map_or("other", |(_, name)| name)
    }

    fn mouse(dx: i32, dy: i32, flags: windows::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS, data: u32) {
        let input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, dy, mouseData: data, dwFlags: flags, ..Default::default() } },
        };
        unsafe {
            SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
        }
    }

    fn hold(down: bool) {
        mouse(0, 0, if down { MOUSEEVENTF_XDOWN } else { MOUSEEVENTF_XUP }, 2);
    }

    /// Moves the mouse by (dx, dy) over `ms`, in reports every 8 ms.
    fn travel(dx: i32, dy: i32, ms: u64) {
        let reports = (ms / 8).max(1) as i32;
        let started = Instant::now();
        for i in 1..=reports {
            let part = |total: i32| total * i / reports - total * (i - 1) / reports;
            mouse(part(dx), part(dy), MOUSEEVENTF_MOVE, 0);
            let due = Duration::from_millis(ms * i as u64 / reports as u64);
            if let Some(wait) = due.checked_sub(started.elapsed()) {
                std::thread::sleep(wait);
            }
        }
    }

    fn current() -> String {
        vdapi::get_current_desktop().map(|d| d.id()).unwrap_or_default()
    }

    struct Report {
        text: String,
        failed: u32,
        dir: String,
    }

    impl Report {
        fn line(&mut self, line: impl AsRef<str>) {
            println!("{}", line.as_ref());
            let _ = writeln!(self.text, "{}", line.as_ref());
        }

        fn check(&mut self, what: &str, ok: bool, detail: impl std::fmt::Display) {
            if !ok {
                self.failed += 1;
            }
            self.line(format!("{} {what}: {detail}", if ok { "PASS" } else { "FAIL" }));
        }

        fn colour(&mut self, shot: &Shot, what: &str, (fx, fy): (f64, f64), want: &str) {
            let got = name(shot.at(fx, fy));
            self.check(what, got == want, format!("({fx:.2}, {fy:.2}) is {got} ({:06x}), want {want}", shot.at(fx, fy)));
        }

        fn shot(&mut self, label: &str) -> Shot {
            let shot = Shot::take(work_area());
            shot.save(&format!("{}/{label}.bmp", self.dir));
            shot
        }
    }

    /// Where along a line across the screen one colour gives way to the next.
    fn edge(shot: &Shot, horizontal: bool, at: f64, from: &str) -> Option<f64> {
        (0..1000).map(|i| i as f64 / 1000.0).find(|&f| {
            let c = if horizontal { shot.at(f, at) } else { shot.at(at, f) };
            name(c) != from
        })
    }

    /// The pixels just outside each run of `color` along a line across the
    /// screen that are neither black nor `color` darkened toward it: what is
    /// beneath the window showing at its edges.
    fn fringe(shot: &Shot, horizontal: bool, at: f64, color: u32) -> Vec<u32> {
        let n = if horizontal { shot.w } else { shot.h };
        let pixel = |i: i32| {
            let f = (i as f64 + 0.5) / n as f64;
            if horizontal { shot.at(f, at) } else { shot.at(at, f) }
        };
        let channels = |c: u32| [(c >> 16 & 0xFF) as f64, (c >> 8 & 0xFF) as f64, (c & 0xFF) as f64];
        let want = channels(color);
        let darkened = |c: u32| {
            let got = channels(c);
            let k = got[2] / want[2];
            (0..3).map(|i| (got[i] - want[i] * k).abs()).sum::<f64>() < 30.0
        };
        let inside: Vec<bool> = (0..n).map(|i| name(pixel(i)) == name(color)).collect();
        let mut found = Vec::new();
        for i in 0..n {
            let edge = (1..=3).any(|d| [i - d, i + d].into_iter().any(|j| (0..n).contains(&j) && inside[j as usize]));
            if !inside[i as usize] && edge && !darkened(pixel(i)) {
                found.push(pixel(i));
            }
        }
        found
    }

    /// A's windows end at the edges of A's cell, and the pinned window stays
    /// beneath the window over it while the screen slides whole, rising in
    /// front only as it recedes.
    fn cliplift(dir: &str) {
        let mut r = Report { text: String::new(), failed: 0, dir: dir.to_owned() };
        let ids: Vec<String> = std::fs::read_to_string(format!("{dir}/ids.txt")).unwrap().lines().map(str::to_owned).collect();
        let (a, b) = (ids[0].clone(), ids[1].clone());
        let desktop = |id: &str| vdapi::get_desktops().unwrap().into_iter().find(|d| d.id() == id).unwrap();
        let _ = vdapi::switch_desktop(desktop(&a));
        std::thread::sleep(Duration::from_millis(800));
        let area = work_area();
        let (w, h) = (area.right - area.left, area.bottom - area.top);
        let me = std::env::current_exe().unwrap();
        let spawn = |color: u32, rect: (i32, i32, i32, i32), on: &str| {
            Command::new(&me)
                .args(["window", &format!("{color:06x}"), &rect.0.to_string(), &rect.1.to_string(), &rect.2.to_string(), &rect.3.to_string(), on, "60"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap()
        };
        let x = |f: f64| area.left + (w as f64 * f) as i32;
        let y = |f: f64| area.top + (h as f64 * f) as i32;
        let mut children = vec![spawn(RED, (area.left, area.top, w, h), &a), spawn(BLUE, (0, 0, 0, 0), &b)];
        std::thread::sleep(Duration::from_millis(1500));
        // Pinned, then a window of A over its lower right; and one of A half off the right.
        children.push(spawn(GREEN, (x(0.40), y(0.10), w / 5, h / 5), "pin"));
        std::thread::sleep(Duration::from_millis(1500));
        children.push(spawn(YELLOW, (x(0.50), y(0.20), w * 4 / 10, h * 3 / 10), &a));
        children.push(spawn(MAGENTA, (x(0.75), y(0.60), w / 2, h / 4), &a));
        std::thread::sleep(Duration::from_millis(2500));
        let start = r.shot("c0-start");
        r.colour(&start, "start: yellow over the pinned window", (0.55, 0.25), "yellow");
        r.colour(&start, "start: pinned window", (0.45, 0.15), "green");

        // A flick: shot while the hand is still going, the screen whole.
        hold(true);
        std::thread::sleep(Duration::from_millis(40));
        travel(140, 0, 50);
        let quick = r.shot("c1-quick");
        travel(40, 0, 20);
        hold(false);
        r.line(format!("quick: A ends at {:?} across (row 0.35)", edge(&quick, true, 0.35, "red")));
        r.line(format!("quick: along y 0.72 {:?}", (0..20).map(|i| name(quick.at(i as f64 / 20.0, 0.72))).collect::<Vec<_>>()));
        r.line(format!("quick: along y 0.25 {:?}", (0..20).map(|i| name(quick.at(i as f64 / 20.0, 0.25))).collect::<Vec<_>>()));
        std::thread::sleep(Duration::from_millis(1500));
        let _ = vdapi::switch_desktop(desktop(&a));
        std::thread::sleep(Duration::from_millis(1500));

        // Unsure: the screen recedes, the pinned window rises in front.
        hold(true);
        std::thread::sleep(Duration::from_millis(60));
        travel(160, 0, 200);
        std::thread::sleep(Duration::from_millis(300));
        let receded = r.shot("c2-receded");
        r.colour(&receded, "receded: pinned window in front of yellow", (0.55, 0.25), "green");
        r.line(format!("receded: along y 0.6 {:?}", (0..20).map(|i| name(receded.at(i as f64 / 20.0, 0.6))).collect::<Vec<_>>()));
        travel(-160, 0, 200);
        std::thread::sleep(Duration::from_millis(300));
        hold(false);
        std::thread::sleep(Duration::from_millis(1000));

        for mut child in children {
            let _ = child.kill();
        }
        r.line(format!("{} failed", r.failed));
        let _ = std::fs::write(format!("{dir}/report.txt"), &r.text);
    }

    fn run(dir: &str, row_exe: &str) {
        let mut r = Report { text: String::new(), failed: 0, dir: dir.to_owned() };
        let ids: Vec<String> = std::fs::read_to_string(format!("{dir}/ids.txt")).unwrap().lines().map(str::to_owned).collect();
        let (a, b, c) = (ids[0].clone(), ids[1].clone(), ids[2].clone());
        let desktop = |id: &str| vdapi::get_desktops().unwrap().into_iter().find(|d| d.id() == id).unwrap();
        let _ = vdapi::switch_desktop(desktop(&a));
        std::thread::sleep(Duration::from_millis(800));
        let area = work_area();
        let (w, h) = (area.right - area.left, area.bottom - area.top);
        r.line(format!("work area {area:?}; A {a} B {b} C {c}"));
        let me = std::env::current_exe().unwrap();
        let spawn = |exe: &std::path::Path, color: u32, rect: (i32, i32, i32, i32), on: &str| {
            Command::new(exe)
                .args(["window", &format!("{color:06x}"), &rect.0.to_string(), &rect.1.to_string(), &rect.2.to_string(), &rect.3.to_string(), on, "120"])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap()
        };
        let whole = (area.left, area.top, w, h);
        // The pinned window at the top left, the row's at the bottom right.
        let pinned = (area.left + w * 5 / 100, area.top + h * 5 / 100, w / 10, h / 10);
        let row = (area.left + w * 75 / 100, area.top + h * 70 / 100, w * 15 / 100, h * 15 / 100);
        let mut children = vec![
            spawn(&me, RED, whole, &a),
            spawn(&me, BLUE, (0, 0, 0, 0), &b),
            spawn(&me, YELLOW, whole, &c),
        ];
        std::thread::sleep(Duration::from_millis(1500));
        children.push(spawn(&me, GREEN, pinned, "pin"));
        children.push(spawn(std::path::Path::new(row_exe), MAGENTA, row, &a));
        std::thread::sleep(Duration::from_millis(2500));
        let pin_at = (0.1, 0.1);
        let row_at = (0.825, 0.775);

        let start = r.shot("0-start");
        r.colour(&start, "start: A", (0.8, 0.35), "red");
        r.colour(&start, "start: pinned window", pin_at, "green");
        r.colour(&start, "start: row window", row_at, "magenta");

        // Sideways, slowly, a little past halfway: B slides in, then is gone to.
        hold(true);
        std::thread::sleep(Duration::from_millis(60));
        travel(160, 0, 200);
        std::thread::sleep(Duration::from_millis(250));
        let mid = r.shot("1-mid-right");
        let held_on = current();
        r.check("held: still on A", held_on == a, &held_on);
        r.colour(&mid, "held: A on the left", (0.15, 0.35), "red");
        r.colour(&mid, "held: B on the right", (0.8, 0.35), "blue");
        r.colour(&mid, "held: pinned window in place", pin_at, "green");
        r.colour(&mid, "held: row window in place sideways", row_at, "magenta");
        let boundary = edge(&mid, true, 0.35, "red");
        // Receded to 90% about the middle, the strip at about 0.66 with the
        // notches: A ends at about 34% across, then a dark gap, then B.
        r.check("held: A ends about 34% across", boundary.is_some_and(|b| (0.29..0.40).contains(&b)), format!("{boundary:?}"));
        r.colour(&mid, "held: receded, dark above A", (0.15, 0.02), "black");
        // B's window is maximized: nothing beneath it shows at its left and bottom edges.
        let seen = [fringe(&mid, true, 0.5, BLUE), fringe(&mid, false, 0.8, BLUE)].concat();
        r.check("held: no wallpaper at B's maximized window's edges", seen.is_empty(), format!("{:06x?}", &seen[..seen.len().min(12)]));
        hold(false);
        let released = Instant::now();
        let mut settling = Vec::new();
        for i in 0..6 {
            let shot = Shot::take(area);
            settling.push((released.elapsed().as_millis(), edge(&shot, true, 0.35, "red")));
            if i == 2 {
                shot.save(&format!("{dir}/2-settling.bmp"));
            }
            std::thread::sleep(Duration::from_millis(40));
        }
        r.line(format!("settling, ms after letting go and where B begins: {settling:?}"));
        std::thread::sleep(Duration::from_millis(1200));
        let after = r.shot("3-after-right");
        r.check("let go: on B", current() == b, current());
        r.colour(&after, "let go: B shown", (0.8, 0.35), "blue");
        r.colour(&after, "let go: pinned window in place", pin_at, "green");
        r.colour(&after, "let go: row window came along", row_at, "magenta");

        // Flung back left from a little way: A.
        hold(true);
        std::thread::sleep(Duration::from_millis(60));
        travel(-90, 0, 80);
        hold(false);
        std::thread::sleep(Duration::from_millis(60));
        r.shot("4-flung");
        std::thread::sleep(Duration::from_millis(1200));
        let flung = r.shot("5-after-fling");
        r.check("flung: back on A", current() == a, current());
        r.colour(&flung, "flung: A shown", (0.8, 0.35), "red");

        // Down, a little past halfway: C comes up, the row window goes up
        // with its row, the pinned one stays.
        hold(true);
        std::thread::sleep(Duration::from_millis(60));
        travel(0, 220, 250);
        std::thread::sleep(Duration::from_millis(250));
        let down = r.shot("6-mid-down");
        r.check("held down: still on A", current() == a, current());
        r.colour(&down, "held down: A above", (0.9, 0.2), "red");
        r.colour(&down, "held down: C below", (0.9, 0.8), "yellow");
        r.colour(&down, "held down: pinned window in place", pin_at, "green");
        let up = edge(&down, false, 0.9, "red");
        r.check("held down: A ends about 40% down", up.is_some_and(|b| (0.34..0.46).contains(&b)), format!("{up:?}"));
        r.colour(&down, "held down: row window receded and went up with its row", (0.79, 0.19), "magenta");
        hold(false);
        std::thread::sleep(Duration::from_millis(1300));
        let below = r.shot("7-after-down");
        r.check("let go: on C", current() == c, current());
        r.colour(&below, "let go: C shown", (0.8, 0.35), "yellow");
        r.colour(&below, "let go: pinned window in place", pin_at, "green");

        // Slantwise from A: four desktops at once.
        let _ = vdapi::switch_desktop(desktop(&a));
        std::thread::sleep(Duration::from_millis(1200));
        hold(true);
        std::thread::sleep(Duration::from_millis(60));
        travel(110, 170, 250);
        std::thread::sleep(Duration::from_millis(250));
        let slant = r.shot("8-slantwise");
        r.colour(&slant, "slantwise: A at the top left", (0.2, 0.3), "red");
        r.colour(&slant, "slantwise: B at the top right", (0.8, 0.3), "blue");
        r.colour(&slant, "slantwise: C at the bottom left", (0.3, 0.9), "yellow");
        // Back where it began, slowly.
        travel(-110, -170, 250);
        std::thread::sleep(Duration::from_millis(300));
        hold(false);
        std::thread::sleep(Duration::from_millis(1000));
        r.check("slantwise and back: on A", current() == a, current());

        // Following turned off: each desktop is gone to as the hand comes to it.
        write_config(false);
        std::thread::sleep(Duration::from_millis(3000));
        hold(true);
        std::thread::sleep(Duration::from_millis(60));
        travel(300, 0, 200);
        std::thread::sleep(Duration::from_millis(500));
        let off = r.shot("9-off-held");
        r.check("off: on B while held", current() == b, current());
        r.colour(&off, "off: B shown while held", (0.8, 0.35), "blue");
        hold(false);
        std::thread::sleep(Duration::from_millis(800));
        r.check("off: on B", current() == b, current());
        write_config(true);

        for mut child in children {
            let _ = child.kill();
        }
        r.line(format!("{} failed", r.failed));
        let _ = std::fs::write(format!("{dir}/report.txt"), &r.text);
    }
}
