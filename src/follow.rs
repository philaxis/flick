//! Variant F on the desktop grid; the same springs drive the board and minimap.
use crate::transition_curve::{Latch, Spring};
pub const GAP: f64 = 0.07;
pub const GLOW: f64 = 0.55;
pub const LIFT: f64 = 0.0;
pub const DIM: f64 = 0.55;
pub fn smoothstep(t: f64) -> f64 { let t = t.clamp(0.0, 1.0); t * t * (3.0 - 2.0 * t) }
pub fn scale(depth: f64) -> f64 { 1.0 - 0.3 * depth.clamp(0.0, 1.0) }
/// Where `rect` (left, top, right, bottom on a screen `size` across and
/// down) of a desktop `apart` desktops from where the strip is (sideways,
/// up and down) is drawn, the screen at `scale`: the strip's desktops side
/// by side with `GAP` between, shrunk about the middle of the screen.
pub fn on_strip(rect: [f64; 4], size: (f64, f64), scale: f64, apart: (f64, f64)) -> [f64; 4] {
    let (w, h) = size;
    let (cx, cy) = (w / 2.0 + apart.0 * w * (1.0 + GAP) * scale, h / 2.0 + apart.1 * h * (1.0 + GAP) * scale);
    [cx + (rect[0] - w / 2.0) * scale, cy + (rect[1] - h / 2.0) * scale, cx + (rect[2] - w / 2.0) * scale, cy + (rect[3] - h / 2.0) * scale]
}

/// As `on_strip`, the desktop `lift` times as large about its own middle
/// (`emphasis`), the strip's spacing as it is.
pub fn placed(rect: [f64; 4], size: (f64, f64), scale: f64, apart: (f64, f64), lift: f64) -> [f64; 4] {
    let (w, h) = size;
    let (cx, cy) = (w / 2.0 + apart.0 * w * (1.0 + GAP) * scale, h / 2.0 + apart.1 * h * (1.0 + GAP) * scale);
    let size = scale * lift;
    [cx + (rect[0] - w / 2.0) * size, cy + (rect[1] - h / 2.0) * size, cx + (rect[2] - w / 2.0) * size, cy + (rect[3] - h / 2.0) * size]
}

/// How much a desktop `apart` (sideways, up and down) from the middle of
/// the screen is the one in the middle: 1 there, 0 a desktop or more away,
/// shared out between those on either side as the strip goes from one to
/// the next, so that standing out goes over from one to the next smoothly.
pub fn emphasis(apart: (f64, f64)) -> f64 {
    1.0 - apart.0.hypot(apart.1).min(1.0)
}

/// What a desktop with `emphasis` looks like, the screen receded
/// `receded`: how much larger (`placed`'s lift), and how dark (0 as it is,
/// `DIM` at most). Whole, everything is as it is.
pub fn standing_out(emphasis: f64, receded: f64) -> (f64, f64) {
    let (emphasis, receded) = (emphasis.clamp(0.0, 1.0), receded.clamp(0.0, 1.0));
    (1.0 + LIFT * emphasis * receded, DIM * (1.0 - emphasis))
}

/// Where a row stands sideways in a single step (`Step`), the strip at
/// `(x, y)`: the row the strip is on goes with it, the others stand where
/// they began; going from row to row, in between.
pub fn step_along(x: f64, y: f64, row: i32) -> f64 {
    x * (1.0 - (row as f64 - y).abs()).clamp(0.0, 1.0)
}

/// `a` going to `b` by `t` (0 to 1).
pub fn between(a: [f64; 4], b: [f64; 4], t: f64) -> [f64; 4] {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Step {
    pub from: (f64, f64), pub to: (i32, i32), pub began: f64,
    pub receded: f64, pub speed: (f64, f64), pub depth_speed: f64,
}
impl Step {
    pub fn new(now: f64, to: (i32, i32)) -> Self {
        Self { from: (0.0, 0.0), to, began: now, receded: 0.0, speed: (0.0, 0.0), depth_speed: 6.0 }
    }
    pub fn advance(&mut self, now: f64) {
        let dt = (now - self.began).max(0.0);
        let mut x = Spring { p: self.from.0, v: self.speed.0 };
        let mut y = Spring { p: self.from.1, v: self.speed.1 };
        x.position(self.to.0 as f64, dt, false); y.position(self.to.1 as f64, dt, true);
        let moving = (x.p - self.to.0 as f64).abs() > 0.01 || (y.p - self.to.1 as f64).abs() > 0.01;
        let mut depth = Spring { p: self.receded, v: self.depth_speed };
        depth.advance(if moving { 1.0 } else { 0.0 }, dt, if moving { 22.0 } else { 14.0 }, 1.0);
        (self.from, self.speed, self.receded, self.depth_speed, self.began) = ((x.p, y.p), (x.v, y.v), depth.p, depth.v, now);
    }
    pub fn then(&self, now: f64, to: (i32, i32)) -> Self {
        let mut next = *self;
        next.advance(now); next.to = to; next.depth_speed += 6.0; next
    }
    pub fn at(&self) -> ((f64, f64), f64) { (self.from, self.receded.clamp(0.0, 1.0)) }
    pub fn done(&self) -> bool {
        Spring { p: self.from.0, v: self.speed.0 }.settled(self.to.0 as f64)
            && Spring { p: self.from.1, v: self.speed.1 }.settled(self.to.1 as f64)
            && Spring { p: self.receded, v: self.depth_speed }.settled(0.0)
    }
}
/// How long the name of the row above the picture takes to go over to
/// another row's, and how far the names slide as they do, in heights.
pub const LABEL_TIME: f64 = 0.2;
pub const LABEL_SLIDE: f64 = 0.4;

/// The name of the row above the picture: the row the hand is on, or the
/// picture settles on, the one before fading out as it comes in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Label {
    pub row: i32,
    pub was: Option<i32>,
    pub since: f64,
}

/// A name shown: its row, how much of it shows (0 to 1), and how far it is
/// from its place, in heights (below positive).
pub type Name = (i32, f64, f64);

impl Label {
    pub fn new(row: i32) -> Label {
        Label { row, was: None, since: f64::NEG_INFINITY }
    }

    fn progress(&self, now: f64) -> f64 {
        ((now - self.since) / LABEL_TIME).clamp(0.0, 1.0)
    }

    /// The name shown is `row`'s from `now`. Back to the one going, it
    /// comes back from as far as it went.
    pub fn show(&mut self, now: f64, row: i32) {
        if row == self.row {
            return;
        }
        let done = self.progress(now);
        let back = self.was == Some(row) && done < 1.0;
        self.was = Some(self.row);
        self.row = row;
        self.since = if back { now - (1.0 - done) * LABEL_TIME } else { now };
    }

    /// The name coming (or there), and the one going, if any: the new one
    /// comes from the way the hand goes, the old one goes the other way.
    pub fn at(&self, now: f64) -> (Name, Option<Name>) {
        let t = smoothstep(self.progress(now));
        let Some(was) = self.was.filter(|_| t < 1.0) else { return ((self.row, 1.0, 0.0), None) };
        let way = (self.row - was).signum() as f64;
        ((self.row, t, way * (1.0 - t) * LABEL_SLIDE), Some((was, 1.0 - t, -way * t * LABEL_SLIDE)))
    }
}

/// Each row keeps its own committed column and spring; changing rows never
/// waits for the previous spring to park before accepting the next input.
#[derive(Clone, Debug)]
pub struct Follow {
    pub rows: (i32, i32), pub cols: Vec<(i32, i32)>, pub row: i32,
    along: Vec<Spring>, columns: Vec<Latch>, vertical: Spring, rows_hand: Latch,
    depth: Spring, last: Option<f64>, axis: usize, hand: (f64, f64), released: bool, path_cursor: usize,
}
impl Follow {
    pub fn new(rows: (i32, i32), cols: Vec<(i32, i32)>) -> Self {
        let n = (rows.1 - rows.0 + 1).max(1) as usize;
        Self { rows, cols, row: 0, along: vec![Spring::default(); n], columns: vec![Latch::default(); n],
            vertical: Spring::default(), rows_hand: Latch::default(), depth: Spring::default(),
            last: None, axis: 0, hand: (0.0, 0.0), released: false, path_cursor: 0 }
    }
    pub fn cols_of(&self, row: i32) -> (i32, i32) {
        self.cols.get((row - self.rows.0) as usize).copied().unwrap_or((0, 0))
    }
    pub fn offset(&self, row: i32) -> f64 { self.along.get((row - self.rows.0) as usize).map_or(0.0, |s| s.p) }
    pub fn depth(&self) -> f64 { self.depth.p.clamp(0.0, 1.0) }
    pub fn reframe(&mut self, rows: (i32, i32), cols: Vec<(i32, i32)>, hand: (f64, f64), row_changed: bool) {
        if row_changed {
            let depth = self.depth;
            let (last, path_cursor) = (self.last, self.path_cursor);
            *self = Self::new(rows, cols);
            self.depth = depth; self.last = last; self.hand = hand; self.path_cursor = path_cursor;
            for column in &mut self.columns { column.rebase(hand.0); }
            self.rows_hand.rebase(hand.1);
        } else {
            self.cols = cols;
            for (column, &(left, right)) in self.columns.iter_mut().zip(&self.cols) { column.cell = column.cell.clamp(left, right); }
        }
    }
    pub fn velocity(&self) -> ((f64, f64), f64) {
        ((self.along[(self.row - self.rows.0) as usize].v, self.vertical.v), self.depth.v)
    }
    pub fn carry(&mut self, at: (f64, f64), speed: (f64, f64), depth: f64, depth_speed: f64) {
        self.along[(-self.rows.0) as usize] = Spring { p: at.0, v: speed.0 };
        self.vertical = Spring { p: at.1, v: speed.1 }; self.depth = Spring { p: depth, v: depth_speed };
    }
    fn advance(&mut self, now: f64) -> ((f64, f64), bool) {
        let dt = self.last.replace(now).map_or(0.0, |last| (now - last).max(0.0));
        let current = (self.row - self.rows.0) as usize;
        let mut moving = !self.released;
        for i in 0..self.along.len() {
            let target = if !self.released && self.axis == 0 && i == current { self.columns[i].target(self.cols[i]) } else { self.columns[i].cell as f64 };
            self.along[i].position(target, dt, false);
            moving |= (self.along[i].p - self.columns[i].cell as f64).abs() > 0.01;
        }
        let target = if !self.released && self.axis == 1 { self.rows_hand.target(self.rows) } else { self.row as f64 };
        self.vertical.position(target, dt, true);
        moving |= (self.vertical.p - self.row as f64).abs() > 0.01;
        self.depth.advance(if moving { 1.0 } else { 0.0 }, dt, if moving { 22.0 } else { 14.0 }, 1.0);
        let done = self.released && self.vertical.settled(self.row as f64)
            && self.along.iter().zip(&self.columns).all(|(s, l)| s.settled(l.cell as f64)) && self.depth.settled(0.0);
        ((self.along[current].p, self.vertical.p), done)
    }
    pub fn track_locked(&mut self, now: f64, at: (f64, f64), axis: usize) -> (f64, f64) {
        // Integrate the previous target up to this input before retargeting it.
        self.advance(now);
        self.axis = axis;
        self.hand = at;
        self.released = false;
        let old_row = self.row;
        let commits = if axis == 1 {
            let n = self.rows_hand.track(at.1, self.rows);
            self.row = self.rows_hand.cell;
            if old_row != self.row { self.columns[(self.row - self.rows.0) as usize].rebase(at.0); }
            n
        } else {
            let i = (self.row - self.rows.0) as usize;
            self.columns[i].track(at.0, self.cols[i])
        };
        self.depth.v += 5.0 * commits.abs() as f64;
        (self.offset(self.row), self.vertical.p)
    }
    pub fn track_path(&mut self, now: f64, path: &[(f64, f64, usize)]) {
        for &(x, y, axis) in path.iter().skip(self.path_cursor) { self.track_locked(now, (x, y), axis); }
        // The last straight leg keeps growing until the next turn.
        self.path_cursor = path.len().saturating_sub(1);
    }
    pub fn finish_path(&mut self, path: &[(f64, f64, usize)]) -> (i32, i32) {
        self.track_path(0.0, path);
        self.let_go(0.0, self.hand)
    }
    pub fn let_go(&mut self, now: f64, at: (f64, f64)) -> (i32, i32) {
        self.track_locked(now, at, self.axis); self.released = true;
        (self.columns[(self.row - self.rows.0) as usize].cell, self.row)
    }
    pub fn settle(&mut self, now: f64) -> ((f64, f64), bool) { self.advance(now) }
}

/// Keep reversals and axis turns even when thumbnail setup misses input frames.
pub fn remember(path: &mut Vec<(f64, f64, usize)>, point: (f64, f64, usize)) {
    if let Some(&last) = path.last() {
        let before = path.get(path.len().saturating_sub(2)).filter(|_| path.len() > 1).copied().unwrap_or((0.0, 0.0, point.2));
        let along = |p: (f64, f64, usize)| if point.2 == 0 { p.0 } else { p.1 };
        if last.2 == point.2 && (along(last) - along(before)) * (along(point) - along(last)) >= 0.0 { path.pop(); }
    }
    path.push(point);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn down_then_right_commits_without_waiting_and_releases_without_fling() {
        let mut f = Follow::new((0, 2), vec![(0, 3); 3]);
        f.track_locked(0.0, (0.0, 1.0), 1);
        assert_eq!(f.row, 1);
        f.track_locked(0.01, (1.0, 1.0), 0);
        assert_eq!(f.let_go(0.02, (1.0, 1.0)), (1, 1));
        for i in 3..250 { f.settle(i as f64 * 0.01); }
        assert!((f.offset(1) - 1.0).abs() < 1e-6 && f.offset(0).abs() < 1e-6);
        assert!(f.settle(3.0).1);
    }
    #[test]
    fn an_immediate_row_change_rebases_the_followed_column() {
        let mut f = Follow::new((0, 0), vec![(0, 3)]);
        f.track_locked(0.0, (1.4, 0.0), 0);
        f.reframe((0, 0), vec![(-1, 0)], (1.4, 0.0), true);
        assert_eq!(f.finish_path(&[(0.4, 0.0, 0)]), (-1, 0));
    }
    #[test]
    fn a_key_step_uses_the_same_position_and_depth_springs() {
        let mut step = Step::new(0.0, (1, 0));
        let mut hand = Follow::new((0, 0), vec![(0, 2)]);
        hand.track_locked(0.0, (1.0, 0.0), 0);
        hand.depth.v += 1.0; // Keys kick by six; a held commit by five.
        hand.let_go(0.0, (1.0, 0.0));
        for i in 1..200 {
            let now = i as f64 / 120.0;
            step.advance(now);
            let (at, _) = hand.settle(now);
            assert!((step.at().0.0 - at.0).abs() < 1e-9);
            assert!((step.at().1 - hand.depth()).abs() < 1e-9);
        }
        assert!(step.done());
        let old_speed = step.speed;
        let speed = step.then(199.0 / 120.0, (2, 0)).speed;
        assert!((speed.0 - old_speed.0).abs() < 1e-6 && (speed.1 - old_speed.1).abs() < 1e-6);
    }
    #[test]
    fn compressed_input_keeps_turns_and_reversals() {
        let mut path = Vec::new();
        for point in [(0.0, 0.5, 1), (0.0, 1.0, 1), (0.5, 1.0, 0), (9.0, 1.0, 0), (8.0, 1.0, 0)] { remember(&mut path, point); }
        assert_eq!(path, vec![(0.0, 1.0, 1), (9.0, 1.0, 0), (8.0, 1.0, 0)]);
        let mut f = Follow::new((0, 2), vec![(0, 2); 3]);
        assert_eq!(f.finish_path(&path), (1, 1));
    }
    #[test]
    fn missed_frames_keep_commits_and_edge_reversal_responds() {
        let mut f = Follow::new((-2, 0), vec![(0, 2); 3]);
        assert_eq!(f.finish_path(&[(0.0, -1.0, 1), (1.0, -1.0, 0)]), (1, -1));
        let mut f = Follow::new((0, 0), vec![(-2, 0)]);
        f.track_locked(0.0, (9.0, 0.0), 0);
        f.track_locked(0.01, (8.0, 0.0), 0);
        assert_eq!(f.let_go(0.02, (8.0, 0.0)), (-1, 0));
    }
}
