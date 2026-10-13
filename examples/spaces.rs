//! Prototype: a desktop switch that slides with the hand, like Spaces on a
//! Mac. Windows cannot draw a desktop that is not shown, so the slide is a
//! picture: the screen as it is now (Desktop Duplication) and a picture of
//! the desktop it goes to, taken the last time that desktop was on screen.
//! Both are kept on the GPU and the slide only copies parts of them into a
//! full-screen swap chain each frame. Not part of the app; it measures what
//! such a slide would cost.
//!   spaces info                  monitors, refresh rates, GPU
//!   spaces capture [n]           cost of one picture of the screen, n times
//!   spaces bench [frames]        the slide's per-frame GPU work into an
//!                                off-screen texture: nothing is shown
//!   spaces slide [ms] [switch <dir>] [shots <prefix>]
//!       shows the slide for `ms` (default 3000) along a made-up hand path,
//!       primary monitor only. `switch right|left|up|down` (up/down are a
//!       desktop index of -1/+1 too; the prototype has no grid) first takes
//!       the target desktop's picture by going there and back, and at the
//!       end really goes there. `shots` writes three frames as raw BGRA
//!       (width, height, pixels) to <prefix>-{a,mid,b}.raw.
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
        let result = match args.get(1).map(String::as_str) {
            Some("info") => proto::info(),
            Some("capture") => proto::capture(args.get(2).and_then(|n| n.parse().ok()).unwrap_or(20)),
            Some("slide") => proto::slide(&args[2..]),
            Some("bench") => proto::bench(args.get(2).and_then(|n| n.parse().ok()).unwrap_or(600)),
            _ => {
                println!("spaces info | capture [n] | slide [ms] [switch <dir>] [shots <prefix>]");
                Ok(())
            }
        };
        if let Err(e) = result {
            println!("error: {e:?}");
        }
    }
}

#[cfg(not(windows))]
fn main() {}

#[cfg(windows)]
mod proto {
    use super::vdapi;
    use std::time::{Duration, Instant};
    use windows::{
        core::{w, ComInterface, Result},
        Win32::{
            Foundation::{BOOL, CloseHandle, FILETIME, HMODULE, HWND, LPARAM, LRESULT, RECT, WPARAM},
            Graphics::{
                Direct3D::D3D_DRIVER_TYPE_UNKNOWN,
                Direct3D11::*,
                Dxgi::{Common::*, *},
                Gdi::{
                    BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, EnumDisplayMonitors,
                    EnumDisplaySettingsW, GetDC, GetMonitorInfoW, ReleaseDC, SelectObject, BITMAPINFO,
                    BITMAPINFOHEADER, DEVMODEW, DIB_RGB_COLORS, ENUM_CURRENT_SETTINGS, HDC, HMONITOR,
                    MONITORINFO, MONITORINFOEXW, SRCCOPY,
                },
            },
            System::{
                Diagnostics::ToolHelp::{
                    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
                },
                LibraryLoader::GetModuleHandleW,
                ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX},
                Threading::{GetCurrentProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
            },
            UI::WindowsAndMessaging::*,
        },
    };

    // ---- what the PC has --------------------------------------------------

    unsafe extern "system" fn monitor(m: HMONITOR, _: HDC, _: *mut RECT, _: LPARAM) -> BOOL {
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        let _ = GetMonitorInfoW(m, &mut info as *mut _ as *mut MONITORINFO);
        let mut mode = DEVMODEW { dmSize: std::mem::size_of::<DEVMODEW>() as u16, ..Default::default() };
        let _ = EnumDisplaySettingsW(windows::core::PCWSTR(info.szDevice.as_ptr()), ENUM_CURRENT_SETTINGS, &mut mode);
        let r = info.monitorInfo.rcMonitor;
        let name = String::from_utf16_lossy(&info.szDevice).trim_end_matches('\0').to_owned();
        println!(
            "monitor {name}: {}x{} at ({},{}) {} Hz{}",
            r.right - r.left,
            r.bottom - r.top,
            r.left,
            r.top,
            mode.dmDisplayFrequency,
            if info.monitorInfo.dwFlags & 1 != 0 { " primary" } else { "" }
        );
        true.into()
    }

    pub unsafe fn info() -> Result<()> {
        println!("windows build: {:?}", vdapi::windows_build());
        let _ = EnumDisplayMonitors(None, None, Some(monitor), LPARAM(0));
        let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
        let mut i = 0;
        while let Ok(adapter) = factory.EnumAdapters1(i) {
            let mut d = DXGI_ADAPTER_DESC1::default();
            adapter.GetDesc1(&mut d)?;
            let name = String::from_utf16_lossy(&d.Description).trim_end_matches('\0').to_owned();
            println!("adapter {i}: {name}, {} MB dedicated, {} MB shared", d.DedicatedVideoMemory >> 20, d.SharedSystemMemory >> 20);
            let mut o = 0;
            while let Ok(out) = adapter.EnumOutputs(o) {
                let mut od = DXGI_OUTPUT_DESC::default();
                out.GetDesc(&mut od)?;
                let r = od.DesktopCoordinates;
                println!("  output {o}: {}x{} at ({},{})", r.right - r.left, r.bottom - r.top, r.left, r.top);
                o += 1;
            }
            i += 1;
        }
        Ok(())
    }

    // ---- the GPU and the picture of the screen ----------------------------

    pub struct Gpu {
        pub device: ID3D11Device,
        pub ctx: ID3D11DeviceContext,
        pub adapter: IDXGIAdapter1,
        pub output: IDXGIOutput1,
        pub width: u32,
        pub height: u32,
        pub left: i32,
        pub top: i32,
    }

    /// The first output of the first adapter: the primary monitor.
    pub unsafe fn gpu() -> Result<Gpu> {
        let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
        let adapter = factory.EnumAdapters1(0)?;
        let output: IDXGIOutput1 = adapter.EnumOutputs(0)?.cast()?;
        let mut od = DXGI_OUTPUT_DESC::default();
        output.GetDesc(&mut od)?;
        let r = od.DesktopCoordinates;
        let mut device = None;
        let mut ctx = None;
        D3D11CreateDevice(
            &adapter,
            D3D_DRIVER_TYPE_UNKNOWN,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut ctx),
        )?;
        Ok(Gpu {
            device: device.unwrap(),
            ctx: ctx.unwrap(),
            adapter,
            output,
            width: (r.right - r.left) as u32,
            height: (r.bottom - r.top) as u32,
            left: r.left,
            top: r.top,
        })
    }

    impl Gpu {
        pub unsafe fn texture(&self, staging: bool) -> Result<ID3D11Texture2D> {
            let desc = D3D11_TEXTURE2D_DESC {
                Width: self.width,
                Height: self.height,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: if staging { D3D11_USAGE_STAGING } else { D3D11_USAGE_DEFAULT },
                BindFlags: 0,
                CPUAccessFlags: if staging { D3D11_CPU_ACCESS_READ.0 as u32 } else { 0 },
                MiscFlags: 0,
            };
            let mut t = None;
            self.device.CreateTexture2D(&desc, None, Some(&mut t))?;
            Ok(t.unwrap())
        }

        /// Waits until the GPU has done everything asked of it so far.
        pub unsafe fn finish(&self) -> Result<()> {
            let mut q = None;
            self.device.CreateQuery(&D3D11_QUERY_DESC { Query: D3D11_QUERY_EVENT, MiscFlags: 0 }, Some(&mut q))?;
            let q = q.unwrap();
            self.ctx.End(&q);
            let mut done = BOOL(0);
            while done.0 == 0 {
                let _ = self.ctx.GetData(&q, Some(&mut done as *mut _ as *mut _), 4, 0);
            }
            Ok(())
        }

        pub unsafe fn duplicate(&self) -> Result<IDXGIOutputDuplication> {
            self.output.DuplicateOutput(&self.device)
        }

        /// Copies the next frame of `dup` into `into`. Returns false on a
        /// timeout: nothing on screen changed in `wait_ms`.
        pub unsafe fn grab(&self, dup: &IDXGIOutputDuplication, into: &ID3D11Texture2D, wait_ms: u32) -> Result<bool> {
            let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut res = None;
            match dup.AcquireNextFrame(wait_ms, &mut info, &mut res) {
                Ok(()) => {}
                Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(false),
                Err(e) => return Err(e),
            }
            let tex: ID3D11Texture2D = res.unwrap().cast()?;
            self.ctx.CopyResource(into, &tex);
            dup.ReleaseFrame()?;
            Ok(true)
        }

        /// One picture of the screen as a newcomer takes it: open the
        /// duplication and take its first frame. Returns false when that
        /// frame did not come within `wait_ms` (nothing changed on screen):
        /// then the picture is taken with GDI instead.
        pub unsafe fn snap(&self, into: &ID3D11Texture2D, wait_ms: u32) -> Result<bool> {
            let dup = self.duplicate()?;
            for _ in 0..2 {
                let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
                let mut res = None;
                match dup.AcquireNextFrame(wait_ms, &mut info, &mut res) {
                    Ok(()) => {}
                    Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => break,
                    Err(e) => return Err(e),
                }
                if info.LastPresentTime != 0 {
                    let tex: ID3D11Texture2D = res.unwrap().cast()?;
                    self.ctx.CopyResource(into, &tex);
                    dup.ReleaseFrame()?;
                    return Ok(true);
                }
                dup.ReleaseFrame()?;
            }
            self.gdi_snap(into)?;
            Ok(false)
        }

        /// One picture through Windows.Graphics.Capture: it hands over the
        /// screen as it is when the capture starts, even when nothing moves.
        /// Returns the time to the first frame.
        pub unsafe fn wgc_snap(&self, into: &ID3D11Texture2D) -> Result<f64> {
            use windows::Graphics::{Capture::*, DirectX::{Direct3D11::IDirect3DDevice, DirectXPixelFormat}};
            use windows::Win32::System::WinRT::{Direct3D11::*, Graphics::Capture::IGraphicsCaptureItemInterop};
            let t = Instant::now();
            let interop = windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
            let hmon = windows::Win32::Graphics::Gdi::MonitorFromPoint(
                windows::Win32::Foundation::POINT { x: self.left, y: self.top },
                windows::Win32::Graphics::Gdi::MONITOR_DEFAULTTOPRIMARY,
            );
            let item: GraphicsCaptureItem = interop.CreateForMonitor(hmon)?;
            let dxgi: IDXGIDevice = self.device.cast()?;
            let d3d: IDirect3DDevice = CreateDirect3D11DeviceFromDXGIDevice(&dxgi)?.cast()?;
            let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(&d3d, DirectXPixelFormat::B8G8R8A8UIntNormalized, 1, item.Size()?)?;
            let session = pool.CreateCaptureSession(&item)?;
            let _ = session.SetIsCursorCaptureEnabled(false);
            if let Err(e) = session.SetIsBorderRequired(false) {
                static ONCE: std::sync::Once = std::sync::Once::new();
                ONCE.call_once(|| println!("  (yellow border cannot be turned off: {e:?})"));
            }
            session.StartCapture()?;
            let frame = loop {
                if let Ok(f) = pool.TryGetNextFrame() {
                    break f;
                }
                if t.elapsed() > Duration::from_millis(1000) {
                    return Err(windows::Win32::Foundation::E_FAIL.into());
                }
                std::thread::sleep(Duration::from_micros(500));
            };
            let access: IDirect3DDxgiInterfaceAccess = frame.Surface()?.cast()?;
            let tex: ID3D11Texture2D = access.GetInterface()?;
            self.ctx.CopyResource(into, &tex);
            let took = ms(t.elapsed());
            let _ = frame.Close();
            let _ = session.Close();
            let _ = pool.Close();
            Ok(took)
        }

        /// The screen through GDI into system memory, then up to the GPU.
        pub unsafe fn gdi_snap(&self, into: &ID3D11Texture2D) -> Result<()> {
            let (w, h) = (self.width as i32, self.height as i32);
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
            let bmp = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
            let old = SelectObject(dc, bmp);
            let r = BitBlt(dc, 0, 0, w, h, screen, self.left, self.top, SRCCOPY);
            self.ctx.UpdateSubresource(into, 0, None, bits, (w * 4) as u32, 0);
            SelectObject(dc, old);
            let _ = DeleteObject(bmp);
            let _ = DeleteDC(dc);
            ReleaseDC(None, screen);
            r
        }

        pub unsafe fn save(&self, tex: &ID3D11Texture2D, path: &str) -> Result<()> {
            let staging = self.texture(true)?;
            self.ctx.CopyResource(&staging, tex);
            let mut m = D3D11_MAPPED_SUBRESOURCE::default();
            self.ctx.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut m))?;
            let mut out = Vec::with_capacity((self.width * self.height * 4 + 8) as usize);
            out.extend_from_slice(&(self.width as i32).to_le_bytes());
            out.extend_from_slice(&(self.height as i32).to_le_bytes());
            for y in 0..self.height as usize {
                let row = std::slice::from_raw_parts((m.pData as *const u8).add(y * m.RowPitch as usize), self.width as usize * 4);
                out.extend_from_slice(row);
            }
            self.ctx.Unmap(&staging, 0);
            let _ = std::fs::write(path, out);
            Ok(())
        }

        pub unsafe fn video_mb(&self) -> f64 {
            let Ok(a3) = self.adapter.cast::<IDXGIAdapter3>() else { return -1.0 };
            let mut local = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
            let mut shared = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
            let _ = a3.QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, &mut local);
            let _ = a3.QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_NON_LOCAL, &mut shared);
            (local.CurrentUsage + shared.CurrentUsage) as f64 / 1048576.0
        }
    }

    fn ms(d: Duration) -> f64 {
        d.as_secs_f64() * 1000.0
    }

    fn stats(name: &str, v: &mut [f64]) {
        if v.is_empty() {
            return println!("{name}: none");
        }
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let mean = v.iter().sum::<f64>() / v.len() as f64;
        let p = |q: f64| v[((v.len() - 1) as f64 * q).round() as usize];
        println!("{name}: n={} mean {mean:.2} ms, median {:.2}, p95 {:.2}, max {:.2}", v.len(), p(0.5), p(0.95), p(1.0));
    }

    pub unsafe fn private_mb() -> f64 {
        let mut c = PROCESS_MEMORY_COUNTERS_EX::default();
        let _ = GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut c as *mut _ as *mut PROCESS_MEMORY_COUNTERS,
            std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        );
        c.PrivateUsage as f64 / 1048576.0
    }

    pub unsafe fn capture(n: usize) -> Result<()> {
        let g = gpu()?;
        println!("primary {}x{}, one picture = {:.1} MB", g.width, g.height, (g.width * g.height * 4) as f64 / 1048576.0);
        let tex = g.texture(false)?;
        let staging = g.texture(true)?;

        // A newcomer's picture, kept on the GPU.
        let mut fresh = vec![];
        let mut fell = 0;
        for _ in 0..n {
            let t = Instant::now();
            if !g.snap(&tex, 100)? {
                fell += 1;
            }
            g.finish()?;
            fresh.push(ms(t.elapsed()));
            std::thread::sleep(Duration::from_millis(30));
        }
        stats("desktop duplication opened each time (GDI if no frame in 100 ms), on GPU", &mut fresh);
        println!("  (fell back to GDI: {fell} of {n})");
        let mut up = vec![];
        for _ in 0..n {
            let t = Instant::now();
            g.gdi_snap(&tex)?;
            g.finish()?;
            up.push(ms(t.elapsed()));
        }
        stats("GDI picture uploaded to a GPU texture", &mut up);
        let mut wgc = vec![];
        let mut first = vec![];
        for _ in 0..n {
            let t = Instant::now();
            first.push(g.wgc_snap(&tex)?);
            g.finish()?;
            wgc.push(ms(t.elapsed()));
        }
        stats("Windows.Graphics.Capture, started each time, picture on GPU", &mut wgc);
        stats("  of which to its first frame", &mut first);

        // The same with the duplication kept open: the price per frame when
        // something on screen changes (times out when nothing does).
        let dup = g.duplicate()?;
        let _ = g.grab(&dup, &tex, 500)?;
        let mut open = vec![];
        let mut timeouts = 0;
        for _ in 0..n {
            let t = Instant::now();
            if g.grab(&dup, &tex, 50)? {
                g.finish()?;
                open.push(ms(t.elapsed()));
            } else {
                timeouts += 1;
            }
        }
        drop(dup);
        stats("desktop duplication kept open, per changed frame (includes waiting for it)", &mut open);
        println!("  (no change within 50 ms: {timeouts} of {n})");

        // Bringing it to the CPU, as one would to keep it in system memory.
        let mut back = vec![];
        for _ in 0..n {
            let t = Instant::now();
            g.ctx.CopyResource(&staging, &tex);
            let mut m = D3D11_MAPPED_SUBRESOURCE::default();
            g.ctx.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut m))?;
            g.ctx.Unmap(&staging, 0);
            back.push(ms(t.elapsed()));
        }
        stats("GPU picture read back to CPU", &mut back);

        // GDI, the old way, straight into system memory.
        let screen = GetDC(None);
        let dc = CreateCompatibleDC(screen);
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: g.width as i32,
                biHeight: -(g.height as i32),
                biPlanes: 1,
                biBitCount: 32,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let bmp = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
        let old = SelectObject(dc, bmp);
        let mut gdi = vec![];
        for _ in 0..n {
            let t = Instant::now();
            BitBlt(dc, 0, 0, g.width as i32, g.height as i32, screen, g.left, g.top, SRCCOPY)?;
            gdi.push(ms(t.elapsed()));
        }
        SelectObject(dc, old);
        let _ = DeleteObject(bmp);
        let _ = DeleteDC(dc);
        ReleaseDC(None, screen);
        stats("GDI BitBlt to system memory", &mut gdi);
        Ok(())
    }

    /// The per-frame work of the slide (two region copies of the whole
    /// screen) into a texture no one sees, waited out each frame.
    pub unsafe fn bench(n: usize) -> Result<()> {
        let mem0 = private_mb();
        let g = gpu()?;
        let vid0 = g.video_mb();
        let (now, next, back) = (g.texture(false)?, g.texture(false)?, g.texture(false)?);
        g.gdi_snap(&now)?;
        g.ctx.CopyResource(&next, &now);
        g.finish()?;
        let vid1 = g.video_mb();
        let mem1 = private_mb();
        let (w, h) = (g.width, g.height);
        let me = GetCurrentProcess();
        let cpu0 = cpu_ms(me);
        let start = Instant::now();
        let mut frames = vec![];
        for i in 0..n {
            let t = Instant::now();
            let o = ((i as f64 / n as f64) * w as f64) as u32;
            let b = |l: u32, r: u32| D3D11_BOX { left: l, top: 0, front: 0, right: r, bottom: h, back: 1 };
            if o < w { g.ctx.CopySubresourceRegion(&back, 0, 0, 0, 0, &now, 0, Some(&b(o, w))); }
            if o > 0 { g.ctx.CopySubresourceRegion(&back, 0, w - o, 0, 0, &next, 0, Some(&b(0, o))); }
            g.finish()?;
            frames.push(ms(t.elapsed()));
        }
        let total = ms(start.elapsed());
        let cpu = cpu_ms(me) - cpu0;
        println!("primary {w}x{h}, {n} frames back to back in {total:.0} ms");
        stats("GPU work per frame (submit + wait)", &mut frames);
        println!("CPU of this process while doing so: {:.1}% of one core (busy-waits on the GPU)", cpu / total * 100.0);
        println!("memory for two pictures + a frame: private +{:.1} MB, video {vid0:.1} -> {vid1:.1} MB", mem1 - mem0);
        Ok(())
    }

    // ---- CPU time of this process and of dwm ------------------------------

    fn ft(f: FILETIME) -> f64 {
        (((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64) as f64 / 10_000.0
    }

    unsafe fn cpu_ms(process: windows::Win32::Foundation::HANDLE) -> f64 {
        let (mut a, mut b, mut k, mut u) = Default::default();
        if GetProcessTimes(process, &mut a, &mut b, &mut k, &mut u).is_err() {
            return -1.0;
        }
        ft(k) + ft(u)
    }

    unsafe fn dwm() -> Option<windows::Win32::Foundation::HANDLE> {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).ok()?;
        let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut pid = None;
        let mut ok = Process32FirstW(snap, &mut e).is_ok();
        while ok {
            let name = String::from_utf16_lossy(&e.szExeFile);
            if name.trim_end_matches('\0').eq_ignore_ascii_case("dwm.exe") {
                pid = Some(e.th32ProcessID);
            }
            ok = Process32NextW(snap, &mut e).is_ok();
        }
        let _ = CloseHandle(snap);
        OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid?).ok()
    }

    // ---- the slide --------------------------------------------------------

    unsafe extern "system" fn wndproc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        DefWindowProcW(h, m, w, l)
    }

    /// Where a made-up hand is at `t` of a `total` drag, as a share of the
    /// screen: out to 0.7 with a turn back in the middle, as a hand that
    /// hesitates does.
    fn hand(t: f64, total: f64) -> f64 {
        let s = (t / total).clamp(0.0, 1.0);
        0.35 * (1.0 - (std::f64::consts::PI * s).cos()) * 1.0 - 0.12 * (2.0 * std::f64::consts::PI * s).sin().max(0.0) * s
    }

    pub unsafe fn slide(args: &[String]) -> Result<()> {
        let mut drag_ms = 3000.0;
        let mut dir: Option<&str> = None;
        let mut shots: Option<&str> = None;
        let mut i = 0;
        while i < args.len() {
            match args[i].as_str() {
                "switch" => {
                    dir = args.get(i + 1).map(String::as_str);
                    i += 1
                }
                "shots" => {
                    shots = args.get(i + 1).map(String::as_str);
                    i += 1
                }
                n => drag_ms = n.parse().unwrap_or(drag_ms),
            }
            i += 1;
        }
        let vertical = matches!(dir, Some("up" | "down"));
        let sign: i32 = if matches!(dir, Some("left" | "up")) { -1 } else { 1 };

        let mem0 = private_mb();
        let g = gpu()?;
        let vid0 = g.video_mb();
        let (w, h) = (g.width, g.height);

        // The two pictures: now, and the desktop to go to.
        let now = g.texture(false)?;
        let next = g.texture(false)?;
        let t = Instant::now();
        let dd = g.wgc_snap(&now).is_ok() || { g.gdi_snap(&now)?; false };
        g.finish()?;
        let snap_ms = ms(t.elapsed());
        let mut target = None;
        let mut home = None;
        if dir.is_some() {
            vdapi::select(false);
            let desktops = vdapi::get_desktops().map_err(|_| windows::core::Error::from(windows::Win32::Foundation::E_FAIL))?;
            let cur = vdapi::get_current_desktop().map_err(|_| windows::core::Error::from(windows::Win32::Foundation::E_FAIL))?;
            let at = desktops.iter().position(|d| *d == cur).unwrap_or(0) as i32;
            let to = at + sign;
            if to < 0 || to >= desktops.len() as i32 {
                println!("no desktop that way");
                return Ok(());
            }
            target = Some(desktops[to as usize].clone());
            home = Some(cur);
            // As Flick would have it from the last visit: go, take, come back.
            let _ = vdapi::switch_desktop(target.clone().unwrap());
            std::thread::sleep(Duration::from_millis(400));
            if g.wgc_snap(&next).is_err() { g.gdi_snap(&next)?; }
            let _ = vdapi::switch_desktop(home.clone().unwrap());
            std::thread::sleep(Duration::from_millis(400));
            if g.wgc_snap(&now).is_err() { g.gdi_snap(&now)?; }
        } else {
            // No target: the same picture, darkened by nothing; the cost of
            // drawing does not depend on what is in it.
            g.ctx.CopyResource(&next, &now);
        }
        g.finish()?;

        // A full-screen window kept out of screen pictures, with a flip
        // swap chain the pictures are copied into.
        let hinst = GetModuleHandleW(None)?;
        let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: hinst.into(), lpszClassName: w!("flick.spaces"), ..Default::default() };
        RegisterClassW(&wc);
        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_NOREDIRECTIONBITMAP,
            w!("flick.spaces"),
            w!(""),
            WS_POPUP,
            g.left,
            g.top,
            w as i32,
            h as i32,
            None,
            None,
            hinst,
            None,
        );
        let _ = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);
        let factory: IDXGIFactory2 = CreateDXGIFactory1()?;
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: w,
            Height: h,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            ..Default::default()
        };
        let chain = factory.CreateSwapChainForHwnd(&g.device, hwnd, &desc, None, None)?;
        if let Ok(d1) = g.device.cast::<IDXGIDevice1>() {
            let _ = d1.SetMaximumFrameLatency(1);
        }

        let draw = |off: f64| -> Result<()> {
            let back: ID3D11Texture2D = chain.GetBuffer(0)?;
            let full = if vertical { h } else { w } as f64;
            let o = (off.clamp(0.0, 1.0) * full).round() as u32;
            let b = |l: u32, t: u32, r: u32, bt: u32| D3D11_BOX { left: l, top: t, front: 0, right: r, bottom: bt, back: 1 };
            // Going right, the screen moves left: now's right part on the
            // left, next's left part after it. The other ways likewise.
            match (vertical, sign) {
                (false, 1) => {
                    if o < w { g.ctx.CopySubresourceRegion(&back, 0, 0, 0, 0, &now, 0, Some(&b(o, 0, w, h))); }
                    if o > 0 { g.ctx.CopySubresourceRegion(&back, 0, w - o, 0, 0, &next, 0, Some(&b(0, 0, o, h))); }
                }
                (false, _) => {
                    if o > 0 { g.ctx.CopySubresourceRegion(&back, 0, 0, 0, 0, &next, 0, Some(&b(w - o, 0, w, h))); }
                    if o < w { g.ctx.CopySubresourceRegion(&back, 0, o, 0, 0, &now, 0, Some(&b(0, 0, w - o, h))); }
                }
                (true, 1) => {
                    if o < h { g.ctx.CopySubresourceRegion(&back, 0, 0, 0, 0, &now, 0, Some(&b(0, o, w, h))); }
                    if o > 0 { g.ctx.CopySubresourceRegion(&back, 0, 0, h - o, 0, &next, 0, Some(&b(0, 0, w, o))); }
                }
                (true, _) => {
                    if o > 0 { g.ctx.CopySubresourceRegion(&back, 0, 0, 0, 0, &next, 0, Some(&b(0, h - o, w, h))); }
                    if o < h { g.ctx.CopySubresourceRegion(&back, 0, 0, o, 0, &now, 0, Some(&b(0, 0, w, h - o))); }
                }
            }
            Ok(())
        };

        draw(0.0)?;
        let _ = chain.Present(1, 0);
        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        let pump = || {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).into() {
                DispatchMessageW(&msg);
            }
        };
        pump();
        let mem1 = private_mb();
        let vid1 = g.video_mb();

        let me = GetCurrentProcess();
        let dwm = dwm();
        let cpu0 = cpu_ms(me);
        let dwm0 = dwm.map(|d| cpu_ms(d)).unwrap_or(-1.0);
        let start = Instant::now();
        let mut last = start;
        let mut frames = vec![];
        let mut shot_mid = false;
        // The drag, then the release: the rest of the way in 180 ms.
        let release_ms = 180.0;
        let mut at_release = 0.0;
        loop {
            let t = ms(start.elapsed());
            let off = if t < drag_ms {
                hand(t, drag_ms)
            } else {
                if at_release == 0.0 {
                    at_release = hand(drag_ms, drag_ms);
                }
                let s = ((t - drag_ms) / release_ms).min(1.0);
                at_release + (1.0 - at_release) * (1.0 - (1.0 - s).powi(3))
            };
            draw(off)?;
            if !shot_mid && t > drag_ms * 0.5 {
                shot_mid = true;
                if let Some(p) = shots {
                    let back: ID3D11Texture2D = chain.GetBuffer(0)?;
                    g.save(&back, &format!("{p}-mid.raw"))?;
                }
            }
            let _ = chain.Present(1, 0);
            pump();
            let n = Instant::now();
            frames.push(ms(n - last));
            last = n;
            if t >= drag_ms + release_ms {
                break;
            }
        }
        let slide_ms = ms(start.elapsed());
        let cpu = cpu_ms(me) - cpu0;
        let dwm_cpu = dwm.map(|d| cpu_ms(d)).unwrap_or(-1.0) - dwm0;
        let mut stat = DXGI_FRAME_STATISTICS::default();
        let _ = chain.GetFrameStatistics(&mut stat);

        // The real switch under the finished slide, then the slide comes off.
        let mut switch_line = String::new();
        if let Some(target) = target {
            let dup = g.duplicate()?;
            let scratch = g.texture(false)?;
            let _ = g.grab(&dup, &scratch, 0);
            let t0 = Instant::now();
            let _ = vdapi::switch_desktop(target);
            let call = ms(t0.elapsed());
            // The window is kept out of duplication, so the first frame
            // that changes after the call is the new desktop being shown.
            let mut first = -1.0;
            while t0.elapsed() < Duration::from_millis(1500) {
                if g.grab(&dup, &scratch, 20)? {
                    first = ms(t0.elapsed());
                    break;
                }
            }
            switch_line = format!("switch call {call:.1} ms, first new frame under the slide {first:.1} ms after the call");
            if let Some(p) = shots {
                g.save(&scratch, &format!("{p}-b-real.raw"))?;
            }
        }
        if let Some(p) = shots {
            g.save(&now, &format!("{p}-a.raw"))?;
            g.save(&next, &format!("{p}-b.raw"))?;
        }
        let _ = DestroyWindow(hwnd);
        pump();

        let n = frames.len();
        let period = 1000.0 / (n as f64 / (slide_ms / 1000.0));
        println!("primary {w}x{h}; picture of the screen {snap_ms:.1} ms ({})", if dd { "Windows.Graphics.Capture" } else { "GDI" });
        println!("slide {slide_ms:.0} ms, {n} frames = {:.1} fps", n as f64 / (slide_ms / 1000.0));
        let late = frames.iter().skip(1).filter(|&&f| f > period * 1.5).count();
        stats("frame time", &mut frames);
        println!("frames over 1.5x the mean: {late}; present count {} refresh count {}", stat.PresentCount, stat.PresentRefreshCount);
        println!(
            "CPU: this process {:.1}% of one core, dwm {:.1}% of one core",
            cpu / slide_ms * 100.0,
            if dwm_cpu >= 0.0 { dwm_cpu / slide_ms * 100.0 } else { -1.0 }
        );
        println!("memory: private +{:.1} MB, video (process) {vid0:.1} -> {vid1:.1} MB", mem1 - mem0);
        if !switch_line.is_empty() {
            println!("{switch_line}");
        }
        if let Some(home) = home {
            std::thread::sleep(Duration::from_millis(800));
            let _ = vdapi::switch_desktop(home);
        }
        Ok(())
    }
}
