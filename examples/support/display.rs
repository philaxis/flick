//! Composed desktop pixels, independently of Flick's render-loop clock.
//! Run navigation-e2e.ps1 -Mode display with a motion-probe build.
use std::{fmt::Write, path::Path, process::{Command, Stdio}, time::{Duration, Instant}};
use windows::{core::{w, ComInterface, Result}, Win32::{
    Foundation::{CloseHandle, HMODULE, HWND, LPARAM, WPARAM},
    Graphics::{Direct3D::D3D_DRIVER_TYPE_UNKNOWN, Direct3D11::*, Dxgi::{*, Common::*}},
    System::{Performance::{QueryPerformanceCounter, QueryPerformanceFrequency}, Threading::*},
    UI::{Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
}};

fn qpc() -> i64 { let mut n = 0; unsafe { QueryPerformanceCounter(&mut n).unwrap(); } n }

struct Capture {
    duplication: IDXGIOutputDuplication,
    context: ID3D11DeviceContext,
    staging: ID3D11Texture2D,
    width: u32,
    height: u32,
    sample: std::cell::RefCell<Option<Vec<u8>>>,
}
impl Capture {
    unsafe fn new() -> Result<Self> {
        let factory: IDXGIFactory1 = CreateDXGIFactory1()?;
        let mut a = 0;
        loop {
            let adapter = factory.EnumAdapters(a)?;
            a += 1;
            let mut o = 0;
            while let Ok(output) = adapter.EnumOutputs(o) {
                o += 1;
                let mut desc = DXGI_OUTPUT_DESC::default(); output.GetDesc(&mut desc)?;
                if !desc.AttachedToDesktop.as_bool() || desc.DesktopCoordinates.left != 0 || desc.DesktopCoordinates.top != 0 { continue; }
                let (mut device, mut context) = (None, None);
                D3D11CreateDevice(&adapter, D3D_DRIVER_TYPE_UNKNOWN, HMODULE(0), D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION, Some(&mut device), None, Some(&mut context))?;
                let device = device.unwrap();
                let duplication = output.cast::<IDXGIOutput1>()?.DuplicateOutput(&device)?;
                let mut desc = DXGI_OUTDUPL_DESC::default(); duplication.GetDesc(&mut desc);
                assert_eq!(desc.Rotation, DXGI_MODE_ROTATION_IDENTITY, "rotated output not supported by this probe");
                let (width, height) = (desc.ModeDesc.Width, desc.ModeDesc.Height);
                let texture = D3D11_TEXTURE2D_DESC { Width: width, Height: height, MipLevels: 1, ArraySize: 1, Format: DXGI_FORMAT_B8G8R8A8_UNORM, SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, Usage: D3D11_USAGE_STAGING, CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32, ..Default::default() };
                let mut staging = None; device.CreateTexture2D(&texture, None, Some(&mut staging))?;
                println!("duplication {width}x{height}, mode {} / {} Hz", desc.ModeDesc.RefreshRate.Numerator, desc.ModeDesc.RefreshRate.Denominator);
                return Ok(Self { duplication, context: context.unwrap(), staging: staging.unwrap(), width, height, sample: std::cell::RefCell::new(None) });
            }
        }
    }
    unsafe fn frame(&self, save: bool) -> Result<Option<(DXGI_OUTDUPL_FRAME_INFO, [(i32,i32,i32,i32,u32); 4], [f64;4], f64)>> {
        let (mut info, mut resource) = (DXGI_OUTDUPL_FRAME_INFO::default(), None);
        match self.duplication.AcquireNextFrame(50, &mut info, &mut resource) {
            Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(None),
            r => r?,
        }
        if info.LastPresentTime == 0 { self.duplication.ReleaseFrame()?; return Ok(None); }
        let start = Instant::now();
        let source = resource.unwrap().cast::<ID3D11Texture2D>();
        if let Ok(source) = &source { self.context.CopyResource(&self.staging, source); }
        self.duplication.ReleaseFrame()?;
        source?;
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        self.context.Map(&self.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
        let mut boxes = [(i32::MAX, i32::MAX, -1, -1, 0); 4];
        let (mut mass,mut moment)=([0u64;4],[0u64;4]);
        for y in (20..self.height.saturating_sub(60)).step_by(4) {
            let line = std::slice::from_raw_parts((mapped.pData as *const u8).add((y * mapped.RowPitch) as usize), self.width as usize * 4);
            scan_line(line,y,&mut boxes,&mut mass,&mut moment);
        }
        if save && self.sample.borrow().is_none() {
            let mut raw=Vec::with_capacity((self.width*self.height*4+8) as usize);
            raw.extend_from_slice(&self.width.to_le_bytes());raw.extend_from_slice(&self.height.to_le_bytes());
            for y in 0..self.height {raw.extend_from_slice(std::slice::from_raw_parts((mapped.pData as *const u8).add((y*mapped.RowPitch) as usize),self.width as usize*4));}
            *self.sample.borrow_mut()=Some(raw);
        }
        self.context.Unmap(&self.staging, 0);
        let centers=std::array::from_fn(|i| if mass[i]>0 {moment[i] as f64/mass[i] as f64} else {f64::NAN});
        Ok(Some((info, boxes, centers, start.elapsed().as_secs_f64()*1000.0)))
    }
}

fn write_frame(csv: &mut String, (info,boxes,centers,ms): (DXGI_OUTDUPL_FRAME_INFO,[(i32,i32,i32,i32,u32);4],[f64;4],f64)) {
    write!(csv,"{},{},{ms:.3}",info.LastPresentTime,info.AccumulatedFrames).unwrap();
    for b in boxes {write!(csv,",{},{},{},{},{}",b.0,b.1,b.2,b.3,b.4).unwrap();}
    for x in centers {write!(csv,",{x:.6}").unwrap();}csv.push('\n');
}

// Long runs isolate fixtures from desktop icons; y subsampling preserves x precision.
fn scan_line(line: &[u8], y: u32, boxes: &mut [(i32,i32,i32,i32,u32);4], mass: &mut [u64;4], moment: &mut [u64;4]) {
    let width=line.len() as u32/4;
            let classify = |x: u32| {
                let p = &line[x as usize * 4..][..3];
                let (b,g,r) = (p[0] as u32,p[1] as u32,p[2] as u32);
                if r > 40 && b > 40 && g * 3 < r.min(b) && r.abs_diff(b)*10 <= r.max(b) { Some((0,r.min(b)-g)) }
                else if b > 40 && g > 40 && r * 3 < b.min(g) && b.abs_diff(g)*10 <= b.max(g) { Some((1,b.min(g)-r)) }
                else if r > 40 && g > 40 && b * 3 < r.min(g) && r.abs_diff(g)*10 <= r.max(g) { Some((2,r.min(g)-b)) }
                else if y < 40 && g > 180 && r < 40 && b < 40 { Some((3,g-r.max(b))) } else { None }
            };
            let mut x=0;
            while x < width {
                let Some((c,_))=classify(x) else { x+=1; continue; };
                let left=x;
                let (mut m,mut mx)=(0u64,0u64);
                while x < width {
                    match classify(x) { Some((other,w)) if other==c => {m+=w as u64;mx+=w as u64*x as u64;x+=1;}, _=>break }
                }
                let width=x-left;
                if width > boxes[c].4 && width >= if c==3 {2} else {32} {
                    boxes[c]=(left as i32,y as i32,x as i32-1,y as i32,width);
                    mass[c]=m;moment[c]=mx;
                }
            }
}

// Smooth velocity ramps and deliberately stationary intervals; units are cells.
fn hand(scenario: &str, t: f64) -> f64 {
    let smooth = |t: f64| { let u=t.clamp(0.0,1.0); u*u*u*(10.0+u*(-15.0+6.0*u)) };
    match scenario {
        "slow" => 0.08 * t,
        "normal" => 0.24 * t,
        "fast" => 0.65 * t,
        "mixed" => {
            let knots = [(0.0,0.0),(1.2,0.75),(1.6,0.75),(2.4,1.65),(3.8,0.1),(4.2,0.1),(5.3,1.1),(6.0,0.3)];
            for pair in knots.windows(2) { if t <= pair[1].0 { return pair[0].1+(pair[1].1-pair[0].1)*smooth((t-pair[0].0)/(pair[1].0-pair[0].0)); } }
            0.3
        }
        _ => panic!("unknown scenario"),
    }
}

unsafe fn input(kind: &str, scenario: &str, out: &Path, duration: f64, frequency: i64, device_clock: bool) {
    let pad = FindWindowExW(HWND_MESSAGE, None, w!("flick.input"), None);
    assert!(kind != "ptp" || pad.0 != 0);
    let mouse = |flags, dx| {
        let input = INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, mouseData: 2, dwFlags: flags, ..Default::default() } } };
        assert_eq!(SendInput(&[input], std::mem::size_of::<INPUT>() as i32), 1);
    };
    let touch = |x: f64, count| {
        let x = (x * 1200.0).round() as i32;
        let scan=((qpc() as i128*10000/frequency as i128) & 65535) as usize;
        let (message,data)=if device_clock {(WM_APP+62,count | scan<<8)} else {(WM_APP+61,count)};
        PostMessageW(pad, message, WPARAM(data), LPARAM(x as u32 as isize)).unwrap();
    };
    if kind == "mouse" { mouse(MOUSEEVENTF_XDOWN,0); } else { touch(0.0,3); }
    std::thread::sleep(Duration::from_millis(80));
    let timer = CreateWaitableTimerExW(None,None,CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,TIMER_ALL_ACCESS.0).unwrap();
    let start = qpc();
    let mut csv = String::from("qpc,seconds,hand_cells,dx\n");
    let (mut last, mut due, mut seed) = (0, 0.0, 17u32);
    while due <= duration {
        let actual = (qpc()-start) as f64/frequency as f64;
        let x=hand(scenario,actual.min(duration));
        let pos=(x*1000.0).round() as i32;
        if kind == "mouse" { mouse(MOUSEEVENTF_MOVE | MOUSEEVENTF_MOVE_NOCOALESCE,pos-last); } else { touch(x,3); }
        writeln!(csv,"{},{actual:.6},{x:.6},{}",qpc(),pos-last).unwrap(); last=pos;
        seed=seed.wrapping_mul(1664525).wrapping_add(1013904223);
        due += if kind=="mouse" { 0.0008+(seed%401) as f64/1e6 } else { 0.007+(seed%1001) as f64/1e6 };
        let remaining=due-(qpc()-start) as f64/frequency as f64;
        if remaining>0.0 { let ticks=-(remaining*1e7).ceil() as i64; SetWaitableTimerEx(timer,&ticks,0,None,None,None,0).unwrap(); WaitForSingleObject(timer,100); }
    }
    if kind == "mouse" { mouse(MOUSEEVENTF_XUP,0); } else { touch(hand(scenario,duration),0); }
    CloseHandle(timer).unwrap();
    std::fs::write(out,csv).unwrap();
}

pub unsafe fn run(out: &Path, flick: &str) {
    let wait = |ms| std::thread::sleep(Duration::from_millis(ms));
    let cfg = std::path::PathBuf::from(std::env::var_os("APPDATA").unwrap()).join("Flick");
    std::fs::write(cfg.join("config.toml"), "config_version = 3\ntrigger = \"xbutton2\"\nminimap = false\nhotkeys = false\n[mouse]\nstep_x = 1000\nstep_y = 1000\nnotch_x = 0\nnotch_y = 0\nfollow_x = true\nfollow_y = true\nreverse = false\n[touchpad]\nthree_finger = true\nstep_x = 12\nstep_y = 12\nnotch_x = 0\nnotch_y = 0\nfollow_x = true\nfollow_y = true\nreverse = false\n").unwrap();
    if let Ok(speed)=std::env::var("FLICK_PROBE_SPEED") {
        assert!(["default","gentle","slow"].contains(&speed.as_str()));
        use std::io::Write;
        writeln!(std::fs::OpenOptions::new().append(true).open(cfg.join("config.toml")).unwrap(),"\n[experimental]\ncommit_speed = \"{}\"",speed).unwrap();
    }
    if std::env::var_os("FLICK_PROBE_BLUR").is_some() {
        use std::io::Write;
        writeln!(std::fs::OpenOptions::new().append(true).open(cfg.join("config.toml")).unwrap(),"\n[experimental]\nmotion_blur = true").unwrap();
    }
    let made=std::fs::read_to_string(out.join("made.txt")).unwrap();
    let ids: Vec<_>=made.lines().collect();
    let all=super::vdapi::get_desktops().unwrap();
    let home=*all.iter().find(|d|d.id()==ids[0]).unwrap();
    let other: Vec<_>=all.iter().map(|d|d.id()).filter(|id|!ids.contains(&id.as_str())).collect();
    std::fs::write(cfg.join("state.json"),serde_json::json!({"rows":[{"name":"Motion","cells":ids},{"name":"Existing","cells":other}],"guide_seen":true}).to_string()).unwrap();
    super::vdapi::switch_desktop(home).unwrap(); wait(800);
    let me=std::env::current_exe().unwrap();
    let mut windows=Vec::new();
    for (i,rgb) in [["255","0","255"],["0","255","255"],["255","255","0"]].iter().enumerate() {
        let title=format!("F motion {i}");
        windows.push(Command::new(&me).args(["hold",rgb[0],rgb[1],rgb[2],&title,"200","200","600","400","1200"])
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap());
        wait(400);
        let hwnd=FindWindowW(w!("flick.test.hold"), &windows::core::HSTRING::from(title)); assert!(hwnd.0!=0);
        super::vdapi::move_window_to_desktop(*all.iter().find(|d|d.id()==ids[i]).unwrap(),hwnd).unwrap();
    }
    let mut frequency=0; QueryPerformanceFrequency(&mut frequency).unwrap();
    let mut timing=windows::Win32::Graphics::Dwm::DWM_TIMING_INFO {cbSize:std::mem::size_of::<windows::Win32::Graphics::Dwm::DWM_TIMING_INFO>() as u32,..Default::default()};
    windows::Win32::Graphics::Dwm::DwmGetCompositionTimingInfo(HWND(0),&mut timing).unwrap();
    let refresh_ticks = timing.qpcRefreshPeriod;
    std::fs::write(out.join("display-meta.json"),serde_json::json!({"frequency":frequency,"refresh_ticks":refresh_ticks,"composition":std::env::var("FLICK_COMPOSITION").map_or("auto".to_string(),|v| if v=="0" {"off".into()} else {"all".into()}),"capture":"desktop duplication, not panel scanout","width":1920}).to_string()).unwrap();
    for mode in ["follow","board"] { for kind in ["mouse","ptp"] { for scenario in ["slow","normal","fast","mixed"] {
        if std::env::var_os("FLICK_SHORT_PROBE").is_some() && scenario != "fast" && scenario != "mixed" { continue; }
        super::vdapi::switch_desktop(home).unwrap(); wait(500);
        let label=format!("{mode}-{kind}-{scenario}");
        if std::env::var("FLICK_PROBE_CASES").is_ok_and(|cases| !cases.split(',').any(|c|c==label)) { continue; }
        let mut child=Command::new(flick).arg("--portable").env("FLICK_INPUT_TRACE",out.join(format!("{label}-received.csv"))).env("FLICK_TRACE",out.join(format!("{label}-app.csv")))
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
        wait(1200);
        let app=FindWindowW(w!("flick.main"),None); assert!(app.0!=0);
        if mode=="board" {PostMessageW(app,WM_APP+2,WPARAM(0),LPARAM(0)).unwrap();wait(700);assert!(IsWindowVisible(FindWindowW(w!("flick.board"),None)).as_bool());}
        let mut gpu=if std::env::var_os("FLICK_GPU_TRACE").is_some() {
            let file=out.join(format!("{label}-gpu.csv"));
            let job=Command::new("powershell.exe").args(["-NoProfile","-File","scripts/gpu-motion.ps1","-FlickPid",&child.id().to_string(),"-CapturePid",&std::process::id().to_string(),"-Output"])
                .arg(&file).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
            let began=Instant::now();while !file.with_extension("ready").exists() && began.elapsed()<Duration::from_secs(8) {wait(20);}
            Some(job)
        } else {None};
        let capture=Capture::new().unwrap();
        // Drain setup before injecting; AccumulatedFrames remains visible in the CSV.
        let initial=capture.frame(false).unwrap();
        let (k,s,path)=(kind.to_owned(),scenario.to_owned(),out.join(format!("{label}-input.csv")));
        let device_clock=!Path::new(flick).file_name().unwrap().to_string_lossy().eq_ignore_ascii_case("baseline.exe");
        let worker=std::thread::spawn(move||input(&k,&s,&path,6.0,frequency,device_clock));
        let start=Instant::now();
        let mut csv=String::from("qpc,accumulated,capture_ms,magenta_l,magenta_t,magenta_r,magenta_b,magenta_n,cyan_l,cyan_t,cyan_r,cyan_b,cyan_n,yellow_l,yellow_t,yellow_r,yellow_b,yellow_n,marker_l,marker_t,marker_r,marker_b,marker_n,magenta_x,cyan_x,yellow_x,marker_x\n");
        if let Some(frame)=initial { write_frame(&mut csv,frame); }
        let(mut frames,mut missed)=(0,0);
        while start.elapsed()<Duration::from_millis(7200) {
            if let Some((info,boxes,centers,ms))=capture.frame(kind=="mouse" && scenario=="fast" && start.elapsed()>Duration::from_millis(1000)).unwrap() {
                frames+=1;missed+=info.AccumulatedFrames.saturating_sub(1);
                write_frame(&mut csv,(info,boxes,centers,ms));
            }
        }
        worker.join().unwrap();
        if let Some(job)=&mut gpu { let _=job.wait(); }
        if let Some(raw)=capture.sample.into_inner() {std::fs::write(out.join(format!("{label}.raw")),raw).unwrap();}
        std::fs::write(out.join(format!("{label}-display.csv")),csv).unwrap();
        if mode=="board" {PostMessageW(FindWindowW(w!("flick.board"),None),WM_KEYDOWN,WPARAM(0x1b),LPARAM(0)).unwrap();wait(300);}
        let _=child.kill();let _=child.wait();
        println!("{label}: captured={frames} accumulated_extra={missed}");
    }}}
    for window in &mut windows {let _=window.kill();let _=window.wait();}
}

impl Capture {
    /// The next composed frame, tightly packed BGRA, and when it was presented.
    unsafe fn grab(&self) -> Result<Option<(i64, Vec<u8>)>> {
        let (mut info, mut resource) = (DXGI_OUTDUPL_FRAME_INFO::default(), None);
        match self.duplication.AcquireNextFrame(30, &mut info, &mut resource) {
            Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(None),
            r => r?,
        }
        if info.LastPresentTime == 0 { self.duplication.ReleaseFrame()?; return Ok(None); }
        let source = resource.unwrap().cast::<ID3D11Texture2D>();
        if let Ok(source) = &source { self.context.CopyResource(&self.staging, source); }
        self.duplication.ReleaseFrame()?;
        source?;
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        self.context.Map(&self.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
        let mut raw = Vec::with_capacity((self.width * self.height * 4) as usize);
        for y in 0..self.height { raw.extend_from_slice(std::slice::from_raw_parts((mapped.pData as *const u8).add((y * mapped.RowPitch) as usize), self.width as usize * 4)); }
        self.context.Unmap(&self.staging, 0);
        Ok(Some((info.LastPresentTime, raw)))
    }
}

/// Half size, as a 24-bit BMP.
fn save_bmp(path: &Path, raw: &[u8], width: u32, height: u32) {
    let (w, h) = (width / 2, height / 2);
    let row = ((w * 3 + 3) / 4 * 4) as usize;
    let mut out = Vec::with_capacity(54 + row * h as usize);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&((54 + row * h as usize) as u32).to_le_bytes());
    out.extend_from_slice(&[0, 0, 0, 0, 54, 0, 0, 0, 40, 0, 0, 0]);
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&(-(h as i32)).to_le_bytes());
    out.extend_from_slice(&[1, 0, 24, 0]);
    out.extend_from_slice(&[0u8; 24]);
    for y in 0..h {
        let start = out.len();
        for x in 0..w { let i = ((y * 2 * width + x * 2) * 4) as usize; out.extend_from_slice(&raw[i..i + 3]); }
        out.resize(start + row, 0);
    }
    std::fs::write(path, out).unwrap();
}

const RECEDE_COLOURS: [(u8, u8, u8); 4] = [(255, 0, 255), (0, 255, 255), (255, 255, 0), (0, 200, 0)];

/// Of a frame: the width of the longest run of the middle desktop's cyan
/// across the middle line, as a fraction of the screen, and the fraction of
/// pixels (every 4th) that are neither dark nor a window's colour, dimmed or not.
fn recede_stats(raw: &[u8], width: u32, height: u32) -> (f64, f64) {
    let px = |x: u32, y: u32| { let i = ((y * width + x) * 4) as usize; (raw[i + 2] as f64, raw[i + 1] as f64, raw[i] as f64) };
    let cyan = |(r, g, b): (f64, f64, f64)| r < 60.0 && g > 60.0 && b > 60.0 && (g - b).abs() < 0.2 * g.max(b);
    let (mut best, mut run) = (0u32, 0u32);
    for x in 0..width { if cyan(px(x, height / 2)) { run += 1; best = best.max(run); } else { run = 0; } }
    let (mut other, mut all) = (0u64, 0u64);
    for y in (0..height).step_by(4) { for x in (0..width).step_by(4) {
        let (r, g, b) = px(x, y);
        all += 1;
        if r.max(g).max(b) < 40.0 { continue; }
        let window = RECEDE_COLOURS.iter().any(|&(cr, cg, cb)| {
            let (cr, cg, cb) = (cr as f64, cg as f64, cb as f64);
            let k = (r * cr + g * cg + b * cb) / (cr * cr + cg * cg + cb * cb);
            (0.15..1.1).contains(&k) && (r - k * cr).abs() + (g - k * cg).abs() + (b - k * cb).abs() < 45.0
        });
        if !window { other += 1; }
    }}
    (best as f64 / width as f64, other as f64 / all as f64)
}

/// The normal view receding over maximized windows, recorded as composed:
/// `recede-frames.csv` (each frame's time from the trigger and from letting
/// go, the middle desktop's width and how much is neither dark nor a
/// window), and frames as BMPs: the real desktops before, the first ones of
/// each hold, every fifth, and all in the half second after letting go.
pub unsafe fn recede(out: &Path, flick: &str) {
    use std::sync::{atomic::{AtomicBool, AtomicI64, Ordering}, Arc, Mutex};
    let wait = |ms| std::thread::sleep(Duration::from_millis(ms));
    let cfg = std::path::PathBuf::from(std::env::var_os("APPDATA").unwrap()).join("Flick");
    std::fs::write(cfg.join("config.toml"), "config_version = 3\ntrigger = \"xbutton2\"\nminimap = false\nhotkeys = false\n[mouse]\nstep_x = 220\nstep_y = 220\nnotch_x = 40\nnotch_y = 100\nfollow_x = true\nfollow_y = true\nreverse = false\n").unwrap();
    let made = std::fs::read_to_string(out.join("made.txt")).unwrap();
    let ids: Vec<_> = made.lines().collect();
    let all = super::vdapi::get_desktops().unwrap();
    let desk = |i: usize| *all.iter().find(|d| d.id() == ids[i]).unwrap();
    let other: Vec<_> = all.iter().map(|d| d.id()).filter(|id| !ids.contains(&id.as_str())).collect();
    std::fs::write(cfg.join("state.json"), serde_json::json!({"rows":[{"name":"Recede","cells":&ids[0..3],"last":ids[1]},{"name":"Below","cells":&ids[3..6],"last":ids[4]},{"name":"Existing","cells":other}],"guide_seen":true}).to_string()).unwrap();
    super::vdapi::switch_desktop(desk(1)).unwrap(); wait(800);
    // Maximized windows on the row's three desktops and the one below the middle;
    // a small one on the next below, the last one bare.
    let me = std::env::current_exe().unwrap();
    let mut windows = Vec::new();
    for (i, rgb, rect) in [(0, (255, 0, 255), "0 0 0 0"), (1, (0, 255, 255), "0 0 0 0"), (2, (255, 255, 0), "0 0 0 0"), (4, (0, 200, 0), "0 0 0 0"), (3, (0, 200, 0), "300 200 700 450")] {
        let title = format!("F recede {i}");
        let mut args = vec!["hold".to_string(), rgb.0.to_string(), rgb.1.to_string(), rgb.2.to_string(), title.clone()];
        args.extend(rect.split(' ').map(str::to_owned));
        args.push("600".into());
        windows.push(Command::new(&me).args(&args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap());
        wait(500);
        let hwnd = FindWindowW(w!("flick.test.hold"), &windows::core::HSTRING::from(title)); assert!(hwnd.0 != 0);
        super::vdapi::move_window_to_desktop(desk(i), hwnd).unwrap();
    }
    let stop = Arc::new(AtomicBool::new(false));
    let since = Arc::new(AtomicI64::new(0));
    let released = Arc::new(AtomicI64::new(0));
    let snap: Arc<Mutex<Option<String>>> = Arc::default();
    let (o, s1, s2, s3, s4) = (out.to_owned(), stop.clone(), since.clone(), snap.clone(), released.clone());
    let recorder = std::thread::spawn(move || {
        let capture = Capture::new().unwrap();
        let mut frequency = 0; QueryPerformanceFrequency(&mut frequency).unwrap();
        let mut csv = String::from("hold,ms,after_release_ms,middle_width,other\n");
        let (mut latest, mut hold, mut nth, mut was) = (Vec::new(), 0, 0, 0i64);
        while !s1.load(Ordering::SeqCst) {
            if let Some((present, raw)) = capture.grab().unwrap() {
                let from = s2.load(Ordering::SeqCst);
                if from != was { was = from; if from != 0 { hold += 1; nth = 0; } }
                if from != 0 {
                    let (middle, other) = recede_stats(&raw, capture.width, capture.height);
                    let ms = (present - from) as f64 * 1000.0 / frequency as f64;
                    let let_go = s4.load(Ordering::SeqCst);
                    let after = if let_go > from { (present - let_go) as f64 * 1000.0 / frequency as f64 } else { f64::NAN };
                    writeln!(csv, "{hold},{ms:.1},{after:.1},{middle:.4},{other:.4}").unwrap();
                    if nth < 14 || nth % 5 == 0 || (0.0..500.0).contains(&after) { save_bmp(&o.join(format!("h{hold}-{nth:03}-{ms:.0}ms.bmp")), &raw, capture.width, capture.height); }
                    nth += 1;
                }
                latest = raw;
            }
            if let Some(label) = s3.lock().unwrap().take() {
                if !latest.is_empty() { save_bmp(&o.join(format!("{label}.bmp")), &latest, capture.width, capture.height); }
            }
        }
        std::fs::write(o.join("recede-frames.csv"), csv).unwrap();
    });
    wait(600);
    for (label, i) in [("real-0", 0), ("real-2", 2), ("real-3", 3), ("real-1", 1)] {
        super::vdapi::switch_desktop(desk(i)).unwrap(); wait(1200);
        *snap.lock().unwrap() = Some(label.into()); wait(150);
    }
    let mut child = Command::new(flick).arg("--portable").env("FLICK_GLIDE_STATS", "1").env("FLICK_TRACE", out.join("trace.csv"))
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
    wait(3000);
    assert!(FindWindowW(w!("flick.main"), None).0 != 0, "Flick did not start");
    let mouse = |flags, dx, dy| {
        let input = INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, dy, mouseData: 2, dwFlags: flags, ..Default::default() } } };
        assert_eq!(SendInput(&[input], std::mem::size_of::<INPUT>() as i32), 1);
    };
    let stroke = |dx: i32, dy: i32, ms: u64| {
        let n = (ms / 8).max(1) as i32;
        for i in 1..=n { mouse(MOUSEEVENTF_MOVE, dx * i / n - dx * (i - 1) / n, dy * i / n - dy * (i - 1) / n); wait(ms / n as u64); }
    };
    let begin = || { since.store(qpc(), Ordering::SeqCst); mouse(MOUSEEVENTF_XDOWN, 0, 0); };
    let end = || { released.store(qpc(), Ordering::SeqCst); mouse(MOUSEEVENTF_XUP, 0, 0); wait(1500); since.store(0, Ordering::SeqCst); wait(300); };
    // Each hold goes on to the next desktop: one that does not is a click, and opens the board.
    // 1: across to the right desktop, slowly, and let go there: the recede, timed.
    begin(); wait(80); stroke(330, 4, 700); wait(400); end();
    // 2: back to the middle one: it comes forward to the whole screen, timed.
    begin(); wait(80); stroke(-330, 4, 700); wait(400); end();
    // 3: down to the row below.
    begin(); wait(80); stroke(4, 330, 700); wait(400); end();
    stop.store(true, Ordering::SeqCst);
    recorder.join().unwrap();
    let _ = child.kill(); let _ = child.wait();
    for window in &mut windows { let _ = window.kill(); let _ = window.wait(); }
}

/// Where a window of pure magenta and one of pure cyan are in a frame:
/// their boxes (left, top, right, bottom), on every 2nd pixel.
fn pin_boxes(raw: &[u8], width: u32, height: u32) -> [Option<(u32, u32, u32, u32)>; 2] {
    let mut boxes = [None; 2];
    for y in (0..height).step_by(2) { for x in (0..width).step_by(2) {
        let i = ((y * width + x) * 4) as usize;
        let (r, g, b) = (raw[i + 2] as f64, raw[i + 1] as f64, raw[i] as f64);
        let which = if r > 90.0 && g < 0.25 * r && (r - b).abs() < 0.2 * r { 0 } else if g > 90.0 && r < 0.25 * g && (g - b).abs() < 0.2 * g { 1 } else { continue };
        let at = boxes[which].get_or_insert((x, y, x, y));
        *at = (at.0.min(x), at.1.min(y), at.2.max(x), at.3.max(y));
    }}
    boxes
}

/// Of a frame: how many pixels of the band at the top middle, where the
/// shelf's name is, are near white (its letters).
fn banner_white(raw: &[u8], width: u32, height: u32) -> u32 {
    let (x0, x1, y1) = (width / 2 - width / 7, width / 2 + width / 7, (height / 18).max(40));
    let mut white = 0;
    for y in 4..y1 { for x in x0..x1 {
        let i = ((y * width + x) * 4) as usize;
        if raw[i] > 200 && raw[i + 1] > 200 && raw[i + 2] > 200 { white += 1; }
    }}
    white
}

/// Pinned windows put by as the screen recedes, as deep as the hand is
/// unsure: a window pinned to every shelf (magenta) and one to its shelf
/// (cyan), a fast, a middling and a slow move. `dock.csv`: each frame's
/// boxes; `report.txt`: per hold the least scale (trace) and the boxes then.
pub unsafe fn dock(out: &Path, flick: &str) {
    use std::sync::{atomic::{AtomicBool, AtomicI64, Ordering}, Arc};
    let wait = |ms| std::thread::sleep(Duration::from_millis(ms));
    let cfg = std::path::PathBuf::from(std::env::var_os("APPDATA").unwrap()).join("Flick");
    std::fs::write(cfg.join("config.toml"), "config_version = 3\ntrigger = \"xbutton2\"\nminimap = false\nhotkeys = false\n[mouse]\nstep_x = 220\nstep_y = 220\nnotch_x = 40\nnotch_y = 100\nfollow_x = true\nfollow_y = true\nreverse = false\n[experimental]\nquick_flick = \"slide\"\n").unwrap();
    let made = std::fs::read_to_string(out.join("made.txt")).unwrap();
    let ids: Vec<_> = made.lines().collect();
    let all = super::vdapi::get_desktops().unwrap();
    let desk = |i: usize| *all.iter().find(|d| d.id() == ids[i]).unwrap();
    let other: Vec<_> = all.iter().map(|d| d.id()).filter(|id| !ids.contains(&id.as_str())).collect();
    std::fs::write(cfg.join("state.json"), serde_json::json!({"rows":[{"name":"Dock","cells":&ids[0..3],"last":ids[1]},{"name":"Below","cells":&ids[3..6],"last":ids[4]},{"name":"Existing","cells":other}],"follow_apps":["dock-row.exe"],"guide_seen":true}).to_string()).unwrap();
    super::vdapi::switch_desktop(desk(1)).unwrap(); wait(800);
    let me = std::env::current_exe().unwrap();
    let row_exe = out.join("dock-row.exe");
    std::fs::copy(&me, &row_exe).unwrap();
    let mut windows = Vec::new();
    for (exe, rgb, title, rect) in [(&me, "255 0 255", "F dock all", "700 160 640 400"), (&row_exe, "0 255 255", "F dock row", "760 600 560 340")] {
        let mut args = vec!["hold".to_string()];
        args.extend(rgb.split(' ').map(str::to_owned));
        args.push(title.into());
        args.extend(rect.split(' ').map(str::to_owned));
        args.push("300".into());
        windows.push(Command::new(exe).args(&args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap());
        wait(600);
        let hwnd = FindWindowW(w!("flick.test.hold"), &windows::core::HSTRING::from(title)); assert!(hwnd.0 != 0, "{title}");
        if title.ends_with("all") { super::vdapi::pin_window(hwnd).unwrap(); } else { super::vdapi::move_window_to_desktop(desk(1), hwnd).unwrap(); }
    }
    let stop = Arc::new(AtomicBool::new(false));
    let since = Arc::new(AtomicI64::new(0));
    let (o, s1, s2) = (out.to_owned(), stop.clone(), since.clone());
    let recorder = std::thread::spawn(move || {
        let capture = Capture::new().unwrap();
        let mut frequency = 0; QueryPerformanceFrequency(&mut frequency).unwrap();
        let mut csv = String::from("hold,ms,banner_white,all_box,row_box\n");
        let (mut hold, mut nth, mut was) = (0, 0, 0i64);
        let mut real = None;
        while !s1.load(Ordering::SeqCst) {
            if let Some((present, raw)) = capture.grab().unwrap() {
                let from = s2.load(Ordering::SeqCst);
                if from != was { was = from; if from != 0 { hold += 1; nth = 0; } }
                let boxes = pin_boxes(&raw, capture.width, capture.height);
                if from == 0 && real.is_none() && boxes.iter().all(Option::is_some) {
                    real = Some(boxes);
                    save_bmp(&o.join("real.bmp"), &raw, capture.width, capture.height);
                    writeln!(csv, "0,0,{},{:?},{:?}", banner_white(&raw, capture.width, capture.height), boxes[0], boxes[1]).unwrap();
                }
                if from != 0 {
                    let ms = (present - from) as f64 * 1000.0 / frequency as f64;
                    writeln!(csv, "{hold},{ms:.1},{},{:?},{:?}", banner_white(&raw, capture.width, capture.height), boxes[0], boxes[1]).unwrap();
                    if nth % 6 == 0 { save_bmp(&o.join(format!("d{hold}-{nth:03}-{ms:.0}ms.bmp")), &raw, capture.width, capture.height); }
                    nth += 1;
                }
            }
        }
        std::fs::write(o.join("dock.csv"), csv).unwrap();
    });
    wait(1500);
    let trace = out.join("trace.csv");
    let _ = std::fs::remove_file(&trace);
    let mut child = Command::new(flick).arg("--portable").env("FLICK_TRACE", &trace)
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
    wait(3000);
    assert!(FindWindowW(w!("flick.main"), None).0 != 0, "Flick did not start");
    let mouse = |flags, dx, dy| {
        let input = INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, dy, mouseData: 2, dwFlags: flags, ..Default::default() } } };
        assert_eq!(SendInput(&[input], std::mem::size_of::<INPUT>() as i32), 1);
    };
    let stroke = |dx: i32, ms: u64| {
        let n = (ms / 8).max(1) as i32;
        for i in 1..=n { mouse(MOUSEEVENTF_MOVE, dx * i / n - dx * (i - 1) / n, 0); wait(ms / n as u64); }
    };
    // 1.5 cells (220 px each): fast (15 a second), middling (2), slow (1, then held).
    let mut report = String::new();
    for (label, dx, ms, rest) in [("fast", 330, 100, 40), ("middling", -330, 750, 30), ("slow", 330, 1500, 400)] {
        since.store(qpc(), Ordering::SeqCst);
        mouse(MOUSEEVENTF_XDOWN, 0, 0); wait(80);
        stroke(dx, ms); wait(rest);
        mouse(MOUSEEVENTF_XUP, 0, 0); wait(1800);
        since.store(0, Ordering::SeqCst); wait(300);
        report += &format!("{label}: on {}\n", super::vdapi::get_current_desktop().unwrap().id());
    }
    stop.store(true, Ordering::SeqCst);
    recorder.join().unwrap();
    let _ = child.kill(); let _ = child.wait();
    for window in &mut windows { let _ = window.kill(); let _ = window.wait(); }
    // Per hold, the least scale and the mean scale while following.
    let text = std::fs::read_to_string(&trace).unwrap_or_default();
    let mut holds: Vec<(String, f64, f64, usize)> = Vec::new();
    for line in text.lines().skip(1) {
        let cols: Vec<_> = line.split(',').collect();
        let (Some(hold), Some(phase), Some(scale)) = (cols.first(), cols.get(6), cols.get(11).and_then(|s| s.parse::<f64>().ok())) else { continue };
        if *phase != "following" { continue; }
        match holds.iter_mut().find(|(h, ..)| h == hold) {
            Some((_, least, sum, n)) => { *least = least.min(scale); *sum += scale; *n += 1; }
            None => holds.push((hold.to_string(), scale, scale, 1)),
        }
    }
    for (hold, least, sum, n) in &holds {
        report += &format!("hold {hold}: least scale {least:.3} (depth {:.2}), mean {:.3} over {n} frames\n", 2.0 * (1.0 - least), sum / *n as f64);
    }
    std::fs::write(out.join("report.txt"), &report).unwrap();
    print!("{report}");
}

/// Of a frame, what tells the size and place of the picture: the first and
/// last white grid line (`hold` "grid") across the middle line and down the
/// middle column, how many there are across, and the box of the small green window.
fn settle_marks(raw: &[u8], width: u32, height: u32) -> [i64; 9] {
    let px = |x: u32, y: u32| { let i = ((y * width + x) * 4) as usize; (raw[i + 2], raw[i + 1], raw[i]) };
    let white = |(r, g, b): (u8, u8, u8)| r > 200 && g > 200 && b > 200;
    let (mut left, mut right, mut lines, mut was) = (-1i64, -1i64, 0, false);
    for x in 0..width { let w = white(px(x, height / 2)); if w { if left < 0 { left = x as i64; } right = x as i64; if !was { lines += 1; } } was = w; }
    let (mut top, mut bottom) = (-1i64, -1i64);
    for y in 0..height { if white(px(width / 2, y)) { if top < 0 { top = y as i64; } bottom = y as i64; } }
    let mut green = [i64::MAX, i64::MAX, -1, -1];
    for y in 0..height { for x in 0..width {
        let (r, g, b) = px(x, y);
        if g > 150 && r < 60 && b < 60 { green = [green[0].min(x as i64), green[1].min(y as i64), green[2].max(x as i64), green[3].max(y as i64)]; }
    }}
    if green[2] < 0 { green = [-1; 4]; }
    [left, right, lines, top, bottom, green[0], green[1], green[2], green[3]]
}

/// How unlike `real` a frame is: the mean difference of every 2nd pixel's
/// channels, and the share of them more than 32 off in any.
fn unlike(raw: &[u8], real: &[u8], width: u32, height: u32) -> (f64, f64) {
    let (mut sum, mut off, mut all) = (0u64, 0u64, 0u64);
    for y in (0..height).step_by(2) { for x in (0..width).step_by(2) {
        let i = ((y * width + x) * 4) as usize;
        let d = (0..3).map(|c| raw[i + c].abs_diff(real[i + c])).collect::<Vec<_>>();
        sum += d.iter().map(|&d| d as u64).sum::<u64>();
        if d.iter().any(|&d| d > 32) { off += 1; }
        all += 1;
    }}
    (sum as f64 / (all * 3) as f64, off as f64 / all as f64)
}

/// The normal view coming forward to the real desktop on letting go, over
/// maximized windows with a grid on them and a small green one: a fast, a
/// slow (receded, then forward) and a middling move. `settle.csv`: each
/// composed frame after letting go, its grid's extent and the green box
/// (`settle_marks`), and how unlike the real desktop it lands on it is,
/// taken before Flick ran (`unlike`); the last frames of each hold as BMPs.
pub unsafe fn settle(out: &Path, flick: &str) {
    use std::sync::{atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering}, Arc, Mutex};
    let wait = |ms| std::thread::sleep(Duration::from_millis(ms));
    let cfg = std::path::PathBuf::from(std::env::var_os("APPDATA").unwrap()).join("Flick");
    std::fs::write(cfg.join("config.toml"), "config_version = 3\ntrigger = \"xbutton2\"\nminimap = false\nhotkeys = false\n[mouse]\nstep_x = 220\nstep_y = 220\nnotch_x = 40\nnotch_y = 100\nfollow_x = true\nfollow_y = true\nreverse = false\n[experimental]\nquick_flick = \"slide\"\n").unwrap();
    let made = std::fs::read_to_string(out.join("made.txt")).unwrap();
    let ids: Vec<_> = made.lines().collect();
    let all = super::vdapi::get_desktops().unwrap();
    let desk = |i: usize| *all.iter().find(|d| d.id() == ids[i]).unwrap();
    let other: Vec<_> = all.iter().map(|d| d.id()).filter(|id| !ids.contains(&id.as_str())).collect();
    std::fs::write(cfg.join("state.json"), serde_json::json!({"rows":[{"name":"Settle","cells":&ids[0..3],"last":ids[1]},{"name":"Below","cells":&ids[3..6],"last":ids[4]},{"name":"Existing","cells":other}],"guide_seen":true}).to_string()).unwrap();
    super::vdapi::switch_desktop(desk(1)).unwrap(); wait(800);
    let me = std::env::current_exe().unwrap();
    let mut windows = Vec::new();
    for (i, rgb, title, rect) in [(0, "90 0 90", "F grid 0", "0 0 0 0"), (1, "0 70 90", "F grid 1", "0 0 0 0"), (2, "90 80 0", "F grid 2", "0 0 0 0"),
                                  (1, "0 200 0", "F green 1", "201 151 300 200"), (2, "0 200 0", "F green 2", "1301 707 333 177")] {
        let mut args = vec!["hold".to_string()];
        args.extend(rgb.split(' ').map(str::to_owned));
        args.push(title.into());
        args.extend(rect.split(' ').map(str::to_owned));
        args.push("600".into());
        windows.push(Command::new(&me).args(&args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap());
        wait(500);
        let hwnd = FindWindowW(w!("flick.test.hold"), &windows::core::HSTRING::from(title)); assert!(hwnd.0 != 0, "{title}");
        super::vdapi::move_window_to_desktop(desk(i), hwnd).unwrap();
    }
    let stop = Arc::new(AtomicBool::new(false));
    let released = Arc::new(AtomicI64::new(0));
    // The desktop being landed on, and a real one to keep as it is.
    let landing = Arc::new(AtomicUsize::new(usize::MAX));
    let keep: Arc<Mutex<Option<usize>>> = Arc::default();
    let (o, s1, s2, s3, s4) = (out.to_owned(), stop.clone(), released.clone(), landing.clone(), keep.clone());
    let recorder = std::thread::spawn(move || {
        let capture = Capture::new().unwrap();
        let (width, height) = (capture.width, capture.height);
        let mut frequency = 0; QueryPerformanceFrequency(&mut frequency).unwrap();
        let mut csv = String::from("hold,after_release_ms,grid_left,grid_right,grid_lines,grid_top,grid_bottom,green_l,green_t,green_r,green_b,mean_diff,share_off\n");
        let mut reals: Vec<Option<Vec<u8>>> = vec![None; 3];
        let (mut latest, mut hold, mut was, mut ring): (Vec<u8>, usize, i64, Vec<(f64, Vec<u8>)>) = (Vec::new(), 0, 0, Vec::new());
        let save = |hold: usize, ring: &mut Vec<(f64, Vec<u8>)>| {
            let n = ring.len();
            for (k, (ms, raw)) in ring.drain(..).enumerate() { save_bmp(&o.join(format!("s{hold}-last{}-{ms:.0}ms.bmp", n - 1 - k)), &raw, width, height); }
        };
        while !s1.load(Ordering::SeqCst) {
            if let Some((present, raw)) = capture.grab().unwrap() {
                let let_go = s2.load(Ordering::SeqCst);
                if let_go != was { if was != 0 { save(hold, &mut ring); } was = let_go; if let_go != 0 { hold += 1; } }
                let on = s3.load(Ordering::SeqCst);
                if let_go != 0 {
                    let ms = (present - let_go) as f64 * 1000.0 / frequency as f64;
                    let m = settle_marks(&raw, width, height);
                    let (mean, off) = reals.get(on).and_then(Option::as_ref).map_or((f64::NAN, f64::NAN), |real| unlike(&raw, real, width, height));
                    writeln!(csv, "{hold},{ms:.1},{},{},{},{},{},{},{},{},{},{mean:.3},{off:.5}", m[0], m[1], m[2], m[3], m[4], m[5], m[6], m[7], m[8]).unwrap();
                    ring.push((ms, raw.clone()));
                    if ring.len() > 4 { ring.remove(0); }
                }
                latest = raw;
            }
            if let Some(i) = s4.lock().unwrap().take() {
                if !latest.is_empty() {
                    let m = settle_marks(&latest, width, height);
                    writeln!(csv, "real-{i},0,{},{},{},{},{},{},{},{},{},0,0", m[0], m[1], m[2], m[3], m[4], m[5], m[6], m[7], m[8]).unwrap();
                    save_bmp(&o.join(format!("real-{i}.bmp")), &latest, width, height);
                    reals[i] = Some(latest.clone());
                }
            }
        }
        if was != 0 { save(hold, &mut ring); }
        std::fs::write(o.join("settle.csv"), csv).unwrap();
    });
    wait(600);
    for i in [0, 2, 1] {
        super::vdapi::switch_desktop(desk(i)).unwrap(); wait(1200);
        *keep.lock().unwrap() = Some(i); wait(150);
    }
    let trace = out.join("trace.csv");
    let _ = std::fs::remove_file(&trace);
    let mut child = Command::new(flick).arg("--portable").env("FLICK_GLIDE_STATS", "1").env("FLICK_TRACE", &trace)
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
    wait(3000);
    assert!(FindWindowW(w!("flick.main"), None).0 != 0, "Flick did not start");
    let mouse = |flags, dx, dy| {
        let input = INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, dy, mouseData: 2, dwFlags: flags, ..Default::default() } } };
        assert_eq!(SendInput(&[input], std::mem::size_of::<INPUT>() as i32), 1);
    };
    let stroke = |dx: i32, ms: u64| {
        let n = (ms / 8).max(1) as i32;
        for i in 1..=n { mouse(MOUSEEVENTF_MOVE, dx * i / n - dx * (i - 1) / n, 0); wait(ms / n as u64); }
    };
    let mut report = String::new();
    for (label, dx, ms, rest, to) in [("fast", 330, 100, 40, 2), ("slow", -330, 1500, 400, 1), ("middling", 330, 750, 30, 2), ("fast-back", -330, 100, 40, 1)] {
        landing.store(to, Ordering::SeqCst);
        mouse(MOUSEEVENTF_XDOWN, 0, 0); wait(80);
        stroke(dx, ms); wait(rest);
        released.store(qpc(), Ordering::SeqCst);
        mouse(MOUSEEVENTF_XUP, 0, 0); wait(1800);
        released.store(0, Ordering::SeqCst); wait(400);
        report += &format!("{label}: landed on the expected desktop: {}\n", super::vdapi::get_current_desktop().unwrap().id() == ids[to]);
    }
    stop.store(true, Ordering::SeqCst);
    recorder.join().unwrap();
    let _ = child.kill(); let _ = child.wait();
    for window in &mut windows { let _ = window.kill(); let _ = window.wait(); }
    std::fs::write(out.join("report.txt"), &report).unwrap();
    print!("{report}");
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn patch_tracking_ignores_isolated_desktop_colors() {
        let mut row=vec![0u8;400*4];
        for x in (10..12).chain(100..300).chain(380..382) {row[x*4]=255;row[x*4+2]=255;}
        let mut boxes=[(i32::MAX,i32::MAX,-1,-1,0);4];let(mut mass,mut moment)=([0;4],[0;4]);
        scan_line(&row,200,&mut boxes,&mut mass,&mut moment);
        assert_eq!((boxes[0].0,boxes[0].2,boxes[0].4),(100,299,200));
        assert_eq!(moment[0] as f64/mass[0] as f64,199.5);
        let sky=[255,140,0,255].repeat(400);
        let mut boxes=[(i32::MAX,i32::MAX,-1,-1,0);4];
        scan_line(&sky,200,&mut boxes,&mut mass,&mut moment);assert_eq!(boxes[1].4,0);
    }
    #[test] fn trajectory_is_continuous_and_has_a_real_pause() {
        for t in [1.2,1.6,2.4,3.8,4.2,5.3] { assert!((hand("mixed",t-1e-5)-hand("mixed",t+1e-5)).abs()<1e-6); }
        assert_eq!(hand("mixed",1.3),hand("mixed",1.5));
        assert!((hand("slow",6.0)-0.48).abs()<1e-9);
    }
}
