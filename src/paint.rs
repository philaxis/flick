//! Direct2D, DirectWrite and GDI plumbing shared by the minimap, the board and
//! the settings window. All draw into a memory bitmap, which is then handed to
//! their window.

use std::{cell::{Cell, OnceCell, RefCell}, collections::HashMap, ffi::c_void};
use windows::{
    core::{w, ComInterface, Interface, Result},
    Foundation::Numerics::Matrix3x2,
    Win32::{
        Foundation::RECT,
        Graphics::{
            Direct2D::{
                Common::{D2D1_FILL_MODE_ALTERNATE, D2D1_ALPHA_MODE, D2D1_ALPHA_MODE_IGNORE, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_POINT_2F, D2D_RECT_F, D2D_SIZE_U},
                D2D1CreateFactory, ID2D1Bitmap, ID2D1DCRenderTarget, ID2D1Factory, ID2D1LinearGradientBrush, ID2D1RenderTarget,
                ID2D1SolidColorBrush, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, D2D1_BITMAP_PROPERTIES, D2D1_DRAW_TEXT_OPTIONS_CLIP,
                D2D1_EXTEND_MODE_CLAMP, D2D1_GAMMA_2_2, D2D1_GRADIENT_STOP, D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES,
                D2D1_ELLIPSE, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_FEATURE_LEVEL_DEFAULT,
                D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_SOFTWARE,
                D2D1_RENDER_TARGET_USAGE_GDI_COMPATIBLE, D2D1_ROUNDED_RECT, ID2D1Geometry,
                D2D1_LAYER_PARAMETERS, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_LAYER_OPTIONS_NONE,
            },
            DirectWrite::{
                DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat, IDWriteTextLayout, DWRITE_FACTORY_TYPE_SHARED,
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

/// The colour of what is pinned, wherever it shows (the board, the light
/// about the pinned windows while the screen follows the hand, whole or
/// receded), and how strongly: Flick's blue, full to every shelf, fainter
/// to its own shelf. Changed here, changed everywhere.
pub const PIN_ALL: ([f32; 3], f32) = ([0.34, 0.62, 1.0], 1.0);
pub const PIN_ROW: ([f32; 3], f32) = ([0.34, 0.62, 1.0], 0.6);
/// How far the light about a pinned window reaches, of the farthest: to
/// every shelf, far, and most below it, as a lamp under it would; to its
/// own shelf, about half as far.
pub const PIN_ALL_REACH: f32 = 1.0;
pub const PIN_ROW_REACH: f32 = 0.55;

/// How strong and how far the light about a pinned window is (`all`
/// pinned to every shelf, or to its own).
pub fn pin_light(all: bool) -> (f32, f32) {
    if all { (PIN_ALL.1, PIN_ALL_REACH) } else { (PIN_ROW.1, PIN_ROW_REACH) }
}

/// `PIN_ALL` or `PIN_ROW` (`all`) at `a`, as strong as it is.
pub fn pinned(all: bool, a: f32) -> D2D1_COLOR_F {
    let ([r, g, b], strength) = if all { PIN_ALL } else { PIN_ROW };
    rgba(r, g, b, a * strength)
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

/// Board labels retain DirectWrite shaping while their transforms animate.
pub struct TextCache {
    factory: IDWriteFactory,
    count: Cell<usize>,
    layouts: RefCell<HashMap<(usize, u32, u32), HashMap<String, IDWriteTextLayout>>>,
}
impl TextCache {
    pub fn new() -> Result<Self> { Ok(Self { factory: dwrite_factory()?, count: Cell::new(0), layouts: RefCell::default() }) }
    pub fn clear(&self) { self.layouts.borrow_mut().clear(); self.count.set(0); }
    fn layout(&self, text: &str, format: &IDWriteTextFormat, width: f32, height: f32) -> Result<IDWriteTextLayout> {
        let key = (format.as_raw() as usize, width.to_bits(), height.to_bits());
        if self.count.get() >= 512 { self.clear(); }
        let mut layouts = self.layouts.borrow_mut();
        let labels = layouts.entry(key).or_default();
        if let Some(layout) = labels.get(text) { return Ok(layout.clone()); }
        let wide: Vec<u16> = text.encode_utf16().collect();
        let layout = unsafe { self.factory.CreateTextLayout(&wide, format, width, height)? };
        labels.insert(text.to_owned(), layout.clone());
        self.count.set(self.count.get() + 1);
        Ok(layout)
    }
}

/// A render target together with the one brush everything is drawn with.
/// Only valid between the target's `BeginDraw` and `EndDraw`.
pub struct Painter<'a> {
    pub target: &'a ID2D1RenderTarget,
    brush: ID2D1SolidColorBrush,
    text_cache: Option<&'a TextCache>,
    /// White fading to nothing, for the light falling on a surface from above.
    sheen: OnceCell<Option<ID2D1LinearGradientBrush>>,
}

impl<'a> Painter<'a> {
    pub fn new(target: &'a ID2D1RenderTarget) -> Result<Painter<'a>> {
        let brush = unsafe { target.CreateSolidColorBrush(&white(1.0), None)? };
        Ok(Painter { target, brush, text_cache: None, sheen: OnceCell::new() })
    }

    pub fn cached(target: &'a ID2D1RenderTarget, text_cache: &'a TextCache) -> Result<Self> {
        Ok(Self { text_cache: Some(text_cache), ..Self::new(target)? })
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
        if let Some(cache) = self.text_cache {
            if let Ok(layout) = cache.layout(text, format, area.right - area.left, area.bottom - area.top) {
                unsafe {
                    self.brush.SetColor(&colour);
                    self.target.DrawTextLayout(D2D_POINT_2F { x: area.left, y: area.top }, &layout, &self.brush, D2D1_DRAW_TEXT_OPTIONS_CLIP);
                }
                return;
            }
        }
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

    /// Light from above on a rounded surface: `top` of white at its top
    /// edge, nothing by its bottom.
    pub fn sheen(&self, area: &Rect, radius: f32, top: f32) {
        let brush = self.sheen.get_or_init(|| unsafe {
            let stops = [
                D2D1_GRADIENT_STOP { position: 0.0, color: white(1.0) },
                D2D1_GRADIENT_STOP { position: 1.0, color: white(0.0) },
            ];
            let stops = self.target.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP).ok()?;
            let line = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES { startPoint: D2D_POINT_2F { x: 0.0, y: 0.0 }, endPoint: D2D_POINT_2F { x: 0.0, y: 1.0 } };
            self.target.CreateLinearGradientBrush(&line, None, &stops).ok()
        });
        let Some(brush) = brush else { return };
        unsafe {
            brush.SetStartPoint(D2D_POINT_2F { x: area.left, y: area.top });
            brush.SetEndPoint(D2D_POINT_2F { x: area.left, y: area.bottom });
            brush.SetOpacity(top);
            self.target.FillRoundedRectangle(&rounded(area, radius), brush);
        }
    }

    /// A line of `colour` in its middle, fading out towards both ends.
    pub fn fading_line(&self, from: (f32, f32), to: (f32, f32), width: f32, colour: D2D1_COLOR_F) {
        let clear = D2D1_COLOR_F { a: 0.0, ..colour };
        let stops = [
            D2D1_GRADIENT_STOP { position: 0.0, color: clear },
            D2D1_GRADIENT_STOP { position: 0.5, color: colour },
            D2D1_GRADIENT_STOP { position: 1.0, color: clear },
        ];
        let point = |(x, y): (f32, f32)| D2D_POINT_2F { x, y };
        unsafe {
            let Ok(stops) = self.target.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP) else { return };
            let line = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES { startPoint: point(from), endPoint: point(to) };
            if let Ok(brush) = self.target.CreateLinearGradientBrush(&line, None, &stops) {
                self.target.DrawLine(point(from), point(to), &brush, width, None);
            }
        }
    }

    /// Draws with nothing of it shown within `hole`, a rectangle with
    /// corners of `radius`: light about something that shows through.
    pub fn outside(&self, hole: &Rect, radius: f32, draw: impl FnOnce()) {
        let far = 1.0e6;
        let masked = unsafe {
            self.target.GetFactory().ok().and_then(|factory: ID2D1Factory| {
                let all: ID2D1Geometry = factory.CreateRectangleGeometry(&Rect { left: -far, top: -far, right: far, bottom: far }).ok()?.cast().ok()?;
                let within: ID2D1Geometry = factory.CreateRoundedRectangleGeometry(&rounded(hole, radius)).ok()?.cast().ok()?;
                let group = factory.CreateGeometryGroup(D2D1_FILL_MODE_ALTERNATE, &[Some(all), Some(within)]).ok()?;
                let parameters = D2D1_LAYER_PARAMETERS {
                    contentBounds: Rect { left: -far, top: -far, right: far, bottom: far },
                    geometricMask: std::mem::ManuallyDrop::new(Some(group.cast().ok()?)),
                    maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                    maskTransform: Matrix3x2::identity(),
                    opacity: 1.0,
                    opacityBrush: std::mem::ManuallyDrop::new(None),
                    layerOptions: D2D1_LAYER_OPTIONS_NONE,
                };
                self.target.PushLayer(&parameters, None);
                let D2D1_LAYER_PARAMETERS { geometricMask, opacityBrush, .. } = parameters;
                drop(std::mem::ManuallyDrop::into_inner(geometricMask));
                drop(std::mem::ManuallyDrop::into_inner(opacityBrush));
                Some(())
            })
        };
        draw();
        if masked.is_some() {
            unsafe { self.target.PopLayer() };
        }
    }

    /// A soft shadow or glow (`Glow`) cast by a rounded rectangle at
    /// `area`: `corner` is how large each of its corners is drawn, which
    /// sets how far it spreads and how round it is.
    pub fn glow(&self, glow: &Glow, area: &Rect, corner: f32, opacity: f32) {
        if opacity <= 0.0 || corner <= 0.0 {
            return;
        }
        for (dest, source) in glow_parts(area, corner) {
            if dest.right - dest.left < 0.01 || dest.bottom - dest.top < 0.01 {
                continue;
            }
            unsafe {
                self.target.DrawBitmap(&glow.0, Some(&dest), opacity, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, Some(&source));
            }
        }
    }
}

/// Where each of the nine parts of a glow cast by a rounded rectangle at
/// `area` goes (`Painter::glow`), row by row, and what of `Glow`'s square
/// it shows: `corner` is how large each of its corners is drawn.
pub fn glow_parts(area: &Rect, corner: f32) -> [(Rect, Rect); 9] {
    let spread = corner * Glow::INSET / Glow::CORNER;
    let d = Rect { left: area.left - spread, top: area.top - spread, right: area.right + spread, bottom: area.bottom + spread };
    // Not narrower than its two corners.
    let k = corner.min((d.right - d.left) / 2.0).min((d.bottom - d.top) / 2.0);
    let (c, n) = (Glow::CORNER, Glow::SIZE as f32);
    let xs = [(d.left, d.left + k, 0.0, c), (d.left + k, d.right - k, c, n - c), (d.right - k, d.right, n - c, n)];
    let ys = [(d.top, d.top + k, 0.0, c), (d.top + k, d.bottom - k, c, n - c), (d.bottom - k, d.bottom, n - c, n)];
    std::array::from_fn(|i| {
        let ((left, right, sx0, sx1), (top, bottom, sy0, sy1)) = (xs[i % 3], ys[i / 3]);
        (Rect { left, top, right, bottom }, Rect { left: sx0, top: sy0, right: sx1, bottom: sy1 })
    })
}

/// A pinned window's light (in `pinned`, at `pin_light`): below it, how
/// large its corners are drawn (so how far it spreads), how far under it
/// it goes and how opaque; about it all round, its corners and opacity;
/// and how round the frame is that none of it shows within. The same on
/// the board and while the screen follows the hand.
pub const PIN_BELOW: (f32, f32, f32) = (80.0, 16.0, 0.95);
pub const PIN_ROUND: (f32, f32) = (26.0, 0.40);
pub const PIN_FRAME: f32 = 8.0;

/// The two glows of a window at `body` pinned to every shelf (`all`) or
/// to its own, `s` times their size: where each is cast, how large its
/// corners and how opaque. Most below it, spreading down as a lamp under
/// it would; faint all round.
pub fn pin_glows(body: &Rect, s: f32, all: bool) -> [(Rect, f32, f32); 2] {
    let reach = pin_light(all).1 * s;
    let (bw, bh) = (body.right - body.left, body.bottom - body.top);
    let under = Rect { left: body.left + bw * 0.04, right: body.right - bw * 0.04, top: body.bottom - bh * 0.30, bottom: body.bottom + PIN_BELOW.1 * reach };
    [(under, PIN_BELOW.0 * reach, PIN_BELOW.2), (*body, PIN_ROUND.0 * reach, PIN_ROUND.1)]
}

/// A blurred rounded square of one colour, drawn stretched in nine parts
/// (`Painter::glow`) under whatever casts it: made once for each target.
pub struct Glow(ID2D1Bitmap);

impl Glow {
    /// Radius of the square's corners and of the blur, in its pixels.
    const RADIUS: f32 = 14.0;
    const BLUR: f32 = 10.0;
    /// How far in from the bitmap's edge the square is.
    const INSET: f32 = 3.0 * Self::BLUR;
    /// The corner parts, out to the edge.
    pub const CORNER: f32 = Self::INSET + Self::RADIUS;
    pub const SIZE: usize = 2 * Self::CORNER as usize + 2;

    pub fn new(target: &ID2D1RenderTarget, colour: D2D1_COLOR_F) -> Option<Glow> {
        let n = Self::SIZE;
        let pixels = premultiplied(&Self::mask(None), [colour.r, colour.g, colour.b], colour.a);
        let properties = D2D1_BITMAP_PROPERTIES {
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
            dpiX: 96.0,
            dpiY: 96.0,
        };
        let size = D2D_SIZE_U { width: n as u32, height: n as u32 };
        unsafe { target.CreateBitmap(size, Some(pixels.as_ptr().cast()), (n * 4) as u32, &properties).ok().map(Glow) }
    }

    /// How opaque each pixel of its square is, row by row, `SIZE` by
    /// `SIZE`; with `cut`, none within what casts it, whose corners are
    /// that round (in its pixels, no rounder than its own).
    pub fn mask(cut: Option<f32>) -> Vec<f32> {
        let n = Self::SIZE;
        let mut mask = glow_mask(n, Self::INSET, Self::RADIUS, Self::BLUR);
        if let Some(radius) = cut {
            let (lo, hi) = (Self::INSET, n as f32 - Self::INSET);
            for (i, a) in mask.iter_mut().enumerate() {
                *a *= 1.0 - covered([lo, lo, hi, hi], radius.min(Self::RADIUS), (i % n) as f32 + 0.5, (i / n) as f32 + 0.5);
            }
        }
        mask
    }
}

/// Blue, green, red and alpha bytes, premultiplied, of `colour` as opaque
/// as `alpha` times each of `mask`.
pub fn premultiplied(mask: &[f32], [r, g, b]: [f32; 3], alpha: f32) -> Vec<u8> {
    let mut pixels = vec![0u8; mask.len() * 4];
    for (pixel, a) in pixels.chunks_exact_mut(4).zip(mask) {
        let a = a * alpha;
        pixel.copy_from_slice(&[(b * a * 255.0) as u8, (g * a * 255.0) as u8, (r * a * 255.0) as u8, (a * 255.0) as u8]);
    }
    pixels
}

/// How much of each pixel of an `n` by `n` square is covered by a
/// rounded square `inset` from its edges, blurred by `blur` (a Gaussian's
/// deviation), row by row.
fn glow_mask(n: usize, inset: f32, radius: f32, blur: f32) -> Vec<f32> {
    let inside = |x: f32, y: f32| {
        let (lo, hi) = (inset + radius, n as f32 - inset - radius);
        let (dx, dy) = ((lo - x).max(x - hi).max(0.0), (lo - y).max(y - hi).max(0.0));
        let edge = inset.max(0.0);
        let within = x >= edge && x <= n as f32 - edge && y >= edge && y <= n as f32 - edge;
        if within && dx.hypot(dy) <= radius { 1.0 } else { 0.0 }
    };
    let mut mask: Vec<f32> = (0..n * n).map(|i| inside((i % n) as f32 + 0.5, (i / n) as f32 + 0.5)).collect();
    spread(&mut mask, n, [blur, blur], [blur, blur]);
    mask
}

/// Blurs a `width` wide mask, row by row, across by Gaussians of
/// `across` deviations and down by `down`: the first of each how far
/// what is covered spreads left (up), the second right (down), each side
/// half the light, so an edge is half lit whichever way it faces.
pub fn spread(mask: &mut [f32], width: usize, across: [f32; 2], down: [f32; 2]) {
    let height = mask.len() / width;
    for (horizontal, [before, after]) in [(true, across), (false, down)] {
        // What lies `o` before a pixel lights it with the `after` side.
        let weight = |o: isize, s: f32| (-(o * o) as f32 / (2.0 * s * s)).exp() / s;
        let reach = |s: f32| (3.0 * s).ceil() as isize;
        let kernel: Vec<(isize, f32)> = (-reach(before)..=reach(after))
            .map(|o| (o, match o { 0 => (weight(0, before) + weight(0, after)) / 2.0, o if o > 0 => weight(o, after), o => weight(o, before) }))
            .collect();
        let total: f32 = kernel.iter().map(|(_, w)| w).sum();
        let from = mask.to_vec();
        for y in 0..height {
            for x in 0..width {
                let mut sum = 0.0;
                for &(o, weight) in &kernel {
                    let (sx, sy) = if horizontal { (x as isize - o, y as isize) } else { (x as isize, y as isize - o) };
                    if sx >= 0 && sy >= 0 && (sx as usize) < width && (sy as usize) < height {
                        sum += weight * from[sy as usize * width + sx as usize];
                    }
                }
                mask[y * width + x] = sum / total;
            }
        }
    }
}

/// How much of a pixel whose middle is at `x`, `y` a rectangle with
/// corners of `radius` covers, smoothly over a pixel at its edge.
pub fn covered(area: [f32; 4], radius: f32, x: f32, y: f32) -> f32 {
    let [left, top, right, bottom] = area;
    let r = radius.min((right - left) / 2.0).min((bottom - top) / 2.0).max(0.0);
    let (dx, dy) = ((left + r - x).max(x - right + r), (top + r - y).max(y - bottom + r));
    let outside = dx.max(0.0).hypot(dy.max(0.0)) + dx.max(dy).min(0.0) - r;
    (0.5 - outside).clamp(0.0, 1.0)
}

#[cfg(test)]
mod text_cache_tests {
    use super::*;
    #[test]
    fn cached_labels_keep_their_own_bounds_and_format() {
        use windows::Win32::Graphics::DirectWrite::{DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING};
        let cache = TextCache::new().unwrap();
        let leading = text_format(&cache.factory, 14.0, 400, DWRITE_TEXT_ALIGNMENT_LEADING).unwrap();
        let centered = text_format(&cache.factory, 14.0, 400, DWRITE_TEXT_ALIGNMENT_CENTER).unwrap();
        let first = cache.layout("Shelf", &leading, 120.0, 30.0).unwrap();
        assert_eq!(first.as_raw(), cache.layout("Shelf", &leading, 120.0, 30.0).unwrap().as_raw());
        let narrow = cache.layout("Shelf", &leading, 60.0, 30.0).unwrap();
        let other = cache.layout("Shelf", &centered, 120.0, 30.0).unwrap();
        unsafe {
            assert!((narrow.GetMaxWidth() - 60.0).abs() < 1e-6);
            assert!((first.GetMaxWidth() - 120.0).abs() < 1e-6);
            assert_eq!(other.GetTextAlignment(), DWRITE_TEXT_ALIGNMENT_CENTER);
            assert_eq!(first.GetTextAlignment(), DWRITE_TEXT_ALIGNMENT_LEADING);
        }
        cache.clear();
        assert_eq!(cache.count.get(), 0);
    }
}
