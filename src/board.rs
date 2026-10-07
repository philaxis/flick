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
        Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM},
        Graphics::{
            Direct2D::{
                Common::{
                    D2D1_ALPHA_MODE_IGNORE, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT,
                    D2D_RECT_F, D2D_SIZE_U,
                },
                D2D1CreateFactory, ID2D1Bitmap, ID2D1Factory, ID2D1RenderTarget,
                D2D1_ANTIALIAS_MODE_ALIASED, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, D2D1_BITMAP_PROPERTIES,
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
                DwmQueryThumbnailSourceSize, DwmRegisterThumbnail, DwmUnregisterThumbnail,
                DwmUpdateThumbnailProperties, DWM_THUMBNAIL_PROPERTIES, DWM_TNP_OPACITY, DWM_TNP_RECTDESTINATION,
                DWM_TNP_RECTSOURCE, DWM_TNP_SOURCECLIENTAREAONLY, DWM_TNP_VISIBLE,
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
                CreateWindowExW, DrawIconEx, GetClassLongPtrW, GetCursorPos, RegisterClassW, SendMessageTimeoutW,
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
    /// Bounding rectangle of all monitors; each cell is a miniature of it.
    pub screen: RECT,
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
    CloseRow(usize),
    Rename(usize, String),
}

type Rect = D2D_RECT_F;

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

struct WindowLayout {
    hwnd: HWND,
    /// Where the window sits inside the miniature, before clipping to it.
    full: Option<Rect>,
    icon: Rect,
}

struct CellLayout {
    id: CellId,
    title: Rect,
    body: Rect,
    close: Rect,
    strip: Rect,
    windows: Vec<WindowLayout>,
}

struct RowLayout {
    name: Rect,
    close: Rect,
    top: f32,
    bottom: f32,
    cells: Vec<CellLayout>,
    plus: Rect,
}

#[derive(Default)]
struct Layout {
    rows: Vec<RowLayout>,
    add_row: Rect,
    /// Label and icons of the windows pinned to every desktop.
    pinned_label: Rect,
    pinned: Vec<WindowLayout>,
}

#[derive(Clone, PartialEq, Default)]
enum Hit {
    #[default]
    Nothing,
    /// `cell` is empty for a window pinned to every desktop.
    Window { cell: CellId, hwnd: isize },
    /// The cell's title bar or its bare background; the handle for dragging it.
    Cell(CellId),
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
    open: bool,
    model: Model,
    /// Copy of the grid used to move the selection, so that looking around
    /// does not disturb the real per-row memory until a cell is chosen.
    nav: Grid,
    selected: CellId,
    layout: Layout,
    size: (i32, i32),
    scale: f32,
    /// (source window, thumbnail handle), bottom-most first.
    thumbs: Vec<(isize, isize)>,
    icons: HashMap<isize, ID2D1Bitmap>,
    hover: Hit,
    press: Option<Press>,
    cursor: (f32, f32),
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
                open: false,
                model: Model { rows: Vec::new(), pinned: Vec::new(), current: String::new(), screen: RECT::default() },
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
            self.invalidate();
        }
    }

    fn invalidate(&self) {
        unsafe {
            InvalidateRect(self.hwnd, None, false);
        }
    }

    // ---- layout -----------------------------------------------------------

    fn relayout(&mut self) {
        self.layout = compute_layout(&self.model, self.size, self.scale);
    }

    fn hit_test(&self, x: f32, y: f32) -> Hit {
        if contains(&self.layout.add_row, x, y) {
            return Hit::AddRow;
        }
        if let Some(window) = self.layout.pinned.iter().find(|w| contains(&w.icon, x, y)) {
            return Hit::Window { cell: String::new(), hwnd: window.hwnd.0 };
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
                for window in &cell.windows {
                    let on_thumb = window.full.is_some_and(|full| contains(&full, x, y) && contains(&cell.body, x, y));
                    if on_thumb || contains(&window.icon, x, y) {
                        return Hit::Window { cell: cell.id.clone(), hwnd: window.hwnd.0 };
                    }
                }
                if contains(&cell.title, x, y) || contains(&cell.body, x, y) || contains(&cell.strip, x, y) {
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
        self.layout.rows.iter().flat_map(|row| &row.cells).find(|cell| {
            contains(&Rect { left: cell.body.left, top: cell.title.top, right: cell.body.right, bottom: cell.strip.bottom }, x, y)
        })
    }

    fn slot_at(&self, dragged: &str, x: f32, y: f32) -> Option<Slot> {
        let rows = &self.layout.rows;
        let (first, last) = (rows.first()?, rows.last()?);
        if y < first.top {
            return Some(Slot::NewRow { at: 0, y: first.top - 12.0 * self.scale });
        }
        if y >= last.bottom {
            return Some(Slot::NewRow { at: rows.len(), y: last.bottom + 12.0 * self.scale });
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
        for (_, thumb) in self.thumbs.drain(..) {
            unsafe {
                let _ = DwmUnregisterThumbnail(thumb);
            }
        }
    }

    /// Registers a live thumbnail per visible window. Later registrations are
    /// drawn above earlier ones, so each cell goes bottom-most first.
    fn register_thumbnails(&mut self) {
        self.clear_thumbnails();
        for cell in self.model.rows.iter().flat_map(|row| &row.cells) {
            for window in cell.windows.iter().rev().filter(|w| w.rect.is_some()) {
                if let Ok(thumb) = unsafe { DwmRegisterThumbnail(self.hwnd, window.hwnd) } {
                    self.thumbs.push((window.hwnd.0, thumb));
                }
            }
        }
        self.update_thumbnails();
    }

    fn window_layout(&self, hwnd: isize) -> Option<(&CellLayout, &WindowLayout)> {
        self.layout.rows.iter().flat_map(|row| &row.cells).find_map(|cell| {
            cell.windows.iter().find(|w| w.hwnd.0 == hwnd).map(|window| (cell, window))
        })
    }

    fn dragged_window(&self) -> Option<isize> {
        match &self.press {
            Some(Press { hit: Hit::Window { hwnd, .. }, dragging: true, .. }) => Some(*hwnd),
            _ => None,
        }
    }

    fn update_thumbnails(&self) {
        let dragged = self.dragged_window();
        for &(hwnd, thumb) in &self.thumbs {
            let Some((cell, window)) = self.window_layout(hwnd) else { continue };
            let Some(full) = window.full else { continue };
            let (fw, fh) = (full.right - full.left, full.bottom - full.top);
            let mut props = DWM_THUMBNAIL_PROPERTIES {
                dwFlags: DWM_TNP_RECTDESTINATION | DWM_TNP_VISIBLE | DWM_TNP_OPACITY | DWM_TNP_SOURCECLIENTAREAONLY,
                opacity: 255,
                fVisible: true.into(),
                fSourceClientAreaOnly: false.into(),
                ..Default::default()
            };
            let to_rect = |r: &Rect| RECT {
                left: r.left.round() as i32,
                top: r.top.round() as i32,
                right: r.right.round() as i32,
                bottom: r.bottom.round() as i32,
            };
            if dragged == Some(hwnd) {
                // Follows the cursor, unclipped.
                let (cx, cy) = self.cursor;
                props.rcDestination = to_rect(&rect(cx - fw / 2.0, cy - fh / 2.0, fw, fh));
                props.opacity = 215;
            } else {
                let clip = Rect {
                    left: full.left.max(cell.body.left),
                    top: full.top.max(cell.body.top),
                    right: full.right.min(cell.body.right),
                    bottom: full.bottom.min(cell.body.bottom),
                };
                if clip.right - clip.left < 1.0 || clip.bottom - clip.top < 1.0 || fw < 1.0 || fh < 1.0 {
                    props.fVisible = false.into();
                } else {
                    props.rcDestination = to_rect(&clip);
                    // Show only the part of the window that is inside the screen.
                    if let Ok(SIZE { cx, cy }) = unsafe { DwmQueryThumbnailSourceSize(thumb) } {
                        let (sw, sh) = (cx as f32, cy as f32);
                        props.dwFlags |= DWM_TNP_RECTSOURCE;
                        props.rcSource = RECT {
                            left: ((clip.left - full.left) / fw * sw).round() as i32,
                            top: ((clip.top - full.top) / fh * sh).round() as i32,
                            right: ((clip.right - full.left) / fw * sw).round() as i32,
                            bottom: ((clip.bottom - full.top) / fh * sh).round() as i32,
                        };
                    }
                }
            }
            unsafe {
                let _ = DwmUpdateThumbnailProperties(thumb, &props);
            }
        }
    }

    /// Thumbnails stack in registration order; re-register one to lift it
    /// above the rest while it is being dragged.
    fn raise_thumbnail(&mut self, hwnd: isize) {
        if let Some(i) = self.thumbs.iter().position(|(h, _)| *h == hwnd) {
            let (_, old) = self.thumbs.remove(i);
            unsafe {
                let _ = DwmUnregisterThumbnail(old);
                if let Ok(thumb) = DwmRegisterThumbnail(self.hwnd, HWND(hwnd)) {
                    self.thumbs.push((hwnd, thumb));
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
        if hover != self.hover {
            self.hover = hover;
            self.invalidate();
        }
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
        if self.fonts.is_none() {
            self.fonts = make_fonts(self.scale).map_err(|e| crate::app::log(&format!("board fonts failed: {e}"))).ok();
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
        let Some(fonts) = &self.fonts else { return Ok(()) };
        unsafe {
            t.Clear(Some(&rgba(0.050, 0.055, 0.068, 1.0)));
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
            let mut hovered_title = None;

            if !self.layout.pinned.is_empty() {
                text("모든 칸에 고정", &fonts.small, &self.layout.pinned_label, white(0.50));
                for (window, window_model) in self.layout.pinned.iter().zip(&self.model.pinned) {
                    if self.hover == (Hit::Window { cell: String::new(), hwnd: window.hwnd.0 }) {
                        hovered_title = Some(window_model.title.as_str());
                        fill(&grow(&window.icon, 3.0 * s), 5.0 * s, white(0.16));
                    }
                    match self.icons.get(&window.hwnd.0) {
                        Some(bitmap) => t.DrawBitmap(bitmap, Some(&window.icon), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None),
                        None => fill(&window.icon, 4.0 * s, white(0.25)),
                    }
                }
            }

            for (r, (row, model)) in self.layout.rows.iter().zip(&self.model.rows).enumerate() {
                // Row header: name, memory, close button.
                let editing = self.editing.as_ref().filter(|(row, _)| *row == r);
                let name = match editing {
                    Some((_, typed)) => format!("{typed}▏"),
                    None if model.name.is_empty() => format!("워크스페이스 {}", r + 1),
                    None => model.name.clone(),
                };
                if editing.is_some() || self.hover == Hit::RowName(r) {
                    fill(&grow(&row.name, 4.0 * s), 5.0 * s, white(if editing.is_some() { 0.10 } else { 0.05 }));
                }
                text(&name, &fonts.label, &row.name, white(0.92));
                let windows: usize = model.cells.iter().map(|c| c.windows.len()).sum();
                let info = Rect { top: row.name.bottom + 4.0 * s, bottom: row.name.bottom + 24.0 * s, ..row.name };
                let asleep = if model.asleep { " · 재움" } else { "" };
                text(&format!("{} · 창 {windows}개{asleep}", format_memory(model.memory)), &fonts.small, &info, white(0.50));
                if self.armed_row == Some(r) {
                    fill(&row.close, 5.0 * s, danger(0.90));
                    text("한 번 더 누르면 닫음", &fonts.centered, &row.close, white(1.0));
                } else {
                    let hot = self.hover == Hit::RowClose(r);
                    fill(&row.close, 5.0 * s, white(if hot { 0.14 } else { 0.05 }));
                    text("이 행 닫기", &fonts.centered, &row.close, white(if hot { 0.95 } else { 0.50 }));
                }

                for (c, (cell, cell_model)) in row.cells.iter().zip(&model.cells).enumerate() {
                    let dim = if dragged_cell == Some(cell.id.as_str()) { 0.35 } else { 1.0 };
                    let is_current = cell.id == self.model.current;
                    let is_selected = cell.id == self.selected;
                    let hovering = matches!(&self.hover, Hit::Cell(id) | Hit::CellClose(id) | Hit::Window { cell: id, .. } if *id == cell.id);

                    if is_selected {
                        for (by, a) in [(7.0, 0.10), (4.5, 0.20)] {
                            fill(&grow(&cell.body, by * s), (6.0 + by) * s, accent(a * dim));
                        }
                    }
                    fill(&cell.body, 6.0 * s, rgba(0.115, 0.125, 0.150, dim));
                    let (width, colour) = if drop_cell == Some(cell.id.as_str()) {
                        (2.5 * s, accent(1.0))
                    } else if is_selected {
                        (2.0 * s, accent(dim))
                    } else {
                        (1.0, white(if hovering { 0.30 } else { 0.10 } * dim))
                    };
                    stroke(&grow(&cell.body, 1.5 * s), 7.0 * s, width, colour);

                    let label = if is_current { format!("{}  ● 현재", c + 1) } else { format!("{}", c + 1) };
                    text(&label, &fonts.small, &cell.title, if is_current { accent(dim) } else { white(0.55 * dim) });
                    if hovering && self.cell_count() > 1 && self.press.is_none() {
                        let hot = self.hover == Hit::CellClose(cell.id.clone());
                        fill(&cell.close, 4.0 * s, if hot { danger(0.90) } else { white(0.10) });
                        text("✕", &fonts.centered, &cell.close, white(if hot { 1.0 } else { 0.70 }));
                    }

                    if cell_model.windows.is_empty() {
                        text("비어 있음", &fonts.centered, &cell.body, white(0.22 * dim));
                    }
                    // Stand-ins under the live thumbnails, bottom-most first, so a
                    // window whose thumbnail cannot be shown is still visible.
                    t.PushAxisAlignedClip(&cell.body, D2D1_ANTIALIAS_MODE_ALIASED);
                    for window in cell.windows.iter().rev() {
                        let Some(full) = window.full else { continue };
                        fill(&full, 3.0 * s, rgba(0.19, 0.205, 0.24, dim));
                        stroke(&full, 3.0 * s, 1.0, white(0.14 * dim));
                        if let Some(bitmap) = self.icons.get(&window.hwnd.0) {
                            let (cx, cy) = ((full.left + full.right) / 2.0, (full.top + full.bottom) / 2.0);
                            let half = (10.0 * s).min((full.right - full.left) / 2.0).min((full.bottom - full.top) / 2.0);
                            let area = rect(cx - half, cy - half, half * 2.0, half * 2.0);
                            t.DrawBitmap(bitmap, Some(&area), dim, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None);
                        }
                    }
                    t.PopAxisAlignedClip();
                    for (window, window_model) in cell.windows.iter().zip(&cell_model.windows) {
                        let hot = self.hover == Hit::Window { cell: cell.id.clone(), hwnd: window.hwnd.0 };
                        if hot {
                            hovered_title = Some(window_model.title.as_str());
                            fill(&grow(&window.icon, 3.0 * s), 5.0 * s, white(0.16));
                        }
                        match self.icons.get(&window.hwnd.0) {
                            Some(bitmap) => t.DrawBitmap(bitmap, Some(&window.icon), dim, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None),
                            None => fill(&window.icon, 4.0 * s, white(0.25 * dim)),
                        }
                        if window_model.follows {
                            let x = (window.icon.left + window.icon.right) / 2.0;
                            fill(&rect(x - 2.0 * s, window.icon.bottom + 2.5 * s, 4.0 * s, 4.0 * s), 2.0 * s, accent(dim));
                        }
                    }
                }

                let hot = self.hover == Hit::Plus(r);
                fill(&row.plus, 6.0 * s, white(if hot { 0.12 } else { 0.04 }));
                text("+", &fonts.big, &row.plus, white(if hot { 0.95 } else { 0.40 }));
            }

            let hot = self.hover == Hit::AddRow;
            fill(&self.layout.add_row, 6.0 * s, white(if hot { 0.12 } else { 0.04 }));
            text("+  새 워크스페이스", &fonts.centered, &self.layout.add_row, white(if hot { 0.95 } else { 0.45 }));

            // Drag feedback.
            if let Some(id) = dragged_cell {
                match self.slot_at(id, self.cursor.0, self.cursor.1) {
                    Some(Slot::Row { row, x, .. }) => {
                        let band = &self.layout.rows[row];
                        let body = band.cells.first().map_or(band.plus, |c| c.body);
                        fill(&rect(x - 2.0 * s, body.top, 4.0 * s, body.bottom - body.top), 2.0 * s, accent(1.0));
                    }
                    Some(Slot::NewRow { y, .. }) => {
                        let left = self.layout.add_row.left;
                        fill(&rect(left, y - 2.0 * s, self.layout.add_row.right - left, 4.0 * s), 2.0 * s, accent(1.0));
                    }
                    None => {}
                }
                let (cx, cy) = self.cursor;
                stroke(&rect(cx - 50.0 * s, cy - 30.0 * s, 100.0 * s, 60.0 * s), 6.0 * s, 2.0 * s, accent(0.9));
            }
            if let Some(hwnd) = self.dragged_window() {
                // A minimized window has no thumbnail to follow the cursor.
                if self.window_layout(hwnd).is_some_and(|(_, w)| w.full.is_none()) {
                    let (cx, cy) = self.cursor;
                    let area = rect(cx - 14.0 * s, cy - 14.0 * s, 28.0 * s, 28.0 * s);
                    if let Some(bitmap) = self.icons.get(&hwnd) {
                        t.DrawBitmap(bitmap, Some(&area), 1.0, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None);
                    }
                }
            }

            let (w, h) = (self.size.0 as f32, self.size.1 as f32);
            let footer = rect(0.0, h - 44.0 * s, w, 28.0 * s);
            match hovered_title {
                Some(title) => text(title, &fonts.centered, &footer, white(0.85)),
                None => text(
                    "칸 클릭: 이동   ·   창을 끌어 다른 칸으로   ·   칸을 끌어 재배치   ·   창 우클릭: 고정   ·   이름 더블클릭: 변경   ·   Esc: 닫기",
                    &fonts.centered,
                    &footer,
                    white(0.30),
                ),
            }
        }
        Ok(())
    }

    /// Draws the board (without live thumbnails) into a bitmap and returns the
    /// BGRA pixels, for reviewing the layout without showing a window.
    pub fn render_to_pixels(&mut self, model: Model, grid: Grid, size: (i32, i32), scale: f32) -> Option<Vec<u8>> {
        self.size = size;
        self.scale = scale;
        self.selected = model.current.clone();
        self.model = model;
        self.nav = grid;
        self.relayout();
        self.fonts = make_fonts(scale).ok();
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

/// Places rows top to bottom and each row's cells left to right, sizing the
/// miniatures so that the longest row and all rows fit the window.
fn compute_layout(model: &Model, size: (i32, i32), s: f32) -> Layout {
    let (w, h) = (size.0 as f32, size.1 as f32);
    let (margin, header_w, row_gap, cell_gap) = (48.0 * s, 190.0 * s, 26.0 * s, 20.0 * s);
    let (title_h, strip_h, add_row_h, footer_h) = (24.0 * s, 38.0 * s, 36.0 * s, 40.0 * s);
    const PLUS: f32 = 0.35;

    let rows = model.rows.len().max(1) as f32;
    let columns = model.rows.iter().map(|r| r.cells.len()).max().unwrap_or(1).max(1) as f32;
    let (screen_w, screen_h) = ((model.screen.right - model.screen.left).max(1) as f32, (model.screen.bottom - model.screen.top).max(1) as f32);
    let aspect = screen_w / screen_h;

    let available_w = w - 2.0 * margin - header_w;
    let available_h = h - 2.0 * margin - footer_h - add_row_h - row_gap;
    let mut cell_w = ((available_w - columns * cell_gap) / (columns + PLUS)).min(460.0 * s);
    let mut cell_h = cell_w / aspect;
    let pinned_room = if model.pinned.is_empty() { 0.0 } else { 40.0 * s };
    let max_h = (available_h - pinned_room - (rows - 1.0) * row_gap) / rows - title_h - strip_h;
    if cell_h > max_h {
        cell_h = max_h.max(40.0 * s);
        cell_w = cell_h * aspect;
    }
    let row_h = title_h + cell_h + strip_h;
    let total_w = header_w + columns * (cell_w + cell_gap) + cell_w * PLUS;
    let pinned_h = if model.pinned.is_empty() { 0.0 } else { 40.0 * s };
    let total_h = pinned_h + rows * row_h + (rows - 1.0) * row_gap + row_gap + add_row_h;
    let x0 = ((w - total_w) / 2.0).max(margin);
    let mut y = ((h - footer_h - total_h) / 2.0).max(margin);
    let cells_x = x0 + header_w;
    let icon = 20.0 * s;

    let mut layout = Layout::default();
    if !model.pinned.is_empty() {
        layout.pinned_label = rect(x0, y, header_w - 28.0 * s, icon);
        layout.pinned = model
            .pinned
            .iter()
            .enumerate()
            .map(|(i, window)| WindowLayout {
                hwnd: window.hwnd,
                full: None,
                icon: rect(cells_x + i as f32 * (icon + 7.0 * s), y, icon, icon),
            })
            .collect();
        y += pinned_h;
    }
    for row in &model.rows {
        let body_top = y + title_h;
        let mut x = cells_x;
        let mut cells = Vec::new();
        for cell in &row.cells {
            let body = rect(x, body_top, cell_w, cell_h);
            let windows = cell
                .windows
                .iter()
                .enumerate()
                .map(|(i, window)| WindowLayout {
                    hwnd: window.hwnd,
                    full: window.rect.map(|r| Rect {
                        left: body.left + (r.left - model.screen.left) as f32 / screen_w * cell_w,
                        top: body.top + (r.top - model.screen.top) as f32 / screen_h * cell_h,
                        right: body.left + (r.right - model.screen.left) as f32 / screen_w * cell_w,
                        bottom: body.top + (r.bottom - model.screen.top) as f32 / screen_h * cell_h,
                    }),
                    icon: rect(x + i as f32 * (icon + 7.0 * s), body.bottom + 8.0 * s, icon, icon),
                })
                .collect();
            cells.push(CellLayout {
                id: cell.id.clone(),
                title: rect(x + 2.0 * s, y, cell_w - 26.0 * s, title_h - 4.0 * s),
                body,
                close: rect(body.right - 20.0 * s, y, 20.0 * s, 20.0 * s),
                strip: rect(x, body.bottom, cell_w, strip_h),
                windows,
            });
            x += cell_w + cell_gap;
        }
        layout.rows.push(RowLayout {
            name: rect(x0, body_top + 2.0 * s, header_w - 28.0 * s, 24.0 * s),
            close: rect(x0, body_top + 58.0 * s, 130.0 * s, 24.0 * s),
            top: y,
            bottom: y + row_h,
            cells,
            plus: rect(x, body_top, cell_w * PLUS, cell_h),
        });
        y += row_h + row_gap;
    }
    layout.add_row = rect(cells_x, y, cell_w, add_row_h);
    layout
}
