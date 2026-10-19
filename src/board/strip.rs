//! The board's preview area as a strip of desktops, as the full-screen view
//! shows them: while the selection moves, a faint plate is put round the
//! area, the cell recedes in it, its neighbours on either side and in the
//! rows above and below show at the plate's edges, and the whole strip
//! slides along until the cell come to fills the plate again.
//!
//! Plain arithmetic with no Windows in it. Positions are in cells from the
//! selected one (positive rightwards and downwards), times in seconds from
//! the board's own epoch. Each row is laid out with its column 0 at
//! `marks()[row].0`: the selected row by the selection, the others by the
//! cell a vertical move would land on (their anchor), as in the map.
//!
//! How things move is `transition_curve`: a discrete move recedes, slides
//! and comes forward in `STEP`; moves made while one is under way carry on
//! from where the strip is; the hand moves it one to one.

use crate::{
    grid::{CellId, Grid, Pos},
    paint::Rect,
    transition_curve::{self as curve, Glide, Recede},
};

/// How large a receded picture is, of the plate.
pub const RECEDE: f32 = 0.64;
/// Between two pictures, of the plate's shorter side at full size.
pub const GAP: f32 = 0.04;
/// How much darker the neighbours are than the cell in the middle, all the
/// way back.
pub const DIM: f32 = 0.55;
/// The most live previews the neighbours have between them.
pub const MAX_THUMBS: usize = 32;

/// Where each row's column 0 is, and the row itself, from the selection at
/// `at`.
pub fn marks(grid: &Grid, at: Pos) -> Vec<(f32, f32)> {
    grid.rows.iter().enumerate().map(|(r, row)| {
        let anchor = if r == at.row { at.col } else { row.anchor() };
        (-(anchor as f32), r as f32 - at.row as f32)
    }).collect()
}

/// The cells shown beside the selection at `at`: the ones left and right
/// of it, and those a move up or down would land on.
pub fn neighbours(grid: &Grid, at: Pos) -> Vec<Pos> {
    let mut out = Vec::new();
    let Some(row) = grid.rows.get(at.row) else { return out };
    for col in [at.col.checked_sub(1), Some(at.col + 1)].into_iter().flatten() {
        if col < row.cells.len() {
            out.push(Pos { row: at.row, col });
        }
    }
    for r in [at.row.checked_sub(1), Some(at.row + 1)].into_iter().flatten() {
        if let Some(other) = grid.rows.get(r).filter(|other| !other.cells.is_empty()) {
            out.push(Pos { row: r, col: other.anchor().min(other.cells.len() - 1) });
        }
    }
    out
}

/// The cells shown while moving from `from` to `to`: the neighbours of
/// both, the two themselves when not the selection, each once.
pub fn shown(grid: &Grid, from: Option<Pos>, to: Pos) -> Vec<(CellId, Pos)> {
    let mut out: Vec<(CellId, Pos)> = Vec::new();
    let around = |at: Pos| std::iter::once(at).chain(neighbours(grid, at));
    for pos in around(to).chain(from.into_iter().flat_map(around)) {
        if pos == to || out.iter().any(|(_, p)| *p == pos) {
            continue;
        }
        if let Some(id) = grid.id_at(pos) {
            out.push((id.clone(), pos));
        }
    }
    out
}

/// How many live previews each of the cells with `wanted` windows gets,
/// at most `most` in all: one each in turn, nearest first.
pub fn budget(wanted: &[usize], most: usize) -> Vec<usize> {
    let mut given = vec![0; wanted.len()];
    let mut left = most;
    while left > 0 {
        let mut any = false;
        for (g, w) in given.iter_mut().zip(wanted) {
            if left > 0 && *g < *w {
                *g += 1;
                left -= 1;
                any = true;
            }
        }
        if !any {
            break;
        }
    }
    given
}

/// How a picture is drawn: `p` goes to `p * s + (dx, dy)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Place {
    pub s: f32,
    pub dx: f32,
    pub dy: f32,
}

impl Place {
    pub const SAME: Place = Place { s: 1.0, dx: 0.0, dy: 0.0 };

    pub fn rect(&self, r: &Rect) -> Rect {
        Rect {
            left: r.left * self.s + self.dx,
            top: r.top * self.s + self.dy,
            right: r.right * self.s + self.dx,
            bottom: r.bottom * self.s + self.dy,
        }
    }

    pub fn is_same(&self) -> bool {
        *self == Place::SAME
    }
}

/// From one picture to the next, across and down, for pictures `scale` of
/// the plate.
pub fn pitch(plate: &Rect, scale: f32) -> (f32, f32) {
    let (w, h) = (plate.right - plate.left, plate.bottom - plate.top);
    let gap = GAP * w.min(h);
    ((w + gap) * scale, (h + gap) * scale)
}

/// The picture `at` cells from the middle of `plate`, `size` of it, the
/// pictures being `spacing` of it apart. It moves by whole pixels, so that
/// what is drawn and the live previews over it move together.
pub fn place(plate: &Rect, at: (f32, f32), size: f32, spacing: f32) -> Place {
    let (cx, cy) = ((plate.left + plate.right) / 2.0, (plate.top + plate.bottom) / 2.0);
    let (px, py) = pitch(plate, spacing);
    let (ox, oy) = (cx + at.0 * px, cy + at.1 * py);
    let place = Place { s: size, dx: (ox - size * cx).round(), dy: (oy - size * cy).round() };
    if (size - 1.0).abs() < 1e-6 && place.dx == 0.0 && place.dy == 0.0 { Place::SAME } else { place }
}

/// What of `dest` shows inside `within`, and which part of a picture of
/// `source` size that is.
pub fn clip(dest: &Rect, within: &Rect, source: (f32, f32)) -> Option<(Rect, Rect)> {
    let shown = Rect {
        left: dest.left.max(within.left),
        top: dest.top.max(within.top),
        right: dest.right.min(within.right),
        bottom: dest.bottom.min(within.bottom),
    };
    let (w, h) = (dest.right - dest.left, dest.bottom - dest.top);
    if shown.right - shown.left < 1.0 || shown.bottom - shown.top < 1.0 || w <= 0.0 || h <= 0.0 {
        return None;
    }
    let x = |v: f32| (v - dest.left) / w * source.0;
    let y = |v: f32| (v - dest.top) / h * source.1;
    Some((shown, Rect { left: x(shown.left), top: y(shown.top), right: x(shown.right), bottom: y(shown.bottom) }))
}

/// How bright a picture is when the strip is `depth` back.
pub fn shade(depth: f32, middle: bool) -> f32 {
    if middle { 1.0 } else { 1.0 - DIM * depth.clamp(0.0, 1.0) }
}

/// The size of a picture `depth` back.
pub fn scale(depth: f32) -> f32 {
    1.0 - (1.0 - RECEDE) * depth.clamp(0.0, 1.0)
}

/// Nothing left to go and not moving.
fn still(glide: &Glide, t: f32) -> bool {
    glide.settled(t) || (glide.at(t).abs() < 1e-6 && glide.velocity(t).abs() < 1e-6)
}

/// Where the strip is: what is left of each move, as `transition_curve`
/// has it, and the hand.
#[derive(Default, Debug)]
pub struct Strip {
    /// How far back the cells are.
    recede: Recede,
    /// How far back the windows pinned to the selected row are: only moves
    /// to another row take them back.
    row_recede: Recede,
    /// What is left of each row's move sideways, and of the rows' move up
    /// or down, in cells.
    along: Vec<Glide>,
    across: Glide,
    /// The hand's pull in the last frame (cells, as `hand_pull`) and the
    /// row it pulled; what it was counted from; how fast it went.
    hand: (f32, f32),
    hand_row: usize,
    counted_from: Option<u64>,
    speed: (f32, f32),
    heard: Option<f32>,
    holding: bool,
}

impl Strip {
    fn along_mut(&mut self, row: usize) -> &mut Glide {
        if self.along.len() <= row {
            self.along.resize(row + 1, Glide::default());
        }
        &mut self.along[row]
    }

    /// The rows were at `before` and are now at `after` (`marks`): each is
    /// still drawn where it was and goes on from there. Unless the hand
    /// holds the strip, the move recedes and comes forward again.
    pub fn moved(&mut self, before: &[(f32, f32)], after: &[(f32, f32)], now: f32) {
        let slide = curve::SLIDE_END - curve::SLIDE_START;
        let mut end: Option<f32> = None;
        for (row, (was, is)) in before.iter().zip(after).enumerate() {
            let jump = was.0 - is.0;
            if jump.abs() > 1e-6 {
                let glide = self.along_mut(row);
                glide.retarget(jump, now, slide);
                end = Some(end.unwrap_or(0.0).max(glide.end()));
            }
        }
        let rows = before.first().zip(after.first()).map_or(0.0, |(was, is)| was.1 - is.1);
        if rows.abs() > 1e-6 {
            self.across.retarget(rows, now, slide);
            end = Some(end.unwrap_or(0.0).max(self.across.end()));
            if self.holding {
                self.row_recede.hold(now);
            } else {
                self.row_recede.step(now, self.across.end());
            }
        }
        if let (Some(end), false) = (end, self.holding) {
            self.recede.step(now, end);
        }
    }

    /// One frame at `now` with the hand pulling the selected `row` by `hand`
    /// (cells), counted from `counted_from`. A hold recedes the strip; let
    /// go, it settles and comes forward, going on as fast as the hand went.
    pub fn follow(&mut self, hand: (f32, f32), row: usize, counted_from: u64, holding: bool, now: f32) {
        let hand = if holding { hand } else { (0.0, 0.0) };
        if holding && !self.holding {
            self.recede.hold(now);
            self.speed = (0.0, 0.0);
        }
        if holding && hand.1.abs() > 0.02 {
            self.row_recede.hold(now);
        }
        let changed = self.counted_from.is_some_and(|from| from != counted_from);
        if self.holding && !holding {
            // Let go: what the hand had pulled is left to go, at its speed.
            let (old, old_row, speed) = (self.hand, self.hand_row, self.speed);
            if old.0 != 0.0 || speed.0 != 0.0 {
                self.along_mut(old_row).release(-old.0, -speed.0, now);
            }
            if old.1 != 0.0 || speed.1 != 0.0 {
                self.across.release(-old.1, -speed.1, now);
            }
            let end = self.along.iter().chain([&self.across]).map(Glide::end).fold(now, f32::max);
            self.recede.release(now, end);
            self.row_recede.release(now, end);
        } else if changed {
            // Counted from elsewhere: the jump is left to go, so nothing jumps.
            let (old, old_row) = (self.hand, self.hand_row);
            if old.0 != 0.0 {
                self.along_mut(old_row).retarget(-old.0, now, curve::SNAP);
            }
            if hand.0 != 0.0 {
                self.along_mut(row).retarget(hand.0, now, curve::SNAP);
            }
            if old.1 != hand.1 {
                self.across.retarget(hand.1 - old.1, now, curve::SNAP);
            }
        } else if holding {
            if let Some(heard) = self.heard.filter(|heard| now > *heard + 1e-4) {
                let dt = now - heard;
                let blend = |was: f32, is: f32| 0.5 * was + 0.5 * is;
                self.speed = (blend(self.speed.0, (hand.0 - self.hand.0) / dt), blend(self.speed.1, (hand.1 - self.hand.1) / dt));
            }
        }
        (self.hand, self.hand_row, self.counted_from, self.holding, self.heard) = (hand, row, Some(counted_from), holding, Some(now));
    }

    pub fn lock_axis(&mut self, axis: usize) {
        if axis == 0 {
            self.across = Glide::default();
            self.hand.1 = 0.0;
            self.speed.1 = 0.0;
        } else {
            self.along.fill(Glide::default());
            self.hand.0 = 0.0;
            self.speed.0 = 0.0;
        }
    }

    /// How far the cells of `row` are drawn from where they are laid out,
    /// in cells.
    pub fn offset(&self, row: usize, t: f32) -> (f32, f32) {
        let along = self.along.get(row).map_or(0.0, |glide| glide.at(t));
        let hand = if row == self.hand_row { self.hand } else { (0.0, 0.0) };
        (along - hand.0, self.across.at(t) - hand.1)
    }

    pub fn depth(&self, t: f32) -> f32 {
        self.recede.depth(t)
    }

    pub fn row_depth(&self, t: f32) -> f32 {
        self.row_recede.depth(t)
    }

    /// Anything moving or back at `t`, or a hand holding it.
    pub fn active(&self, t: f32) -> bool {
        self.holding
            || !self.recede.settled(t)
            || !self.row_recede.settled(t)
            || !still(&self.across, t)
            || self.along.iter().any(|glide| !still(glide, t))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::Row;
    use crate::paint::rect;

    const FRAME: f32 = 1.0 / 240.0;

    fn grid(rows: &[(usize, usize)]) -> Grid {
        let rows = rows.iter().enumerate().map(|(r, &(n, anchor))| {
            let cells: Vec<CellId> = (0..n).map(|c| format!("{r}.{c}")).collect();
            Row { last: cells.get(anchor).cloned(), cells, ..Default::default() }
        }).collect();
        Grid { rows, ..Default::default() }
    }

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    /// Beside the selection: left, right, and where moving up or down
    /// would land, lined up under it; none past the ends.
    #[test]
    fn neighbours_are_either_side_and_the_landing_cells_of_the_rows_around() {
        let g = grid(&[(3, 2), (4, 1), (1, 0)]);
        assert_eq!(neighbours(&g, Pos { row: 1, col: 1 }), vec![
            Pos { row: 1, col: 0 }, Pos { row: 1, col: 2 }, Pos { row: 0, col: 2 }, Pos { row: 2, col: 0 },
        ]);
        assert_eq!(neighbours(&g, Pos { row: 0, col: 0 }), vec![Pos { row: 0, col: 1 }, Pos { row: 1, col: 1 }]);
        // Every row has its landing cell in the selection's column.
        let m = marks(&g, Pos { row: 1, col: 1 });
        assert!(near(m[0].0 + 2.0, 0.0) && near(m[1].0 + 1.0, 0.0) && near(m[2].0, 0.0));
        assert_eq!((m[0].1, m[1].1, m[2].1), (-1.0, 0.0, 1.0));
        // Going right shows what was beside both, the cell left once.
        let ids: Vec<CellId> = shown(&g, Some(Pos { row: 1, col: 1 }), Pos { row: 1, col: 2 }).into_iter().map(|(id, _)| id).collect();
        assert_eq!(ids, ["1.1", "1.3", "0.2", "2.0", "1.0"]);
    }

    /// Live previews are shared out one each in turn, never past the most.
    #[test]
    fn previews_are_shared_out_up_to_the_most() {
        assert_eq!(budget(&[5, 1, 3], 6), vec![3, 1, 2]);
        assert_eq!(budget(&[2, 2], 10), vec![2, 2]);
        assert_eq!(budget(&[40, 40], MAX_THUMBS).iter().sum::<usize>(), MAX_THUMBS);
    }

    /// In front the cell fills the plate and its neighbours are off it;
    /// back, it is in the middle at `RECEDE` with a little of each
    /// neighbour showing at the edges, the same gap all round.
    #[test]
    fn receded_neighbours_show_at_the_edges_of_the_plate() {
        let plate = rect(40.0, 64.0, 1840.0, 900.0);
        assert!(place(&plate, (0.0, 0.0), 1.0, 1.0).is_same());
        let right = place(&plate, (1.0, 0.0), 1.0, 1.0).rect(&plate);
        assert!(right.left > plate.right);
        let s = RECEDE;
        let middle = place(&plate, (0.0, 0.0), s, s).rect(&plate);
        assert!((middle.right - middle.left - 1840.0 * s).abs() < 1.5);
        assert!(((middle.left + middle.right) / 2.0 - 960.0).abs() <= 1.0);
        let (right, below) = (place(&plate, (1.0, 0.0), s, s).rect(&plate), place(&plate, (0.0, 1.0), s, s).rect(&plate));
        assert!(right.left > middle.right && right.left < plate.right - 100.0, "{right:?}");
        assert!(below.top > middle.bottom && below.top < plate.bottom - 60.0, "{below:?}");
        assert!(((right.left - middle.right) - (below.top - middle.bottom)).abs() <= 2.0);
        // Moved by whole pixels.
        let p = place(&plate, (0.37, -0.21), 0.8, 0.8);
        assert_eq!((p.dx.fract(), p.dy.fract()), (0.0, 0.0));
    }

    /// A preview half out of the plate shows only its inner half, from the
    /// matching half of the window.
    #[test]
    fn previews_are_cut_at_the_plate() {
        let within = rect(0.0, 0.0, 100.0, 100.0);
        let (shown, source) = clip(&rect(50.0, 20.0, 100.0, 50.0), &within, (1000.0, 500.0)).unwrap();
        assert_eq!(shown, rect(50.0, 20.0, 50.0, 50.0));
        assert!(near(source.left, 0.0) && near(source.right, 500.0) && near(source.bottom, 500.0));
        assert!(clip(&rect(120.0, 0.0, 50.0, 50.0), &within, (10.0, 10.0)).is_none());
    }

    /// The neighbours darken as the strip goes back; the middle never.
    #[test]
    fn neighbours_darken_with_depth_only() {
        assert!(near(shade(0.0, false), 1.0));
        assert!(near(shade(1.0, false), 1.0 - DIM));
        assert!(near(shade(1.0, true), 1.0));
        assert!(near(scale(1.0), RECEDE) && near(scale(0.0), 1.0));
    }

    fn times(from: f32, until: f32) -> impl Iterator<Item = f32> {
        (0..=((until - from) / FRAME).ceil() as usize).map(move |i| from + i as f32 * FRAME)
    }

    /// A move right: the strip recedes, the selected row slides one cell
    /// with no jump while the other rows stay, and in about `STEP` the cell
    /// gone to is in front in the middle and all is still.
    #[test]
    fn a_step_sideways_slides_its_row_only_and_settles_in_front() {
        let g = grid(&[(3, 0), (3, 0), (3, 0)]);
        let before = marks(&g, Pos { row: 1, col: 0 });
        let mut g2 = g.clone();
        g2.visit("1.1");
        let after = marks(&g2, Pos { row: 1, col: 1 });
        let mut strip = Strip::default();
        strip.moved(&before, &after, 0.0);
        // Drawn where it was at once: the old cell in the middle.
        assert!(near(strip.offset(1, 0.0).0, 1.0));
        let mut last = strip.offset(1, 0.0).0;
        let mut deepest = 0.0f32;
        for t in times(0.0, curve::STEP + 1e-3) {
            let (x, y) = strip.offset(1, t);
            assert!(x <= last + 1e-5 && (last - x) < 0.1, "{t}: {last} to {x}");
            let other = strip.offset(0, t);
            assert!(near(other.0, 0.0) && near(other.1, 0.0), "other rows stay");
            assert!(near(y, 0.0));
            assert!(strip.active(t) || t >= curve::STEP - 1e-3);
            deepest = deepest.max(strip.depth(t));
            last = x;
        }
        assert!(deepest > 0.99);
        assert!(strip.row_depth(0.2) < 1e-6, "this row's windows stay");
        let end = curve::STEP + 1e-3;
        assert!(strip.offset(1, end).0.abs() < 1e-6 && strip.depth(end) < 1e-6 && !strip.active(end + 1e-3));
    }

    /// A move down slides every row up together, and takes the windows
    /// pinned to the row back with it.
    #[test]
    fn a_step_down_slides_all_rows_and_takes_row_windows_back() {
        let g = grid(&[(3, 1), (3, 2), (2, 0)]);
        let before = marks(&g, Pos { row: 0, col: 1 });
        let after = marks(&g, Pos { row: 1, col: 2 });
        let mut strip = Strip::default();
        strip.moved(&before, &after, 0.0);
        for row in 0..3 {
            let (x, y) = strip.offset(row, 0.0);
            assert!(near(after[row].0 + x, before[row].0) && near(after[row].1 + y, before[row].1), "row {row}");
        }
        assert!(strip.row_depth(0.2) > 0.99);
        assert!(strip.offset(2, curve::STEP + 1e-3).1.abs() < 1e-6);
    }

    /// Moves in a row carry on without a jump in place or speed and stay
    /// back between them.
    #[test]
    fn moves_in_a_row_carry_on() {
        let mut strip = Strip::default();
        let at = |col: f32| vec![(0.0, 0.0), (-col, 0.0)];
        strip.moved(&at(0.0), &at(1.0), 0.0);
        let mut target = 1.0;
        // Where the middle of the strip is, in cells from the first.
        let mut last = target - strip.offset(1, 0.0).0;
        for t in times(0.0, 1.0) {
            if (t - 0.12).abs() < FRAME / 2.0 || (t - 0.24).abs() < FRAME / 2.0 {
                strip.moved(&at(target), &at(target + 1.0), t);
                target += 1.0;
            }
            let x = target - strip.offset(1, t).0;
            assert!((x - last).abs() < 0.08, "{t}: {last} to {x}");
            assert!(x <= target + 0.02);
            if t > curve::STEP_RECEDE && t < 0.3 {
                assert!(strip.depth(t) > 0.99, "{t}");
            }
            last = x;
        }
        assert!(near(target - strip.offset(1, 1.0).0, 3.0) && strip.depth(1.0) < 1e-6);
    }

    /// The hand moves the strip one to one; the app getting to the next
    /// cell changes nothing drawn; let go, it settles on the cell and comes
    /// forward with no jump.
    #[test]
    fn the_hand_moves_it_one_to_one_and_nothing_jumps() {
        let mut strip = Strip::default();
        let drawn = |strip: &Strip, layout: f32, t: f32| layout + strip.offset(0, t).0;
        strip.follow((0.0, 0.0), 0, 1, true, 0.0);
        assert!(strip.active(0.0));
        strip.follow((0.5, 0.0), 0, 1, true, 0.1);
        assert!(near(drawn(&strip, 0.0, 0.1), -0.5));
        strip.follow((0.98, 0.0), 0, 1, true, 0.15);
        assert!(strip.depth(0.15) > 0.99);
        let was = drawn(&strip, 0.0, 0.15);
        // The app gets there first, then the hand is counted from it.
        strip.moved(&[(0.0, 0.0)], &[(-1.0, 0.0)], 0.16);
        strip.follow((-0.02, 0.0), 0, 2, true, 0.16);
        assert!((drawn(&strip, -1.0, 0.16) - was).abs() < 0.01);
        // Let go.
        strip.follow((0.0, 0.0), 0, 3, false, 0.17);
        assert!((drawn(&strip, -1.0, 0.17) - was).abs() < 0.02);
        let mut last = drawn(&strip, -1.0, 0.17);
        for t in times(0.17, 0.8) {
            let x = drawn(&strip, -1.0, t);
            assert!((x - last).abs() < 0.06, "{t}");
            last = x;
        }
        assert!(near(drawn(&strip, -1.0, 0.8), -1.0) && strip.depth(0.8) < 1e-6 && !strip.active(0.8));
    }
}
