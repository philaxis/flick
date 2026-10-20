//! Pinned windows put by as the screen recedes: in the normal view, the
//! windows pinned to every shelf and those pinned to their shelf would
//! cover the receded desktops where they are, so the farther the screen
//! recedes, the nearer they go to small places at its left edge, the ones
//! the board keeps for them (`tiles::pin_column`): every shelf's above,
//! the shelf's below, filled as the board fills them (`tiles::fill_slot`):
//! small pictures, fewer than the cells beside the middle one are large,
//! then icons, then the least icons and a count of the rest. Whole, they
//! are where they are; coming forward, they go back.
use crate::tiles;

/// Left, top, right, bottom, in pixels on a screen.
pub type Edges = [f64; 4];

/// Where a pinned window goes with the screen receded all the way.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Spot {
    /// A small picture of it, its own shape.
    Preview(Edges),
    /// Its app's icon, a square.
    Icon(Edges),
    /// Folded into the cell standing for those that did not fit.
    Folded(Edges),
}

impl Spot {
    pub fn edges(&self) -> Edges {
        match *self { Spot::Preview(e) | Spot::Icon(e) | Spot::Folded(e) => e }
    }

    /// The same `dy` pixels down.
    pub fn moved(self, dy: f64) -> Spot {
        let down = |[l, t, r, b]: Edges| [l, t + dy, r, b + dy];
        match self { Spot::Preview(e) => Spot::Preview(down(e)), Spot::Icon(e) => Spot::Icon(down(e)), Spot::Folded(e) => Spot::Folded(down(e)) }
    }

    /// Where a window of frame `frame` is drawn in it: a picture keeps its
    /// shape, as large as fits.
    pub fn fitted(&self, frame: Edges) -> Edges {
        let [l, t, r, b] = self.edges();
        let (w, h) = (frame[2] - frame[0], frame[3] - frame[1]);
        if w <= 0.0 || h <= 0.0 { return self.edges(); }
        let fit = ((r - l) / w).min((b - t) / h);
        let (cx, cy) = ((l + r) / 2.0, (t + b) / 2.0);
        [cx - w * fit / 2.0, cy - h * fit / 2.0, cx + w * fit / 2.0, cy + h * fit / 2.0]
    }
}

/// A place filled: where each of its windows goes, in their order, and
/// the cell standing for the rest, with how many.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Place {
    pub spots: Vec<Spot>,
    pub more: Option<(Edges, usize)>,
}

/// The places on a screen of `size` pixels at `scale` (1.0 at 100%): for
/// the windows of every shelf, and for those of the shelf, as the board's.
pub fn places(size: (f64, f64), scale: f64) -> (Edges, Edges) {
    let s = scale as f32;
    // As the board's preview area: in from the edges, below the shelf's name.
    let (left, top, bottom) = (40.0 * s, 62.0 * s, 40.0 * s);
    let area = ((size.0 as f32 - 2.0 * left).max(0.0), (size.1 as f32 - top - bottom).max(0.0));
    let column = tiles::pin_column(area, 28.0 * s, (210.0 * s, 400.0 * s));
    let at = |(x, y, w, h): tiles::Part| [(left + x) as f64, (top + y) as f64, (left + x + w) as f64, (top + y + h) as f64];
    (at(column.all), at(column.row))
}

/// `frames` (each window's frame) put in `part`, as the board fills a place.
pub fn fill(part: Edges, frames: &[Edges], scale: f64) -> Place {
    let s = scale as f32;
    let rules = tiles::SlotRules { gap: 16.0 * s, foot: 0.0, least: (110.0 * s, 62.0 * s), icon: (24.0 * s, 56.0 * s) };
    let size = ((part[2] - part[0]) as f32, (part[3] - part[1]) as f32);
    let fill = tiles::fill_slot(frames.len(), size, &rules);
    let at = |(x, y, w, h): tiles::Part| [part[0] + x as f64, part[1] + y as f64, part[0] + (x + w) as f64, part[1] + (y + h) as f64];
    let shown = fill.cells.len() - usize::from(fill.more > 0);
    let more = (fill.more > 0).then(|| (at(fill.cells[shown]), fill.more));
    let spots = (0..frames.len())
        .map(|i| match fill.cells.get(i).filter(|_| i < shown) {
            Some(&cell) if fill.previews => Spot::Preview(Spot::Preview(at(cell)).fitted(frames[i])),
            Some(&cell) => Spot::Icon(at(cell)),
            None => Spot::Folded(more.map_or(part, |(cell, _)| cell)),
        })
        .collect();
    Place { spots, more }
}

/// A window at `real` going to `spot` by `share` (the screen's depth, 0
/// to 1): edges in between, and how opaque its picture is, an icon or a
/// fold taking its place at the end (`overlay`).
pub fn at(real: Edges, spot: Spot, share: f64) -> (Edges, f64) {
    let share = share.clamp(0.0, 1.0);
    let to = spot.fitted(real);
    let edges = std::array::from_fn(|i| real[i] + (to[i] - real[i]) * share);
    let shown = match spot { Spot::Preview(_) => 1.0, _ => 1.0 - overlay(share) };
    (edges, shown)
}

/// How opaque the icons and counts are, the screen's depth `share`.
pub fn overlay(share: f64) -> f64 {
    crate::follow::smoothstep((share - 0.55) / 0.4)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_places_are_the_board_s_at_the_left_smaller_than_the_cells_beside() {
        let (all, row) = places((1920.0, 1080.0), 1.0);
        assert_eq!(all[0], 40.0);
        assert!(all[3] < row[1] && all[0] == row[0] && all[2] == row[2]);
        // A cell beside the middle one is 35% across; a place under 20%.
        let wide = all[2] - all[0];
        assert!(wide / 1920.0 > 0.15 && wide / 1920.0 <= 0.2, "{wide}");
        let (all, _) = places((3840.0, 2160.0), 2.0);
        assert!(((all[2] - all[0]) / 3840.0 - wide / 1920.0).abs() < 0.01);
    }

    #[test]
    fn many_pinned_windows_go_from_pictures_to_icons_to_a_count() {
        let (all, _) = places((1920.0, 1080.0), 1.0);
        let frame = [100.0, 100.0, 1700.0, 1000.0];
        let one = fill(all, &[frame], 1.0);
        let Spot::Preview(p) = one.spots[0] else { panic!("{one:?}") };
        assert!(((p[2] - p[0]) / (p[3] - p[1]) - 1600.0 / 900.0).abs() < 0.01 && p[2] - p[0] < 0.2 * 1920.0);
        let some = fill(all, &[frame; 12], 1.0);
        assert!(some.spots.iter().all(|s| matches!(s, Spot::Icon(_))) && some.more.is_none(), "{some:?}");
        let many = fill(all, &vec![frame; 400], 1.0);
        let (cell, n) = many.more.unwrap();
        assert!(cell[2] - cell[0] >= 24.0 - 1e-9);
        assert_eq!(many.spots.iter().filter(|s| matches!(s, Spot::Folded(_))).count(), n);
    }

    #[test]
    fn a_pinned_window_goes_to_its_place_as_the_screen_recedes_and_back() {
        let real = [600.0, 200.0, 1400.0, 800.0];
        let spot = Spot::Preview([40.0, 62.0, 390.0, 262.0]);
        assert_eq!(at(real, spot, 0.0), (real, 1.0));
        let (whole, _) = at(real, spot, 1.0);
        assert!((0..4).all(|i| (whole[i] - spot.fitted(real)[i]).abs() < 1e-9));
        let (half, _) = at(real, spot, 0.5);
        assert!((0..4).all(|i| (half[i] - (real[i] + whole[i]) / 2.0).abs() < 1e-9));
        // An icon takes the place of its picture only near the end.
        let icon = Spot::Icon([40.0, 62.0, 96.0, 118.0]);
        assert_eq!(at(real, icon, 0.5).1, 1.0);
        assert_eq!(at(real, icon, 1.0).1, 0.0);
        assert_eq!(overlay(0.0), 0.0);
    }
}
