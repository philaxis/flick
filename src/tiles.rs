//! How large the preview of each window is, and where it goes.
//!
//! Plain numbers with no Windows in them. Every window is shown at one common
//! scale, so a small window looks small next to a large one; the scale is as
//! large as fits, but never above the windows' real size and never so large
//! that a preview outgrows its share of the area.

/// What the previews have to fit into.
pub struct Space {
    /// Width and height of the area they share.
    pub area: (f32, f32),
    /// Height of the title bar on top of each preview.
    pub title: f32,
    /// Room between two previews, sideways and between rows.
    pub gap: f32,
    /// The largest a preview may be.
    pub max: (f32, f32),
    /// The lowest a preview may be, so that a sliver of a window can still
    /// be pointed at. (How narrow is up to each window: see `arrange`.)
    pub min_height: f32,
}

/// Where one preview goes: the top-left corner of its tile (title bar
/// included) inside the area, and the size of the preview under the title.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// Previews are laid out in at most this many rows.
const MAX_ROWS: usize = 4;

/// Places the previews of windows of the given sizes, in that order, in rows
/// centred in the area. `min_widths` is how narrow each one may get (what its
/// title bar needs). Nothing is returned when there is no room for them.
pub fn arrange(windows: &[(f32, f32)], min_widths: &[f32], space: &Space) -> Vec<Placed> {
    if windows.is_empty() {
        return Vec::new();
    }
    // The windows at scale `k`, none smaller than `m` times the least.
    let fit = |k: f32, m: f32| {
        let sizes: Vec<(f32, f32)> = windows
            .iter()
            .zip(min_widths)
            .map(|((w, h), min_w)| ((w * k).max(min_w * m), (h * k).max(space.min_height * m)))
            .collect();
        (1..=sizes.len().min(MAX_ROWS)).find_map(|rows| place(&sizes, rows, space))
    };
    // The largest scale that fits, between `low` (which does) and `high`.
    let largest = |mut low: f32, mut high: f32, fit: &dyn Fn(f32) -> Option<Vec<Placed>>| {
        for _ in 0..24 {
            let middle = (low + high) / 2.0;
            if fit(middle).is_some() {
                low = middle;
            } else {
                high = middle;
            }
        }
        fit(low).unwrap_or_default()
    };
    // Real size at most, and the largest window within the largest preview.
    let cap = windows.iter().map(|(w, h)| (space.max.0 / w.max(1.0)).min(space.max.1 / h.max(1.0))).fold(1.0, f32::min);
    if let Some(placed) = fit(cap, 1.0) {
        return placed;
    }
    if fit(0.0, 1.0).is_some() {
        return largest(0.0, cap, &|k| fit(k, 1.0));
    }
    // So many that they do not fit even at their least: all the same and
    // smaller still, down to where nothing could be made out.
    let least = 8.0 / space.min_height.max(8.0);
    if fit(0.0, least).is_none() {
        return Vec::new();
    }
    largest(least, 1.0, &|m| fit(0.0, m))
}

/// Splits previews of the given sizes into up to `rows` rows of about the
/// same width and centres them in the area; `None` when they do not fit.
/// The previews of a row are lined up at their tops.
fn place(sizes: &[(f32, f32)], rows: usize, space: &Space) -> Option<Vec<Placed>> {
    let (aw, ah) = space.area;
    let target = sizes.iter().map(|size| size.0).sum::<f32>() / rows as f32;
    let mut split: Vec<Vec<usize>> = vec![Vec::new()];
    let mut filled = 0.0;
    for (i, (w, _)) in sizes.iter().enumerate() {
        if filled + w / 2.0 > target && split.len() < rows && !split[split.len() - 1].is_empty() {
            split.push(Vec::new());
            filled = 0.0;
        }
        let last = split.len() - 1;
        split[last].push(i);
        filled += w;
    }
    let width = |row: &[usize]| row.iter().map(|i| sizes[*i].0).sum::<f32>() + space.gap * (row.len() as f32 - 1.0);
    let height = |row: &[usize]| space.title + row.iter().map(|i| sizes[*i].1).fold(0.0, f32::max);
    let block = split.iter().map(|row| height(row)).sum::<f32>() + space.gap * (split.len() as f32 - 1.0);
    // (Half a pixel of slack for the rounding of the search.)
    if block > ah + 0.5 || split.iter().any(|row| width(row) > aw + 0.5) {
        return None;
    }
    let mut placed = vec![Placed { x: 0.0, y: 0.0, w: 0.0, h: 0.0 }; sizes.len()];
    let mut y = (ah - block) / 2.0;
    for row in &split {
        let mut x = (aw - width(row)) / 2.0;
        for &i in row {
            let (w, h) = sizes[i];
            placed[i] = Placed { x, y, w, h };
            x += w + space.gap;
        }
        y += height(row) + space.gap;
    }
    Some(placed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn space() -> Space {
        Space { area: (1820.0, 575.0), title: 57.0, gap: 32.0, max: (1092.0, 518.0), min_height: 75.0 }
    }

    /// A small dialog next to a maximized window must look as much smaller
    /// as it is, the large one must stay within the largest preview allowed,
    /// and a small window on its own must not be blown up past its real size.
    #[test]
    fn previews_keep_the_windows_relative_sizes() {
        let placed = arrange(&[(1936.0, 1056.0), (400.0, 300.0)], &[200.0, 200.0], &space());
        let (large, small) = (placed[0], placed[1]);
        assert!(large.w <= 1092.0 && large.h <= 518.0, "{large:?}");
        assert!((small.w / large.w - 400.0 / 1936.0).abs() < 0.02, "{small:?} beside {large:?}");
        assert!((small.h / large.h - 300.0 / 1056.0).abs() < 0.02, "{small:?} beside {large:?}");

        let alone = arrange(&[(400.0, 300.0)], &[200.0], &space());
        assert_eq!((alone[0].w, alone[0].h), (400.0, 300.0));
    }

    /// With more windows than fit at any pleasant size they all shrink
    /// together: every tile stays inside the area and none lies on another.
    #[test]
    fn many_windows_all_stay_inside_the_area() {
        let space = space();
        let windows: Vec<(f32, f32)> = (0..30).map(|i| if i % 3 == 0 { (1936.0, 1056.0) } else { (640.0, 480.0) }).collect();
        let placed = arrange(&windows, &vec![200.0; 30], &space);
        assert_eq!(placed.len(), 30);
        for (i, a) in placed.iter().enumerate() {
            assert!(a.x >= -0.5 && a.y >= -0.5 && a.x + a.w <= space.area.0 + 0.5 && a.y + space.title + a.h <= space.area.1 + 0.5, "{a:?}");
            for b in &placed[i + 1..] {
                let apart = a.x + a.w <= b.x || b.x + b.w <= a.x || a.y + space.title + a.h <= b.y || b.y + space.title + b.h <= a.y;
                assert!(apart, "{a:?} on {b:?}");
            }
        }
    }
}
