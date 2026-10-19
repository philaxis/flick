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
                return Ok(Self { duplication, context: context.unwrap(), staging: staging.unwrap(), width, height });
            }
        }
    }
    unsafe fn frame(&self) -> Result<Option<(DXGI_OUTDUPL_FRAME_INFO, [(i32,i32,i32,i32,u32); 4], f64)>> {
        let start = Instant::now();
        let (mut info, mut resource) = (DXGI_OUTDUPL_FRAME_INFO::default(), None);
        match self.duplication.AcquireNextFrame(50, &mut info, &mut resource) {
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
        let mut boxes = [(i32::MAX, i32::MAX, -1, -1, 0); 4];
        for y in 0..self.height {
            let line = std::slice::from_raw_parts((mapped.pData as *const u8).add((y * mapped.RowPitch) as usize), self.width as usize * 4);
            for x in 0..self.width {
                let p = &line[x as usize * 4..][..3];
                // Chroma ratios also find shaded thumbnails; green is the D2D marker.
                let (b,g,r) = (p[0] as u32,p[1] as u32,p[2] as u32);
                let c = if r > 40 && b > 40 && g * 3 < r.min(b) { Some(0) }
                    else if b > 40 && g > 40 && r * 3 < b.min(g) { Some(1) }
                    else if r > 40 && g > 40 && b * 3 < r.min(g) { Some(2) }
                    else if y < 40 && g > 180 && r < 40 && b < 40 { Some(3) } else { None };
                if let Some(c) = c { let v = &mut boxes[c]; v.0=v.0.min(x as i32); v.1=v.1.min(y as i32); v.2=v.2.max(x as i32); v.3=v.3.max(y as i32); v.4+=1; }
            }
        }
        self.context.Unmap(&self.staging, 0);
        Ok(Some((info, boxes, start.elapsed().as_secs_f64()*1000.0)))
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

unsafe fn input(kind: &str, scenario: &str, out: &Path, duration: f64, frequency: i64) {
    let pad = FindWindowExW(HWND_MESSAGE, None, w!("flick.input"), None);
    assert!(kind != "ptp" || pad.0 != 0);
    let mouse = |flags, dx| {
        let input = INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, mouseData: 2, dwFlags: flags, ..Default::default() } } };
        assert_eq!(SendInput(&[input], std::mem::size_of::<INPUT>() as i32), 1);
    };
    let touch = |x: f64, count| {
        let x = (x * 1200.0).round() as i32;
        PostMessageW(pad, WM_APP+61, WPARAM(count), LPARAM(x as u32 as isize)).unwrap();
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
        windows.push(Command::new(&me).args(["hold",rgb[0],rgb[1],rgb[2],&title,"200","200","600","400","240"])
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap());
        wait(400);
        let hwnd=FindWindowW(w!("flick.test.hold"), &windows::core::HSTRING::from(title)); assert!(hwnd.0!=0);
        super::vdapi::move_window_to_desktop(*all.iter().find(|d|d.id()==ids[i]).unwrap(),hwnd).unwrap();
    }
    let mut frequency=0; QueryPerformanceFrequency(&mut frequency).unwrap();
    let mut timing=windows::Win32::Graphics::Dwm::DWM_TIMING_INFO {cbSize:std::mem::size_of::<windows::Win32::Graphics::Dwm::DWM_TIMING_INFO>() as u32,..Default::default()};
    windows::Win32::Graphics::Dwm::DwmGetCompositionTimingInfo(HWND(0),&mut timing).unwrap();
    let refresh_ticks = timing.qpcRefreshPeriod;
    std::fs::write(out.join("display-meta.json"),serde_json::json!({"frequency":frequency,"refresh_ticks":refresh_ticks,"baseline":"0078a16 + motion probe","capture":"desktop duplication, not panel scanout"}).to_string()).unwrap();
    for mode in ["follow","board"] { for kind in ["mouse","ptp"] { for scenario in ["slow","normal","fast","mixed"] {
        super::vdapi::switch_desktop(home).unwrap(); wait(500);
        let label=format!("{mode}-{kind}-{scenario}");
        let mut child=Command::new(flick).arg("--portable").env("FLICK_TRACE",out.join(format!("{label}-app.csv")))
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
        wait(1200);
        let app=FindWindowW(w!("flick.main"),None); assert!(app.0!=0);
        if mode=="board" {PostMessageW(app,WM_APP+2,WPARAM(0),LPARAM(0)).unwrap();wait(700);assert!(IsWindowVisible(FindWindowW(w!("flick.board"),None)).as_bool());}
        let capture=Capture::new().unwrap();
        // Drain setup before injecting; AccumulatedFrames remains visible in the CSV.
        let _=capture.frame().unwrap();
        let (k,s,path)=(kind.to_owned(),scenario.to_owned(),out.join(format!("{label}-input.csv")));
        let worker=std::thread::spawn(move||input(&k,&s,&path,6.0,frequency));
        let start=Instant::now();
        let mut csv=String::from("qpc,accumulated,capture_ms,magenta_l,magenta_t,magenta_r,magenta_b,magenta_n,cyan_l,cyan_t,cyan_r,cyan_b,cyan_n,yellow_l,yellow_t,yellow_r,yellow_b,yellow_n,marker_l,marker_t,marker_r,marker_b,marker_n\n");
        let(mut frames,mut missed)=(0,0);
        while start.elapsed()<Duration::from_millis(7200) {
            if let Some((info,boxes,ms))=capture.frame().unwrap() {
                frames+=1;missed+=info.AccumulatedFrames.saturating_sub(1);
                write!(csv,"{},{},{ms:.3}",info.LastPresentTime,info.AccumulatedFrames).unwrap();
                for b in boxes {write!(csv,",{},{},{},{},{}",b.0,b.1,b.2,b.3,b.4).unwrap();}csv.push('\n');
            }
        }
        worker.join().unwrap();
        std::fs::write(out.join(format!("{label}-display.csv")),csv).unwrap();
        if mode=="board" {PostMessageW(FindWindowW(w!("flick.board"),None),WM_KEYDOWN,WPARAM(0x1b),LPARAM(0)).unwrap();wait(300);}
        let _=child.kill();let _=child.wait();
        println!("{label}: captured={frames} accumulated_extra={missed}");
    }}}
    for window in &mut windows {let _=window.kill();let _=window.wait();}
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn trajectory_is_continuous_and_has_a_real_pause() {
        for t in [1.2,1.6,2.4,3.8,4.2,5.3] { assert!((hand("mixed",t-1e-5)-hand("mixed",t+1e-5)).abs()<1e-6); }
        assert_eq!(hand("mixed",1.3),hand("mixed",1.5));
        assert!((hand("slow",6.0)-0.48).abs()<1e-9);
    }
}
