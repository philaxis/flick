//! One commit for live window visuals and the board's painted surface.
//! Shared thumbnails use dwmapi ordinal 147; keep the public path as fallback.
use std::{cell::RefCell, collections::HashMap, ffi::c_void};
use windows::{core::{w, ComInterface, Interface, PCSTR, HRESULT, Result, Error}, Foundation::Numerics::Matrix3x2, Win32::{
    Foundation::{HWND, HMODULE, POINT, RECT, E_FAIL},
    Graphics::{Direct3D::D3D_DRIVER_TYPE_HARDWARE, Direct3D11::*, DirectComposition::*, Direct2D::{*, Common::*}, Dwm::*, Dxgi::{*, Common::*}},
    System::LibraryLoader::{GetModuleHandleW, GetProcAddress},
}};

type SharedThumbnail = unsafe extern "system" fn(HWND, HWND, u32, *const DWM_THUMBNAIL_PROPERTIES, *mut c_void, *mut *mut c_void, *mut isize) -> HRESULT;
struct Thumbnail {
    visual: IDCompositionVisual2,
    opacity: IDCompositionEffectGroup,
    host: isize,
}
struct Scene {
    device: IDCompositionDesktopDevice,
    _target: IDCompositionTarget,
    root: IDCompositionVisual2,
    surface: Option<(IDCompositionSurface, (i32,i32))>,
}
#[derive(Default)]
struct Scenes { hosts: HashMap<isize,Scene>, thumbs: HashMap<isize,Thumbnail> }
thread_local! { static SCENES: RefCell<Scenes> = RefCell::default(); }

pub fn enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("FLICK_COMPOSITION").is_some()
        && crate::vdapi::windows_build().is_some_and(|(build,_)| matches!(build,22621|22631|26100|26200)))
}

impl Scene {
    unsafe fn new(hwnd: HWND) -> Result<Self> {
        let mut gpu=None;
        D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, HMODULE(0), D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION, Some(&mut gpu), None, None)?;
        let dxgi=gpu.unwrap().cast::<IDXGIDevice>()?;
        let factory: ID2D1Factory1=D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED,None)?;
        let d2d=factory.CreateDevice(&dxgi)?;
        let device: IDCompositionDesktopDevice=DCompositionCreateDevice3(&d2d)?;
        let target=device.CreateTargetForHwnd(hwnd,true)?;
        let root=device.CreateVisual()?;
        target.SetRoot(&root)?;
        Ok(Self {device,_target:target,root,surface:None})
    }
}

pub unsafe fn register(destination: HWND, source: HWND) -> Result<isize> {
    if !enabled() {return DwmRegisterThumbnail(destination,source);}
    let legacy=DwmRegisterThumbnail(destination,source)?;
    let size=match DwmQueryThumbnailSourceSize(legacy) {Ok(size)=>size,Err(e)=>{let _=DwmUnregisterThumbnail(legacy);return Err(e)}};
    let result=SCENES.with(|scenes| -> Result<isize> {
        let mut scenes=scenes.borrow_mut();
        if !scenes.hosts.contains_key(&destination.0) {scenes.hosts.insert(destination.0,Scene::new(destination)?);}
        let scene=&scenes.hosts[&destination.0];
        let library=GetModuleHandleW(w!("dwmapi.dll"))?;
        let proc=GetProcAddress(library,PCSTR(147usize as *const u8)).ok_or_else(||Error::from(E_FAIL))?;
        let create: SharedThumbnail=std::mem::transmute(proc);
        let props=DWM_THUMBNAIL_PROPERTIES {dwFlags:DWM_TNP_RECTDESTINATION|DWM_TNP_RECTSOURCE|DWM_TNP_VISIBLE|DWM_TNP_OPACITY|DWM_TNP_SOURCECLIENTAREAONLY|0x04000000,
            rcDestination:RECT {left:0,top:0,right:size.cx,bottom:size.cy},rcSource:RECT {left:0,top:0,right:size.cx,bottom:size.cy},opacity:255,fVisible:true.into(),fSourceClientAreaOnly:false.into()};
        let(mut raw,mut id)=(std::ptr::null_mut(),0);
        create(destination,source,2,&props,scene.device.as_raw(),&mut raw,&mut id).ok()?;
        if raw.is_null() {return Err(Error::from(E_FAIL));}
        let visual=IDCompositionVisual2::from_raw(raw);
        let opacity=scene.device.CreateEffectGroup()?;
        opacity.SetOpacity2(0.0)?;visual.SetEffect(&opacity)?;
        visual.SetBitmapInterpolationMode(DCOMPOSITION_BITMAP_INTERPOLATION_MODE_LINEAR)?;
        scene.root.AddVisual(&visual,true,None)?;
        scenes.thumbs.insert(id,Thumbnail {visual,opacity,host:destination.0});
        Ok(id)
    });
    match result {
        Ok(id)=>{DwmUnregisterThumbnail(legacy)?;Ok(id)}
        Err(e)=>{crate::app::log(&format!("composition thumbnail fallback: {e}"));Ok(legacy)}
    }
}

pub unsafe fn unregister(id: isize) -> Result<()> {
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
    let updated=SCENES.with(|scenes| -> Result<bool> {
        let scenes=scenes.borrow();
        let Some(thumb)=scenes.thumbs.get(&id) else {return Ok(false)};
        let source=if props.dwFlags&DWM_TNP_RECTSOURCE!=0 {props.rcSource} else {let s=DwmQueryThumbnailSourceSize(id)?;RECT {left:0,top:0,right:s.cx,bottom:s.cy}};
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
    SCENES.with(|scenes| {let scenes=scenes.borrow();match scenes.hosts.get(&hwnd.0) {Some(scene)=>unsafe {scene.device.Commit()},None=>Ok(())}})
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
            let surface=scene.device.CreateSurface(size.0 as u32,size.1 as u32,DXGI_FORMAT_B8G8R8A8_UNORM,DXGI_ALPHA_MODE_PREMULTIPLIED)?;
            scene.root.SetContent(&surface)?;
            scene.surface=Some((surface,size));
        }
        let surface=scene.surface.as_ref().unwrap().0.clone();
        let mut offset=POINT::default();
        let context: ID2D1DeviceContext=surface.BeginDraw(None,&mut offset)?;
        match context.cast() {Ok(target)=>Ok((surface,target,offset)),Err(e)=>{let _=surface.EndDraw();Err(e)}}
    })
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
