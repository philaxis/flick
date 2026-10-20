//! One commit for live window visuals and the board's painted surface.
//! Shared thumbnails use dwmapi ordinal 147; keep the public path as fallback.
use std::{cell::RefCell, collections::HashMap, ffi::c_void};
use windows::{core::{w, ComInterface, Interface, PCSTR, HRESULT, Result, Error}, Foundation::Numerics::Matrix3x2, Win32::{
    Foundation::{HWND, HMODULE, POINT, RECT, SIZE, E_FAIL},
    Graphics::{Direct3D::D3D_DRIVER_TYPE_HARDWARE, Direct3D11::*, DirectComposition::*, Direct2D::{*, Common::*}, Dwm::*, Dxgi::{*, Common::*}},
    System::LibraryLoader::{GetModuleHandleW, GetProcAddress}, UI::WindowsAndMessaging::GetWindowRect,
}};

type SharedThumbnail = unsafe extern "system" fn(HWND, HWND, u32, *const DWM_THUMBNAIL_PROPERTIES, *mut c_void, *mut *mut c_void, *mut isize) -> HRESULT;
struct Thumbnail {
    visual: IDCompositionVisual2,
    opacity: IDCompositionEffectGroup,
    host: isize,
    size: SIZE,
}
struct Scene {
    device: IDCompositionDesktopDevice,
    _target: IDCompositionTarget,
    root: IDCompositionVisual2,
    surface: Option<(IDCompositionSurface, (i32,i32))>,
}
#[derive(Default)]
struct Scenes { hosts: HashMap<isize,Scene>, thumbs: HashMap<isize,Thumbnail>,
    /// Each source's thumbnail size, kept while its window keeps the size it had (`GetWindowRect`).
    sizes: HashMap<isize,(SIZE,SIZE)> }
thread_local! { static SCENES: RefCell<Scenes> = RefCell::default(); }
// One device per thread outlives its hosts: the normal view's windows go with each hold, and a
// new D3D/D2D/DComp device costs about two refreshes before the first frame.
thread_local! { static DEVICE: RefCell<Option<IDCompositionDesktopDevice>> = RefCell::default(); }

static BLUR: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub fn configure_blur(on: bool) { BLUR.store(on, std::sync::atomic::Ordering::Relaxed); }
pub fn blur() -> bool { BLUR.load(std::sync::atomic::Ordering::Relaxed) }
struct Trails { ids: [isize;2], last: Option<([f32;4], std::time::Instant)>, shown: bool }
thread_local! { static TRAILS: RefCell<HashMap<isize, Trails>> = RefCell::default(); }
/// Composed thumbnails keep fractional edges their rounded state cannot compare; trails fade each frame.
pub fn needs_update(id: isize) -> bool {
    SCENES.with(|s|s.borrow().thumbs.contains_key(&id)) || TRAILS.with(|t|t.borrow().contains_key(&id))
}
/// A trail is still on screen: draw once more, even without movement, so it clears.
pub fn trails_shown() -> bool { TRAILS.with(|t|t.borrow().values().any(|t|t.shown)) }

// A half-refresh shutter, bounded so a late frame cannot leave a huge ghost.
// Slow drags and settling (under 4 px a frame) get none: a ghost there only blurs text.
fn trail(previous: [f32;4], current: [f32;4], elapsed: f32) -> (f32,f32) {
    if !(0.001..=0.05).contains(&elapsed) { return (0.0,0.0); }
    let dx=(current[0]+current[2]-previous[0]-previous[2])*0.25;
    let dy=(current[1]+current[3]-previous[1]-previous[3])*0.25;
    if dx.hypot(dy) < 2.0 { return (0.0,0.0); }
    let scale=(32.0/dx.hypot(dy).max(32.0)).min(1.0);
    (dx*scale,dy*scale)
}

/// `composed`: the host shows shared visuals (`follow_enabled`, `board_enabled`).
pub unsafe fn register(destination: HWND, source: HWND, composed: bool) -> Result<isize> {
    // shortcut: public DWM trails are discrete exposures, use captured textures for a true convolution.
    if !blur() || composed { return register_inner(destination,source,composed); }
    let id=register_inner(destination,source,false)?;
    // Later registrations overlay the opaque source, softening its contents as well as its edge.
    let a=match DwmRegisterThumbnail(destination,source) {Ok(a)=>a,Err(e)=>{let _=DwmUnregisterThumbnail(id);return Err(e)}};
    let b=match DwmRegisterThumbnail(destination,source) {Ok(b)=>b,Err(e)=>{let _=DwmUnregisterThumbnail(a);let _=DwmUnregisterThumbnail(id);return Err(e)}};
    TRAILS.with(|t|t.borrow_mut().insert(id,Trails{ids:[a,b],last:None,shown:false}));
    Ok(id)
}

/// 0: public thumbnails, 1: the normal view composed, 2: the board too. Fixed for the process.
static MODE: std::sync::OnceLock<u8> = std::sync::OnceLock::new();
/// The first config read decides; `FLICK_COMPOSITION` (0 off, otherwise all) overrides it.
pub fn configure(mode: crate::config::Compositor) { let _ = MODE.set(resolve(mode)); }
fn resolve(mode: crate::config::Compositor) -> u8 {
    use crate::config::Compositor;
    let mode = match std::env::var("FLICK_COMPOSITION").as_deref() { Ok("0")=>Compositor::Off, Ok(_)=>Compositor::All, Err(_)=>mode };
    // An ABI guard for ordinal 147, not compatibility certification.
    let supported = crate::vdapi::windows_build().is_some_and(|(build,_)| matches!(build,22621|22631|26100|26200));
    match mode { Compositor::Off=>0, _ if !supported=>0, Compositor::Auto=>1, Compositor::All=>2 }
}
fn mode() -> u8 { *MODE.get_or_init(|| resolve(crate::config::Compositor::Auto)) }
/// Public trails need public thumbnails, so blur keeps the normal view on them.
pub fn follow_enabled() -> bool { mode() >= 1 && !blur() }
pub fn board_enabled() -> bool { mode() == 2 }

unsafe fn device() -> Result<IDCompositionDesktopDevice> {
    if let Some(device)=DEVICE.with(|d|d.borrow().clone()) {return Ok(device);}
    let mut gpu=None;
    D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, HMODULE(0), D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION, Some(&mut gpu), None, None)?;
    let dxgi=gpu.unwrap().cast::<IDXGIDevice>()?;
    let factory: ID2D1Factory1=D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED,None)?;
    let d2d=factory.CreateDevice(&dxgi)?;
    let device: IDCompositionDesktopDevice=DCompositionCreateDevice3(&d2d)?;
    DEVICE.with(|d|*d.borrow_mut()=Some(device.clone()));
    Ok(device)
}
/// Creates this thread's device ahead of the first hold.
pub fn warm() { if follow_enabled() { if let Err(e)=unsafe {device()} {crate::app::log(&format!("composition device failed: {e}"));} } }

impl Scene {
    unsafe fn new(hwnd: HWND) -> Result<Self> {
        let device=device()?;
        let target=device.CreateTargetForHwnd(hwnd,true)?;
        let root=device.CreateVisual()?;
        target.SetRoot(&root)?;
        Ok(Self {device,_target:target,root,surface:None})
    }
}

unsafe fn register_inner(destination: HWND, source: HWND, composed: bool) -> Result<isize> {
    if !composed {return DwmRegisterThumbnail(destination,source);}
    // Each public handle costs DWM round trips before the first frame; ask only for a resized window.
    let mut frame=RECT::default();
    let window=GetWindowRect(source,&mut frame).ok().map(|_| SIZE {cx:frame.right-frame.left,cy:frame.bottom-frame.top});
    let known=SCENES.with(|s|s.borrow().sizes.get(&source.0).copied()).filter(|(w,_)| Some(*w)==window).map(|(_,size)| size);
    let size=match known {
        Some(size)=>size,
        None=>{
            let legacy=DwmRegisterThumbnail(destination,source)?;
            let size=DwmQueryThumbnailSourceSize(legacy);
            let _=DwmUnregisterThumbnail(legacy);
            let size=size?;
            if let Some(window)=window {SCENES.with(|s|s.borrow_mut().sizes.insert(source.0,(window,size)));}
            size
        }
    };
    let result=SCENES.with(|scenes| -> Result<isize> {
        let mut scenes=scenes.borrow_mut();
        if !scenes.hosts.contains_key(&destination.0) {scenes.hosts.insert(destination.0,Scene::new(destination)?);}
        let scene=&scenes.hosts[&destination.0];
        let library=GetModuleHandleW(w!("dwmapi.dll"))?;
        let proc=GetProcAddress(library,PCSTR(147usize as *const u8)).ok_or_else(||Error::from(E_FAIL))?;
        let create: SharedThumbnail=std::mem::transmute(proc);
        // Its picture begins at the window's corner, invisible borders and all: the whole of it, or
        // the frame's right and bottom edges are cut off by the borders' width.
        let whole=window.map_or(size,|w| SIZE {cx:w.cx.max(size.cx),cy:w.cy.max(size.cy)});
        let props=DWM_THUMBNAIL_PROPERTIES {dwFlags:DWM_TNP_RECTDESTINATION|DWM_TNP_RECTSOURCE|DWM_TNP_VISIBLE|DWM_TNP_OPACITY|DWM_TNP_SOURCECLIENTAREAONLY|0x04000000,
            rcDestination:RECT {left:0,top:0,right:whole.cx,bottom:whole.cy},rcSource:RECT {left:0,top:0,right:whole.cx,bottom:whole.cy},opacity:255,fVisible:true.into(),fSourceClientAreaOnly:false.into()};
        let(mut raw,mut id)=(std::ptr::null_mut(),0);
        create(destination,source,2,&props,scene.device.as_raw(),&mut raw,&mut id).ok()?;
        if raw.is_null() {return Err(Error::from(E_FAIL));}
        let visual=IDCompositionVisual2::from_raw(raw);
        let opacity=scene.device.CreateEffectGroup()?;
        opacity.SetOpacity2(0.0)?;visual.SetEffect(&opacity)?;
        visual.SetBitmapInterpolationMode(DCOMPOSITION_BITMAP_INTERPOLATION_MODE_LINEAR)?;
        // With a null sibling, FALSE inserts above all children (TRUE inserts below them).
        scene.root.AddVisual(&visual,false,None)?;
        scenes.thumbs.insert(id,Thumbnail {visual,opacity,host:destination.0,size});
        Ok(id)
    });
    match result {
        Ok(id)=>Ok(id),
        Err(e)=>{crate::app::log(&format!("composition thumbnail fallback: {e}"));DwmRegisterThumbnail(destination,source)}
    }
}

pub unsafe fn source_size(id: isize) -> Result<SIZE> {
    match SCENES.with(|s|s.borrow().thumbs.get(&id).map(|t|t.size)) {
        Some(size)=>Ok(size),None=>DwmQueryThumbnailSourceSize(id)
    }
}

pub unsafe fn unregister(id: isize) -> Result<()> {
    TRAILS.with(|t| {if let Some(trails)=t.borrow_mut().remove(&id) {for id in trails.ids {let _=DwmUnregisterThumbnail(id);}}});
    let removed=SCENES.with(|scenes| -> Result<bool> {
        let mut scenes=scenes.borrow_mut();
        if let Some(thumb)=scenes.thumbs.remove(&id) {
            if let Some(scene)=scenes.hosts.get(&thumb.host) {scene.root.RemoveVisual(&thumb.visual)?;}
            return Ok(true);
        }
        Ok(false)
    })?;
    if removed {Ok(())} else {DwmUnregisterThumbnail(id)}
}

/// Precise edges reach the compositor; public thumbnails keep their rounded RECT.
pub unsafe fn update(id: isize, props: &DWM_THUMBNAIL_PROPERTIES, dest: [f32;4]) -> Result<()> {
    TRAILS.with(|trails| -> Result<()> {
        let mut trails=trails.borrow_mut();
        if let Some(trails)=trails.get_mut(&id) {
            let now=std::time::Instant::now();
            let (dx,dy)=trails.last.map_or((0.0,0.0),|(last,at)|trail(last,dest,now.duration_since(at).as_secs_f32()));
            trails.last=Some((dest,now));
            trails.shown=false;
            for (i,id) in trails.ids.iter().enumerate() {
                let mut ghost=*props;
                let part=(i+1) as f32/3.0;
                let (x,y)=((-dx*part).round() as i32,(-dy*part).round() as i32);
                ghost.rcDestination.left+=x;ghost.rcDestination.right+=x;
                ghost.rcDestination.top+=y;ghost.rcDestination.bottom+=y;
                ghost.dwFlags|=DWM_TNP_OPACITY|DWM_TNP_VISIBLE;
                ghost.opacity=(props.opacity as f32*[0.12,0.06][i]*dx.hypot(dy).min(1.0)) as u8;
                ghost.fVisible=(props.fVisible.as_bool() && (x!=0 || y!=0) && blur()).into();
                trails.shown|=ghost.fVisible.as_bool();
                DwmUpdateThumbnailProperties(*id,&ghost)?;
            }
        }
        Ok(())
    })?;
    let updated=SCENES.with(|scenes| -> Result<bool> {
        let scenes=scenes.borrow();
        let Some(thumb)=scenes.thumbs.get(&id) else {return Ok(false)};
        let source=if props.dwFlags&DWM_TNP_RECTSOURCE!=0 {props.rcSource} else {let s=thumb.size;RECT {left:0,top:0,right:s.cx,bottom:s.cy}};
        let visible=props.fVisible.as_bool() && source.right>source.left && source.bottom>source.top && dest[2]>dest[0] && dest[3]>dest[1];
        thumb.opacity.SetOpacity2(if visible {props.opacity as f32/255.0} else {0.0})?;
        if visible {
            let matrix = transform(source, dest);
            thumb.visual.SetClip2(&D2D_RECT_F {left:source.left as f32,top:source.top as f32,right:source.right as f32,bottom:source.bottom as f32})?;
            thumb.visual.SetTransform2(&matrix)?;
        }
        Ok(true)
    })?;
    if updated {Ok(())} else {DwmUpdateThumbnailProperties(id,props)}
}

pub fn commit(hwnd: HWND) -> Result<()> {
    let done=SCENES.with(|scenes| {let scenes=scenes.borrow();match scenes.hosts.get(&hwnd.0) {Some(scene)=>unsafe {scene.device.Commit()},None=>Ok(())}});
    // A lost device is made anew for the next host.
    if done.is_err() {DEVICE.with(|d|*d.borrow_mut()=None);}
    done
}
pub fn forget(hwnd: HWND) {
    SCENES.with(|scenes| {let mut scenes=scenes.borrow_mut();scenes.thumbs.retain(|_,t|t.host!=hwnd.0);scenes.hosts.remove(&hwnd.0);});
}

/// Begin/EndDraw pairs ensure D2D pixels and thumbnail transforms share a commit.
pub unsafe fn begin_paint(hwnd: HWND, size: (i32,i32)) -> Result<(IDCompositionSurface, ID2D1RenderTarget, POINT)> {
    SCENES.with(|scenes| {
        let mut scenes=scenes.borrow_mut();
        if !scenes.hosts.contains_key(&hwnd.0) {scenes.hosts.insert(hwnd.0,Scene::new(hwnd)?);}
        let scene=scenes.hosts.get_mut(&hwnd.0).unwrap();
        if scene.surface.as_ref().map(|s|s.1)!=Some(size) {
            // Opaque like the public board's window target: nothing beneath needs blending.
            let surface=scene.device.CreateSurface(size.0 as u32,size.1 as u32,DXGI_FORMAT_B8G8R8A8_UNORM,DXGI_ALPHA_MODE_IGNORE)?;
            scene.root.SetContent(&surface)?;
            scene.surface=Some((surface,size));
        }
        let surface=scene.surface.as_ref().unwrap().0.clone();
        let mut offset=POINT::default();
        let context: ID2D1DeviceContext=surface.BeginDraw(None,&mut offset)?;
        match context.cast() {Ok(target)=>Ok((surface,target,offset)),Err(e)=>{let _=surface.EndDraw();Err(e)}}
    })
}

/// A host's surface black all over, beneath its thumbnails and in their commit.
pub unsafe fn back(hwnd: HWND, size: (i32,i32)) -> Result<()> {
    let (surface,target,offset)=begin_paint(hwnd,size)?;
    target.SetTransform(&Matrix3x2::identity());
    let (x,y)=(offset.x as f32,offset.y as f32);
    target.PushAxisAlignedClip(&D2D_RECT_F {left:x,top:y,right:x+size.0 as f32,bottom:y+size.1 as f32},D2D1_ANTIALIAS_MODE_ALIASED);
    target.Clear(Some(&D2D1_COLOR_F {r:0.0,g:0.0,b:0.0,a:1.0}));
    target.PopAxisAlignedClip();
    surface.EndDraw()
}

fn transform(source: RECT, dest: [f32;4]) -> Matrix3x2 {
    let sx=(dest[2]-dest[0])/(source.right-source.left) as f32;
    let sy=(dest[3]-dest[1])/(source.bottom-source.top) as f32;
    Matrix3x2 {M11:sx,M12:0.0,M21:0.0,M22:sy,M31:dest[0]-source.left as f32*sx,M32:dest[1]-source.top as f32*sy}
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn trails_follow_direction_but_disappear_after_stalls() {
        let a=[0.0,0.0,100.0,100.0];let b=[20.0,-10.0,120.0,90.0];
        assert_eq!(trail(a,b,1.0/60.0),(10.0,-5.0));
        assert_eq!(trail(b,a,1.0/144.0),(-10.0,5.0));
        assert_eq!(trail(a,b,0.1),(0.0,0.0));
        assert_eq!(trail(a,a,0.016),(0.0,0.0));
        let t=trail(a,[1000.0,0.0,1100.0,100.0],0.016);assert!(t.0.hypot(t.1)<=32.0);
        // A slow drag (3 px a frame) and a settled frame leave no trail.
        assert_eq!(trail(a,[3.0,0.0,103.0,100.0],0.016),(0.0,0.0));
    }
    #[test]
    fn cropped_sources_keep_fractional_destination_edges() {
        let source=RECT {left:17,top:31,right:817,bottom:631};
        let dest=[-12.25,4.125,387.75,304.125];
        let m=transform(source,dest);
        assert_eq!(source.left as f32*m.M11+m.M31,dest[0]);
        assert_eq!(source.right as f32*m.M11+m.M31,dest[2]);
        assert_eq!(source.top as f32*m.M22+m.M32,dest[1]);
        assert_eq!(source.bottom as f32*m.M22+m.M32,dest[3]);
    }
}
