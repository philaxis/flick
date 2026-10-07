//! The full-screen grid view: every workspace row with a live miniature of
//! each desktop, where windows and cells can be dragged around.
//!
//! The board owns layout, hit-testing, drawing and the DWM thumbnails. It does
//! not touch desktops itself; input is turned into an `Action` for the app.

use crate::grid::{CellId, Dir, Grid};
use std::collections::HashMap;
use windows::{
    core::{w, ComInterface, Result},
    Foundation::Numerics::Matrix3x2,
    Win32::{
        Foundation::{BOOL, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
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
                DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TRIMMING, DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                DWRITE_WORD_WRAPPING_NO_WRAP,
            },
            Dwm::{
                DwmQueryThumbnailSourceSize, DwmRegisterThumbnail, DwmUnregisterThumbnail,
                DwmUpdateThumbnailProperties, DWM_THUMBNAIL_PROPERTIES, DWM_TNP_OPACITY, DWM_TNP_RECTDESTINATION,
                DWM_TNP_RECTSOURCE, DWM_TNP_SOURCECLIENTAREAONLY, DWM_TNP_VISIBLE,
            },
            Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM,
            Gdi::{
                BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, EnumDisplayMonitors, GetDC,
                GetMonitorInfoW, HMONITOR,
                InvalidateRect, MonitorFromPoint, ReleaseDC, SelectObject, ValidateRect, BITMAPINFO, BITMAPINFOHEADER,
                DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ, MONITORINFO, MONITOR_DEFAULTTOPRIMARY, SRCCOPY,
            },
        },
        System::LibraryLoader::GetModuleHandleW,
        UI::{
            HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI},
            Input::KeyboardAndMouse::{
                ReleaseCapture, SetCapture, VK_BACK, VK_DOWN, VK_ESCAPE, VK_F2, VK_LEFT, VK_RETURN, VK_RIGHT, VK_UP,
            },
            WindowsAndMessaging::{
                CreateWindowExW, DrawIconEx, GetClassLongPtrW, LoadCursorW, RegisterClassW, IDC_ARROW, SendMessageTimeoutW,
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
    /// Screen rectangle; for a minimized window, where it will be restored to.
    pub rect: RECT,
    pub minimized: bool,
    pub title: String,
    /// Executable name; windows of one app are counted together in the map.
    pub app: String,
    /// The app's name as people know it; may be empty.
    pub app_name: String,
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
    /// Move a window to another cell, another monitor (given in screen
    /// coordinates), or both.
    MoveWindow { window: HWND, cell: Option<CellId>, monitor: Option<RECT> },
    MoveCell { id: CellId, row: usize, index: usize },
    MoveCellToNewRow { id: CellId, at: usize },
    /// Add a desktop to a row, at its front or its end.
    AddCell { row: usize, front: bool },
    /// Add a row with one desktop, above the first or below the last.
    AddRow { top: bool },
    RemoveCell(CellId),
    /// Ask a window to close.
    CloseWindow(HWND),
    /// Set how one window is pinned: following within its row, shown on
    /// every desktop, or neither (never both).
    SetPin { window: HWND, row: bool, all: bool },
    /// Show the menu of a workspace (close its windows, remove it).
    RowMenu(usize),
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
    top: f32,
    bottom: f32,
    cells: Vec<CellLayout>,
    /// The "add a cell" buttons after and before the row.
    plus: Rect,
    plus_left: Rect,
}

/// One window of the selected cell, shown large with its title.
struct TileLayout {
    hwnd: HWND,
    frame: Rect,
    icon: Rect,
    title: Rect,
    thumb: Rect,
    /// The pin button (cycles off / workspace / everywhere) and the close
    /// button at the right of the title bar.
    pin: Rect,
    close: Rect,
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
    /// The "add a row" buttons below and above the rows.
    add_row: Rect,
    add_row_top: Rect,
}

#[derive(Clone, PartialEq, Default)]
enum Hit {
    #[default]
    Nothing,
    /// `cell` is empty for a window pinned to every desktop.
    Window { cell: CellId, hwnd: isize },
    /// The cell's title bar or its bare background; the handle for dragging it.
    Cell(CellId),
    /// A tile's pin button and its close button.
    Pin(isize),
    TileClose(isize),
    CellClose(CellId),
    Plus(usize),
    PlusLeft(usize),
    AddRow,
    AddRowTop,
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
    /// A tile's window title and, above it, the smaller app name.
    title: IDWriteTextFormat,
    sub: IDWriteTextFormat,
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
    /// The map again on every other monitor: (monitor index, its layout).
    other_maps: Vec<(usize, Layout)>,
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
    /// A confirmation box owned by the board is up.
    modal: bool,
    /// Every monitor, in the board window's coordinates. The board spans
    /// them all; `home` (the one the cursor was on) also holds the map.
    monitors: Vec<Rect>,
    home: Rect,
    /// Screen position of the board window's top-left corner.
    origin: (i32, i32),
    editing: Option<(usize, String)>,
}

impl Board {
    pub fn new(wndproc: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT) -> Result<Board> {
        unsafe {
            let instance = GetModuleHandleW(None)?;
            let class = w!("flick.board");
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
                w!("Flick"),
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
                other_maps: Vec::new(),
                size: (0, 0),
                scale: 1.0,
                thumbs: Vec::new(),
                icons: HashMap::new(),
                hover: Hit::Nothing,
                press: None,
                cursor: (0.0, 0.0),
                cursor_before: (0.0, 0.0),
                modal: false,
                monitors: Vec::new(),
                home: Rect::default(),
                origin: (0, 0),
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

    /// A workspace's label for its menu: name, memory estimate, window count.
    pub fn row_summary(&self, row: usize) -> Option<(String, String)> {
        let model = self.model.rows.get(row)?;
        let name = if model.name.is_empty() { format!("워크스페이스 {}", row + 1) } else { model.name.clone() };
        let windows: usize = model.cells.iter().map(|c| c.windows.len()).sum();
        let asleep = if model.asleep { ", 재움" } else { "" };
        Some((name, format!("창 {windows}개, {}{asleep}", format_memory(model.memory))))
    }

    pub fn set_modal(&mut self, modal: bool) {
        self.modal = modal;
    }

    pub fn selected(&self) -> CellId {
        self.selected.clone()
    }

    /// Covers the monitor under the cursor and shows `model`.
    pub fn open(&mut self, model: Model, grid: Grid) {
        unsafe extern "system" fn collect(_: HMONITOR, _: HDC, area: *mut RECT, out: LPARAM) -> BOOL {
            (*(out.0 as *mut Vec<RECT>)).push(*area);
            true.into()
        }
        unsafe {
            // The primary monitor is "home" (it carries the label and sets the
            // scale), so the board looks the same wherever the cursor is.
            let monitor = MonitorFromPoint(POINT::default(), MONITOR_DEFAULTTOPRIMARY);
            let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            if !GetMonitorInfoW(monitor, &mut info).as_bool() {
                return;
            }
            let (mut dpi, mut dpi_y) = (96u32, 96u32);
            let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi, &mut dpi_y);
            let mut screens: Vec<RECT> = Vec::new();
            EnumDisplayMonitors(None, None, Some(collect), LPARAM(&mut screens as *mut _ as isize));
            if screens.is_empty() {
                screens.push(info.rcMonitor);
            }
            // One window over every monitor, so no screen is left showing
            // the desktop while the board is up.
            let left = screens.iter().map(|r| r.left).min().unwrap_or(0);
            let top = screens.iter().map(|r| r.top).min().unwrap_or(0);
            let right = screens.iter().map(|r| r.right).max().unwrap_or(0);
            let bottom = screens.iter().map(|r| r.bottom).max().unwrap_or(0);
            let local = |r: &RECT| Rect {
                left: (r.left - left) as f32,
                top: (r.top - top) as f32,
                right: (r.right - left) as f32,
                bottom: (r.bottom - top) as f32,
            };
            self.monitors = screens.iter().map(local).collect();
            self.home = local(&info.rcMonitor);
            let size = (right - left, bottom - top);
            if self.scale != dpi as f32 / 96.0 {
                self.fonts = None;
                self.map_fonts = None;
            }
            self.scale = dpi as f32 / 96.0;
            self.size = size;
            self.origin = (left, top);
            self.selected = model.current.clone();
            self.hover = Hit::Nothing;
            self.press = None;
            self.editing = None;
            self.open = true;
            let _ = SetWindowPos(self.hwnd, HWND_TOPMOST, left, top, size.0, size.1, SWP_SHOWWINDOW);
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
    /// The windows shown as tiles: those of the selected cell and the ones
    /// pinned to every desktop (flagged), which are visible there as well.
    /// Their order depends only on what the windows are (app, then the
    /// window's handle), never on which was used last, so the same windows
    /// always appear in the same places.
    fn tile_windows(&self) -> Vec<(&WindowModel, bool)> {
        let cell = self.selected_cell().into_iter().flat_map(|cell| &cell.windows).map(|w| (w, false));
        let mut windows: Vec<(&WindowModel, bool)> = cell.chain(self.model.pinned.iter().map(|w| (w, true))).collect();
        windows.sort_by_cached_key(|(w, _)| (w.app_name.to_lowercase(), w.app.clone(), w.hwnd.0));
        windows
    }

    /// Index of the monitor a window is on (by its centre), the home monitor
    /// when it is off every screen.
    fn monitor_of(&self, window: &WindowModel) -> usize {
        let r = window.rect;
        let x = ((r.left + r.right) / 2 - self.origin.0) as f32;
        let y = ((r.top + r.bottom) / 2 - self.origin.1) as f32;
        let home = self.monitors.iter().position(|m| *m == self.home).unwrap_or(0);
        self.monitors.iter().position(|m| contains(m, x, y)).unwrap_or(home)
    }

    fn relayout(&mut self) {
        let s = self.scale;
        let (w, h) = (self.home.right - self.home.left, self.home.bottom - self.home.top);
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
        // The map and the label are laid out inside the home monitor (and
        // drawn shifted to it); the tiles are placed in window coordinates,
        // each on the monitor its window is on.
        let mut layout = compute_layout(&self.model, &anchors, (w, h - FOOTER * s), s * MAP_SCALE);
        let margin = 40.0 * s;
        layout.tiles_label = rect(margin, 22.0 * s, w - 2.0 * margin, 30.0 * s);
        let windows = self.tile_windows();
        let mut tiles = Vec::new();
        let mut other_maps = Vec::new();
        for (m, monitor) in self.monitors.iter().enumerate() {
            let is_home = *monitor == self.home;
            // Every monitor has the map at its bottom, so a window can be
            // dropped on a cell on the very monitor it should appear on.
            let map_top = if is_home {
                layout.map_top
            } else {
                let size = (monitor.right - monitor.left, monitor.bottom - monitor.top - FOOTER * s);
                let map = compute_layout(&self.model, &anchors, size, s * MAP_SCALE);
                let top = map.map_top;
                other_maps.push((m, map));
                top
            };
            let area = Rect {
                left: monitor.left + margin,
                top: monitor.top + if is_home { 64.0 * s } else { margin },
                right: monitor.right - margin,
                bottom: monitor.top + map_top - 26.0 * s,
            };
            if is_home {
                layout.tiles_area = area;
            }
            let here: Vec<(&WindowModel, bool)> = windows.iter().filter(|(w, _)| self.monitor_of(w) == m).copied().collect();
            tiles.extend(layout_tiles(&here, &area, s));
        }
        layout.tiles = tiles;
        self.layout = layout;
        self.other_maps = other_maps;
    }

    /// The map on each monitor: (monitor index, monitor rectangle, layout in
    /// that monitor's own coordinates), the home monitor first.
    fn maps(&self) -> Vec<(usize, Rect, &Layout)> {
        let home = self.monitors.iter().position(|m| *m == self.home).unwrap_or(0);
        let mut maps = vec![(home, self.home, &self.layout)];
        maps.extend(self.other_maps.iter().filter_map(|(m, map)| Some((*m, *self.monitors.get(*m)?, map))));
        maps
    }

    /// The map of the monitor the point is on (the home one otherwise), with
    /// the point in that monitor's coordinates.
    fn map_at(&self, x: f32, y: f32) -> (usize, &Layout, f32, f32) {
        let maps = self.maps();
        let (m, monitor, map) = maps.iter().find(|(_, monitor, _)| contains(monitor, x, y)).copied().unwrap_or(maps[0]);
        (m, map, x - monitor.left, y - monitor.top)
    }

    /// A tile acts like its window in the map; one pinned everywhere belongs
    /// to no cell.
    fn tile_hit(&self, hwnd: HWND) -> Hit {
        let pinned = self.model.pinned.iter().any(|w| w.hwnd == hwnd);
        Hit::Window { cell: if pinned { String::new() } else { self.selected.clone() }, hwnd: hwnd.0 }
    }

    fn hit_test(&self, x: f32, y: f32) -> Hit {
        if let Some(tile) = self.layout.tiles.iter().find(|tile| contains(&tile.frame, x, y)) {
            if contains(&tile.pin, x, y) {
                return Hit::Pin(tile.hwnd.0);
            }
            if contains(&tile.close, x, y) {
                return Hit::TileClose(tile.hwnd.0);
            }
            return self.tile_hit(tile.hwnd);
        }
        let (_, map, x, y) = self.map_at(x, y);
        if contains(&map.add_row, x, y) {
            return Hit::AddRow;
        }
        if contains(&map.add_row_top, x, y) {
            return Hit::AddRowTop;
        }
        for (r, row) in map.rows.iter().enumerate() {
            if contains(&row.plus_left, x, y) {
                return Hit::PlusLeft(r);
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
        self.model.rows.iter().map(|row| row.cells.len()).sum()
    }

    /// The map cell under a point, and the monitor whose map it is on.
    fn cell_at(&self, x: f32, y: f32) -> Option<(&CellLayout, usize)> {
        // A little slack around each square makes it an easier drop target.
        let slack = 5.0 * self.scale;
        let (m, map, x, y) = self.map_at(x, y);
        let cell = map.rows.iter().flat_map(|row| &row.cells).find(|cell| contains(&grow(&cell.body, slack), x, y))?;
        Some((cell, m))
    }

    fn slot_at(&self, dragged: &str, x: f32, y: f32) -> Option<Slot> {
        let (_, map, x, y) = self.map_at(x, y);
        let rows = &map.rows;
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
        let wanted: Vec<HWND> = self.tile_windows().into_iter().map(|(w, _)| w.hwnd).collect();
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
                Some(tile) => {
                    // Fill the tile's width without distorting the picture:
                    // a window taller than the tile shows its upper part, a
                    // shorter one sits at the top.
                    let t = &tile.thumb;
                    let (tw, th) = (t.right - t.left, t.bottom - t.top);
                    let mut dest = *t;
                    if let Ok(size) = unsafe { DwmQueryThumbnailSourceSize(thumb) } {
                        let (sw, sh) = (size.cx.max(1) as f32, size.cy.max(1) as f32);
                        let shown = th * sw / tw;
                        if shown < sh {
                            props.dwFlags |= DWM_TNP_RECTSOURCE;
                            props.rcSource = RECT { left: 0, top: 0, right: size.cx, bottom: shown.round() as i32 };
                        } else {
                            dest.bottom = t.top + sh * tw / sw;
                        }
                    }
                    props.rcDestination = to_rect(&dest);
                    // A minimized window shows what it last looked like, faded.
                    // (Without a kept picture the source is only a sliver.)
                    if self.tile_windows().iter().any(|(w, _)| w.hwnd.0 == hwnd && w.minimized) {
                        props.opacity = 150;
                        let kept = unsafe { DwmQueryThumbnailSourceSize(thumb) }.is_ok_and(|size| size.cy >= 80);
                        props.fVisible = kept.into();
                    }
                }
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
            // Losing focus closes the board, except to its own confirmation box.
            WM_ACTIVATE if wparam.0 & 0xFFFF == 0 && !self.modal => Action::Dismiss,
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
                let (_, map, lx, ly) = self.map_at(x, y);
                match map.rows.iter().position(|row| contains(&row.header, lx, ly)) {
                    Some(row) => Action::RowMenu(row),
                    None => Action::None,
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
        let over_header = |px: f32, py: f32| {
            let (m, map, px, py) = self.map_at(px, py);
            map.rows.iter().position(|row| contains(&row.header, px, py)).map(|row| (m, row))
        };
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
                Hit::Window { cell, hwnd } => {
                    // Dropped on a map cell: that cell, on the monitor whose
                    // map it was. Dropped elsewhere: just that monitor.
                    let from = self.tile_windows().iter().find(|(w, _)| w.hwnd.0 == *hwnd).map(|(w, _)| self.monitor_of(w));
                    let (to_cell, to_monitor) = match self.cell_at(x, y) {
                        Some((target, m)) => (Some(target.id.clone()), m),
                        None => (None, self.map_at(x, y).0),
                    };
                    let cell = to_cell.filter(|id| id != cell);
                    let monitor = (Some(to_monitor) != from).then(|| self.monitors.get(to_monitor)).flatten().map(|m| RECT {
                        left: m.left as i32 + self.origin.0,
                        top: m.top as i32 + self.origin.1,
                        right: m.right as i32 + self.origin.0,
                        bottom: m.bottom as i32 + self.origin.1,
                    });
                    if cell.is_none() && monitor.is_none() {
                        Action::None
                    } else {
                        Action::MoveWindow { window: HWND(*hwnd), cell, monitor }
                    }
                }
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
            Hit::Pin(hwnd) => {
                let windows = self.tile_windows();
                let Some((window, everywhere)) = windows.iter().find(|(w, _)| w.hwnd.0 == hwnd) else { return Action::None };
                // Cycles: off -> within the workspace -> everywhere -> off.
                let (row, all) = match (window.follows, *everywhere) {
                    (false, false) => (true, false),
                    (true, false) => (false, true),
                    _ => (false, false),
                };
                Action::SetPin { window: HWND(hwnd), row, all }
            }
            Hit::TileClose(hwnd) => Action::CloseWindow(HWND(hwnd)),
            Hit::CellClose(id) => Action::RemoveCell(id),
            Hit::Plus(row) => Action::AddCell { row, front: false },
            Hit::PlusLeft(row) => Action::AddCell { row, front: true },
            Hit::AddRow => Action::AddRow { top: false },
            Hit::AddRowTop => Action::AddRow { top: true },
            Hit::RowName(_) => Action::None,
            // A click on nothing dismisses the board, like Task View.
            Hit::Nothing => Action::Cancel,
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
        // Everything from here on belongs to the home monitor.
        let mut hovered = Ok(None);
        for (_, monitor, map) in self.maps() {
            unsafe {
                t.SetTransform(&Matrix3x2::translation(monitor.left, monitor.top));
            }
            let drawn = self.draw_map(t, map_fonts, s * MAP_SCALE, map, &monitor);
            if hovered.is_ok() {
                hovered = drawn;
            }
        }
        // The label and the hint belong to the home monitor.
        unsafe {
            t.SetTransform(&Matrix3x2::translation(self.home.left, self.home.top));
        }
        let footer = self.draw_footer(t, fonts, s, hovered.as_ref().ok().cloned().flatten());
        unsafe {
            t.SetTransform(&Matrix3x2::identity());
        }
        hovered.and(footer)
    }

    /// The cell label at the top and the hint line at the bottom of the home monitor.
    fn draw_footer(&self, t: &ID2D1RenderTarget, fonts: &Fonts, s: f32, hovered: Option<String>) -> Result<()> {
        unsafe {
            let brush = t.CreateSolidColorBrush(&white(1.0), None)?;
            let (w, h) = (self.home.right - self.home.left, self.home.bottom - self.home.top);
            let place = self.nav.find(&self.selected);
            if let Some(p) = place {
                let name = self.model.rows.get(p.row).map(|row| row.name.clone()).unwrap_or_default();
                let row = if name.is_empty() { format!("워크스페이스 {}", p.row + 1) } else { name };
                let here = if self.selected == self.model.current { "   ● 현재" } else { "" };
                let wide: Vec<u16> = format!("{row}  ·  {}번 칸{here}", p.col + 1).encode_utf16().collect();
                brush.SetColor(&white(0.80));
                t.DrawText(&wide, &fonts.label, &self.layout.tiles_label, &brush, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL);
            }
            let footer = rect(0.0, h - (FOOTER - 8.0) * s, w, 26.0 * s);
            let pointed = match &self.hover {
                Hit::Window { hwnd, .. } | Hit::Pin(hwnd) | Hit::TileClose(hwnd) => Some(*hwnd),
                _ => None,
            };
            // The full title of the tile under the cursor, in case it was shortened.
            let hovered = hovered.or_else(|| {
                let windows = self.tile_windows();
                let (window, _) = windows.iter().find(|(w, _)| Some(w.hwnd.0) == pointed)?;
                Some(if window.app_name.is_empty() { window.title.clone() } else { format!("{}  —  {}", window.app_name, window.title) })
            });
            let (line, alpha) = match &hovered {
                Some(title) => (title.as_str(), 0.85),
                None => ("창을 아래 칸으로 끌면 옮겨집니다   ·   칸을 끌면 재배치됩니다", 0.22),
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
                if window.minimized {
                    let note = Rect { top: tile.thumb.bottom - 26.0 * s, ..tile.thumb };
                    text("최소화됨", &fonts.centered, &note, white(0.40 * dim));
                }
                // Two lines: the app, then what this window of it is about.
                let width = tile.title.right - tile.title.left;
                let title = display_title(&window.title, &window.app_name, (width / (7.4 * s)) as usize);
                let bright = white(if hot { 1.0 } else { 0.88 } * dim);
                if window.app_name.is_empty() || title.is_empty() || title.eq_ignore_ascii_case(&window.app_name) {
                    let only = if title.is_empty() { &window.app_name } else { &title };
                    text(only, &fonts.title, &tile.title, bright);
                } else {
                    let middle = (tile.title.top + tile.title.bottom) / 2.0;
                    let upper = Rect { top: tile.title.top + 5.0 * s, bottom: middle, ..tile.title };
                    let lower = Rect { top: middle - 2.0 * s, bottom: tile.title.bottom - 4.0 * s, ..tile.title };
                    text(&window.app_name, &fonts.sub, &upper, white(0.50 * dim));
                    text(&title, &fonts.title, &lower, bright);
                }

                // One pin button cycling through its three states.
                let over = self.hover == Hit::Pin(tile.hwnd.0);
                let (label, colour) = match (window.follows, *everywhere) {
                    (_, true) => ("전체 고정", rgba(0.62, 0.45, 1.0, dim)),
                    (true, _) => ("워크스페이스 고정", accent(dim)),
                    _ => ("고정", white(if over { 0.20 } else { 0.07 } * dim)),
                };
                brush.SetColor(&colour);
                t.FillRoundedRectangle(&rounded(&tile.pin, 5.0 * s), &brush);
                let on = window.follows || *everywhere;
                text(label, &fonts.centered, &tile.pin, white(if on || over { 1.0 } else { 0.50 } * dim));

                let over = self.hover == Hit::TileClose(tile.hwnd.0);
                if over {
                    brush.SetColor(&danger(0.90));
                    t.FillRoundedRectangle(&rounded(&tile.close, 5.0 * s), &brush);
                }
                text("✕", &fonts.centered, &tile.close, white(if over { 1.0 } else { 0.55 } * dim));

                brush.SetColor(&if hot { accent(1.0) } else { white(0.10 * dim) });
                t.DrawRoundedRectangle(&rounded(&grow(&tile.frame, 1.0 * s), 10.0 * s), &brush, if hot { 2.5 * s } else { 1.0 }, None);
            }
        }
        Ok(())
    }

    /// The map of all rows and cells: plain squares holding each cell's apps
    /// (icon and count), with the controls appearing on hover.
    fn draw_map(&self, t: &ID2D1RenderTarget, fonts: &Fonts, s: f32, map: &Layout, monitor: &Rect) -> Result<Option<String>> {
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
            let drop_cell = self.dragged_window().and_then(|_| self.cell_at(self.cursor.0, self.cursor.1)).map(|(c, _)| c.id.as_str());
            // Drag feedback is drawn only on the monitor the cursor is on.
            let here = contains(monitor, self.cursor.0, self.cursor.1);
            let selected_row = self.nav.find(&self.selected).map(|pos| pos.row);
            let radius = 8.0 * s;
            // The map is drawn shifted onto its monitor.
            let cursor = (self.cursor.0 - monitor.left, self.cursor.1 - monitor.top);

            // The column vertical moves travel along.
            fill(&map.spine, 10.0 * s, white(0.035));

            for (r, (row, model)) in map.rows.iter().zip(&self.model.rows).enumerate() {
                // Row label, in a box as wide as its text (right-click: menu).
                let editing = self.editing.as_ref().filter(|(row, _)| *row == r);
                let name = match editing {
                    Some((_, typed)) => format!("{typed}▏"),
                    None if model.name.is_empty() => format!("워크스페이스 {}", r + 1),
                    None => model.name.clone(),
                };
                let hot = self.hover == Hit::RowName(r) && self.press.is_none();
                if editing.is_some() || hot {
                    // While typing the box follows the text being typed.
                    let width = text_width(&name, 12.0 * s) + 4.0 * s;
                    let area = Rect { right: row.name.left + width.max(row.name.right - row.name.left), ..row.name };
                    fill(&grow(&area, 4.0 * s), 5.0 * s, white(if editing.is_some() { 0.12 } else { 0.06 }));
                }
                let wide = Rect { right: row.header.right, ..row.name };
                text(&name, &fonts.small, &wide, white(if selected_row == Some(r) || hot { 0.90 } else { 0.45 }));

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

                for (area, hit) in [(&row.plus, Hit::Plus(r)), (&row.plus_left, Hit::PlusLeft(r))] {
                    let hot = self.hover == hit;
                    if hot {
                        fill(area, radius, white(0.10));
                    }
                    text("+", &fonts.big, area, white(if hot { 0.95 } else { 0.22 }));
                }
            }

            for (area, hit) in [(&map.add_row, Hit::AddRow), (&map.add_row_top, Hit::AddRowTop)] {
                let hot = self.hover == hit;
                if hot {
                    fill(area, radius, white(0.10));
                }
                text("+", &fonts.big, area, white(if hot { 0.95 } else { 0.22 }));
            }

            // Drag feedback.
            if let Some(id) = dragged_cell.filter(|_| here) {
                match self.slot_at(id, self.cursor.0, self.cursor.1) {
                    Some(Slot::Row { row, x, .. }) => {
                        let band = &map.rows[row];
                        let body = band.cells.first().map_or(band.plus, |c| c.body);
                        fill(&rect(x - 2.0 * s, body.top, 4.0 * s, body.bottom - body.top), 2.0 * s, accent(1.0));
                    }
                    Some(Slot::NewRow { y, .. }) => {
                        let spine = &map.spine;
                        fill(&rect(spine.left, y - 2.0 * s, spine.right - spine.left, 4.0 * s), 2.0 * s, accent(1.0));
                    }
                    None => {}
                }
                let (cx, cy) = cursor;
                stroke(&rect(cx - 40.0 * s, cy - 25.0 * s, 80.0 * s, 50.0 * s), radius, 2.0 * s, accent(0.9));
            }
            if let Some(hwnd) = self.dragged_window().filter(|_| here) {
                // A minimized window has no thumbnail to follow the cursor.
                if !self.thumbs.iter().any(|&(h, _, _)| h == hwnd) {
                    let (cx, cy) = cursor;
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
        self.home = rect(0.0, 0.0, size.0 as f32, size.1 as f32);
        self.monitors = vec![self.home];
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
            // Text that does not fit ends in an ellipsis instead of being cut.
            let trimming = DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, delimiter: 0, delimiterCount: 0 };
            let sign = factory.CreateEllipsisTrimmingSign(&format)?;
            format.SetTrimming(&trimming, &sign)?;
            Ok(format)
        };
        Ok(Fonts {
            title: make(13.5, 600, DWRITE_TEXT_ALIGNMENT_LEADING)?,
            sub: make(11.0, 400, DWRITE_TEXT_ALIGNMENT_LEADING)?,
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

/// Rough width of a line of text at `size` pixels: enough to size a box
/// around a short label without laying the text out.
fn text_width(text: &str, size: f32) -> f32 {
    text.chars().map(|c| if c.is_ascii() { 0.56 } else { 1.0 }).sum::<f32>() * size
}

/// What to show as a window's title next to its app's name: the part that
/// tells this window from the app's others, shortened where it is long.
fn display_title(title: &str, app: &str, max_chars: usize) -> String {
    let mut title = title.trim();
    // "Document - App" repeats the app line; keep the document.
    if let Some((head, tail)) = title.rsplit_once(" - ") {
        let (tail, app) = (tail.trim().to_lowercase(), app.to_lowercase());
        if !head.trim().is_empty() && !app.is_empty() && (app.contains(&tail) || tail.contains(&app)) {
            title = head.trim();
        }
    }
    // The telling part of a path is its end, so cut paths at the front.
    let count = title.chars().count();
    let is_path = title.contains(":\\") || title.starts_with('/') || title.starts_with('~');
    if is_path && max_chars > 4 && count > max_chars {
        let tail: String = title.chars().skip(count - (max_chars - 1)).collect();
        return format!("…{tail}");
    }
    title.to_owned()
}

/// Lays out the windows of one cell as rows of previews that keep each
/// window's proportions, as large as fits (like Task View).
fn layout_tiles(windows: &[(&WindowModel, bool)], area: &Rect, s: f32) -> Vec<TileLayout> {
    if windows.is_empty() {
        return Vec::new();
    }
    let aspects: Vec<f32> = windows
        .iter()
        .map(|(w, _)| {
            // Not narrower than the title bar needs for its buttons.
            let r = w.rect;
            ((r.right - r.left).max(1) as f32 / (r.bottom - r.top).max(1) as f32).clamp(1.1, 2.4)
        })
        .collect();
    let (aw, ah) = (area.right - area.left, area.bottom - area.top);
    let (title_h, gap) = (46.0 * s, 26.0 * s);
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
    let icon = 24.0 * s;
    let mut tiles = Vec::new();
    for row in &split {
        let width: f32 = row.iter().map(|i| aspects[*i] * height).sum::<f32>() + gap * (row.len() as f32 - 1.0);
        let mut x = area.left + (aw - width) / 2.0;
        for &i in row {
            let w = aspects[i] * height;
            // The pin button is only as wide as its current label.
            let pin_w = match (windows[i].0.follows, windows[i].1) {
                (_, true) => 74.0,
                (true, _) => 118.0,
                _ => 46.0,
            } * s;
            let (close_w, pin_h) = (28.0 * s, 24.0 * s);
            let pin_y = y + (title_h - pin_h) / 2.0;
            let close = rect(x + w - 6.0 * s - close_w, pin_y, close_w, pin_h);
            let pin = rect(close.left - 4.0 * s - pin_w, pin_y, pin_w, pin_h);
            tiles.push(TileLayout {
                hwnd: windows[i].0.hwnd,
                frame: rect(x, y, w, title_h + height),
                icon: rect(x + 10.0 * s, y + (title_h - icon) / 2.0, icon, icon),
                title: rect(x + 18.0 * s + icon, y, (pin.left - 8.0 * s - (x + 18.0 * s + icon)).max(0.0), title_h),
                thumb: rect(x, y + title_h, w, height),
                pin,
                close,
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
    let fit_w = (w - 2.0 * margin - header_w) / (columns + 1.2) - gap;
    let fit_h = (h * MAX_SHARE) / (rows + 1.2) - gap;
    let cell_h = (62.0 * s).min(fit_h).min(fit_w * 0.62).max(22.0 * s);
    let cell_w = cell_h / 0.62;
    let pitch = cell_w + gap;
    let row_pitch = cell_h + gap;
    let plus = cell_h * 0.6;

    let total_w = header_w + plus + gap + columns * pitch + plus;
    let total_h = plus + gap + rows * row_pitch + plus;
    let x0 = ((w - total_w) / 2.0).max(margin);
    let mut y = h - total_h - 10.0 * s;
    let cells_x = x0 + header_w + plus + gap;
    let spine_x = cells_x + lead as f32 * pitch;
    let icon = (cell_h * 0.36).min(22.0 * s);
    let count_w = icon * 0.62;

    let mut layout = Layout { map_top: y, ..Layout::default() };
    layout.add_row_top = rect(spine_x + (cell_w - plus) / 2.0, y, plus, plus);
    y += plus + gap;
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
            name: {
                // As wide as the name itself, so the clickable box matches what is read.
                let name = if row.name.is_empty() { format!("워크스페이스 {}", r + 1) } else { row.name.clone() };
                let width = (text_width(&name, 12.0 * s) + 4.0 * s).min(header_w - 12.0 * s);
                rect(x0, y + (cell_h - 20.0 * s) / 2.0, width, 20.0 * s)
            },
            top: y - gap / 2.0,
            bottom: y + cell_h + gap / 2.0,
            cells,
            plus: rect(x, y + (cell_h - plus) / 2.0, plus, plus),
            plus_left: rect(first_x - gap - plus, y + (cell_h - plus) / 2.0, plus, plus),
        });
        y += row_pitch;
    }
    layout.spine = rect(spine_x - gap / 2.0, rows_top - gap / 2.0, pitch, (y - rows_top).max(0.0));
    layout.add_row = rect(spine_x + (cell_w - plus) / 2.0, y - gap / 2.0 + 2.0 * s, plus, plus);
    layout
}
