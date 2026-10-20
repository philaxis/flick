//! What of a window shows. Some windows (a borderless one drawn see-through,
//! as a webview app with a transparent background is) are larger than what
//! is seen of them, the rest of their frame clear; whatever is cast about a
//! window (a pinned one's light) is cast about what shows instead.
//!
//! Found once from a picture of the window, the box of its pixels opaque
//! enough, and kept while the window keeps its size: never frame by frame.
//! A window of any other shape is taken as that box.

/// How opaque a pixel must be to count as shown: a faint shadow about
/// what is drawn does not.
const LEAST: u8 = 64;

/// The box (left, top, right, bottom) of the pixels of `pixels`, blue,
/// green, red and alpha bytes row by row `size` wide and high, more
/// opaque than `least`; `None` if there are none.
pub fn opaque_bounds(pixels: &[u8], size: (i32, i32), least: u8) -> Option<[i32; 4]> {
    let (w, h) = (size.0.max(0) as usize, size.1.max(0) as usize);
    if pixels.len() < w * h * 4 {
        return None;
    }
    let mut bounds: Option<[i32; 4]> = None;
    for (y, row) in pixels.chunks_exact(w * 4).take(h).enumerate() {
        let mut opaque = row.chunks_exact(4).enumerate().filter(|(_, pixel)| pixel[3] > least).map(|(x, _)| x as i32);
        let Some(first) = opaque.next() else { continue };
        let last = opaque.last().unwrap_or(first);
        let y = y as i32;
        bounds = Some(match bounds {
            None => [first, y, last + 1, y + 1],
            Some([l, t, r, _]) => [l.min(first), t, r.max(last + 1), y + 1],
        });
    }
    bounds
}

/// What of a frame `frame` (left, top, right, bottom, in a picture's
/// pixels) the box `shown` of that picture is, as shares of the frame's
/// width and height; `None` when all of it is (or none, which tells
/// nothing).
pub fn share(shown: [i32; 4], frame: [i32; 4]) -> Option<[f64; 4]> {
    let (w, h) = ((frame[2] - frame[0]) as f64, (frame[3] - frame[1]) as f64);
    let clipped = [shown[0].max(frame[0]), shown[1].max(frame[1]), shown[2].min(frame[2]), shown[3].min(frame[3])];
    if w <= 0.0 || h <= 0.0 || clipped[2] <= clipped[0] || clipped[3] <= clipped[1] || clipped == frame {
        return None;
    }
    Some([
        (clipped[0] - frame[0]) as f64 / w,
        (clipped[1] - frame[1]) as f64 / h,
        (clipped[2] - frame[0]) as f64 / w,
        (clipped[3] - frame[1]) as f64 / h,
    ])
}

/// The part `share` of a frame drawn at `at` (left, top, right, bottom).
pub fn within(at: [f64; 4], share: [f64; 4]) -> [f64; 4] {
    let (w, h) = (at[2] - at[0], at[3] - at[1]);
    [at[0] + share[0] * w, at[1] + share[1] * h, at[0] + share[2] * w, at[1] + share[3] * h]
}

#[cfg(windows)]
pub use shown::shown;

#[cfg(windows)]
mod shown {
    use super::{opaque_bounds, share, LEAST};
    use std::{collections::HashMap, sync::Mutex};
    use windows::Win32::{
        Foundation::{HWND, RECT},
        Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS},
        Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS},
        UI::WindowsAndMessaging::{
            GetWindowLongPtrW, GetWindowRect, IsHungAppWindow, IsIconic, GWL_EXSTYLE, GWL_STYLE, PW_RENDERFULLCONTENT, WS_CAPTION,
            WS_EX_LAYERED, WS_EX_NOREDIRECTIONBITMAP,
        },
    };

    /// Each window's size when it was looked at, and what of it showed.
    static SEEN: Mutex<Option<HashMap<isize, ((i32, i32), Option<[f64; 4]>)>>> = Mutex::new(None);

    /// What of the visible frame of `hwnd` (`DWMWA_EXTENDED_FRAME_BOUNDS`)
    /// shows, as shares of it (`share`); `None` when all of it does, as of
    /// any window with a title bar, or when it cannot be told.
    pub fn shown(hwnd: HWND) -> Option<[f64; 4]> {
        let mut outer = RECT::default();
        if unsafe { IsIconic(hwnd) }.as_bool() || unsafe { GetWindowRect(hwnd, &mut outer) }.is_err() {
            return None;
        }
        let size = (outer.right - outer.left, outer.bottom - outer.top);
        let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
        let seen = seen.get_or_insert_with(HashMap::new);
        if let Some((at, shown)) = seen.get(&hwnd.0) {
            if *at == size {
                return *shown;
            }
        }
        let shown = look(hwnd, outer, size);
        // Of windows long gone, none kept.
        if seen.len() > 256 {
            seen.clear();
        }
        seen.insert(hwnd.0, (size, shown));
        shown
    }

    /// A picture of `hwnd`, `size` large, and the box of what in it is
    /// opaque enough. Only a window that can be see-through is looked at:
    /// one drawn as layers, or by composition alone, or without a title
    /// bar; nor one that does not answer.
    fn look(hwnd: HWND, outer: RECT, size: (i32, i32)) -> Option<[f64; 4]> {
        let (style, ex) = unsafe { (GetWindowLongPtrW(hwnd, GWL_STYLE) as u32, GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32) };
        let clear = ex & (WS_EX_LAYERED.0 | WS_EX_NOREDIRECTIONBITMAP.0) != 0 || style & WS_CAPTION.0 != WS_CAPTION.0;
        if !clear || size.0 <= 0 || size.1 <= 0 || unsafe { IsHungAppWindow(hwnd) }.as_bool() {
            return None;
        }
        let mut frame = RECT::default();
        if unsafe { DwmGetWindowAttribute(hwnd, DWMWA_EXTENDED_FRAME_BOUNDS, &mut frame as *mut _ as _, std::mem::size_of::<RECT>() as u32) }.is_err() {
            frame = outer;
        }
        let mut dib = crate::paint::Dib::new(size).ok()?;
        dib.pixels_mut().fill(0);
        if !unsafe { PrintWindow(hwnd, dib.dc, PRINT_WINDOW_FLAGS(PW_RENDERFULLCONTENT)) }.as_bool() {
            return None;
        }
        // A picture opaque all over, or clear all over (one with no alpha
        // at all), tells nothing.
        let shown = opaque_bounds(dib.pixels(), size, LEAST)?;
        let frame = [frame.left - outer.left, frame.top - outer.top, frame.right - outer.left, frame.bottom - outer.top];
        share(shown, frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A picture `size` large, clear but for the box `opaque`.
    fn picture(size: (i32, i32), opaque: [i32; 4]) -> Vec<u8> {
        let mut pixels = vec![0u8; (size.0 * size.1 * 4) as usize];
        for y in opaque[1]..opaque[3] {
            for x in opaque[0]..opaque[2] {
                let i = ((y * size.0 + x) * 4) as usize;
                pixels[i..i + 4].copy_from_slice(&[40, 80, 120, 255]);
            }
        }
        pixels
    }

    #[test]
    fn a_window_clear_about_what_it_draws_shows_only_that() {
        let mut pixels = picture((40, 30), [6, 4, 31, 22]);
        // A faint shadow about it is not counted.
        pixels[((2 * 40 + 3) * 4 + 3) as usize] = 30;
        assert_eq!(opaque_bounds(&pixels, (40, 30), LEAST), Some([6, 4, 31, 22]));
        let shown = share([6, 4, 31, 22], [0, 0, 40, 30]).unwrap();
        assert_eq!(shown, [6.0 / 40.0, 4.0 / 30.0, 31.0 / 40.0, 22.0 / 30.0]);
        // Drawn twice as large, what shows is too.
        assert_eq!(within([100.0, 50.0, 180.0, 110.0], shown), [112.0, 58.0, 162.0, 94.0]);
    }

    #[test]
    fn an_opaque_window_shows_all_its_frame() {
        let pixels = picture((40, 30), [0, 0, 40, 30]);
        assert_eq!(opaque_bounds(&pixels, (40, 30), LEAST), Some([0, 0, 40, 30]));
        assert_eq!(share([0, 0, 40, 30], [0, 0, 40, 30]), None);
        // Opaque past its frame, into its invisible borders: all of the frame.
        assert_eq!(share([0, 0, 40, 30], [8, 0, 32, 22]), None);
        // A picture with nothing opaque in it tells nothing.
        assert_eq!(opaque_bounds(&vec![0u8; 40 * 30 * 4], (40, 30), LEAST), None);
    }
}
