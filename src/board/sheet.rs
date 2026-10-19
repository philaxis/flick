//! The apps' icons laid out on a window of their own that is never on
//! screen. What the board draws itself lies under the windows' live
//! previews; a thumbnail of one icon of this sheet, registered after them,
//! lies over them, so an icon can sit on a preview's corner.

use std::collections::HashMap;

use crate::paint::Dib;
use windows::{
    core::{w, Error, Result},
    Win32::{
        Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, SIZE, WPARAM},
        Graphics::Gdi::{AC_SRC_ALPHA, AC_SRC_OVER, BLENDFUNCTION},
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, RegisterClassW, ShowWindow, UpdateLayeredWindow, SW_SHOWNOACTIVATE,
            ULW_ALPHA, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT, WS_POPUP,
        },
    },
};

const COLUMNS: usize = 16;
/// Far off every monitor: DWM keeps the picture for the thumbnails.
const AWAY: POINT = POINT { x: -32000, y: -32000 };

pub struct IconSheet {
    pub hwnd: HWND,
    /// The side of one icon, in pixels.
    side: i32,
    slots: HashMap<isize, usize>,
    /// Each icon's premultiplied pixels, `side` by `side`.
    pixels: Vec<Vec<u8>>,
    changed: bool,
    shown: bool,
}

impl IconSheet {
    pub fn new() -> Result<IconSheet> {
        unsafe {
            let instance = GetModuleHandleW(None)?;
            let class = w!("flick.icons");
            // Fails harmlessly when a board before this one registered it.
            RegisterClassW(&WNDCLASSW { lpfnWndProc: Some(proc), hInstance: instance.into(), lpszClassName: class, ..Default::default() });
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                class,
                w!("Flick icons"),
                WS_POPUP,
                AWAY.x,
                AWAY.y,
                0,
                0,
                None,
                None,
                instance,
                None,
            );
            if hwnd.0 == 0 {
                return Err(Error::from_win32());
            }
            Ok(IconSheet { hwnd, side: 0, slots: HashMap::new(), pixels: Vec::new(), changed: false, shown: false })
        }
    }

    pub fn clear(&mut self) {
        self.slots.clear();
        self.pixels.clear();
        self.changed = true;
    }

    /// Puts the icon of window `hwnd` on the sheet, unless it is there.
    pub fn add(&mut self, hwnd: isize, side: i32, pixels: &[u8]) {
        if side != self.side {
            self.clear();
            self.side = side;
        }
        if self.slots.contains_key(&hwnd) || pixels.len() != (side * side * 4) as usize {
            return;
        }
        self.slots.insert(hwnd, self.pixels.len());
        self.pixels.push(pixels.to_vec());
        self.changed = true;
    }

    /// Where on the sheet the icon of `hwnd` is: its left, top and side.
    pub fn slot(&self, hwnd: isize) -> Option<(f32, f32, f32)> {
        let i = *self.slots.get(&hwnd)?;
        let side = self.side as f32;
        Some(((i % COLUMNS) as f32 * side, (i / COLUMNS) as f32 * side, side))
    }

    /// Gives the window the icons added since it was last given them.
    pub fn update(&mut self) -> Result<()> {
        if !self.changed || self.pixels.is_empty() {
            return Ok(());
        }
        let side = self.side as usize;
        let columns = self.pixels.len().min(COLUMNS);
        let rows = self.pixels.len().div_ceil(COLUMNS);
        let mut dib = Dib::new(((columns * side) as i32, (rows * side) as i32))?;
        let pitch = columns * side * 4;
        let sheet = dib.pixels_mut();
        sheet.fill(0);
        for (i, icon) in self.pixels.iter().enumerate() {
            let (x, y) = ((i % COLUMNS) * side, (i / COLUMNS) * side);
            for (row, line) in icon.chunks_exact(side * 4).enumerate() {
                let at = (y + row) * pitch + x * 4;
                sheet[at..at + side * 4].copy_from_slice(line);
            }
        }
        let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
        unsafe {
            UpdateLayeredWindow(
                self.hwnd,
                None,
                Some(&AWAY),
                Some(&SIZE { cx: dib.size.0, cy: dib.size.1 }),
                dib.dc,
                Some(&POINT::default()),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            )?;
            if !self.shown {
                // Only a shown window has a picture to take thumbnails of.
                ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
                self.shown = true;
            }
        }
        self.changed = false;
        Ok(())
    }
}

extern "system" fn proc(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}
