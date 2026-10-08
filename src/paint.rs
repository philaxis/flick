//! Direct2D, DirectWrite and GDI plumbing shared by the minimap, the board and
//! the settings window. All draw into a memory bitmap, which is then handed to
//! their window.

use std::ffi::c_void;
use windows::{
    core::{w, ComInterface, Result},
    Win32::{
        Foundation::RECT,
        Graphics::{
            Direct2D::{
                Common::{
                    D2D1_ALPHA_MODE, D2D1_ALPHA_MODE_IGNORE, D2D1_COLOR_F, D2D1_FIGURE_BEGIN_FILLED,
                    D2D1_FIGURE_END_CLOSED, D2D1_PIXEL_FORMAT, D2D_POINT_2F, D2D_RECT_F, D2D_SIZE_F,
                },
                D2D1CreateFactory, ID2D1Bitmap, ID2D1DCRenderTarget, ID2D1Factory, ID2D1PathGeometry,
                ID2D1RenderTarget, ID2D1SolidColorBrush, D2D1_ARC_SEGMENT, D2D1_ARC_SIZE_LARGE, D2D1_ARC_SIZE_SMALL,
                D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, D2D1_DRAW_TEXT_OPTIONS_CLIP, D2D1_ELLIPSE,
                D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_FEATURE_LEVEL_DEFAULT, D2D1_RENDER_TARGET_PROPERTIES,
                D2D1_RENDER_TARGET_TYPE_SOFTWARE, D2D1_RENDER_TARGET_USAGE_GDI_COMPATIBLE, D2D1_ROUNDED_RECT,
                D2D1_SWEEP_DIRECTION_CLOCKWISE,
            },
            DirectWrite::{
                DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat, DWRITE_FACTORY_TYPE_SHARED,
                DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_WEIGHT, DWRITE_MEASURING_MODE_NATURAL, DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
                DWRITE_TEXT_ALIGNMENT, DWRITE_WORD_WRAPPING_NO_WRAP,
            },
            Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
            Gdi::{
                CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, SelectObject, BITMAPINFO,
                BITMAPINFOHEADER, DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ,
            },
        },
    },
};

pub type Rect = D2D_RECT_F;

pub fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect { left: x, top: y, right: x + w, bottom: y + h }
}

pub fn rounded(r: &Rect, radius: f32) -> D2D1_ROUNDED_RECT {
    D2D1_ROUNDED_RECT { rect: *r, radiusX: radius, radiusY: radius }
}

pub fn rgba(r: f32, g: f32, b: f32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r, g, b, a }
}

pub fn white(a: f32) -> D2D1_COLOR_F {
    rgba(1.0, 1.0, 1.0, a)
}

/// The colour of the current cell and of whatever is selected.
pub fn accent(a: f32) -> D2D1_COLOR_F {
    rgba(0.34, 0.62, 1.0, a)
}

/// The colour of a cell the user marked to stand out. Warm, so that it is
/// taken for neither the current cell nor the selection.
pub fn warm(a: f32) -> D2D1_COLOR_F {
    rgba(1.0, 0.66, 0.24, a)
}

/// The dark background of the full windows (the board, the settings).
pub fn backdrop() -> D2D1_COLOR_F {
    rgba(0.050, 0.055, 0.068, 1.0)
}

/// A top-down 32-bit BGRA bitmap, selected into a memory DC of its own.
pub struct Dib {
    pub dc: HDC,
    pub size: (i32, i32),
    bitmap: HBITMAP,
    /// What the DC held before, to be put back before the bitmap is deleted.
    stock: HGDIOBJ,
    bits: *mut c_void,
}

impl Dib {
    pub fn new(size: (i32, i32)) -> Result<Dib> {
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size.0,
                biHeight: -size.1,
                biPlanes: 1,
                biBitCount: 32,
                ..Default::default()
            },
            ..Default::default()
        };
        unsafe {
            let mut bits = std::ptr::null_mut();
            let bitmap = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
            let dc = CreateCompatibleDC(None);
            let stock = SelectObject(dc, bitmap);
            Ok(Dib { dc, size, bitmap, stock, bits })
        }
    }

    fn len(&self) -> usize {
        (self.size.0 * self.size.1 * 4) as usize
    }

    pub fn pixels(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.bits as *const u8, self.len()) }
    }

    pub fn pixels_mut(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.bits as *mut u8, self.len()) }
    }

    /// The pixels as Direct2D takes them, with the length of one row in bytes.
    pub fn raw(&self) -> (*const c_void, u32) {
        (self.bits, (self.size.0 * 4) as u32)
    }

    pub fn bounds(&self) -> RECT {
        RECT { left: 0, top: 0, right: self.size.0, bottom: self.size.1 }
    }
}

impl Drop for Dib {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.stock);
            DeleteObject(self.bitmap);
            DeleteDC(self.dc);
        }
    }
}

pub fn d2d_factory() -> Result<ID2D1Factory> {
    unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None) }
}

pub fn dwrite_factory() -> Result<IDWriteFactory> {
    unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED) }
}

/// A Direct2D target that draws into the bitmap it was last bound to. One
/// unit is one pixel; what is drawn is scaled by its own code.
pub struct DcTarget {
    dc: ID2D1DCRenderTarget,
    /// The same object as the plain render target the drawing calls take.
    pub target: ID2D1RenderTarget,
}

impl DcTarget {
    pub fn new(factory: &ID2D1Factory, alpha: D2D1_ALPHA_MODE) -> Result<DcTarget> {
        unsafe {
            // Drawn by the processor: a few rounded squares and lines of text
            // take it no time, while a target on the graphics card brings a
            // Direct3D device with it, some 15 MB and a dozen threads each.
            let dc = factory.CreateDCRenderTarget(&D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: alpha },
                dpiX: 96.0,
                dpiY: 96.0,
                usage: D2D1_RENDER_TARGET_USAGE_GDI_COMPATIBLE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            })?;
            let target = dc.cast()?;
            Ok(DcTarget { dc, target })
        }
    }

    pub fn bind(&self, dib: &Dib) -> Result<()> {
        unsafe { self.dc.BindDC(dib.dc, &dib.bounds()) }
    }
}

/// An opaque memory bitmap with a Direct2D target bound to it.
pub struct Canvas {
    pub dib: Dib,
    pub target: DcTarget,
}

impl Canvas {
    pub fn new(factory: &ID2D1Factory, size: (i32, i32)) -> Result<Canvas> {
        let target = DcTarget::new(factory, D2D1_ALPHA_MODE_IGNORE)?;
        let dib = Dib::new(size)?;
        target.bind(&dib)?;
        Ok(Canvas { dib, target })
    }
}

/// The point `radius` from `centre` at `degrees` clockwise from straight up.
pub fn on_circle(centre: (f32, f32), radius: f32, degrees: f32) -> (f32, f32) {
    let (sin, cos) = degrees.to_radians().sin_cos();
    (centre.0 + radius * sin, centre.1 - radius * cos)
}

/// A slice of a disc, from `from` to `to` degrees clockwise from straight up.
pub fn pie(factory: &ID2D1Factory, centre: (f32, f32), radius: f32, from: f32, to: f32) -> Result<ID2D1PathGeometry> {
    let point = |(x, y): (f32, f32)| D2D_POINT_2F { x, y };
    unsafe {
        let path = factory.CreatePathGeometry()?;
        let sink = path.Open()?;
        sink.BeginFigure(point(centre), D2D1_FIGURE_BEGIN_FILLED);
        sink.AddLine(point(on_circle(centre, radius, from)));
        sink.AddArc(&D2D1_ARC_SEGMENT {
            point: point(on_circle(centre, radius, to)),
            size: D2D_SIZE_F { width: radius, height: radius },
            rotationAngle: 0.0,
            sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
            arcSize: if to - from > 180.0 { D2D1_ARC_SIZE_LARGE } else { D2D1_ARC_SIZE_SMALL },
        });
        sink.EndFigure(D2D1_FIGURE_END_CLOSED);
        sink.Close()?;
        Ok(path)
    }
}

/// A single-line text format in the UI font, centred vertically in its box.
pub fn text_format(
    factory: &IDWriteFactory,
    size: f32,
    weight: i32,
    alignment: DWRITE_TEXT_ALIGNMENT,
) -> Result<IDWriteTextFormat> {
    unsafe {
        let format = factory.CreateTextFormat(
            w!("Segoe UI"),
            None,
            DWRITE_FONT_WEIGHT(weight),
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            size,
            w!("ko-kr"),
        )?;
        format.SetTextAlignment(alignment)?;
        format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
        format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
        Ok(format)
    }
}

/// A render target together with the one brush everything is drawn with.
/// Only valid between the target's `BeginDraw` and `EndDraw`.
pub struct Painter<'a> {
    pub target: &'a ID2D1RenderTarget,
    brush: ID2D1SolidColorBrush,
}

impl<'a> Painter<'a> {
    pub fn new(target: &'a ID2D1RenderTarget) -> Result<Painter<'a>> {
        let brush = unsafe { target.CreateSolidColorBrush(&white(1.0), None)? };
        Ok(Painter { target, brush })
    }

    /// Fills a rectangle with rounded corners.
    pub fn fill(&self, area: &Rect, radius: f32, colour: D2D1_COLOR_F) {
        unsafe {
            self.brush.SetColor(&colour);
            self.target.FillRoundedRectangle(&rounded(area, radius), &self.brush);
        }
    }

    /// Fills a rectangle with square corners.
    pub fn fill_square(&self, area: &Rect, colour: D2D1_COLOR_F) {
        unsafe {
            self.brush.SetColor(&colour);
            self.target.FillRectangle(area, &self.brush);
        }
    }

    /// Outlines a rectangle with rounded corners.
    pub fn stroke(&self, area: &Rect, radius: f32, width: f32, colour: D2D1_COLOR_F) {
        unsafe {
            self.brush.SetColor(&colour);
            self.target.DrawRoundedRectangle(&rounded(area, radius), &self.brush, width, None);
        }
    }

    /// Writes one line of text, cut off at the edges of `area`.
    pub fn text(&self, text: &str, format: &IDWriteTextFormat, area: &Rect, colour: D2D1_COLOR_F) {
        let wide: Vec<u16> = text.encode_utf16().collect();
        unsafe {
            self.brush.SetColor(&colour);
            self.target.DrawText(
                &wide,
                format,
                area,
                &self.brush,
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
    }

    pub fn fill_shape(&self, shape: &ID2D1PathGeometry, colour: D2D1_COLOR_F) {
        unsafe {
            self.brush.SetColor(&colour);
            self.target.FillGeometry(shape, &self.brush, None);
        }
    }

    pub fn disc(&self, centre: (f32, f32), radius: f32, colour: D2D1_COLOR_F) {
        let ellipse = D2D1_ELLIPSE { point: D2D_POINT_2F { x: centre.0, y: centre.1 }, radiusX: radius, radiusY: radius };
        unsafe {
            self.brush.SetColor(&colour);
            self.target.FillEllipse(&ellipse, &self.brush);
        }
    }

    pub fn line(&self, from: (f32, f32), to: (f32, f32), width: f32, colour: D2D1_COLOR_F) {
        let point = |(x, y): (f32, f32)| D2D_POINT_2F { x, y };
        unsafe {
            self.brush.SetColor(&colour);
            self.target.DrawLine(point(from), point(to), &self.brush, width, None);
        }
    }

    pub fn bitmap(&self, bitmap: &ID2D1Bitmap, area: &Rect, opacity: f32) {
        unsafe {
            self.target.DrawBitmap(bitmap, Some(area), opacity, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None);
        }
    }
}
