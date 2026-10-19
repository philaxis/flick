//! How large the preview of each window is, and where it goes.
//!
//! Plain numbers with no Windows in them. The board supplies desired sizes
//! from importance and aspect ratio. One common scale preserves their relative
//! areas, as large as fits within the supplied sizes and preview limits.

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
    // Supplied size at most, and the largest within the preview limits.
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

/// A part of an area: left, top, width and height, from the area's corner.
pub type Part = (f32, f32, f32, f32);

/// The places of the pinned windows, at the left of the preview area and
/// of one size whatever is in them: those pinned to every desktop at the
/// top, those pinned to their row under them, and the other windows in
/// what is left at the right.
#[derive(Debug, PartialEq)]
pub struct PinColumn {
    pub all: Part,
    pub row: Part,
    pub rest: Part,
}

/// The share of the width the pinned places take.
const COLUMN_SHARE: f32 = 0.19;
/// Width per height of a window as it usually is, for telling how large
/// previews come out before the windows are known.
const USUAL_ASPECT: f32 = 1.6;

/// Splits an area of size `area` into the pinned places and the rest,
/// the column of places being between `width.0` and `width.1` wide (and
/// never more than half).
pub fn pin_column(area: (f32, f32), gap: f32, width: (f32, f32)) -> PinColumn {
    let (w, h) = area;
    let column = (w * COLUMN_SHARE).clamp(width.0, width.1.max(width.0)).min((w - gap) / 2.0).max(0.0);
    let upper = ((h - gap) / 2.0).max(0.0);
    PinColumn {
        all: (0.0, 0.0, column, upper),
        row: (0.0, upper + gap, column, (h - upper - gap).max(0.0)),
        rest: (column + gap, 0.0, (w - column - gap).max(0.0), h),
    }
}

/// How a place of fixed size is filled with its windows.
pub struct SlotRules {
    pub gap: f32,
    /// Under each preview, for its app's icon and its buttons.
    pub foot: f32,
    /// The least a preview may be (without its foot); smaller, the
    /// windows are shown by their apps' icons.
    pub least: (f32, f32),
    /// The least and the most an icon may be.
    pub icon: (f32, f32),
}

#[derive(Debug, PartialEq)]
pub struct SlotFill {
    /// Previews (with their feet), or icons.
    pub previews: bool,
    /// One for each window shown, in order; with `more`, one more after
    /// them, standing for those that did not fit.
    pub cells: Vec<Part>,
    pub more: usize,
}

/// Fills a place of size `area` with `n` windows: as previews in a grid,
/// as large as they come; too small, as icons, down to the least icon; and
/// with still more, the last cell standing for the rest.
pub fn fill_slot(n: usize, area: (f32, f32), rules: &SlotRules) -> SlotFill {
    let (w, h) = area;
    if n == 0 {
        return SlotFill { previews: false, cells: Vec::new(), more: 0 };
    }
    // Cells of a grid `columns` across, the last line centred.
    let grid = |columns: usize, cell: (f32, f32), count: usize, gap: f32| -> Vec<Part> {
        let rows = count.div_ceil(columns);
        let block = (cell.0 * columns.min(count) as f32 + gap * (columns.min(count) as f32 - 1.0), cell.1 * rows as f32 + gap * (rows as f32 - 1.0));
        let (x0, y0) = ((w - block.0) / 2.0, (h - block.1) / 2.0);
        (0..count).map(|i| {
            let (row, col) = (i / columns, i % columns);
            let in_row = if row + 1 == rows { count - row * columns } else { columns };
            let x = x0 + (block.0 - (cell.0 * in_row as f32 + gap * (in_row as f32 - 1.0))) / 2.0;
            (x + col as f32 * (cell.0 + gap), y0 + row as f32 * (cell.1 + gap), cell.0, cell.1)
        }).collect()
    };
    let cell = |columns: usize, gap: f32| {
        let rows = n.div_ceil(columns) as f32;
        ((w - gap * (columns as f32 - 1.0)) / columns as f32, (h - gap * (rows - 1.0)) / rows)
    };
    let preview = |(cw, ch): (f32, f32)| {
        let tall = (ch - rules.foot).max(0.0);
        let pw = cw.min(tall * USUAL_ASPECT);
        (pw, pw / USUAL_ASPECT)
    };
    let best = (1..=n).max_by(|a, b| preview(cell(*a, rules.gap)).0.total_cmp(&preview(cell(*b, rules.gap)).0)).unwrap_or(1);
    let (pw, ph) = preview(cell(best, rules.gap));
    if pw >= rules.least.0 && ph >= rules.least.1 {
        let (cw, _) = cell(best, rules.gap);
        return SlotFill { previews: true, cells: grid(best, (cw.min(pw), ph + rules.foot), n, rules.gap), more: 0 };
    }
    let gap = rules.gap * 0.5;
    let square = |columns: usize| { let (cw, ch) = cell(columns, gap); cw.min(ch) };
    let columns = (1..=n).max_by(|a, b| square(*a).total_cmp(&square(*b))).unwrap_or(1);
    let size = square(columns).min(rules.icon.1);
    if size >= rules.icon.0 {
        return SlotFill { previews: false, cells: grid(columns, (size, size), n, gap), more: 0 };
    }
    let size = rules.icon.0.min(w).min(h).max(0.0);
    let fit = |room: f32| (((room + gap) / (size + gap)).floor() as usize).max(1);
    let (columns, rows) = (fit(w), fit(h));
    let room = columns * rows;
    if n <= room {
        return SlotFill { previews: false, cells: grid(columns, (size, size), n, gap), more: 0 };
    }
    SlotFill { previews: false, cells: grid(columns, (size, size), room, gap), more: n - (room - 1) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn space() -> Space {
        Space { area: (1820.0, 575.0), title: 57.0, gap: 32.0, max: (1092.0, 518.0), min_height: 75.0 }
    }

    /// Desired sizes keep their proportions under one common fit scale,
    /// within preview limits and without growing beyond the supplied sizes.
    #[test]
    fn previews_keep_the_requested_relative_sizes() {
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
    /// A place of fixed size shows its windows as previews while they come
    /// out large enough, then as icons, then the icons that fit with the
    /// last cell standing for the rest; always within the place.
    #[test]
    fn a_pinned_place_is_filled_by_previews_then_icons_then_a_count() {
        let rules = SlotRules { gap: 12.0, foot: 26.0, least: (110.0, 60.0), icon: (24.0, 56.0) };
        let area = (330.0, 260.0);
        let inside = |fill: &SlotFill| fill.cells.iter().all(|c| c.0 >= -0.01 && c.1 >= -0.01 && c.0 + c.2 <= area.0 + 0.01 && c.1 + c.3 <= area.1 + 0.01);
        let one = fill_slot(1, area, &rules);
        assert!(one.previews && one.cells.len() == 1 && one.cells[0].2 > 300.0, "{one:?}");
        let four = fill_slot(4, area, &rules);
        assert!(four.previews && four.cells.len() == 4 && inside(&four), "{four:?}");
        let many = fill_slot(12, area, &rules);
        assert!(!many.previews && many.cells.len() == 12 && many.more == 0 && inside(&many), "{many:?}");
        assert!(many.cells.iter().all(|c| c.2 >= 24.0 && c.2 <= 56.0 && c.2 == c.3));
        let crowd = fill_slot(400, area, &rules);
        assert!(!crowd.previews && crowd.more > 0 && inside(&crowd), "{crowd:?}");
        assert_eq!(crowd.cells.len() - 1 + crowd.more, 400);
        assert!(crowd.cells.iter().all(|c| c.2 == 24.0));
        for (a, b) in [&four, &many, &crowd].iter().flat_map(|f| f.cells.iter().enumerate().flat_map(move |(i, a)| f.cells[i + 1..].iter().map(move |b| (a, b)))) {
            assert!(a.0 + a.2 <= b.0 + 0.01 || b.0 + b.2 <= a.0 + 0.01 || a.1 + a.3 <= b.1 + 0.01 || b.1 + b.3 <= a.1 + 0.01, "{a:?} on {b:?}");
        }
        assert!(fill_slot(0, area, &rules).cells.is_empty());
    }
}
