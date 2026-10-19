//! Prototype: a desktop switch that slides with the hand, like Spaces on a
//! Mac, made of live DWM thumbnails instead of pictures (examples/spaces.rs
//! is the picture version). A topmost, click-through window over the work
//! area shows the wallpaper of each desktop and every one of its windows as
//! thumbnails, stacked as the windows are; a slide only moves their
//! rectangles. Windows shown on all desktops stay where they are. At the end
//! the overlay shows the target desktop exactly where it will be, the real
//! switch happens under it, and the overlay goes. Not part of the app; it
//! finds out whether this works and what it costs.
//! "Right" is the next desktop in Windows' order and "down" the one after
//! (the prototype has no grid).
//!   slide_poc stage        test windows on this desktop and the next two:
//!                          GDI tickers, Edge pages (one with Chromium's
//!                          occlusion off), a console, Calculator; one pinned
//!                          to all desktops, two minimized
//!   slide_poc unstage      closes them, removes the desktops stage made
//!   slide_poc list         the windows and the layer each goes in
//!   slide_poc run [right|down] [ms <n>] [hold <ms>] [hand] [switch] [stay]
//!                 [picture] [out <dir>]
//!       measures: fidelity, live thumbnails of the other desktop, frame
//!       cost, and with `switch` the real switch under the overlay, recorded
//!       frame by frame. Writes report.txt and .bmp pictures to <dir>
//!       (default poc-out\<direction>). This desktop's background is the
//!       shell's desktop window (icons and all, live); `picture` draws its
//!       wallpaper file instead, as for the other desktops.
#[allow(dead_code)]
#[path = "../src/vdapi.rs"]
mod vdapi;

#[cfg(windows)]
fn main() {
    let args: Vec<String> = std::env::args().collect();
    unsafe {
        let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
            windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        );
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
    }
    if args.get(1).map(String::as_str) == Some("ticker") {
        return poc::ticker(&args[2..]);
    }
    if vdapi::select(false).is_none() {
        return println!("no virtual desktop backend for this Windows");
    }
    let result = match args.get(1).map(String::as_str) {
        Some("stage") => poc::stage(),
        Some("unstage") => poc::unstage(),
        Some("list") => poc::list(),
        Some("run") => poc::run(&args[2..]),
        _ => {
            println!("slide_poc stage | unstage | list | run [right|down] [ms n] [hold ms] [hand] [switch] [stay] [out dir]");
            Ok(())
        }
    };
    if let Err(e) = result {
        println!("error: {e}");
    }
}

#[cfg(not(windows))]
fn main() {}

#[cfg(windows)]
mod poc {
    use super::vdapi;
    use std::{
        collections::HashSet,
        fmt::Write as _,
        os::windows::process::CommandExt,
        process::{Command, Stdio},
        sync::{
            atomic::{AtomicBool, AtomicU32, Ordering},
            mpsc, Arc, OnceLock,
        },
        time::{Duration, Instant},
    };
    use windows::{
        core::{w, HSTRING, PCWSTR, PWSTR},
        Win32::{
            Foundation::{
                CloseHandle, BOOL, COLORREF, SIZE, FILETIME, GENERIC_READ, HANDLE, HWND, LPARAM, LRESULT, POINT, RECT,
                TRUE, WPARAM,
            },
            Graphics::{
                Dwm::*,
                Dxgi::{CreateDXGIFactory1, IDXGIFactory1, IDXGIOutput, DXGI_OUTPUT_DESC},
                Gdi::*,
                Imaging::*,
            },
            System::{
                Com::{CoCreateInstance, CLSCTX_INPROC_SERVER},
                Diagnostics::ToolHelp::{
                    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
                },
                LibraryLoader::GetModuleHandleW,
                Power::{SetThreadExecutionState, ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED},
                Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_SZ},
                SystemInformation::GetLocalTime,
                Threading::{
                    GetCurrentProcess, GetProcessTimes, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
                    PROCESS_QUERY_LIMITED_INFORMATION,
                },
            },
            UI::{
                HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI},
                Input::KeyboardAndMouse::{SendInput, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_MOVE, MOUSEINPUT},
                WindowsAndMessaging::*,
            },
        },
    };

    type R<T> = std::result::Result<T, Box<dyn std::error::Error>>;

    fn vd<T>(r: vdapi::Result<T>) -> R<T> {
        r.map_err(|e| format!("{e:?}").into())
    }

    fn ms(d: Duration) -> f64 {
        d.as_secs_f64() * 1000.0
    }

    /// Prints and keeps a copy for report.txt.
    struct Report(String);

    impl Report {
        fn line(&mut self, s: impl AsRef<str>) {
            println!("{}", s.as_ref());
            let _ = writeln!(self.0, "{}", s.as_ref());
        }
    }

    // ---- windows and desktops ---------------------------------------------

    fn text(hwnd: HWND) -> String {
        let mut b = [0u16; 256];
        let n = unsafe { GetWindowTextW(hwnd, &mut b) }.max(0) as usize;
        String::from_utf16_lossy(&b[..n])
    }

    fn class(hwnd: HWND) -> String {
        let mut b = [0u16; 128];
        let n = unsafe { GetClassNameW(hwnd, &mut b) }.max(0) as usize;
        String::from_utf16_lossy(&b[..n])
    }

    fn exe(hwnd: HWND) -> String {
        unsafe {
            let mut pid = 0;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            let Ok(p) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return format!("pid {pid}") };
            let mut b = [0u16; 512];
            let mut n = b.len() as u32;
            let ok = QueryFullProcessImageNameW(p, PROCESS_NAME_WIN32, PWSTR(b.as_mut_ptr()), &mut n).is_ok();
            let _ = CloseHandle(p);
            if !ok {
                return format!("pid {pid}");
            }
            let path = String::from_utf16_lossy(&b[..n as usize]);
            path.rsplit('\\').next().unwrap_or("").to_string()
        }
    }

    fn cloaked(hwnd: HWND) -> u32 {
        let mut v = 0u32;
        let _ = unsafe { DwmGetWindowAttribute(hwnd, DWMWA_CLOAKED, &mut v as *mut u32 as *mut _, 4) };
        v
    }

    /// The visible frame, without the invisible resize borders; a
    /// thumbnail's source starts here.
    fn frame_rect(hwnd: HWND) -> RECT {
        let mut r = RECT::default();
        if unsafe { DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, &mut r as *mut RECT as *mut _, 16) }.is_err() {
            return window_rect(hwnd);
        }
        r
    }

    fn window_rect(hwnd: HWND) -> RECT {
        let mut r = RECT::default();
        let _ = unsafe { GetWindowRect(hwnd, &mut r) };
        r
    }

    /// The same test as the app's: visible, unowned, titled, no tool window.
    fn is_app_window(hwnd: HWND) -> bool {
        unsafe {
            IsWindowVisible(hwnd).as_bool()
                && GetWindow(hwnd, GW_OWNER).0 == 0
                && GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 == 0
                && GetWindowTextLengthW(hwnd) > 0
                && hwnd != GetShellWindow()
                // A store app's inner window: its frame is the app window.
                && class(hwnd) != "Windows.UI.Core.CoreWindow"
        }
    }

    /// App windows, topmost first.
    fn app_windows() -> Vec<HWND> {
        unsafe extern "system" fn each(hwnd: HWND, out: LPARAM) -> BOOL {
            if is_app_window(hwnd) {
                (*(out.0 as *mut Vec<HWND>)).push(hwnd);
            }
            TRUE
        }
        let mut out: Vec<HWND> = Vec::new();
        let _ = unsafe { EnumWindows(Some(each), LPARAM(&mut out as *mut _ as isize)) };
        out
    }

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    enum Layer {
        Here,
        Right,
        Down,
        Pinned,
        /// The overlay's own wallpaper windows.
        Wall(usize),
    }

    struct Win {
        hwnd: HWND,
        layer: Layer,
        rect: RECT,
        minimized: bool,
        title: String,
        class: String,
        exe: String,
        cloaked: u32,
    }

    struct Desks {
        all: Vec<vdapi::Desktop>,
        here: usize,
    }

    impl Desks {
        fn read() -> R<Desks> {
            let all = vd(vdapi::get_desktops())?;
            let current = vd(vdapi::get_current_desktop())?;
            let here = all.iter().position(|d| *d == current).ok_or("current desktop not in the list")?;
            Ok(Desks { all, here })
        }
        fn at(&self, by: usize) -> Option<vdapi::Desktop> {
            self.all.get(self.here + by).copied()
        }
    }

    /// Every app window on this desktop and the two after it, plus the
    /// pinned ones, topmost first; minimized ones are kept but marked.
    fn survey(desks: &Desks) -> Vec<Win> {
        let ids: Vec<Option<String>> = (0..3).map(|i| desks.at(i).map(|d| d.id())).collect();
        let mut out = Vec::new();
        for hwnd in app_windows() {
            let pinned = vdapi::is_pinned_window(hwnd).unwrap_or(false) || vdapi::is_pinned_app(hwnd).unwrap_or(false);
            let layer = if pinned {
                Layer::Pinned
            } else {
                let Ok(d) = vdapi::get_desktop_by_window(hwnd) else { continue };
                let id = Some(d.id());
                if id == ids[0] {
                    Layer::Here
                } else if id == ids[1] {
                    Layer::Right
                } else if id == ids[2] {
                    Layer::Down
                } else {
                    continue;
                }
            };
            let c = cloaked(hwnd);
            // On the desktop being shown a cloaked window is a suspended store
            // app, not one the user sees.
            if layer == Layer::Here && c != 0 {
                continue;
            }
            out.push(Win {
                hwnd,
                layer,
                rect: frame_rect(hwnd),
                minimized: unsafe { IsIconic(hwnd) }.as_bool(),
                title: text(hwnd).chars().take(28).collect(),
                class: class(hwnd).chars().take(28).collect(),
                exe: exe(hwnd),
                cloaked: c,
            });
        }
        out
    }

    pub fn list() -> R<()> {
        let desks = Desks::read()?;
        println!("{} desktops, current index {}", desks.all.len(), desks.here);
        for w in survey(&desks) {
            println!(
                "{:?}{} cloaked {} {:?} {} | {} | {}",
                w.layer,
                if w.minimized { " (minimized)" } else { "" },
                w.cloaked,
                (w.rect.left, w.rect.top, w.rect.right, w.rect.bottom),
                w.exe,
                w.class,
                w.title
            );
        }
        Ok(())
    }

    // ---- monitors -----------------------------------------------------------

    struct Monitor {
        handle: HMONITOR,
        bounds: RECT,
        work: RECT,
        dpi: u32,
        primary: bool,
    }

    fn monitors() -> Vec<Monitor> {
        unsafe extern "system" fn each(m: HMONITOR, _: HDC, _: *mut RECT, out: LPARAM) -> BOOL {
            let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            if GetMonitorInfoW(m, &mut info).as_bool() {
                let (mut x, mut y) = (96, 96);
                let _ = GetDpiForMonitor(m, MDT_EFFECTIVE_DPI, &mut x, &mut y);
                (*(out.0 as *mut Vec<Monitor>)).push(Monitor {
                    handle: m,
                    bounds: info.rcMonitor,
                    work: info.rcWork,
                    dpi: x,
                    primary: info.dwFlags & 1 != 0,
                });
            }
            TRUE
        }
        let mut out: Vec<Monitor> = Vec::new();
        unsafe { EnumDisplayMonitors(None, None, Some(each), LPARAM(&mut out as *mut _ as isize)) };
        out
    }

    fn output_for(monitor: HMONITOR) -> Option<IDXGIOutput> {
        unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
            let mut a = 0;
            while let Ok(adapter) = factory.EnumAdapters(a) {
                let mut o = 0;
                while let Ok(output) = adapter.EnumOutputs(o) {
                    let mut desc = DXGI_OUTPUT_DESC::default();
                    if output.GetDesc(&mut desc).is_ok() && desc.Monitor == monitor {
                        return Some(output);
                    }
                    o += 1;
                }
                a += 1;
            }
        }
        None
    }

    // ---- messages -----------------------------------------------------------

    fn pump() {
        unsafe {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }

    fn wait(ms: u64) {
        let end = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < end {
            pump();
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn flush() {
        unsafe {
            let _ = DwmFlush();
            let _ = DwmFlush();
        }
    }

    // ---- pictures of the screen ---------------------------------------------

    #[derive(Clone)]
    struct Img {
        w: i32,
        h: i32,
        px: Vec<u8>,
    }

    /// The screen in `r` as DWM composed it, layered windows included,
    /// scaled down by `div`.
    fn shot(r: RECT, div: i32) -> Img {
        unsafe {
            let (w, h) = (r.right - r.left, r.bottom - r.top);
            let (sw, sh) = (w / div, h / div);
            let screen = GetDC(None);
            let dc = CreateCompatibleDC(screen);
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: sw,
                    biHeight: -sh,
                    biPlanes: 1,
                    biBitCount: 32,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, None, 0).unwrap();
            let old = SelectObject(dc, bitmap);
            let rop = ROP_CODE(SRCCOPY.0 | CAPTUREBLT.0);
            if div == 1 {
                let _ = BitBlt(dc, 0, 0, w, h, screen, r.left, r.top, rop);
            } else {
                SetStretchBltMode(dc, COLORONCOLOR);
                StretchBlt(dc, 0, 0, sw, sh, screen, r.left, r.top, w, h, rop);
            }
            let px = std::slice::from_raw_parts(bits as *const u8, (sw * sh * 4) as usize).to_vec();
            SelectObject(dc, old);
            DeleteObject(bitmap);
            DeleteDC(dc);
            ReleaseDC(None, screen);
            Img { w: sw, h: sh, px }
        }
    }

    fn save_bmp(img: &Img, path: &str) {
        let row = ((img.w * 3 + 3) & !3) as usize;
        let size = 54 + row * img.h as usize;
        let mut b = Vec::with_capacity(size);
        b.extend_from_slice(b"BM");
        b.extend_from_slice(&(size as u32).to_le_bytes());
        b.extend_from_slice(&[0, 0, 0, 0, 54, 0, 0, 0, 40, 0, 0, 0]);
        b.extend_from_slice(&img.w.to_le_bytes());
        b.extend_from_slice(&img.h.to_le_bytes());
        b.extend_from_slice(&[1, 0, 24, 0, 0, 0, 0, 0]);
        b.extend_from_slice(&((row * img.h as usize) as u32).to_le_bytes());
        b.extend_from_slice(&[0; 16]);
        for y in (0..img.h).rev() {
            let start = b.len();
            for x in 0..img.w {
                let i = ((y * img.w + x) * 4) as usize;
                b.extend_from_slice(&img.px[i..i + 3]);
            }
            b.resize(start + row, 0);
        }
        let _ = std::fs::write(path, b);
    }

    /// Mean difference per channel (0..255) and the share of pixels that
    /// changed by more than a little, over the pixels `mask` allows.
    fn diff(a: &Img, b: &Img, mask: Option<&dyn Fn(i32, i32) -> bool>) -> (f64, f64) {
        let (mut sum, mut changed, mut n) = (0u64, 0u64, 0u64);
        for y in 0..a.h.min(b.h) {
            for x in 0..a.w.min(b.w) {
                if mask.is_some_and(|m| !m(x, y)) {
                    continue;
                }
                let i = ((y * a.w + x) * 4) as usize;
                let j = ((y * b.w + x) * 4) as usize;
                let mut most = 0;
                for c in 0..3 {
                    let d = (a.px[i + c] as i32 - b.px[j + c] as i32).unsigned_abs();
                    sum += d as u64;
                    most = most.max(d);
                }
                if most > 24 {
                    changed += 1;
                }
                n += 1;
            }
        }
        if n == 0 {
            return (0.0, 0.0);
        }
        (sum as f64 / (n * 3) as f64, changed as f64 / n as f64)
    }

    /// Mean brightness and its spread.
    fn luma(a: &Img, mask: Option<&dyn Fn(i32, i32) -> bool>) -> (f64, f64) {
        let (mut sum, mut sq, mut n) = (0f64, 0f64, 0f64);
        for y in 0..a.h {
            for x in 0..a.w {
                if mask.is_some_and(|m| !m(x, y)) {
                    continue;
                }
                let i = ((y * a.w + x) * 4) as usize;
                let l = 0.114 * a.px[i] as f64 + 0.587 * a.px[i + 1] as f64 + 0.299 * a.px[i + 2] as f64;
                sum += l;
                sq += l * l;
                n += 1.0;
            }
        }
        if n == 0.0 {
            return (0.0, 0.0);
        }
        let mean = sum / n;
        (mean, (sq / n - mean * mean).max(0.0).sqrt())
    }

    /// Keeps taking small pictures of the screen on its own thread.
    struct Recorder {
        stop: Arc<AtomicBool>,
        thread: std::thread::JoinHandle<Vec<(f64, Img)>>,
    }

    impl Recorder {
        fn start(r: RECT, div: i32, t0: Instant) -> Recorder {
            let stop = Arc::new(AtomicBool::new(false));
            let flag = stop.clone();
            let thread = std::thread::spawn(move || {
                let mut frames = Vec::new();
                while !flag.load(Ordering::Relaxed) && frames.len() < 2000 {
                    let img = shot(r, div);
                    frames.push((ms(t0.elapsed()), img));
                }
                frames
            });
            Recorder { stop, thread }
        }
        fn finish(self) -> Vec<(f64, Img)> {
            self.stop.store(true, Ordering::Relaxed);
            self.thread.join().unwrap_or_default()
        }
    }

    // ---- wallpapers -----------------------------------------------------------

    /// The wallpaper file of the desktop at `index` in Windows' order, and
    /// where the path came from.
    fn wallpaper_of(index: u32) -> (String, &'static str) {
        let own = match vdapi::select(false) {
            Some(vdapi::Backend::Win23H2) => winvd_23h2::get_desktop(index).get_wallpaper().ok(),
            Some(vdapi::Backend::Win24H2) => winvd_24h2::get_desktop(index).get_wallpaper().ok(),
            None => None,
        };
        if let Some(p) = own.filter(|p| !p.is_empty()) {
            return (p, "IVirtualDesktop::GetWallpaper");
        }
        // Where Explorer keeps each desktop's own wallpaper.
        if let Some(id) = vdapi::get_desktops().ok().and_then(|all| all.get(index as usize).map(|d| d.id())) {
            let key = HSTRING::from(format!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\VirtualDesktops\\Desktops\\{{{id}}}"));
            let mut b = [0u16; 260];
            let mut size = std::mem::size_of_val(&b) as u32;
            let ok = unsafe { RegGetValueW(HKEY_CURRENT_USER, &key, w!("Wallpaper"), RRF_RT_REG_SZ, None, Some(b.as_mut_ptr().cast()), Some(&mut size)) }.is_ok();
            let n = b.iter().position(|c| *c == 0).unwrap_or(0);
            if ok && n > 0 {
                return (String::from_utf16_lossy(&b[..n]), "registry VirtualDesktops\\Desktops\\{id}\\Wallpaper");
            }
        }
        let mut b = [0u16; 260];
        let ok = unsafe {
            SystemParametersInfoW(SPI_GETDESKWALLPAPER, b.len() as u32, Some(b.as_mut_ptr().cast()), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0))
        }
        .is_ok();
        let n = b.iter().position(|c| *c == 0).unwrap_or(0);
        if ok && n > 0 {
            return (String::from_utf16_lossy(&b[..n]), "SPI_GETDESKWALLPAPER");
        }
        (String::new(), "none (solid colour)")
    }

    fn load_image(path: &str) -> R<(u32, u32, Vec<u8>)> {
        unsafe {
            let f: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
            let dec = f.CreateDecoderFromFilename(&HSTRING::from(path), None, GENERIC_READ, WICDecodeMetadataCacheOnDemand)?;
            let frame = dec.GetFrame(0)?;
            let bgra = WICConvertBitmapSource(&GUID_WICPixelFormat32bppBGRA, &frame)?;
            let (mut w, mut h) = (0, 0);
            bgra.GetSize(&mut w, &mut h)?;
            let mut px = vec![0u8; (w * h * 4) as usize];
            bgra.CopyPixels(std::ptr::null(), w * 4, &mut px)?;
            Ok((w, h, px))
        }
    }

    /// A window off every screen that shows one desktop's wallpaper as it
    /// fills a monitor; the overlay shows it through a thumbnail like any
    /// other window, so it moves in the same composed frame as they do.
    struct Wall {
        hwnd: HWND,
        dc: HDC,
        note: String,
    }

    unsafe extern "system" fn wall_proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        if m == WM_PAINT {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(h, &mut ps);
            let src = HDC(GetWindowLongPtrW(h, GWLP_USERDATA));
            let r = window_rect(h);
            let _ = BitBlt(hdc, 0, 0, r.right - r.left, r.bottom - r.top, src, 0, 0, SRCCOPY);
            EndPaint(h, &ps);
            return LRESULT(0);
        }
        if m == WM_ERASEBKGND {
            return LRESULT(1);
        }
        DefWindowProcW(h, m, w, l)
    }

    unsafe fn make_wall(index: u32, mon: &Monitor, slot: i32) -> Wall {
        let (w, h) = (mon.bounds.right - mon.bounds.left, mon.bounds.bottom - mon.bounds.top);
        let screen = GetDC(None);
        let dc = CreateCompatibleDC(screen);
        let bitmap = CreateCompatibleBitmap(screen, w, h);
        ReleaseDC(None, screen);
        SelectObject(dc, bitmap);
        let (path, source) = wallpaper_of(index);
        let t = Instant::now();
        let note = match (!path.is_empty()).then(|| load_image(&path)) {
            Some(Ok((iw, ih, px))) => {
                // Fill: as large as covers the monitor, the middle of it.
                let scale = (w as f64 / iw as f64).max(h as f64 / ih as f64);
                let (sw, sh) = ((w as f64 / scale) as i32, (h as f64 / scale) as i32);
                let info = BITMAPINFO {
                    bmiHeader: BITMAPINFOHEADER {
                        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: iw as i32,
                        biHeight: -(ih as i32),
                        biPlanes: 1,
                        biBitCount: 32,
                        ..Default::default()
                    },
                    ..Default::default()
                };
                SetStretchBltMode(dc, HALFTONE);
                let _ = SetBrushOrgEx(dc, 0, 0, None);
                StretchDIBits(dc, 0, 0, w, h, (iw as i32 - sw) / 2, (ih as i32 - sh) / 2, sw, sh, Some(px.as_ptr().cast()), &info, DIB_RGB_COLORS, SRCCOPY);
                format!("{path} ({source}) {iw}x{ih}, decoded and scaled in {:.0} ms", ms(t.elapsed()))
            }
            other => {
                let brush = CreateSolidBrush(COLORREF(GetSysColor(COLOR_BACKGROUND)));
                FillRect(dc, &RECT { left: 0, top: 0, right: w, bottom: h }, brush);
                DeleteObject(brush);
                match other {
                    Some(Err(e)) => format!("{path} ({source}) could not be read: {e}; solid colour instead"),
                    _ => format!("{source}"),
                }
            }
        };
        let x = GetSystemMetrics(SM_XVIRTUALSCREEN) + GetSystemMetrics(SM_CXVIRTUALSCREEN) + 100 + slot * (w + 100);
        let hinst = GetModuleHandleW(None).unwrap();
        // Layered, so its picture is kept by DWM although it is never on a
        // screen (an off-screen window gets no WM_PAINT).
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_LAYERED,
            w!("flick.poc.wall"),
            w!("slide_poc wall"),
            WS_POPUP,
            x,
            mon.bounds.top,
            w,
            h,
            None,
            None,
            hinst,
            None,
        );
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, dc.0);
        let blend = BLENDFUNCTION { BlendOp: 0, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: 0 };
        let screen = GetDC(None);
        let _ = UpdateLayeredWindow(hwnd, screen, Some(&POINT { x, y: mon.bounds.top }), Some(&SIZE { cx: w, cy: h }), dc, Some(&POINT::default()), COLORREF(0), Some(&blend), ULW_OPAQUE);
        ReleaseDC(None, screen);
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        Wall { hwnd, dc, note }
    }

    // ---- the overlay ------------------------------------------------------------

    struct Thumb {
        id: isize,
        layer: Layer,
        /// Where the window is, in overlay coordinates.
        dest: RECT,
        /// For a wallpaper: the part of it under the overlay.
        source: Option<RECT>,
        win: usize,
    }

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    enum Dir {
        Right,
        Down,
    }

    struct Overlay {
        hwnd: HWND,
        area: RECT,
        thumbs: Vec<Thumb>,
        calls: u64,
        call_time: f64,
    }

    unsafe extern "system" fn overlay_proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        DefWindowProcW(h, m, w, l)
    }

    impl Overlay {
        unsafe fn new(area: RECT) -> Overlay {
            let hinst = GetModuleHandleW(None).unwrap();
            let hwnd = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT | WS_EX_LAYERED,
                w!("flick.poc.overlay"),
                w!("slide_poc"),
                WS_POPUP,
                area.left,
                area.top,
                area.right - area.left,
                area.bottom - area.top,
                None,
                None,
                hinst,
                None,
            );
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
            Overlay { hwnd, area, thumbs: Vec::new(), calls: 0, call_time: 0.0 }
        }

        fn size(&self) -> (i32, i32) {
            (self.area.right - self.area.left, self.area.bottom - self.area.top)
        }

        /// Adds a thumbnail above the ones before it.
        unsafe fn add(&mut self, hwnd: HWND, layer: Layer, rect: RECT, source: Option<RECT>, win: usize) -> Option<f64> {
            let t = Instant::now();
            let id = DwmRegisterThumbnail(self.hwnd, hwnd).ok()?;
            let took = ms(t.elapsed());
            // The picture is drawn 1:1 from the frame's corner; its size is
            // the source's (a window's frame can be a pixel off it).
            let size = match source {
                Some(s) => SIZE { cx: s.right - s.left, cy: s.bottom - s.top },
                None => DwmQueryThumbnailSourceSize(id).unwrap_or(SIZE { cx: rect.right - rect.left, cy: rect.bottom - rect.top }),
            };
            let (left, top) = (rect.left - self.area.left, rect.top - self.area.top);
            let dest = RECT { left, top, right: left + size.cx, bottom: top + size.cy };
            self.thumbs.push(Thumb { id, layer, dest, source, win });
            Some(took)
        }

        /// Where a layer is drawn at progress `p` of a slide towards `dir`,
        /// or None when it is not part of this slide.
        fn offset(&self, layer: Layer, dir: Dir, p: f64) -> Option<(i32, i32)> {
            let (w, h) = self.size();
            let (w, h) = (w as f64, h as f64);
            let layer = match layer {
                Layer::Wall(0) => Layer::Here,
                Layer::Wall(1) => Layer::Right,
                Layer::Wall(_) => Layer::Down,
                other => other,
            };
            let at = |x: f64, y: f64| Some((x.round() as i32, y.round() as i32));
            match (layer, dir) {
                (Layer::Pinned, _) => at(0.0, 0.0),
                (Layer::Here, Dir::Right) => at(-p * w, 0.0),
                (Layer::Here, Dir::Down) => at(0.0, -p * h),
                (Layer::Right, Dir::Right) => at((1.0 - p) * w, 0.0),
                (Layer::Down, Dir::Down) => at(0.0, (1.0 - p) * h),
                _ => None,
            }
        }

        /// Moves every thumbnail to progress `p`; the pinned ones only when
        /// `all`, as they never move.
        unsafe fn place(&mut self, dir: Dir, p: f64, all: bool) {
            for i in 0..self.thumbs.len() {
                let t = &self.thumbs[i];
                if t.layer == Layer::Pinned && !all {
                    continue;
                }
                let mut props = DWM_THUMBNAIL_PROPERTIES {
                    dwFlags: DWM_TNP_RECTDESTINATION | DWM_TNP_VISIBLE | DWM_TNP_OPACITY | DWM_TNP_SOURCECLIENTAREAONLY,
                    opacity: 255,
                    fSourceClientAreaOnly: false.into(),
                    ..Default::default()
                };
                match self.offset(t.layer, dir, p) {
                    Some((x, y)) => {
                        props.fVisible = true.into();
                        props.rcDestination =
                            RECT { left: t.dest.left + x, top: t.dest.top + y, right: t.dest.right + x, bottom: t.dest.bottom + y };
                    }
                    None => props.fVisible = false.into(),
                }
                if let Some(source) = t.source {
                    props.dwFlags |= DWM_TNP_RECTSOURCE;
                    props.rcSource = source;
                }
                let id = t.id;
                let start = Instant::now();
                let _ = DwmUpdateThumbnailProperties(id, &props);
                self.call_time += ms(start.elapsed());
                self.calls += 1;
            }
        }

        unsafe fn show(&self) {
            let _ = SetWindowPos(self.hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW);
        }
    }

    impl Drop for Overlay {
        fn drop(&mut self) {
            unsafe {
                for t in &self.thumbs {
                    let _ = DwmUnregisterThumbnail(t.id);
                }
                let _ = DestroyWindow(self.hwnd);
            }
        }
    }

    // ---- timing ---------------------------------------------------------------

    fn ft(f: FILETIME) -> f64 {
        (((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64) as f64 / 10_000.0
    }

    fn cpu_ms(process: HANDLE) -> f64 {
        let (mut a, mut b, mut k, mut u) = Default::default();
        if unsafe { GetProcessTimes(process, &mut a, &mut b, &mut k, &mut u) }.is_err() {
            return -1.0;
        }
        ft(k) + ft(u)
    }

    fn process_named(name: &str) -> Option<HANDLE> {
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?;
            let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
            let mut pid = None;
            let mut ok = Process32FirstW(snap, &mut e).is_ok();
            while ok {
                let n = String::from_utf16_lossy(&e.szExeFile);
                if n.trim_end_matches('\0').eq_ignore_ascii_case(name) {
                    pid = Some(e.th32ProcessID);
                }
                ok = Process32NextW(snap, &mut e).is_ok();
            }
            let _ = CloseHandle(snap);
            OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid?).ok()
        }
    }

    fn dwm_timing() -> DWM_TIMING_INFO {
        let mut t = DWM_TIMING_INFO { cbSize: std::mem::size_of::<DWM_TIMING_INFO>() as u32, ..Default::default() };
        let _ = unsafe { DwmGetCompositionTimingInfo(None, &mut t) };
        t
    }

    fn stats(v: &mut [f64]) -> String {
        if v.is_empty() {
            return "-".into();
        }
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let mean = v.iter().sum::<f64>() / v.len() as f64;
        let at = |q: f64| v[((v.len() - 1) as f64 * q).round() as usize];
        format!("mean {mean:.3} p50 {:.3} p95 {:.3} max {:.3}", at(0.5), at(0.95), v[v.len() - 1])
    }

    fn ease(s: f64) -> f64 {
        1.0 - (1.0 - s.clamp(0.0, 1.0)).powi(3)
    }

    /// Where a made-up hand is at share `s` of a drag, as a share of the
    /// screen: out past the middle, back a little, then on, as a hand that
    /// hesitates does.
    fn hand(s: f64) -> f64 {
        let s = s.clamp(0.0, 1.0);
        0.35 * (1.0 - (std::f64::consts::PI * s).cos()) - 0.12 * (2.0 * std::f64::consts::PI * s).sin().max(0.0) * s
    }

    struct Frames {
        frames: usize,
        loop_ms: f64,
        work: Vec<f64>,
        gaps: Vec<f64>,
        cpu: f64,
        dwm_cpu: f64,
        composed: u64,
        refreshes: u64,
        calls: u64,
        call_time: f64,
    }

    /// Runs one slide from `p0` to `p1`, a frame per refresh: either along
    /// an eased curve for `length` ms, or (with `drag`) after a cursor that
    /// is moved along a hand's path.
    unsafe fn slide(o: &mut Overlay, out: &Option<IDXGIOutput>, dir: Dir, p0: f64, p1: f64, length: f64, drag: bool, dwm: Option<HANDLE>) -> Frames {
        let (w, h) = o.size();
        let mut start_cursor = POINT::default();
        let _ = GetCursorPos(&mut start_cursor);
        let (cpu0, dwm0, t0) = (cpu_ms(GetCurrentProcess()), dwm.map(cpu_ms).unwrap_or(-1.0), dwm_timing());
        let (calls0, time0) = (o.calls, o.call_time);
        let mut work = Vec::new();
        let mut gaps = Vec::new();
        let begin = Instant::now();
        let mut last = begin;
        loop {
            match out {
                Some(output) => {
                    let _ = output.WaitForVBlank();
                }
                None => {
                    let _ = DwmFlush();
                }
            }
            let now = Instant::now();
            gaps.push(ms(now - last));
            last = now;
            let s = ms(now - begin) / length;
            let p = if drag {
                // The cursor stands in for a hand; the slide reads it back as
                // the app would read the hand.
                let along = hand(s);
                let (x, y) = match dir {
                    Dir::Right => (start_cursor.x - (along * w as f64) as i32, start_cursor.y),
                    Dir::Down => (start_cursor.x, start_cursor.y - (along * h as f64) as i32),
                };
                let _ = SetCursorPos(x, y);
                let mut c = POINT::default();
                let _ = GetCursorPos(&mut c);
                match dir {
                    Dir::Right => (start_cursor.x - c.x) as f64 / w as f64,
                    Dir::Down => (start_cursor.y - c.y) as f64 / h as f64,
                }
            } else {
                p0 + (p1 - p0) * ease(s)
            };
            let t = Instant::now();
            o.place(dir, p, false);
            work.push(ms(t.elapsed()));
            if s >= 1.0 {
                break;
            }
        }
        if !drag {
            o.place(dir, p1, false);
        }
        let loop_ms = ms(begin.elapsed());
        let t1 = dwm_timing();
        gaps.remove(0);
        Frames {
            frames: work.len(),
            loop_ms,
            work,
            gaps,
            cpu: cpu_ms(GetCurrentProcess()) - cpu0,
            dwm_cpu: if dwm0 >= 0.0 { dwm.map(cpu_ms).unwrap_or(-1.0) - dwm0 } else { -1.0 },
            composed: t1.cFrame - t0.cFrame,
            refreshes: t1.cRefresh - t0.cRefresh,
            calls: o.calls - calls0,
            call_time: o.call_time - time0,
        }
    }

    fn report_frames(rep: &mut Report, name: &str, mut f: Frames) {
        let secs = f.loop_ms / 1000.0;
        rep.line(format!(
            "{name}: {} frames in {:.0} ms = {:.1} fps (loop); DWM composed {} frames over {} refreshes = {:.1} fps",
            f.frames,
            f.loop_ms,
            f.frames as f64 / secs,
            f.composed,
            f.refreshes,
            f.composed as f64 / secs
        ));
        let late = f.gaps.iter().filter(|g| **g > 1000.0 / 60.0 * 1.5).count();
        rep.line(format!("  frame gap ms: {}  (gaps over 25 ms: {late})", stats(&mut f.gaps)));
        rep.line(format!("  per-frame update work ms: {}", stats(&mut f.work)));
        rep.line(format!(
            "  DwmUpdateThumbnailProperties: {} calls, {:.1} us each, {:.0} per frame",
            f.calls,
            f.call_time * 1000.0 / f.calls.max(1) as f64,
            f.calls as f64 / f.frames.max(1) as f64
        ));
        rep.line(format!(
            "  CPU: this process {:.2} ms per frame ({:.1}% of a core); dwm.exe {}",
            f.cpu / f.frames.max(1) as f64,
            f.cpu / f.loop_ms * 100.0,
            if f.dwm_cpu >= 0.0 { format!("{:.1}% of a core", f.dwm_cpu / f.loop_ms * 100.0) } else { "not readable".into() }
        ));
    }

    // ---- the measurements ---------------------------------------------------------

    fn register_classes() {
        unsafe {
            let hinst = GetModuleHandleW(None).unwrap();
            let black = HBRUSH(GetStockObject(BLACK_BRUSH).0);
            for (name, proc_) in [
                (w!("flick.poc.overlay"), overlay_proc as unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT),
                (w!("flick.poc.wall"), wall_proc),
            ] {
                let wc = WNDCLASSW { lpfnWndProc: Some(proc_), hInstance: hinst.into(), lpszClassName: name, hbrBackground: black, ..Default::default() };
                RegisterClassW(&wc);
            }
        }
    }

    pub fn run(args: &[String]) -> R<()> {
        let mut dir = Dir::Right;
        let (mut length, mut hold, mut drag, mut switch, mut stay, mut progman) = (400.0, 2000u64, false, false, false, true);
        let mut out_dir = None;
        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "right" => dir = Dir::Right,
                "down" => dir = Dir::Down,
                "hand" => drag = true,
                "switch" => switch = true,
                "stay" => stay = true,
                "picture" => progman = false,
                "ms" => {
                    i += 1;
                    length = args[i].parse()?
                }
                "hold" => {
                    i += 1;
                    hold = args[i].parse()?
                }
                "out" => {
                    i += 1;
                    out_dir = Some(args[i].clone())
                }
                other => return Err(format!("unknown argument {other}").into()),
            }
            i += 1;
        }
        let out_dir = out_dir.unwrap_or_else(|| format!("poc-out\\{}", if dir == Dir::Right { "right" } else { "down" }));
        std::fs::create_dir_all(&out_dir)?;
        let file = |name: &str| format!("{out_dir}\\{name}");
        let mut rep = Report(String::new());
        unsafe { run_inner(&mut rep, dir, length, hold, drag, switch, stay, progman, &file)? };
        std::fs::write(file("report.txt"), &rep.0)?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    unsafe fn run_inner(rep: &mut Report, dir: Dir, length: f64, hold: u64, drag: bool, switch: bool, stay: bool, progman: bool, file: &dyn Fn(&str) -> String) -> R<()> {
        register_classes();
        // A display that has gone to sleep is not composed: keep it awake.
        SetThreadExecutionState(ES_CONTINUOUS | ES_DISPLAY_REQUIRED | ES_SYSTEM_REQUIRED);
        for dx in [1, -1] {
            let input = INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, dwFlags: MOUSEEVENTF_MOVE, ..Default::default() } } };
            SendInput(&[input], std::mem::size_of::<INPUT>() as i32);
        }
        wait(1500);
        let desks = Desks::read()?;
        let target_by = if dir == Dir::Right { 1 } else { 2 };
        let target = desks.at(target_by).ok_or("no desktop to go to; run `slide_poc stage` first")?;
        let home = desks.at(0).unwrap();
        let build = vdapi::windows_build().unwrap_or((0, 0));
        rep.line(format!("Windows build {}.{}, backend {:?}; {} desktops, at index {}, going {:?} to index {}", build.0, build.1, vdapi::select(false), desks.all.len(), desks.here, dir, desks.here + target_by));

        let mons = monitors();
        for m in &mons {
            rep.line(format!(
                "monitor {:?} work {:?} dpi {} ({}%) {}",
                (m.bounds.left, m.bounds.top, m.bounds.right, m.bounds.bottom),
                (m.work.left, m.work.top, m.work.right, m.work.bottom),
                m.dpi,
                m.dpi * 100 / 96,
                if m.primary { "primary" } else { "" }
            ));
        }
        let mon = mons.iter().find(|m| m.primary).ok_or("no primary monitor")?;
        let area = mon.work;
        let output = output_for(mon.handle);
        let timing = dwm_timing();
        let (num, den) = (timing.rateRefresh.uiNumerator, timing.rateRefresh.uiDenominator);
        rep.line(format!("DWM refresh {num}/{} Hz; frame pacing by {}", den.max(1), if output.is_some() { "IDXGIOutput::WaitForVBlank" } else { "DwmFlush" }));
        let dwm = process_named("dwm.exe");

        // Windows, by layer.
        let wins = survey(&desks);
        let count = |l: Layer| wins.iter().filter(|w| w.layer == l && !w.minimized).count();
        rep.line(format!(
            "windows: here {}, right {}, down {}, pinned {}, minimized (left out) {}",
            count(Layer::Here),
            count(Layer::Right),
            count(Layer::Down),
            count(Layer::Pinned),
            wins.iter().filter(|w| w.minimized).count()
        ));
        for w in &wins {
            rep.line(format!(
                "  {:?}{} cloaked={} {:?} {} | {} | {}",
                w.layer,
                if w.minimized { " MIN" } else { "" },
                w.cloaked,
                (w.rect.left, w.rect.top, w.rect.right - w.rect.left, w.rect.bottom - w.rect.top),
                w.exe,
                w.class,
                w.title
            ));
        }

        // Wallpapers.
        let mut walls = Vec::new();
        for (slot, by) in [0usize, 1, 2].into_iter().enumerate() {
            let Some(_) = desks.at(by) else { continue };
            let wall = make_wall((desks.here + by) as u32, mon, slot as i32);
            rep.line(format!("wallpaper {}: {}", ["here", "right", "down"][slot], wall.note));
            walls.push(wall);
        }
        wait(300);

        // The overlay and its thumbnails, bottom first: wallpapers, the other
        // desktops, this desktop, then the pinned windows over everything.
        let mut o = Overlay::new(area);
        let pin_overlay_note = format!("{:?}", vdapi::pin_window(o.hwnd));
        let mut reg = Vec::new();
        let source = RECT {
            left: area.left - mon.bounds.left,
            top: area.top - mon.bounds.top,
            right: area.right - mon.bounds.left,
            bottom: area.bottom - mon.bounds.top,
        };
        for (slot, wall) in walls.iter().enumerate() {
            // `progman`: this desktop's background is the shell's own desktop
            // window, icons and all, live.
            let hwnd = if slot == 0 && progman { FindWindowW(w!("Progman"), None) } else { wall.hwnd };
            if let Some(t) = o.add(hwnd, Layer::Wall(slot), area, Some(source), usize::MAX) {
                reg.push(t);
            }
        }
        let mut failed = 0;
        for layer in [Layer::Right, Layer::Down, Layer::Here, Layer::Pinned] {
            for (i, w) in wins.iter().enumerate().rev() {
                if w.layer != layer || w.minimized {
                    continue;
                }
                match o.add(w.hwnd, layer, w.rect, None, i) {
                    Some(t) => reg.push(t),
                    None => failed += 1,
                }
            }
        }
        rep.line(format!("overlay pinned to all desktops: {pin_overlay_note}"));
        rep.line(format!("thumbnails: {} registered, {failed} refused; DwmRegisterThumbnail ms: {}", o.thumbs.len(), stats(&mut reg.clone())));
        for t in &o.thumbs {
            let s = DwmQueryThumbnailSourceSize(t.id).unwrap_or_default();
            if t.win != usize::MAX {
                let w = &wins[t.win];
                let (rw, rh) = (w.rect.right - w.rect.left, w.rect.bottom - w.rect.top);
                if s.cx != rw || s.cy != rh {
                    rep.line(format!("  source size {}x{} differs from window {rw}x{rh}: {}", s.cx, s.cy, w.title));
                }
            }
        }

        // A minimized window and the desktop itself, as thumbnails.
        if let Some(w) = wins.iter().find(|w| w.minimized) {
            if let Ok(id) = DwmRegisterThumbnail(o.hwnd, w.hwnd) {
                let s = DwmQueryThumbnailSourceSize(id).unwrap_or_default();
                rep.line(format!("minimized window as a thumbnail: source {}x{} ({})", s.cx, s.cy, w.title));
                let _ = DwmUnregisterThumbnail(id);
            }
        }
        let progman = FindWindowW(w!("Progman"), None);
        match DwmRegisterThumbnail(o.hwnd, progman) {
            Ok(id) => {
                let s = DwmQueryThumbnailSourceSize(id).unwrap_or_default();
                rep.line(format!("Progman (desktop icons/wallpaper) as a thumbnail: source {}x{}", s.cx, s.cy));
                let _ = DwmUnregisterThumbnail(id);
            }
            Err(e) => rep.line(format!("Progman as a thumbnail: refused ({e})")),
        }

        // Cost of one property update, without waiting for frames.
        o.place(dir, 0.0, true);
        let (c0, t0) = (o.calls, o.call_time);
        let bench = Instant::now();
        for k in 0..200 {
            o.place(dir, (k % 2) as f64 * 0.01, false);
        }
        rep.line(format!(
            "bench: {} DwmUpdateThumbnailProperties calls in {:.1} ms = {:.1} us each",
            o.calls - c0,
            ms(bench.elapsed()),
            (o.call_time - t0) * 1000.0 / (o.calls - c0).max(1) as f64
        ));
        o.place(dir, 0.0, true);

        // Fidelity: the real screen against the overlay at rest; and whether
        // putting the overlay up shows a frame of something else.
        let real = shot(area, 1);
        let t0 = Instant::now();
        let rec = Recorder::start(mon.bounds, 4, t0);
        wait(150);
        let shown_at = ms(t0.elapsed());
        o.show();
        flush();
        wait(400);
        let frames = rec.finish();
        let p0 = shot(area, 1);
        save_bmp(&real, &file("a-real-before.bmp"));
        save_bmp(&p0, &file("b-overlay-p0.bmp"));
        let (d, c) = diff(&real, &p0, None);
        rep.line(format!("overlay at rest vs the real screen: mean diff {d:.2}/255, {:.1}% of pixels differ", c * 100.0));
        analyse(rep, "putting the overlay up", &frames, &[("show", shown_at)], file, "show");

        // Live thumbnails: this desktop (control) and then the other one.
        live(rep, &mut o, &wins, dir, 0.0, Layer::Here, 1000, file, "here");
        let target_layer = if dir == Dir::Right { Layer::Right } else { Layer::Down };
        live(rep, &mut o, &wins, dir, 1.0, target_layer, hold, file, "target");

        o.place(dir, 0.5, true);
        flush();
        wait(100);
        save_bmp(&shot(area, 1), &file("d-overlay-half.bmp"));

        // Slides.
        o.place(dir, 0.0, true);
        flush();
        let mut all = Vec::new();
        for _ in 0..3 {
            all.push(slide(&mut o, &output, dir, 0.0, 1.0, length, false, dwm));
            all.push(slide(&mut o, &output, dir, 1.0, 0.0, length, false, dwm));
        }
        let mut sum = all.remove(0);
        for f in all {
            sum.frames += f.frames;
            sum.loop_ms += f.loop_ms;
            sum.work.extend(f.work);
            sum.gaps.extend(f.gaps);
            sum.cpu += f.cpu;
            sum.dwm_cpu = if sum.dwm_cpu >= 0.0 && f.dwm_cpu >= 0.0 { sum.dwm_cpu + f.dwm_cpu } else { -1.0 };
            sum.composed += f.composed;
            sum.refreshes += f.refreshes;
            sum.calls += f.calls;
            sum.call_time += f.call_time;
        }
        report_frames(rep, &format!("timed slides (6 x {length:.0} ms, there and back), {} thumbnails moving", o.thumbs.iter().filter(|t| t.layer != Layer::Pinned).count()), sum);
        if drag {
            let f = slide(&mut o, &output, dir, 0.0, 0.0, 1500.0, true, dwm);
            report_frames(rep, "following the cursor (1500 ms)", f);
            // Let go where the hand stopped: on to the end.
            let f = slide(&mut o, &output, dir, hand(1.0), 1.0, 250.0, false, dwm);
            report_frames(rep, "letting go, to the end (250 ms)", f);
        }

        // The real switch under the finished slide.
        if switch {
            o.place(dir, 1.0, true);
            flush();
            wait(300);
            let fg_before = GetForegroundWindow();
            let before = shot(area, 1);
            let (tx, rx) = mpsc::channel();
            let _listener = vd(vdapi::listen_desktop_events(tx))?;
            wait(200);
            while rx.try_recv().is_ok() {}
            let t0 = Instant::now();
            // A band across the screen: small enough to read back every few
            // ms, so a single wrong frame is not missed between pictures.
            let mid = mon.bounds.top + (mon.bounds.bottom - mon.bounds.top) * 2 / 5;
            let band = RECT { left: mon.bounds.left, top: mid, right: mon.bounds.right, bottom: mid + 8 };
            let rec = Recorder::start(band, 1, t0);
            wait(200);
            let call_at = ms(t0.elapsed());
            let result = vdapi::switch_desktop(target);
            let returned = ms(t0.elapsed());
            let mut event = -1.0;
            while t0.elapsed() < Duration::from_millis(3000) {
                pump();
                if let Ok(vdapi::DesktopEvent::DesktopChanged { .. }) = rx.recv_timeout(Duration::from_millis(5)) {
                    event = ms(t0.elapsed());
                    break;
                }
            }
            flush();
            let flushed = ms(t0.elapsed());
            ShowWindow(o.hwnd, SW_HIDE);
            let hidden = ms(t0.elapsed());
            wait(800);
            let frames = rec.finish();
            let after = shot(area, 1);
            save_bmp(&before, &file("e-overlay-p1-before-switch.bmp"));
            save_bmp(&after, &file("f-real-after-switch.bmp"));
            rep.line(format!(
                "switch: {result:?}; call took {:.1} ms; desktop-changed event {:.1} ms after the call; DwmFlush x2 done at +{:.1}; overlay hidden at +{:.1}",
                returned - call_at,
                if event >= 0.0 { event - call_at } else { -1.0 },
                flushed - call_at,
                hidden - call_at
            ));
            let (d, c) = diff(&before, &after, None);
            rep.line(format!("overlay at the end vs the real target desktop: mean diff {d:.2}/255, {:.1}% of pixels differ", c * 100.0));
            analyse(rep, "the switch", &frames, &[("call", call_at), ("event", event), ("hide", hidden)], file, "switch");
            wait(300);
            let fg = GetForegroundWindow();
            rep.line(format!(
                "foreground: before '{}' ({}) after '{}' ({}), on the target desktop: {:?}",
                text(fg_before),
                class(fg_before),
                text(fg),
                class(fg),
                vdapi::is_window_on_current_desktop(fg)
            ));
            rep.line(format!(
                "overlay window after the switch: cloaked={} wallpaper windows cloaked={:?}",
                cloaked(o.hwnd),
                walls.iter().map(|w| cloaked(w.hwnd)).collect::<Vec<_>>()
            ));
            if !stay {
                wait(500);
                let _ = vdapi::switch_desktop(home);
                wait(500);
            }
        }
        drop(o);
        for w in walls {
            let _ = DestroyWindow(w.hwnd);
            DeleteDC(w.dc);
        }
        Ok(())
    }

    /// Holds the overlay at `p` and watches the thumbnails of `layer`: for
    /// each window, how much of its visible part changed over `hold` ms.
    #[allow(clippy::too_many_arguments)]
    unsafe fn live(rep: &mut Report, o: &mut Overlay, wins: &[Win], dir: Dir, p: f64, layer: Layer, hold: u64, file: &dyn Fn(&str) -> String, name: &str) {
        o.place(dir, p, true);
        flush();
        wait(300);
        let (w, h) = o.size();
        let a = shot(o.area, 1);
        wait(hold / 2);
        let b = shot(o.area, 1);
        wait(hold / 2);
        let c = shot(o.area, 1);
        save_bmp(&a, &file(&format!("c-live-{name}-0.bmp")));
        save_bmp(&c, &file(&format!("c-live-{name}-{hold}.bmp")));
        // Which thumbnail is on top at each pixel.
        let mut owner = vec![usize::MAX; (w * h) as usize];
        for (k, t) in o.thumbs.iter().enumerate() {
            let Some((x, y)) = o.offset(t.layer, dir, p) else { continue };
            for py in (t.dest.top + y).max(0)..(t.dest.bottom + y).min(h) {
                for px in (t.dest.left + x).max(0)..(t.dest.right + x).min(w) {
                    owner[(py * w + px) as usize] = k;
                }
            }
        }
        rep.line(format!("live thumbnails of {name} ({layer:?}), {hold} ms; changed = share of the visible part that changed between pictures"));
        for (k, t) in o.thumbs.iter().enumerate() {
            if t.layer != layer || t.win == usize::MAX {
                continue;
            }
            let win = &wins[t.win];
            let mask: &dyn Fn(i32, i32) -> bool = &|x, y| owner[(y * w + x) as usize] == k;
            let visible = owner.iter().filter(|o| **o == k).count();
            if visible == 0 {
                rep.line(format!("  {:<28} hidden under others", win.title));
                continue;
            }
            let (_, c1) = diff(&a, &b, Some(mask));
            let (_, c2) = diff(&b, &c, Some(mask));
            let (l, sd) = luma(&c, Some(mask));
            rep.line(format!(
                "  {:<28} {:<22} {:<16} cloaked={} visible {:>7} px  changed {:>5.1}% then {:>5.1}%  luma {:>5.1} sd {:>5.1}{}",
                win.title,
                win.class,
                win.exe,
                win.cloaked,
                visible,
                c1 * 100.0,
                c2 * 100.0,
                l,
                sd,
                if sd < 2.0 { "  FLAT" } else { "" }
            ));
        }
    }

    /// Looks through recorded frames for one that is neither the first nor
    /// the last picture (a glitch) or much darker than both.
    fn analyse(rep: &mut Report, what: &str, frames: &[(f64, Img)], marks: &[(&str, f64)], file: &dyn Fn(&str) -> String, name: &str) {
        if frames.len() < 3 {
            return rep.line(format!("{what}: too few frames recorded"));
        }
        let first = &frames[0].1;
        let last = &frames[frames.len() - 1].1;
        let (l_first, _) = luma(first, None);
        let (l_last, _) = luma(last, None);
        let mut gaps: Vec<f64> = frames.windows(2).map(|w| w[1].0 - w[0].0).collect();
        rep.line(format!("{what}: {} frames recorded, frame gap ms {}; marks {:?}", frames.len(), stats(&mut gaps), marks));
        let mut odd = 0;
        for (k, (t, img)) in frames.iter().enumerate() {
            let (df, _) = diff(img, first, None);
            let (dl, _) = diff(img, last, None);
            let (l, _) = luma(img, None);
            let dark = l < 0.6 * l_first.min(l_last);
            let between = df.min(dl) > 4.0;
            let thin = img.h <= 8;
            let near = marks.iter().any(|(_, m)| *m >= 0.0 && (t - m).abs() < if thin { 40.0 } else { 400.0 });
            if (dark || between || near) && !thin {
                save_bmp(img, &file(&format!("rec-{name}-{k:03}.bmp")));
            }
            if dark || between {
                odd += 1;
            }
            if (near || dark || between) && (odd <= 40 || near) {
                rep.line(format!(
                    "  {t:7.1} ms  luma {l:5.1}  diff to first {df:5.2} to last {dl:5.2}{}{}",
                    if dark { "  DARK" } else { "" },
                    if between { "  NEITHER" } else { "" }
                ));
            }
        }
        rep.line(format!("{what}: {odd} odd frames (luma first {l_first:.1}, last {l_last:.1})"));
    }

    // ---- test windows ---------------------------------------------------------------

    static TICKER: OnceLock<(String, u32)> = OnceLock::new();
    static COUNT: AtomicU32 = AtomicU32::new(0);

    unsafe extern "system" fn ticker_proc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        match m {
            WM_TIMER => {
                COUNT.fetch_add(1, Ordering::Relaxed);
                InvalidateRect(h, None, false);
                LRESULT(0)
            }
            WM_ERASEBKGND => LRESULT(1),
            WM_PAINT => {
                let (label, rgb) = TICKER.get().map(|t| (t.0.as_str(), t.1)).unwrap();
                let mut ps = PAINTSTRUCT::default();
                let hdc = BeginPaint(h, &mut ps);
                let mut rc = RECT::default();
                let _ = GetClientRect(h, &mut rc);
                let (cw, ch) = (rc.right.max(1), rc.bottom.max(1));
                let mem = CreateCompatibleDC(hdc);
                let bmp = CreateCompatibleBitmap(hdc, cw, ch);
                let old = SelectObject(mem, bmp);
                let bgr = ((rgb & 0xff) << 16) | (rgb & 0xff00) | (rgb >> 16);
                let brush = CreateSolidBrush(COLORREF(bgr));
                FillRect(mem, &rc, brush);
                DeleteObject(brush);
                let n = COUNT.load(Ordering::Relaxed) as i32;
                let white = CreateSolidBrush(COLORREF(0xffffff));
                let x = (n * 12) % cw;
                FillRect(mem, &RECT { left: x, top: ch - 50, right: x + 80, bottom: ch - 10 }, white);
                DeleteObject(white);
                let mut lf = LOGFONTW { lfHeight: -44, lfWeight: 700, ..Default::default() };
                for (k, c) in "Segoe UI".encode_utf16().enumerate() {
                    lf.lfFaceName[k] = c;
                }
                let font = CreateFontIndirectW(&lf);
                let old_font = SelectObject(mem, font);
                SetBkMode(mem, TRANSPARENT);
                SetTextColor(mem, COLORREF(0xffffff));
                let t = GetLocalTime();
                let s = format!("{label}\n{n}\n{:02}:{:02}:{:02}.{:03}", t.wHour, t.wMinute, t.wSecond, t.wMilliseconds);
                let mut wide: Vec<u16> = s.encode_utf16().collect();
                let mut tr = RECT { left: 0, top: 20, right: cw, bottom: ch };
                DrawTextW(mem, &mut wide, &mut tr, DT_CENTER | DT_NOCLIP);
                let _ = BitBlt(hdc, 0, 0, cw, ch, mem, 0, 0, SRCCOPY);
                SelectObject(mem, old_font);
                DeleteObject(font);
                SelectObject(mem, old);
                DeleteObject(bmp);
                DeleteDC(mem);
                EndPaint(h, &ps);
                LRESULT(0)
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(h, m, w, l),
        }
    }

    /// A window that repaints a counter, the time and a moving bar 60 times
    /// a second with GDI: content that is known to change.
    pub fn ticker(args: &[String]) {
        let label = args.first().cloned().unwrap_or_else(|| "T".into());
        let rgb = args.get(1).and_then(|c| u32::from_str_radix(c, 16).ok()).unwrap_or(0x3060a0);
        let _ = TICKER.set((label.clone(), rgb));
        unsafe {
            let hinst = GetModuleHandleW(None).unwrap();
            let wc = WNDCLASSW {
                lpfnWndProc: Some(ticker_proc),
                hInstance: hinst.into(),
                lpszClassName: w!("flick.poc.ticker"),
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                ..Default::default()
            };
            RegisterClassW(&wc);
            let title = HSTRING::from(format!("PocTicker {label}"));
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("flick.poc.ticker"),
                PCWSTR(title.as_ptr()),
                WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                500,
                360,
                None,
                None,
                hinst,
                None,
            );
            SetTimer(hwnd, 1, 16, None);
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }

    #[derive(serde::Serialize, serde::Deserialize, Default)]
    struct Stage {
        created: Vec<String>,
        windows: Vec<isize>,
        pids: Vec<u32>,
        wallpapers: Vec<(u32, String)>,
    }

    fn stage_path() -> std::path::PathBuf {
        std::env::temp_dir().join("slide_poc_stage.json")
    }

    const PAGE: &str = r#"<!doctype html><meta charset=utf-8><title>PocEdge</title>
<body style="margin:0;overflow:hidden;background:#000"><canvas id=c></canvas><script>
const L=decodeURIComponent(location.search.slice(1));document.title='PocEdge '+L;
const c=document.getElementById('c'),x=c.getContext('2d');let n=0;
function f(){c.width=innerWidth;c.height=innerHeight;n++;
x.fillStyle='hsl('+(n*2%360)+',60%,35%)';x.fillRect(0,0,c.width,c.height);
x.fillStyle='#fff';x.fillRect((n*10)%c.width,c.height-60,90,40);
x.font='bold 48px Segoe UI';x.fillText(L+' '+n,20,70);
x.fillText(new Date().toISOString().substr(11,12),20,130);requestAnimationFrame(f)}
requestAnimationFrame(f);</script>"#;

    const CONSOLE: &str = "$host.UI.RawUI.WindowTitle = 'PocConsole ' + $args[0]\r\n$i = 0\r\nwhile ($true) { $i++; Write-Host ('{0,6} {1:HH:mm:ss.fff}' -f $i, (Get-Date)); Start-Sleep -Milliseconds 50 }\r\n";

    /// Starts `cmd` and waits for a new app window (whose title holds `hint`).
    fn launch(cmd: &mut Command, hint: Option<&str>) -> (Option<HWND>, Option<u32>) {
        let before: HashSet<isize> = app_windows().iter().map(|h| h.0).collect();
        let pid = match cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
            Ok(child) => Some(child.id()),
            Err(e) => {
                println!("  could not start {cmd:?}: {e}");
                return (None, None);
            }
        };
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(20) {
            std::thread::sleep(Duration::from_millis(150));
            for h in app_windows() {
                if !before.contains(&h.0) && hint.map_or(true, |t| text(h).contains(t)) {
                    std::thread::sleep(Duration::from_millis(400));
                    return (Some(h), pid);
                }
            }
        }
        println!("  no window appeared for {cmd:?}");
        (None, pid)
    }

    fn edge() -> Option<String> {
        ["C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe", "C:\\Program Files\\Microsoft\\Edge\\Application\\msedge.exe"]
            .into_iter()
            .find(|p| std::path::Path::new(p).exists())
            .map(String::from)
    }

    fn find_wallpapers() -> Vec<String> {
        let mut out = Vec::new();
        let mut dirs = vec![std::path::PathBuf::from("C:\\Windows\\Web\\Wallpaper")];
        while let Some(d) = dirs.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    dirs.push(p);
                } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("jpg")) {
                    out.push(p.to_string_lossy().into_owned());
                }
            }
        }
        out.sort();
        out
    }

    fn set_wallpaper(index: u32, path: &str) -> bool {
        match vdapi::select(false) {
            Some(vdapi::Backend::Win23H2) => winvd_23h2::get_desktop(index).set_wallpaper(path).is_ok(),
            Some(vdapi::Backend::Win24H2) => winvd_24h2::get_desktop(index).set_wallpaper(path).is_ok(),
            None => false,
        }
    }

    #[derive(Clone, Copy, Debug)]
    enum Place {
        Here,
        Right,
        Down,
        Pinned,
    }

    pub fn stage() -> R<()> {
        if stage_path().exists() {
            println!("cleaning up an earlier stage first");
            let _ = unstage();
        }
        let mut st = Stage::default();
        let mut desks = Desks::read()?;
        while desks.all.len() < desks.here + 3 {
            st.created.push(vd(vdapi::create_desktop())?.id());
            desks = Desks::read()?;
        }
        // Different wallpapers on the next two desktops, to see that each
        // desktop's own is found.
        let pictures = find_wallpapers();
        println!("{} wallpapers found under C:\\Windows\\Web\\Wallpaper", pictures.len());
        for by in 1..3u32 {
            let index = desks.here as u32 + by;
            let (old, _) = wallpaper_of(index);
            if let Some(p) = pictures.get(pictures.len().saturating_sub(by as usize)).filter(|p| **p != old) {
                if set_wallpaper(index, p) {
                    st.wallpapers.push((index, old));
                    println!("desktop {index}: wallpaper {p}");
                }
            }
        }

        let mons = monitors();
        let mon = mons.iter().find(|m| m.primary).ok_or("no primary monitor")?;
        let a = mon.work;
        let (cw, ch) = ((a.right - a.left) / 4, (a.bottom - a.top) / 3);
        let cell = |col: i32, row: i32, wide: f64| RECT {
            left: a.left + col * cw * 4 / 5,
            top: a.top + row * ch * 3 / 4,
            right: a.left + col * cw * 4 / 5 + (cw as f64 * wide) as i32,
            bottom: a.top + row * ch * 3 / 4 + (ch as f64 * 1.4) as i32,
        };
        let me = std::env::current_exe()?;
        let temp = std::env::temp_dir();
        let page = temp.join("slide_poc.html");
        std::fs::write(&page, PAGE)?;
        let script = temp.join("slide_poc_console.ps1");
        std::fs::write(&script, CONSOLE)?;
        let page_url = format!("file:///{}", page.to_string_lossy().replace('\\', "/"));
        let edge = edge();

        enum Kind {
            Ticker(&'static str, &'static str),
            Edge(&'static str, bool),
            Console(&'static str),
            Calc,
            Notepad,
        }
        use Kind::*;
        let fixtures: Vec<(Kind, Place, RECT, bool)> = vec![
            (Ticker("H1", "a03030"), Place::Here, cell(0, 0, 1.6), false),
            (Ticker("H2", "30a030"), Place::Here, cell(1, 1, 1.6), false),
            (Edge("Here", false), Place::Here, cell(2, 0, 2.0), false),
            (Ticker("H3", "3030a0"), Place::Here, cell(3, 1, 1.2), false),
            (Notepad, Place::Here, cell(0, 2, 1.6), false),
            (Ticker("Hmin", "808080"), Place::Here, cell(1, 0, 1.2), true),
            (Ticker("R1", "c06000"), Place::Right, cell(0, 0, 1.5), false),
            (Edge("Right", false), Place::Right, cell(1, 0, 1.8), false),
            (Edge("RightNoOcclusion", true), Place::Right, cell(2, 1, 1.8), false),
            (Console("Right"), Place::Right, cell(0, 2, 1.8), false),
            (Calc, Place::Right, cell(3, 0, 1.0), false),
            (Ticker("R2", "008080"), Place::Right, cell(3, 2, 1.2), false),
            (Ticker("Rmin", "808080"), Place::Right, cell(1, 1, 1.2), true),
            (Ticker("D1", "800080"), Place::Down, cell(0, 0, 1.6), false),
            (Ticker("D2", "806000"), Place::Down, cell(1, 1, 1.6), false),
            (Ticker("D3", "006080"), Place::Down, cell(2, 0, 1.6), false),
            (Console("Down"), Place::Down, cell(2, 2, 1.8), false),
            (Ticker("PIN", "202020"), Place::Pinned, cell(4, 2, 0.9), false),
        ];
        for (kind, place, r, minimize) in fixtures {
            let (hwnd, pid) = match kind {
                Ticker(label, rgb) => launch(Command::new(&me).args(["ticker", label, rgb]), Some(format!("PocTicker {label}").as_str())),
                Edge(label, no_occlusion) => {
                    let Some(edge) = &edge else {
                        println!("  no Edge");
                        continue;
                    };
                    let profile = temp.join(if no_occlusion { "slide_poc_edge2" } else { "slide_poc_edge" });
                    let mut c = Command::new(edge);
                    c.arg(format!("--user-data-dir={}", profile.to_string_lossy()))
                        .args(["--no-first-run", "--no-default-browser-check", "--new-window"]);
                    if no_occlusion {
                        c.args(["--disable-features=CalculateNativeWinOcclusion", "--disable-backgrounding-occluded-windows"]);
                    }
                    c.arg(format!("--app={page_url}?{label}"));
                    launch(&mut c, Some(format!("PocEdge {label}").as_str()))
                }
                Console(label) => {
                    let mut c = Command::new("powershell.exe");
                    c.args(["-NoExit", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File"]).arg(&script).arg(label);
                    c.creation_flags(0x10); // CREATE_NEW_CONSOLE
                    launch(&mut c, Some(format!("PocConsole {label}").as_str()))
                }
                Calc => launch(&mut Command::new("calc.exe"), None),
                Notepad => launch(&mut Command::new("notepad.exe"), None),
            };
            if let Some(pid) = pid {
                st.pids.push(pid);
            }
            let Some(hwnd) = hwnd else { continue };
            st.windows.push(hwnd.0);
            unsafe {
                let _ = SetWindowPos(hwnd, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOACTIVATE | SWP_NOZORDER);
            }
            let moved = match place {
                Place::Here => Ok(()),
                Place::Right => vdapi::move_window_to_desktop(desks.at(1).unwrap(), hwnd),
                Place::Down => vdapi::move_window_to_desktop(desks.at(2).unwrap(), hwnd),
                Place::Pinned => vdapi::pin_window(hwnd),
            };
            if minimize {
                unsafe { ShowWindow(hwnd, SW_MINIMIZE) };
            }
            println!("{place:?} '{}' ({}) -> {:?}", text(hwnd), exe(hwnd), moved);
        }
        std::fs::write(stage_path(), serde_json::to_string(&st)?)?;
        Ok(())
    }

    pub fn unstage() -> R<()> {
        let st: Stage = serde_json::from_str(&std::fs::read_to_string(stage_path())?)?;
        for &h in &st.windows {
            unsafe {
                let _ = PostMessageW(HWND(h), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
        }
        std::thread::sleep(Duration::from_millis(1500));
        for pid in &st.pids {
            let _ = Command::new("taskkill").args(["/PID", &pid.to_string(), "/T", "/F"]).output();
        }
        for (index, old) in &st.wallpapers {
            if !old.is_empty() {
                set_wallpaper(*index, old);
            }
        }
        let desks = Desks::read()?;
        let home = desks.at(0).unwrap();
        for id in &st.created {
            if let Some(d) = desks.all.iter().find(|d| d.id() == *id) {
                let fallback = if *d == home { desks.all[0] } else { home };
                println!("remove desktop {id}: {:?}", vdapi::remove_desktop(*d, fallback));
            }
        }
        std::fs::remove_file(stage_path())?;
        Ok(())
    }
}
