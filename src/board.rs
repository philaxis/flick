//! The full-screen grid view: every workspace row with a live miniature of
//! each desktop, where windows and cells can be dragged around.
//!
//! The board owns layout, hit-testing, drawing and the DWM thumbnails. It does
//! not touch desktops itself; input is turned into an `Action` for the app.

use crate::grid::{CellId, Dir, Grid};
use std::collections::HashMap;
use windows::{
    core::{w, ComInterface, Result},
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
        Graphics::{
            Direct2D::{
                Common::{
                    D2D1_ALPHA_MODE_IGNORE, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT,
                    D2D_RECT_F, D2D_SIZE_U,
                },
                D2D1CreateFactory, ID2D1Bitmap, ID2D1Factory, ID2D1RenderTarget,
                D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, D2D1_BITMAP_PROPERTIES,
                D2D1_DRAW_TEXT_OPTIONS_CLIP, D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_FEATURE_LEVEL_DEFAULT,
                D2D1_RENDER_TARGET_PROPERTIES,
                D2D1_RENDER_TARGET_TYPE_DEFAULT, D2D1_RENDER_TARGET_USAGE_GDI_COMPATIBLE,
                D2D1_ROUNDED_RECT,
            },
            DirectWrite::{
                DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat, DWRITE_FACTORY_TYPE_SHARED,
                DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT, DWRITE_MEASURING_MODE_NATURAL,
                DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT, DWRITE_TEXT_ALIGNMENT_CENTER,
                DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_WORD_WRAPPING_NO_WRAP,
            },
            Dwm::{
                DwmRegisterThumbnail, DwmUnregisterThumbnail,
                DwmUpdateThumbnailProperties, DWM_THUMBNAIL_PROPERTIES, DWM_TNP_OPACITY, DWM_TNP_RECTDESTINATION,
                DWM_TNP_SOURCECLIENTAREAONLY, DWM_TNP_VISIBLE,
            },
            Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
            Gdi::{
                BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, GetMonitorInfoW,
                InvalidateRect, MonitorFromPoint, ReleaseDC, SelectObject, ValidateRect, BITMAPINFO, BITMAPINFOHEADER,
                DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ, MONITORINFO, MONITOR_DEFAULTTONEAREST, SRCCOPY,
            },
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI},
            Input::KeyboardAndMouse::{
                ReleaseCapture, SetCapture, VK_BACK, VK_DOWN, VK_ESCAPE, VK_F2, VK_LEFT, VK_RETURN, VK_RIGHT, VK_UP,
            },
            WindowsAndMessaging::{
                CreateWindowExW, DrawIconEx, GetClassLongPtrW, GetCursorPos, LoadCursorW, RegisterClassW, IDC_ARROW, SendMessageTimeoutW,
                SetWindowPos, ShowWindow, CS_DBLCLKS, DI_NORMAL, GCLP_HICON, HICON, HWND_TOPMOST, ICON_BIG,
                SMTO_ABORTIFHUNG, SWP_SHOWWINDOW, SW_HIDE, WM_ACTIVATE, WM_CHAR, WM_ERASEBKGND, WM_GETICON, WM_KEYDOWN,
                WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_PAINT, WM_RBUTTONUP, WNDCLASSW,
                WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
            },
        },
    },
};

pub struct WindowModel {
    pub hwnd: HWND,
    /// Screen rectangle, `None` while minimized.
    pub rect: Option<RECT>,
    pub title: String,
    /// Executable name; windows of one app are counted together in the map.
    pub app: String,
    /// Follows the user from cell to cell inside its row.
    pub follows: bool,
}

pub struct CellModel {
    pub id: CellId,
    /// Topmost first.
    pub windows: Vec<WindowModel>,
}

pub struct RowModel {
    pub name: String,
    pub memory: u64,
    /// Its apps' memory was pushed out of RAM after being idle.
    pub asleep: bool,
    pub cells: Vec<CellModel>,
}

/// Everything the board shows, built by the app from the grid and the windows.
pub struct Model {
    pub rows: Vec<RowModel>,
    /// Windows shown on every desktop.
    pub pinned: Vec<WindowModel>,
    pub current: CellId,
}

/// What the user asked for; carried out by the app.
pub enum Action {
    None,
    /// Close without going anywhere and give focus back.
    Cancel,
    /// The board lost focus; close without touching focus.
    Dismiss,
    /// Switch to a cell and close; optionally focus one of its windows.
    Go(CellId, Option<HWND>),
    MoveWindow(HWND, CellId),
    MoveCell { id: CellId, row: usize, index: usize },
    MoveCellToNewRow { id: CellId, at: usize },
    AddCell(usize),
    AddRow,
    RemoveCell(CellId),
    /// Show the pin menu for a window.
    WindowMenu(HWND),
    /// Set how one window is pinned: following within its row, shown on
    /// every desktop, or neither (never both).
    SetPin { window: HWND, row: bool, all: bool },
    CloseRow(usize),
    Rename(usize, String),
}

type Rect = D2D_RECT_F;

/// The map is drawn at this fraction of the board's scale.
const MAP_SCALE: f32 = 1.0;
/// Height of the hint line at the bottom, in unscaled units.
const FOOTER: f32 = 44.0;

fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect { left: x, top: y, right: x + w, bottom: y + h }
}

fn contains(r: &Rect, x: f32, y: f32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

fn rounded(r: &Rect, radius: f32) -> D2D1_ROUNDED_RECT {
    D2D1_ROUNDED_RECT { rect: *r, radiusX: radius, radiusY: radius }
}

fn grow(r: &Rect, by: f32) -> Rect {
    Rect { left: r.left - by, top: r.top - by, right: r.right + by, bottom: r.bottom + by }
}

fn rgba(r: f32, g: f32, b: f32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r, g, b, a }
}

fn white(a: f32) -> D2D1_COLOR_F {
    rgba(1.0, 1.0, 1.0, a)
}

fn accent(a: f32) -> D2D1_COLOR_F {
    rgba(0.34, 0.62, 1.0, a)
}

fn danger(a: f32) -> D2D1_COLOR_F {
    rgba(0.93, 0.33, 0.33, a)
}

fn format_memory(bytes: u64) -> String {
    let mb = bytes as f64 / (1024.0 * 1024.0);
    if mb >= 1024.0 {
        format!("≈ {:.1} GB", mb / 1024.0)
    } else {
        format!("≈ {mb:.0} MB")
    }
}

/// One app in a map cell: its icon and how many windows it has there.
struct AppBadge {
    /// A window of the app, whose icon is shown.
    hwnd: HWND,
    icon: Rect,
    count: usize,
    /// Where the count is written (only when above one).
    label: Rect,
}

struct CellLayout {
    id: CellId,
    body: Rect,
    close: Rect,
    apps: Vec<AppBadge>,
    /// Apps that did not fit.
    more: usize,
}

struct RowLayout {
    /// The whole label area left of the row; its extras show on hover.
    header: Rect,
    name: Rect,
    close: Rect,
    top: f32,
    bottom: f32,
    cells: Vec<CellLayout>,
    plus: Rect,
}

/// One window of the selected cell, shown large with its title.
struct TileLayout {
    hwnd: HWND,
    frame: Rect,
    icon: Rect,
    title: Rect,
    thumb: Rect,
    /// The two pin toggles at the right of the title bar.
    pin_row: Rect,
    pin_all: Rect,
}

#[derive(Default)]
struct Layout {
    /// The selected cell's windows, the main content of the board.
    tiles: Vec<TileLayout>,
    tiles_area: Rect,
    tiles_label: Rect,
    /// Top edge of the map at the bottom of the board.
    map_top: f32,
    /// The column on which every row's landing cell is lined up.
    spine: Rect,
    rows: Vec<RowLayout>,
    add_row: Rect,
}

#[derive(Clone, PartialEq, Default)]
enum Hit {
    #[default]
    Nothing,
    /// `cell` is empty for a window pinned to every desktop.
    Window { cell: CellId, hwnd: isize },
    /// The cell's title bar or its bare background; the handle for dragging it.
    Cell(CellId),
    /// The "follow within the row" / "on every desktop" toggle of a tile.
    PinRow(isize),
    PinAll(isize),
    CellClose(CellId),
    Plus(usize),
    AddRow,
    RowClose(usize),
    RowName(usize),
}

/// Where a dragged cell would be dropped.
#[derive(Clone, Copy, PartialEq)]
enum Slot {
    /// Into `row` at `index` (counted without the dragged cell).
    Row { row: usize, index: usize, x: f32 },
    NewRow { at: usize, y: f32 },
}

struct Press {
    hit: Hit,
    x: f32,
    y: f32,
    dragging: bool,
}

struct Fonts {
    label: IDWriteTextFormat,
    small: IDWriteTextFormat,
    centered: IDWriteTextFormat,
    big: IDWriteTextFormat,
}

pub struct Board {
    hwnd: HWND,
    factory: ID2D1Factory,
    canvas: Option<Canvas>,
    fonts: Option<Fonts>,
    /// The same fonts at the map's smaller scale.
    map_fonts: Option<Fonts>,
    open: bool,
    model: Model,
    /// Copy of the grid used to move the selection, so that looking around
    /// does not disturb the real per-row memory until a cell is chosen.
    nav: Grid,
    selected: CellId,
    layout: Layout,
    size: (i32, i32),
    scale: f32,
    /// (source window, thumbnail handle, is a tile rather than part of the
    /// map), in drawing order.
    thumbs: Vec<(isize, isize, bool)>,
    icons: HashMap<isize, ID2D1Bitmap>,
    hover: Hit,
    press: Option<Press>,
    cursor: (f32, f32),
    cursor_before: (f32, f32),
    /// Row whose close button was clicked once and waits for confirmation.
    armed_row: Option<usize>,
    editing: Option<(usize, String)>,
}

impl Board {
    pub fn new(wndproc: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT) -> Result<Board> {
        unsafe {
            let instance = GetModuleHandleW(None)?;
            let class = w!("kankan.board");
            RegisterClassW(&WNDCLASSW {
                style: CS_DBLCLKS,
                // Without a class cursor Windows keeps showing the "busy" one.
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                lpfnWndProc: Some(wndproc),
                hInstance: instance.into(),
                lpszClassName: class,
                ..Default::default()
            });
            // A tool window stays out of the taskbar and Alt+Tab and is not
            // counted as an app window on any desktop.
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                class,
                w!("KanKan"),
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                instance,
                None,
            );
            Ok(Board {
                hwnd,
                factory: D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?,
                canvas: None,
                fonts: None,
                map_fonts: None,
                open: false,
                model: Model { rows: Vec::new(), pinned: Vec::new(), current: String::new() },
                nav: Grid::default(),
                selected: String::new(),
                layout: Layout::default(),
                size: (0, 0),
                scale: 1.0,
                thumbs: Vec::new(),
                icons: HashMap::new(),
                hover: Hit::Nothing,
                press: None,
                cursor: (0.0, 0.0),
                cursor_before: (0.0, 0.0),
                armed_row: None,
                editing: None,
            })
        }
    }

    pub fn hwnd(&self) -> HWND {
        self.hwnd
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn selected(&self) -> CellId {
        self.selected.clone()
    }

    /// Covers the monitor under the cursor and shows `model`.
    pub fn open(&mut self, model: Model, grid: Grid) {
        unsafe {
            let mut cursor = POINT::default();
            let _ = GetCursorPos(&mut cursor);
            let monitor = MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST);
            let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            if !GetMonitorInfoW(monitor, &mut info).as_bool() {
                return;
            }
            let (mut dpi, mut dpi_y) = (96u32, 96u32);
            let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi, &mut dpi_y);
            let area = info.rcMonitor;
            let size = (area.right - area.left, area.bottom - area.top);
            if self.scale != dpi as f32 / 96.0 {
                self.fonts = None;
                self.map_fonts = None;
            }
            self.scale = dpi as f32 / 96.0;
            self.size = size;
            self.selected = model.current.clone();
            self.hover = Hit::Nothing;
            self.press = None;
            self.armed_row = None;
            self.editing = None;
            self.open = true;
            let _ = SetWindowPos(self.hwnd, HWND_TOPMOST, area.left, area.top, size.0, size.1, SWP_SHOWWINDOW);
        }
        self.set_model(model, grid);
    }

    /// Replaces the content, keeping the selection when its cell still exists.
    pub fn set_model(&mut self, model: Model, grid: Grid) {
        if !self.open {
            return;
        }
        self.model = model;
        self.nav = grid;
        if self.nav.find(&self.selected).is_none() {
            self.selected = self.model.current.clone();
        }
        // Makes the selected cell its row's landing cell, which the map lines up.
        let selected = self.selected.clone();
        self.nav.visit(&selected);
        self.press = None;
        self.relayout();
        self.register_thumbnails();
        self.invalidate();
    }

    pub fn close(&mut self) {
        if !self.open {
            return;
        }
        self.open = false;
        self.clear_thumbnails();
        self.icons.clear();
        self.press = None;
        unsafe {
            let _ = ReleaseCapture();
            ShowWindow(self.hwnd, SW_HIDE);
        }
    }

    /// Moves the selection one cell, by the same rules as real navigation.
    pub fn select_dir(&mut self, dir: Dir) {
        let Some(from) = self.nav.find(&self.selected) else { return };
        if let Some(id) = self.nav.target(from, dir).and_then(|pos| self.nav.id_at(pos).cloned()) {
            self.nav.visit(&id);
            self.selected = id;
            // Other windows to show, and the rows slide to stay lined up.
            self.relayout();
            self.register_thumbnails();
            self.invalidate();
        }
    }

    fn invalidate(&self) {
        unsafe {
            InvalidateRect(self.hwnd, None, false);
        }
    }

    // ---- layout -----------------------------------------------------------

    fn selected_cell(&self) -> Option<&CellModel> {
        self.model.rows.iter().flat_map(|row| &row.cells).find(|cell| cell.id == self.selected)
    }

    /// The windows shown as tiles: those of the selected cell, then the ones
    /// pinned to every desktop (flagged), which are visible there as well.
    fn tile_windows(&self) -> Vec<(&WindowModel, bool)> {
        let cell = self.selected_cell().into_iter().flat_map(|cell| &cell.windows).map(|w| (w, false));
        cell.chain(self.model.pinned.iter().map(|w| (w, true))).collect()
    }

    fn relayout(&mut self) {
        let s = self.scale;
        let (w, h) = (self.size.0 as f32, self.size.1 as f32);
        // Each row is shifted so that the cell a vertical move would land on
        // (the one it was last left on) sits in the same column for all rows.
        let anchors: Vec<usize> = self
            .model
            .rows
            .iter()
            .enumerate()
            .map(|(r, row)| {
                let last = self.nav.rows.get(r).and_then(|nav| nav.last.as_ref());
                last.and_then(|id| row.cells.iter().position(|cell| cell.id == *id)).unwrap_or(0)
            })
            .collect();
        let mut layout = compute_layout(&self.model, &anchors, (w, h - FOOTER * s), s * MAP_SCALE);
        let margin = 40.0 * s;
        layout.tiles_label = rect(margin, 22.0 * s, w - 2.0 * margin, 30.0 * s);
        layout.tiles_area = Rect { left: margin, top: 64.0 * s, right: w - margin, bottom: layout.map_top - 26.0 * s };
        layout.tiles = layout_tiles(&self.tile_windows(), &layout.tiles_area, s);
        self.layout = layout;
    }

    /// A tile acts like its window in the map; one pinned everywhere belongs
    /// to no cell.
    fn tile_hit(&self, hwnd: HWND) -> Hit {
        let pinned = self.model.pinned.iter().any(|w| w.hwnd == hwnd);
        Hit::Window { cell: if pinned { String::new() } else { self.selected.clone() }, hwnd: hwnd.0 }
    }

    fn hit_test(&self, x: f32, y: f32) -> Hit {
        if let Some(tile) = self.layout.tiles.iter().find(|tile| contains(&tile.frame, x, y)) {
            if contains(&tile.pin_row, x, y) {
                return Hit::PinRow(tile.hwnd.0);
            }
            if contains(&tile.pin_all, x, y) {
                return Hit::PinAll(tile.hwnd.0);
            }
            return self.tile_hit(tile.hwnd);
        }
        if contains(&self.layout.add_row, x, y) {
            return Hit::AddRow;
        }
        for (r, row) in self.layout.rows.iter().enumerate() {
            if contains(&row.close, x, y) {
                return Hit::RowClose(r);
            }
            if contains(&row.name, x, y) {
                return Hit::RowName(r);
            }
            if contains(&row.plus, x, y) {
                return Hit::Plus(r);
            }
            for cell in &row.cells {
                if contains(&cell.close, x, y) && self.cell_count() > 1 {
                    return Hit::CellClose(cell.id.clone());
                }
                if contains(&cell.body, x, y) {
                    return Hit::Cell(cell.id.clone());
                }
            }
        }
        Hit::Nothing
    }

    fn cell_count(&self) -> usize {
        self.layout.rows.iter().map(|row| row.cells.len()).sum()
    }

    fn cell_at(&self, x: f32, y: f32) -> Option<&CellLayout> {
        // A little slack around each square makes it an easier drop target.
        let slack = 5.0 * self.scale;
        self.layout.rows.iter().flat_map(|row| &row.cells).find(|cell| contains(&grow(&cell.body, slack), x, y))
    }

    fn slot_at(&self, dragged: &str, x: f32, y: f32) -> Option<Slot> {
        let rows = &self.layout.rows;
        let (first, last) = (rows.first()?, rows.last()?);
        if y < first.top {
            return Some(Slot::NewRow { at: 0, y: first.top - 8.0 * self.scale });
        }
        if y >= last.bottom {
            return Some(Slot::NewRow { at: rows.len(), y: last.bottom + 8.0 * self.scale });
        }
        for (r, row) in rows.iter().enumerate() {
            if y >= row.top && y < row.bottom {
                let others: Vec<&CellLayout> = row.cells.iter().filter(|c| c.id != dragged).collect();
                let index = others.iter().filter(|c| (c.body.left + c.body.right) / 2.0 < x).count();
                let gap = 10.0 * self.scale;
                let x = match others.get(index) {
                    Some(next) => next.body.left - gap,
                    None => others.last().map_or(row.plus.left, |c| c.body.right + gap),
                };
                return Some(Slot::Row { row: r, index, x });
            }
            if let Some(next) = rows.get(r + 1) {
                if y >= row.bottom && y < next.top {
                    return Some(Slot::NewRow { at: r + 1, y: (row.bottom + next.top) / 2.0 });
                }
            }
        }
        None
    }

    // ---- thumbnails -------------------------------------------------------

    fn clear_thumbnails(&mut self) {
        for (_, thumb, _) in self.thumbs.drain(..) {
            unsafe {
                let _ = DwmUnregisterThumbnail(thumb);
            }
        }
    }

    /// Registers a live thumbnail for every tile.
    fn register_thumbnails(&mut self) {
        self.clear_thumbnails();
        let wanted: Vec<HWND> = self.tile_windows().into_iter().filter(|(w, _)| w.rect.is_some()).map(|(w, _)| w.hwnd).collect();
        for hwnd in wanted {
            if let Ok(thumb) = unsafe { DwmRegisterThumbnail(self.hwnd, hwnd) } {
                self.thumbs.push((hwnd.0, thumb, true));
            }
        }
        self.update_thumbnails();
    }

    fn dragged_window(&self) -> Option<isize> {
        match &self.press {
            Some(Press { hit: Hit::Window { hwnd, .. }, dragging: true, .. }) => Some(*hwnd),
            _ => None,
        }
    }

    fn update_thumbnails(&self) {
        let dragged = self.dragged_window();
        let to_rect = |r: &Rect| RECT {
            left: r.left.round() as i32,
            top: r.top.round() as i32,
            right: r.right.round() as i32,
            bottom: r.bottom.round() as i32,
        };
        for &(hwnd, thumb, _) in &self.thumbs {
            let mut props = DWM_THUMBNAIL_PROPERTIES {
                dwFlags: DWM_TNP_RECTDESTINATION | DWM_TNP_VISIBLE | DWM_TNP_OPACITY | DWM_TNP_SOURCECLIENTAREAONLY,
                opacity: 255,
                fVisible: true.into(),
                fSourceClientAreaOnly: false.into(),
                ..Default::default()
            };
            match self.layout.tiles.iter().find(|tile| tile.hwnd.0 == hwnd) {
                Some(tile) if dragged == Some(hwnd) => {
                    // Follows the cursor, small enough to see the map under it.
                    let t = &tile.thumb;
                    let aspect = (t.right - t.left) / (t.bottom - t.top).max(1.0);
                    let (cx, cy) = self.cursor;
                    let w = 200.0 * self.scale;
                    props.rcDestination = to_rect(&rect(cx - w / 2.0, cy - w / aspect / 2.0, w, w / aspect));
                    props.opacity = 215;
                }
                Some(tile) => props.rcDestination = to_rect(&tile.thumb),
                None => props.fVisible = false.into(),
            }
            unsafe {
                let _ = DwmUpdateThumbnailProperties(thumb, &props);
            }
        }
    }

    /// Thumbnails stack in registration order; re-register the one that will
    /// follow the cursor to lift it above the rest.
    fn raise_thumbnail(&mut self, hwnd: isize) {
        let index = self
            .thumbs
            .iter()
            .position(|&(h, _, tile)| h == hwnd && tile)
            .or_else(|| self.thumbs.iter().position(|&(h, _, _)| h == hwnd));
        if let Some(i) = index {
            let (_, old, tile) = self.thumbs.remove(i);
            unsafe {
                let _ = DwmUnregisterThumbnail(old);
                if let Ok(thumb) = DwmRegisterThumbnail(self.hwnd, HWND(hwnd)) {
                    self.thumbs.push((hwnd, thumb, tile));
                }
            }
        }
    }

    // ---- input ------------------------------------------------------------

    /// Handles a window message. `None` means "not ours, use the default".
    pub fn handle(&mut self, message: u32, wparam: WPARAM, lparam: LPARAM) -> Option<Action> {
        if !self.open {
            return None;
        }
        let point = || ((lparam.0 & 0xFFFF) as i16 as f32, ((lparam.0 >> 16) & 0xFFFF) as i16 as f32);
        Some(match message {
            WM_PAINT => {
                self.paint();
                unsafe {
                    ValidateRect(self.hwnd, None);
                }
                Action::None
            }
            WM_ERASEBKGND => return None,
            WM_ACTIVATE if wparam.0 & 0xFFFF == 0 => Action::Dismiss,
            WM_LBUTTONDOWN => {
                let (x, y) = point();
                self.mouse_down(x, y)
            }
            WM_LBUTTONDBLCLK => {
                let (x, y) = point();
                if let Hit::RowName(r) = self.hit_test(x, y) {
                    self.start_rename(r);
                }
                Action::None
            }
            WM_MOUSEMOVE => {
                let (x, y) = point();
                self.mouse_move(x, y);
                Action::None
            }
            WM_LBUTTONUP => {
                let (x, y) = point();
                self.mouse_up(x, y)
            }
            WM_RBUTTONUP => {
                let (x, y) = point();
                match self.hit_test(x, y) {
                    Hit::Window { hwnd, .. } => Action::WindowMenu(HWND(hwnd)),
                    _ => Action::None,
                }
            }
            WM_KEYDOWN => self.key_down(wparam.0 as u16),
            WM_CHAR => {
                if let (Some((_, text)), Some(c)) = (&mut self.editing, char::from_u32(wparam.0 as u32)) {
                    if !c.is_control() && text.chars().count() < 24 {
                        text.push(c);
                        self.invalidate();
                    }
                }
                Action::None
            }
            _ => return None,
        })
    }

    fn start_rename(&mut self, row: usize) {
        if let Some(model) = self.model.rows.get(row) {
            self.editing = Some((row, model.name.clone()));
            self.invalidate();
        }
    }

    fn key_down(&mut self, key: u16) -> Action {
        if let Some((row, text)) = &mut self.editing {
            let row = *row;
            if key == VK_RETURN.0 {
                let name = text.trim().to_owned();
                self.editing = None;
                return Action::Rename(row, name);
            }
            if key == VK_ESCAPE.0 {
                self.editing = None;
            } else if key == VK_BACK.0 {
                text.pop();
            }
            self.invalidate();
            return Action::None;
        }
        match key {
            k if k == VK_LEFT.0 => self.select_dir(Dir::Left),
            k if k == VK_RIGHT.0 => self.select_dir(Dir::Right),
            k if k == VK_UP.0 => self.select_dir(Dir::Up),
            k if k == VK_DOWN.0 => self.select_dir(Dir::Down),
            k if k == VK_RETURN.0 => return Action::Go(self.selected.clone(), None),
            k if k == VK_ESCAPE.0 => return Action::Cancel,
            k if k == VK_F2.0 => {
                if let Some(pos) = self.nav.find(&self.selected) {
                    self.start_rename(pos.row);
                }
            }
            _ => {}
        }
        Action::None
    }

    fn mouse_down(&mut self, x: f32, y: f32) -> Action {
        let hit = self.hit_test(x, y);
        if !matches!(hit, Hit::RowClose(r) if self.armed_row == Some(r)) {
            self.armed_row = None;
        }
        self.cursor = (x, y);
        self.press = Some(Press { hit, x, y, dragging: false });
        unsafe {
            SetCapture(self.hwnd);
        }
        self.invalidate();
        // A click anywhere else finishes a rename in progress.
        match self.editing.take() {
            Some((row, text)) => Action::Rename(row, text.trim().to_owned()),
            None => Action::None,
        }
    }

    fn mouse_move(&mut self, x: f32, y: f32) {
        self.cursor = (x, y);
        let threshold = 6.0 * self.scale;
        let mut raise = None;
        if let Some(press) = &mut self.press {
            let draggable = match &press.hit {
                Hit::Window { cell, .. } => !cell.is_empty(),
                Hit::Cell(_) => true,
                _ => false,
            };
            if !press.dragging && draggable && (x - press.x).hypot(y - press.y) > threshold {
                press.dragging = true;
                if let Hit::Window { hwnd, .. } = press.hit {
                    raise = Some(hwnd);
                }
            }
            if press.dragging {
                if let Some(hwnd) = raise {
                    self.raise_thumbnail(hwnd);
                }
                self.update_thumbnails();
                self.invalidate();
            }
            return;
        }
        let hover = self.hit_test(x, y);
        let over_header = |px: f32, py: f32| self.layout.rows.iter().position(|row| contains(&row.header, px, py));
        if hover != self.hover || over_header(x, y) != over_header(self.cursor_before.0, self.cursor_before.1) {
            self.hover = hover;
            self.invalidate();
        }
        self.cursor_before = (x, y);
    }

    fn mouse_up(&mut self, x: f32, y: f32) -> Action {
        unsafe {
            let _ = ReleaseCapture();
        }
        let Some(press) = self.press.take() else { return Action::None };
        self.invalidate();
        if press.dragging {
            let action = match &press.hit {
                Hit::Window { cell, hwnd } => match self.cell_at(x, y) {
                    Some(target) if target.id != *cell => Action::MoveWindow(HWND(*hwnd), target.id.clone()),
                    _ => Action::None,
                },
                Hit::Cell(id) => match self.slot_at(id, x, y) {
                    Some(Slot::Row { row, index, .. }) => Action::MoveCell { id: id.clone(), row, index },
                    Some(Slot::NewRow { at, .. }) => Action::MoveCellToNewRow { id: id.clone(), at },
                    None => Action::None,
                },
                _ => Action::None,
            };
            // Put the thumbnail back; a successful move replaces the model anyway.
            self.update_thumbnails();
            return action;
        }
        if self.hit_test(x, y) != press.hit {
            return Action::None;
        }
        match press.hit {
            Hit::Window { cell, hwnd } => Action::Go(cell, Some(HWND(hwnd))),
            Hit::Cell(id) => Action::Go(id, None),
            Hit::PinRow(hwnd) | Hit::PinAll(hwnd) => {
                let windows = self.tile_windows();
                let Some((window, everywhere)) = windows.iter().find(|(w, _)| w.hwnd.0 == hwnd) else { return Action::None };
                // Each toggle turns itself on or off and always clears the other.
                let row = matches!(press.hit, Hit::PinRow(_)) && !window.follows;
                let all = matches!(press.hit, Hit::PinAll(_)) && !everywhere;
                Action::SetPin { window: HWND(hwnd), row, all }
            }
            Hit::CellClose(id) => Action::RemoveCell(id),
            Hit::Plus(row) => Action::AddCell(row),
            Hit::AddRow => Action::AddRow,
            Hit::RowClose(row) if self.armed_row == Some(row) => {
                self.armed_row = None;
                Action::CloseRow(row)
            }
            Hit::RowClose(row) => {
                self.armed_row = Some(row);
                Action::None
            }
            Hit::RowName(_) | Hit::Nothing => Action::None,
        }
    }

    // ---- drawing ----------------------------------------------------------

    /// The off-screen bitmap the board is drawn into before being copied to
    /// the window. (Drawing straight to the window with a Direct2D window
    /// target left the window blank on the machine this was developed on.)
    fn ensure_canvas(&mut self) -> Option<ID2D1RenderTarget> {
        if !self.canvas.as_ref().is_some_and(|c| c.size == self.size) {
            self.icons.clear();
            self.canvas = Canvas::new(&self.factory, self.size)
                .map_err(|e| crate::app::log(&format!("board canvas failed: {e}")))
                .ok();
        }
        self.canvas.as_ref().map(|c| c.target.clone())
    }

    fn paint(&mut self) {
        let Some(target) = self.ensure_canvas() else { return };
        if self.fonts.is_none() || self.map_fonts.is_none() {
            let log = |e| crate::app::log(&format!("board fonts failed: {e}"));
            self.fonts = make_fonts(self.scale).map_err(log).ok();
            self.map_fonts = make_fonts(self.scale * MAP_SCALE).map_err(log).ok();
        }
        self.load_icons(&target);
        unsafe {
            target.BeginDraw();
            let drawn = self.draw(&target);
            if let Err(e) = target.EndDraw(None, None).and(drawn) {
                crate::app::log(&format!("board draw failed: {e}"));
                // Rebuild everything on the next paint.
                self.canvas = None;
                return;
            }
            if let Some(canvas) = &self.canvas {
                let window = GetDC(self.hwnd);
                let _ = BitBlt(window, 0, 0, canvas.size.0, canvas.size.1, canvas.dc, 0, 0, SRCCOPY);
                ReleaseDC(self.hwnd, window);
            }
        }
    }

    fn load_icons(&mut self, target: &ID2D1RenderTarget) {
        let size = (20.0 * self.scale).round() as i32;
        let in_cells = self.model.rows.iter().flat_map(|r| &r.cells).flat_map(|c| &c.windows);
        for window in in_cells.chain(&self.model.pinned) {
            if !self.icons.contains_key(&window.hwnd.0) {
                if let Some(bitmap) = icon_bitmap(target, window.hwnd, size) {
                    self.icons.insert(window.hwnd.0, bitmap);
                }
            }
        }
    }

    fn draw(&self, t: &ID2D1RenderTarget) -> Result<()> {
        let s = self.scale;
        let (Some(fonts), Some(map_fonts)) = (&self.fonts, &self.map_fonts) else { return Ok(()) };
        unsafe {
            t.Clear(Some(&rgba(0.050, 0.055, 0.068, 1.0)));
        }
        self.draw_tiles(t, fonts, s)?;
        let hovered = self.draw_map(t, map_fonts, s * MAP_SCALE)?;
        unsafe {
            let brush = t.CreateSolidColorBrush(&white(1.0), None)?;
            let (w, h) = (self.size.0 as f32, self.size.1 as f32);
            let footer = rect(0.0, h - (FOOTER - 8.0) * s, w, 26.0 * s);
            let (line, alpha) = match &hovered {
                Some(title) => (title.as_str(), 0.85),
                None => ("창 클릭: 그 창으로 이동   ·   창을 아래 칸으로 끌기: 옮기기   ·   칸 클릭: 이동, 끌기: 재배치   ·   Esc: 닫기", 0.28),
            };
            let wide: Vec<u16> = line.encode_utf16().collect();
            brush.SetColor(&white(alpha));
            t.DrawText(&wide, &fonts.centered, &footer, &brush, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
        }
        Ok(())
    }

    /// The selected cell's windows as large titled previews.
    fn draw_tiles(&self, t: &ID2D1RenderTarget, fonts: &Fonts, s: f32) -> Result<()> {
        unsafe {
            let brush = t.CreateSolidColorBrush(&white(1.0), None)?;
            let text = |string: &str, font: &IDWriteTextFormat, area: &Rect, colour: D2D1_COLOR_F| {
                let wide: Vec<u16> = string.encode_utf16().collect();
                brush.SetColor(&colour);
                t.DrawText(&wide, font, area, &brush, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
            };
            let place = self.nav.find(&self.selected);
            let row_name = place.and_then(|p| self.model.rows.get(p.row)).map(|row| row.name.clone()).unwrap_or_default();
            if let Some(p) = place {
                let row = if row_name.is_empty() { format!("워크스페이스 {}", p.row + 1) } else { row_name };
                let here = if self.selected == self.model.current { "   ● 현재" } else { "" };
                text(&format!("{row}  ·  {}번 칸{here}", p.col + 1), &fonts.label, &self.layout.tiles_label, white(0.80));
            }
            if self.layout.tiles.is_empty() {
                text("이 칸에는 창이 없습니다", &fonts.centered, &self.layout.tiles_area, white(0.28));
            }
            let dragged = self.dragged_window();
            let windows = self.tile_windows();
            for tile in &self.layout.tiles {
                let Some((window, everywhere)) = windows.iter().find(|(w, _)| w.hwnd == tile.hwnd) else { continue };
                let hot = self.hover == self.tile_hit(tile.hwnd);
                let dim = if dragged == Some(tile.hwnd.0) { 0.35 } else { 1.0 };
                brush.SetColor(&rgba(0.125, 0.135, 0.165, dim));
                t.FillRoundedRectangle(&rounded(&tile.frame, 9.0 * s), &brush);
                // Stand-in under the live thumbnail (and all there is for a minimized window).
                brush.SetColor(&rgba(0.075, 0.08, 0.095, dim));
                t.FillRectangle(&tile.thumb, &brush);
                if let Some(bitmap) = self.icons.get(&tile.hwnd.0) {
                    t.DrawBitmap(bitmap, Some(&tile.icon), dim, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None);
                    let (cx, cy) = ((tile.thumb.left + tile.thumb.right) / 2.0, (tile.thumb.top + tile.thumb.bottom) / 2.0);
                    let big = rect(cx - 20.0 * s, cy - 20.0 * s, 40.0 * s, 40.0 * s);
                    t.DrawBitmap(bitmap, Some(&big), 0.8 * dim, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None);
                }
                text(&window.title, &fonts.small, &tile.title, white(if hot { 1.0 } else { 0.82 } * dim));

                // Pin toggles: at most one of the two is on.
                for (area, label, on, hit) in [
                    (&tile.pin_row, "행 고정", window.follows, Hit::PinRow(tile.hwnd.0)),
                    (&tile.pin_all, "전체 고정", *everywhere, Hit::PinAll(tile.hwnd.0)),
                ] {
                    let over = self.hover == hit;
                    brush.SetColor(&if on { accent(dim) } else { white(if over { 0.20 } else { 0.07 } * dim) });
                    t.FillRoundedRectangle(&rounded(area, 5.0 * s), &brush);
                    text(label, &fonts.centered, area, white(if on || over { 1.0 } else { 0.50 } * dim));
                }

                brush.SetColor(&if hot { accent(1.0) } else { white(0.10 * dim) });
                t.DrawRoundedRectangle(&rounded(&grow(&tile.frame, 1.0 * s), 10.0 * s), &brush, if hot { 2.5 * s } else { 1.0 }, None);
            }
        }
        Ok(())
    }

    /// The map of all rows and cells: plain squares holding each cell's apps
    /// (icon and count), with the controls appearing on hover.
    fn draw_map(&self, t: &ID2D1RenderTarget, fonts: &Fonts, s: f32) -> Result<Option<String>> {
        unsafe {
            let brush = t.CreateSolidColorBrush(&white(1.0), None)?;
            let text = |string: &str, font: &IDWriteTextFormat, area: &Rect, colour: D2D1_COLOR_F| {
                let wide: Vec<u16> = string.encode_utf16().collect();
                brush.SetColor(&colour);
                t.DrawText(&wide, font, area, &brush, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
            };
            let fill = |area: &Rect, radius: f32, colour: D2D1_COLOR_F| {
                brush.SetColor(&colour);
                t.FillRoundedRectangle(&rounded(area, radius), &brush);
            };
            let stroke = |area: &Rect, radius: f32, width: f32, colour: D2D1_COLOR_F| {
                brush.SetColor(&colour);
                t.DrawRoundedRectangle(&rounded(area, radius), &brush, width, None);
            };

            let dragged_cell = match &self.press {
                Some(Press { hit: Hit::Cell(id), dragging: true, .. }) => Some(id.as_str()),
                _ => None,
            };
            let drop_cell = self.dragged_window().and_then(|_| self.cell_at(self.cursor.0, self.cursor.1)).map(|c| c.id.as_str());
            let selected_row = self.nav.find(&self.selected).map(|pos| pos.row);
            let radius = 8.0 * s;

            // The column vertical moves travel along.
            fill(&self.layout.spine, 10.0 * s, white(0.035));

            for (r, (row, model)) in self.layout.rows.iter().zip(&self.model.rows).enumerate() {
                // Row label; memory and the close button only while pointed at.
                let editing = self.editing.as_ref().filter(|(row, _)| *row == r);
                let pointed = contains(&row.header, self.cursor.0, self.cursor.1) && self.press.is_none();
                let name = match editing {
                    Some((_, typed)) => format!("{typed}▏"),
                    None if model.name.is_empty() => format!("워크스페이스 {}", r + 1),
                    None => model.name.clone(),
                };
                if editing.is_some() {
                    fill(&grow(&row.name, 3.0 * s), 5.0 * s, white(0.10));
                }
                text(&name, &fonts.small, &row.name, white(if selected_row == Some(r) { 0.90 } else { 0.45 }));
                if self.armed_row == Some(r) {
                    fill(&row.close, 5.0 * s, danger(0.90));
                    text("한 번 더: 닫기", &fonts.centered, &row.close, white(1.0));
                } else if pointed {
                    let hot = self.hover == Hit::RowClose(r);
                    fill(&row.close, 5.0 * s, white(if hot { 0.18 } else { 0.08 }));
                    let asleep = if model.asleep { " · 재움" } else { "" };
                    text(&format!("닫기 · {}{asleep}", format_memory(model.memory)), &fonts.centered, &row.close, white(if hot { 0.95 } else { 0.60 }));
                }

                for cell in &row.cells {
                    let dim = if dragged_cell == Some(cell.id.as_str()) { 0.35 } else { 1.0 };
                    let is_selected = cell.id == self.selected;
                    let hovering = matches!(&self.hover, Hit::Cell(id) | Hit::CellClose(id) if *id == cell.id);
                    if is_selected {
                        for (by, a) in [(5.0, 0.08), (3.0, 0.14)] {
                            fill(&grow(&cell.body, by * s), radius + by * s, accent(a * dim));
                        }
                    }
                    fill(&cell.body, radius, white(if selected_row == Some(r) { 0.14 } else { 0.07 } * dim));
                    if drop_cell == Some(cell.id.as_str()) {
                        stroke(&cell.body, radius, 2.5 * s, accent(1.0));
                    } else if is_selected {
                        stroke(&cell.body, radius, 2.0 * s, accent(dim));
                    } else if hovering {
                        stroke(&cell.body, radius, 1.0, white(0.35 * dim));
                    }
                    if cell.id == self.model.current {
                        // Where the user actually is, as opposed to what is selected.
                        let dot = rect(cell.body.left + 7.0 * s, cell.body.top + 7.0 * s, 6.0 * s, 6.0 * s);
                        fill(&dot, 3.0 * s, accent(dim));
                    }
                    for badge in &cell.apps {
                        match self.icons.get(&badge.hwnd.0) {
                            Some(bitmap) => t.DrawBitmap(bitmap, Some(&badge.icon), dim, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None),
                            None => fill(&badge.icon, 4.0 * s, white(0.25 * dim)),
                        }
                        if badge.count > 1 {
                            text(&badge.count.to_string(), &fonts.small, &badge.label, white(0.75 * dim));
                        }
                    }
                    if cell.more > 0 {
                        let area = Rect { top: cell.body.bottom - 18.0 * s, ..cell.body };
                        text(&format!("+{}", cell.more), &fonts.centered, &area, white(0.45 * dim));
                    }
                    if hovering && self.cell_count() > 1 && self.press.is_none() {
                        let hot = self.hover == Hit::CellClose(cell.id.clone());
                        fill(&cell.close, 4.0 * s, if hot { danger(0.90) } else { rgba(0.2, 0.21, 0.25, 1.0) });
                        text("✕", &fonts.centered, &cell.close, white(if hot { 1.0 } else { 0.75 }));
                    }
                }

                let hot = self.hover == Hit::Plus(r);
                if hot {
                    fill(&row.plus, radius, white(0.10));
                }
                text("+", &fonts.big, &row.plus, white(if hot { 0.95 } else { 0.22 }));
            }

            let hot = self.hover == Hit::AddRow;
            if hot {
                fill(&self.layout.add_row, radius, white(0.10));
            }
            text("+", &fonts.big, &self.layout.add_row, white(if hot { 0.95 } else { 0.22 }));

            // Drag feedback.
            if let Some(id) = dragged_cell {
                match self.slot_at(id, self.cursor.0, self.cursor.1) {
                    Some(Slot::Row { row, x, .. }) => {
                        let band = &self.layout.rows[row];
                        let body = band.cells.first().map_or(band.plus, |c| c.body);
                        fill(&rect(x - 2.0 * s, body.top, 4.0 * s, body.bottom - body.top), 2.0 * s, accent(1.0));
                    }
                    Some(Slot::NewRow { y, .. }) => {
                        let spine = &self.layout.spine;
                        fill(&rect(spine.left, y - 2.0 * s, spine.right - spine.left, 4.0 * s), 2.0 * s, accent(1.0));
                    }
                    None => {}
                }
                let (cx, cy) = self.cursor;
                stroke(&rect(cx - 40.0 * s, cy - 25.0 * s, 80.0 * s, 50.0 * s), radius, 2.0 * s, accent(0.9));
            }
            if let Some(hwnd) = self.dragged_window() {
                // A minimized window has no thumbnail to follow the cursor.
                if !self.thumbs.iter().any(|&(h, _, _)| h == hwnd) {
                    let (cx, cy) = self.cursor;
                    let area = rect(cx - 16.0 * s, cy - 16.0 * s, 32.0 * s, 32.0 * s);
                    if let Some(bitmap) = self.icons.get(&hwnd) {
                        t.DrawBitmap(bitmap, Some(&area), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None);
                    }
                }
            }
            Ok(None)
        }
    }

    /// Draws the board (without live thumbnails) into a bitmap and returns the
    /// BGRA pixels, for reviewing the layout without showing a window.
    pub fn render_to_pixels(&mut self, model: Model, grid: Grid, size: (i32, i32), scale: f32) -> Option<Vec<u8>> {
        self.size = size;
        self.scale = scale;
        self.selected = model.current.clone();
        self.model = model;
        self.nav = grid;
        let selected = self.selected.clone();
        self.nav.visit(&selected);
        self.relayout();
        self.fonts = make_fonts(scale).ok();
        self.map_fonts = make_fonts(scale * MAP_SCALE).ok();
        let target = self.ensure_canvas()?;
        self.load_icons(&target);
        unsafe {
            target.BeginDraw();
            let _ = self.draw(&target);
            let _ = target.EndDraw(None, None);
        }
        let canvas = self.canvas.as_ref()?;
        Some(unsafe { std::slice::from_raw_parts(canvas.bits as *const u8, (size.0 * size.1 * 4) as usize) }.to_vec())
    }
}

/// A memory bitmap with a Direct2D target bound to it.
struct Canvas {
    dc: HDC,
    bitmap: HBITMAP,
    stock: HGDIOBJ,
    bits: *mut std::ffi::c_void,
    size: (i32, i32),
    target: ID2D1RenderTarget,
}

impl Canvas {
    fn new(factory: &ID2D1Factory, size: (i32, i32)) -> Result<Canvas> {
        unsafe {
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
            let target = factory.CreateDCRenderTarget(&D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_IGNORE },
                dpiX: 96.0,
                dpiY: 96.0,
                usage: D2D1_RENDER_TARGET_USAGE_GDI_COMPATIBLE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            })?;
            let mut bits = std::ptr::null_mut();
            let bitmap = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
            let dc = CreateCompatibleDC(None);
            let stock = SelectObject(dc, bitmap);
            // From here on `Canvas::drop` releases the GDI objects.
            let base = target.cast::<ID2D1RenderTarget>();
            let bound = target.BindDC(dc, &RECT { left: 0, top: 0, right: size.0, bottom: size.1 });
            match (base, bound) {
                (Ok(target), Ok(())) => Ok(Canvas { dc, bitmap, stock, bits, size, target }),
                (Err(e), _) | (_, Err(e)) => {
                    SelectObject(dc, stock);
                    DeleteObject(bitmap);
                    DeleteDC(dc);
                    Err(e)
                }
            }
        }
    }
}

impl Drop for Canvas {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.stock);
            DeleteObject(self.bitmap);
            DeleteDC(self.dc);
        }
    }
}

fn make_fonts(scale: f32) -> Result<Fonts> {
    unsafe {
        let factory: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
        let make = |size: f32, weight: i32, alignment: DWRITE_TEXT_ALIGNMENT| -> Result<IDWriteTextFormat> {
            let format = factory.CreateTextFormat(
                w!("Segoe UI"),
                None,
                DWRITE_FONT_WEIGHT(weight),
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                size * scale,
                w!("ko-kr"),
            )?;
            format.SetTextAlignment(alignment)?;
            format.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            format.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
            Ok(format)
        };
        Ok(Fonts {
            label: make(15.0, 600, DWRITE_TEXT_ALIGNMENT_LEADING)?,
            small: make(12.0, 400, DWRITE_TEXT_ALIGNMENT_LEADING)?,
            centered: make(12.0, 400, DWRITE_TEXT_ALIGNMENT_CENTER)?,
            big: make(22.0, 300, DWRITE_TEXT_ALIGNMENT_CENTER)?,
        })
    }
}

/// The window's icon as a Direct2D bitmap of `size` pixels.
fn icon_bitmap(target: &ID2D1RenderTarget, hwnd: HWND, size: i32) -> Option<ID2D1Bitmap> {
    unsafe {
        let mut result = 0usize;
        SendMessageTimeoutW(hwnd, WM_GETICON, WPARAM(ICON_BIG as usize), LPARAM(0), SMTO_ABORTIFHUNG, 40, Some(&mut result));
        if result == 0 {
            result = GetClassLongPtrW(hwnd, GCLP_HICON);
        }
        if result == 0 {
            return None;
        }
        let dc = CreateCompatibleDC(None);
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size,
                biHeight: -size,
                biPlanes: 1,
                biBitCount: 32,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let Ok(bitmap) = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0) else {
            DeleteDC(dc);
            return None;
        };
        let stock = SelectObject(dc, bitmap);
        let drawn = DrawIconEx(dc, 0, 0, HICON(result as isize), size, size, 0, None, DI_NORMAL).is_ok();
        let pixels = std::slice::from_raw_parts_mut(bits as *mut u8, (size * size * 4) as usize);
        // Icons without an alpha channel leave alpha at zero; make what they drew opaque.
        if pixels.chunks_exact(4).all(|p| p[3] == 0) {
            for p in pixels.chunks_exact_mut(4).filter(|p| p[0] | p[1] | p[2] != 0) {
                p[3] = 255;
            }
        }
        let made = drawn
            .then(|| {
                target.CreateBitmap(
                    D2D_SIZE_U { width: size as u32, height: size as u32 },
                    Some(bits as *const _),
                    (size * 4) as u32,
                    &D2D1_BITMAP_PROPERTIES {
                        pixelFormat: D2D1_PIXEL_FORMAT {
                            format: DXGI_FORMAT_B8G8R8A8_UNORM,
                            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                        },
                        dpiX: 96.0,
                        dpiY: 96.0,
                    },
                )
            })
            .and_then(|bitmap| bitmap.ok());
        SelectObject(dc, stock);
        DeleteObject(bitmap);
        DeleteDC(dc);
        made
    }
}

/// Lays out the windows of one cell as rows of previews that keep each
/// window's proportions, as large as fits (like Task View).
fn layout_tiles(windows: &[(&WindowModel, bool)], area: &Rect, s: f32) -> Vec<TileLayout> {
    if windows.is_empty() {
        return Vec::new();
    }
    let aspects: Vec<f32> = windows
        .iter()
        .map(|(w, _)| match w.rect {
            // Not narrower than the title bar needs for the pin toggles.
            Some(r) => ((r.right - r.left).max(1) as f32 / (r.bottom - r.top).max(1) as f32).clamp(0.9, 2.4),
            None => 1.6,
        })
        .collect();
    let (aw, ah) = (area.right - area.left, area.bottom - area.top);
    let (title_h, gap) = (34.0 * s, 26.0 * s);
    let total: f32 = aspects.iter().sum();

    // Try 1..4 rows and keep the split that gives the tallest previews.
    let mut best: (f32, Vec<Vec<usize>>) = (0.0, Vec::new());
    for rows in 1..=windows.len().min(4) {
        let target = total / rows as f32;
        let mut split: Vec<Vec<usize>> = vec![Vec::new()];
        let mut filled = 0.0;
        for (i, aspect) in aspects.iter().enumerate() {
            if filled + aspect / 2.0 > target && split.len() < rows && !split[split.len() - 1].is_empty() {
                split.push(Vec::new());
                filled = 0.0;
            }
            let last = split.len() - 1;
            split[last].push(i);
            filled += aspect;
        }
        let count = split.len() as f32;
        let by_height = (ah - gap * (count - 1.0)) / count - title_h;
        let by_width = split
            .iter()
            .map(|row| (aw - gap * (row.len() as f32 - 1.0)) / row.iter().map(|i| aspects[*i]).sum::<f32>())
            .fold(f32::INFINITY, f32::min);
        let height = by_height.min(by_width).min(430.0 * s);
        if height > best.0 {
            best = (height, split);
        }
    }
    let (height, split) = best;
    if height <= 8.0 {
        return Vec::new();
    }
    let block = split.len() as f32 * (title_h + height) + (split.len() as f32 - 1.0) * gap;
    let mut y = area.top + (ah - block) / 2.0;
    let icon = 18.0 * s;
    let mut tiles = Vec::new();
    for row in &split {
        let width: f32 = row.iter().map(|i| aspects[*i] * height).sum::<f32>() + gap * (row.len() as f32 - 1.0);
        let mut x = area.left + (aw - width) / 2.0;
        for &i in row {
            let w = aspects[i] * height;
            let (pin_w, all_w, pin_h) = (50.0 * s, 62.0 * s, 22.0 * s);
            let pin_y = y + (title_h - pin_h) / 2.0;
            let pin_all = rect(x + w - 8.0 * s - all_w, pin_y, all_w, pin_h);
            let pin_row = rect(pin_all.left - 6.0 * s - pin_w, pin_y, pin_w, pin_h);
            tiles.push(TileLayout {
                hwnd: windows[i].0.hwnd,
                frame: rect(x, y, w, title_h + height),
                icon: rect(x + 10.0 * s, y + (title_h - icon) / 2.0, icon, icon),
                title: rect(x + 18.0 * s + icon, y, (pin_row.left - 8.0 * s - (x + 18.0 * s + icon)).max(0.0), title_h),
                thumb: rect(x, y + title_h, w, height),
                pin_row,
                pin_all,
            });
            x += w + gap;
        }
        y += title_h + height + gap;
    }
    tiles
}

/// Lays out the map at the bottom of the board: rows top to bottom, each
/// shifted sideways so that its landing cell (`anchors[row]`) sits on a common
/// column, the spine. `size` is the space above the footer; `s` the scale.
fn compute_layout(model: &Model, anchors: &[usize], size: (f32, f32), s: f32) -> Layout {
    let (w, h) = size;
    let (margin, header_w, gap) = (60.0 * s, 170.0 * s, 10.0 * s);
    /// Share of the board's height the map may take.
    const MAX_SHARE: f32 = 0.34;

    let anchor = |r: usize| anchors.get(r).copied().unwrap_or(0);
    let rows = model.rows.len().max(1) as f32;
    let lead = (0..model.rows.len()).map(anchor).max().unwrap_or(0);
    let tail = model.rows.iter().enumerate().map(|(r, row)| row.cells.len().saturating_sub(anchor(r))).max().unwrap_or(1).max(1);
    let columns = (lead + tail) as f32;

    // Squares of a fixed look, shrunk only when the grid would not fit.
    let fit_w = (w - 2.0 * margin - header_w) / (columns + 0.6) - gap;
    let fit_h = (h * MAX_SHARE) / (rows + 0.6) - gap;
    let cell_h = (62.0 * s).min(fit_h).min(fit_w * 0.62).max(22.0 * s);
    let cell_w = cell_h / 0.62;
    let pitch = cell_w + gap;
    let row_pitch = cell_h + gap;
    let plus = cell_h * 0.6;

    let total_w = header_w + columns * pitch + plus;
    let total_h = rows * row_pitch + plus;
    let x0 = ((w - total_w) / 2.0).max(margin);
    let mut y = h - total_h - 10.0 * s;
    let cells_x = x0 + header_w;
    let spine_x = cells_x + lead as f32 * pitch;
    let icon = (cell_h * 0.36).min(22.0 * s);
    let count_w = icon * 0.62;

    let mut layout = Layout { map_top: y, ..Layout::default() };
    let rows_top = y;
    for (r, row) in model.rows.iter().enumerate() {
        let mut x = spine_x - anchor(r) as f32 * pitch;
        let mut cells = Vec::new();
        for cell in &row.cells {
            let body = rect(x, y, cell_w, cell_h);
            // One badge per app, in the order its windows are stacked.
            let mut apps: Vec<(HWND, &str, usize)> = Vec::new();
            for window in &cell.windows {
                match apps.iter_mut().find(|(_, app, _)| *app == window.app.as_str() && !window.app.is_empty()) {
                    Some(entry) => entry.2 += 1,
                    None => apps.push((window.hwnd, window.app.as_str(), 1)),
                }
            }
            let width = |count: usize| icon + if count > 1 { count_w } else { 0.0 };
            let room = cell_w - 16.0 * s;
            let between = 5.0 * s;
            let mut shown = 0;
            let mut used = 0.0;
            for (_, _, count) in &apps {
                let next = used + width(*count) + if shown > 0 { between } else { 0.0 };
                if next > room {
                    break;
                }
                used = next;
                shown += 1;
            }
            let mut bx = body.left + (cell_w - used) / 2.0;
            let by = body.top + (cell_h - icon) / 2.0;
            let badges = apps[..shown]
                .iter()
                .map(|&(hwnd, _, count)| {
                    let badge = AppBadge { hwnd, icon: rect(bx, by, icon, icon), count, label: rect(bx + icon + 1.0 * s, by, count_w, icon) };
                    bx += width(count) + between;
                    badge
                })
                .collect();
            cells.push(CellLayout {
                id: cell.id.clone(),
                body,
                close: rect(body.right - 15.0 * s, body.top - 5.0 * s, 20.0 * s, 20.0 * s),
                apps: badges,
                more: apps.len() - shown,
            });
            x += pitch;
        }
        let first_x = spine_x - anchor(r) as f32 * pitch;
        layout.rows.push(RowLayout {
            header: rect(x0, y, header_w, cell_h),
            name: rect(x0, y + cell_h / 2.0 - 20.0 * s, (first_x - x0 - 12.0 * s).min(header_w - 12.0 * s).max(40.0 * s), 20.0 * s),
            close: rect(x0, y + cell_h / 2.0 + 2.0 * s, 140.0 * s, 20.0 * s),
            top: y - gap / 2.0,
            bottom: y + cell_h + gap / 2.0,
            cells,
            plus: rect(x, y + (cell_h - plus) / 2.0, plus, plus),
        });
        y += row_pitch;
    }
    layout.spine = rect(spine_x - gap / 2.0, rows_top - gap / 2.0, pitch, (y - rows_top).max(0.0));
    layout.add_row = rect(spine_x + (cell_w - plus) / 2.0, y - gap / 2.0 + 2.0 * s, plus, plus);
    layout
}
