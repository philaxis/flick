//! The screen following the hand between desktops: where the picture is
//! while the hand moves, and the desktop it settles on once let go of.
//!
//! Plain arithmetic with no Windows in it. The hand moves one axis at a
//! time; the previous axis parks before a turn begins. Positions are in desktops from the one the hold began on (positive
//! rightwards and downwards); times in seconds.
//!
//! While the hand moves the picture is where the hand is, held back a
//! little at each desktop by its notches (`notched`) and only giving a
//! little past the first and the last (`rubber`). Let go of, it goes on to
//! the nearest desktop, or, flung, to where the fling would carry it
//! (`landing`, after GNOME's swipe tracker), easing out (`Snap`).
//!
//! The screen is a sheet in a strip of desktops: it recedes to `RECEDE` of
//! its size as the hand begins (`recede`), the strip slides along at that
//! size, and the desktop come to comes forward to the whole screen again
//! (`come_forward`). A single step (a hotkey) does all three in one
//! (`Step`). Where each thing is drawn: `on_strip`, `placed`.
//!
//! Only the row the hand is on goes sideways; the others stand with the
//! cell a move there lands on in the middle (`Follow::along`). The desktop
//! in the middle stands out (`emphasis`), and the name of the row is shown
//! above while receded (`Label`).

use crate::slide::notch;
use std::collections::VecDeque;

/// How far past the first or the last desktop the picture gives, at most.
pub const GIVE: f64 = 0.15;
/// Movement older than this when let go of does not count towards a fling.
const HISTORY: f64 = 0.15;
/// Slower than this when let go of, in desktops a second, is no fling: the
/// picture goes to the nearest desktop.
pub const FLING: f64 = 1.5;
/// How fast a fling slows down, per millisecond (GNOME's touchpad 0.997):
/// it carries on `speed * CARRY` desktops.
const DECELERATION: f64 = 0.997;
const CARRY: f64 = DECELERATION / (1.0 - DECELERATION) / 1000.0;
/// How long the picture takes to settle once let go of, at least and at
/// most: long enough to see it go, as in macOS Spaces.
pub const SNAP_MIN: f64 = 0.25;
pub const SNAP_MAX: f64 = 0.35;
/// Settling goes this many times as long as the hand would have taken at
/// the speed it was let go of, so it carries on as fast as the hand went
/// (easing out cubically starts at three times its average speed).
const SNAP_STRETCH: f64 = 3.0;
/// The speed settling is timed by when the hand was not going that way.
const SNAP_SPEED: f64 = 2.0;

/// How firm notches `notch` (0 to 100, `Config::mouse.notch_x` and the
/// like) are, as the minimap and the board take them (`notch::position`):
/// a touchpad's up and down twice as firm.
pub fn firmness(notch: u32, touchpad: bool, vertical: bool) -> f64 {
    let scale = if touchpad && vertical { notch::TOUCHPAD_VERTICAL_SCALE } else { 1 };
    notch.saturating_mul(scale) as f64 / 100.0
}

/// Where the picture is for a hand at `at`, with notches `firm`
/// (`firmness`): held back near each desktop, catching up on the way to
/// the next, as the minimap and the board are (`notch::position`). Every
/// desktop and every halfway point is where the hand is.
pub fn notched(at: f64, firm: f64) -> f64 {
    notch::position(at as f32, firm as f32) as f64
}

/// How far past the end the picture is for a hand `past` beyond it: as far
/// as the hand at first, never as far as `GIVE`.
pub fn rubber(past: f64) -> f64 {
    let past = past.max(0.0);
    GIVE * past / (past + GIVE)
}

/// Where the picture is for a hand at `at` along a way with desktops from
/// `room.0` to `room.1` (`room.0 <= 0 <= room.1`).
pub fn shown(at: f64, room: (i32, i32), firm: f64) -> f64 {
    let (first, last) = (room.0 as f64, room.1 as f64);
    if at < first {
        first - rubber(first - at)
    } else if at > last {
        last + rubber(at - last)
    } else {
        notched(at, firm)
    }
}

/// The desktop a hand let go of at `at` going `speed` (desktops a second)
/// comes to: the nearest one when slow, else where the fling would carry
/// it, and at least the next one on from where the hold began.
pub fn landing(at: f64, speed: f64, room: (i32, i32)) -> i32 {
    let (first, last) = (room.0 as f64, room.1 as f64);
    let at = at.clamp(first, last);
    if speed.abs() < FLING {
        return at.round() as i32;
    }
    let carried = (at + speed * CARRY).clamp(first, last);
    // GNOME: one carried no farther than the desktop the hold began on still
    // goes on to the next one that way.
    let (before, after) = (carried.floor(), carried.ceil());
    if speed > 0.0 && before == 0.0 {
        return after as i32;
    }
    if speed < 0.0 && after == 0.0 {
        return before as i32;
    }
    carried.round() as i32
}

/// How long settling from `from` to `to` takes when let go of at `speed`.
pub fn snap_time(from: f64, to: f64, speed: f64) -> f64 {
    let way = to - from;
    if way.abs() < 1e-6 {
        return 0.0;
    }
    let towards = if speed * way > 0.0 { speed.abs().max(SNAP_SPEED) } else { SNAP_SPEED };
    (way.abs() / towards * SNAP_STRETCH).clamp(SNAP_MIN, SNAP_MAX)
}

/// Settling once let go of: from where the picture was to a desktop.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Snap {
    pub from: f64,
    pub to: i32,
    pub began: f64,
    pub time: f64,
    /// How fast it goes as it begins, desktops a second: the hand's, so it
    /// goes on as fast as the hand went; at most three times as fast as on
    /// average, and never back, so it never goes past the desktop or back
    /// before going on. Without one (`None`) it eases out cubically, which
    /// begins at three times its average speed.
    pub speed: Option<f64>,
}

impl Snap {
    pub fn new(from: f64, to: i32, began: f64, time: f64) -> Snap {
        Snap { from, to, began, time, speed: None }
    }

    pub fn at(&self, now: f64) -> f64 {
        if self.time <= 0.0 {
            return self.to as f64;
        }
        let t = ((now - self.began) / self.time).clamp(0.0, 1.0);
        let way = self.to as f64 - self.from;
        self.from + way * match self.speed { Some(speed) => crate::axis::settle_with_speed(t, speed * self.time / way), None => crate::axis::settle(t) }
    }

    pub fn done(&self, now: f64) -> bool {
        // A hair under, for the rounding of `now - began`.
        now - self.began >= self.time - 1e-9
    }
}

/// How large the screen is while the strip slides, and the gap between
/// two desktops on it, as a share of the screen: far enough back that a
/// desktop each way, slantwise too, is clearly in sight.
pub const RECEDE: f64 = 0.6;
pub const GAP: f64 = 0.07;
/// The desktop in the middle, receded, is this much larger than the others
/// (`placed`), as if in front of them; within the gap.
pub const LIFT: f64 = 0.06;
/// How much darker the desktops not in the middle are, receded.
pub const DIM: f64 = 0.5;
/// How bright the thin edge of light round the desktop in the middle is.
pub const GLOW: f64 = 0.55;
/// How long receding takes as a hold begins, and coming forward once the
/// picture has settled.
pub const RECEDE_TIME: f64 = 0.12;
pub const FORWARD_TIME: f64 = 0.15;
/// Coming forward starts this long before settling ends, to run on from it.
const FORWARD_OVERLAP: f64 = 0.05;
/// A single step: how long it takes, and when along it the strip slides.
pub const STEP_TIME: f64 = 0.4;
const STEP_SLIDE: (f64, f64) = (0.05, 0.35);

pub fn smoothstep(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// How far the screen has receded (0 whole, 1 at `RECEDE`) `t` into a hold.
pub fn recede(t: f64) -> f64 {
    smoothstep(t / RECEDE_TIME)
}

/// How far it has receded `t` into a hold that began receded `from`: on
/// from there, as a hold that begins while the last one comes forward.
pub fn recede_from(from: f64, t: f64) -> f64 {
    let from = from.clamp(0.0, 1.0);
    from + (1.0 - from) * recede(t)
}

/// How far it is still receded `t` after coming forward began.
pub fn come_forward(t: f64) -> f64 {
    1.0 - smoothstep(t / FORWARD_TIME)
}

/// When coming forward begins after letting go, settling taking `settle`.
pub fn forward_after(settle: f64) -> f64 {
    (settle - FORWARD_OVERLAP).max(0.0)
}

/// The screen's size for receding `receded` (0 to 1).
pub fn scale(receded: f64) -> f64 {
    1.0 - (1.0 - RECEDE) * receded.clamp(0.0, 1.0)
}

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
    let near = |a: f64| (1.0 - a.abs()).clamp(0.0, 1.0);
    smoothstep(near(apart.0) * near(apart.1))
}

/// What a desktop with `emphasis` looks like, the screen receded
/// `receded`: how much larger (`placed`'s lift), and how dark (0 as it is,
/// `DIM` at most). Whole, everything is as it is.
pub fn standing_out(emphasis: f64, receded: f64) -> (f64, f64) {
    let (emphasis, receded) = (emphasis.clamp(0.0, 1.0), receded.clamp(0.0, 1.0));
    (1.0 + LIFT * emphasis * receded, DIM * (1.0 - emphasis) * receded)
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

/// A single step, or steps one upon another (a hotkey held or pressed
/// again): the screen recedes, the strip slides to the desktop, and it
/// comes forward, in `STEP_TIME`. A step taken while one is under way goes
/// on from where the strip is without coming forward in between.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Step {
    pub from: (f64, f64),
    pub to: (i32, i32),
    pub began: f64,
    /// How far receded it was as it began.
    pub receded: f64,
    /// How fast the strip went as it began, desktops a second each way: a
    /// step upon one goes on at the pace that one had, not from standing.
    pub speed: (f64, f64),
}

impl Step {
    pub fn new(now: f64, to: (i32, i32)) -> Step {
        Step { from: (0.0, 0.0), to, began: now, receded: 0.0, speed: (0.0, 0.0) }
    }

    /// One more step, to `to`, at `now`: from where this one is, as fast.
    pub fn then(&self, now: f64, to: (i32, i32)) -> Step {
        let (at, receded) = self.at(now);
        let (ahead, _) = self.at(now + 1e-5);
        let speed = ((ahead.0 - at.0) / 1e-5, (ahead.1 - at.1) / 1e-5);
        Step { from: at, to, began: now, receded, speed }
    }

    /// Where the strip is and how far the screen has receded.
    pub fn at(&self, now: f64) -> ((f64, f64), f64) {
        let t = now - self.began;
        // Already receded, the strip slides at once.
        let (start, end) = if self.receded > 0.0 { (0.0, STEP_SLIDE.1 - STEP_SLIDE.0) } else { STEP_SLIDE };
        let s = (t - start) / (end - start);
        let way = |from: f64, to: i32, speed: f64| {
            let way = to as f64 - from;
            if way.abs() < 1e-9 { from } else { from + way * crate::axis::settle_with_speed(s, speed * (end - start) / way) }
        };
        let at = (way(self.from.0, self.to.0, self.speed.0), way(self.from.1, self.to.1, self.speed.1));
        let total = self.time();
        let rising = self.receded + (1.0 - self.receded) * smoothstep(t / RECEDE_TIME);
        let falling = smoothstep((total - t) / FORWARD_TIME);
        (at, rising.min(falling))
    }

    pub fn time(&self) -> f64 {
        if self.receded > 0.0 { STEP_TIME - STEP_SLIDE.0 } else { STEP_TIME }
    }

    pub fn done(&self, now: f64) -> bool {
        // A hair under, for the rounding of `now - began`.
        now - self.began >= self.time() - 1e-9
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

/// Recent places of the hand one way, for how fast it went.
#[derive(Clone, Debug, Default)]
pub struct History {
    samples: VecDeque<(f64, f64)>,
}

impl History {
    pub fn add(&mut self, now: f64, at: f64) {
        while self.samples.front().is_some_and(|(then, _)| now - then > HISTORY) {
            self.samples.pop_front();
        }
        self.samples.push_back((now, at));
    }

    /// Desktops a second over the recent places; none with fewer than two.
    pub fn speed(&self) -> f64 {
        match (self.samples.front(), self.samples.back()) {
            (Some(&(t0, p0)), Some(&(t1, p1))) if t1 - t0 > 1e-3 => (p1 - p0) / (t1 - t0),
            _ => 0.0,
        }
    }
}

/// One way of a hold followed by the picture.
#[derive(Clone, Debug, Default)]
pub struct Way {
    pub firm: f64,
    pub history: History,
    /// Where the picture is, and once let go of, where it settles.
    pub shown: f64,
    pub snap: Option<Snap>,
}

/// Where a row stands sideways, in cells from the one a move there lands on.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Along {
    /// Where the hand is along it, while it is the row the hand is on;
    /// where it was left at (a cell) otherwise.
    pub at: f64,
    /// Where it is drawn.
    pub shown: f64,
    /// Going to the cell it was left nearest to, or settling.
    pub snap: Option<Snap>,
}

/// Up and down travel past halfway to the next row before the hand is on
/// it, sideways: less, and a hand going straight down crossing halfway with
/// a little wobble would go to and fro between the two.
const ROW_SWITCH: f64 = 0.08;
/// Both ways of a hold. Sideways the desktops there are depend on the row
/// (rows differ in length): `cols[row - rows.0]` for each row from
/// `rows.0` to `rows.1`, counted from the one the hold began on.
///
/// Only the row the hand is on goes sideways with it (`row`): a row come
/// to goes on from where it stands (the cell a move there lands on, at
/// first), the hand going along it from there; the row left goes to the
/// cell it is nearest to and stands there.
#[derive(Clone, Debug)]
pub struct Follow {
    pub rows: (i32, i32),
    pub cols: Vec<(i32, i32)>,
    pub x: Way,
    pub y: Way,
    /// The row the hand is on.
    pub row: i32,
    /// Where the hand was sideways (from where the hold began) where the
    /// row it is on is at 0.
    base: f64,
    /// Each row, from `rows.0`.
    pub along: Vec<Along>,
    picture: crate::axis::Picture,
    active_axis: usize,
}

impl Follow {
    /// `firm`: the notches sideways and up and down (`firmness`).
    pub fn new(rows: (i32, i32), cols: Vec<(i32, i32)>, firm: (f64, f64)) -> Follow {
        let way = |firm| Way { firm, ..Way::default() };
        let along = vec![Along::default(); (rows.1 - rows.0 + 1).max(1) as usize];
        Follow { rows, cols, x: way(firm.0), y: way(firm.1), row: 0, base: 0.0, along, picture: crate::axis::Picture::default(), active_axis: 0 }
    }

    /// The desktops along row `row` (counted from the first).
    pub fn cols_of(&self, row: i32) -> (i32, i32) {
        let row = row.clamp(self.rows.0, self.rows.1);
        self.cols.get((row - self.rows.0) as usize).copied().unwrap_or((0, 0))
    }

    fn along_mut(&mut self, row: i32) -> &mut Along {
        let at = (row.clamp(self.rows.0, self.rows.1) - self.rows.0) as usize;
        &mut self.along[at]
    }

    /// Where row `row` is drawn sideways: the cell along it in the middle.
    pub fn offset(&self, row: i32) -> f64 {
        if !(self.rows.0..=self.rows.1).contains(&row) {
            return 0.0;
        }
        self.along.get((row - self.rows.0) as usize).map_or(0.0, |along| along.shown)
    }

    /// The hand comes to row `to` at `now`, `hand` sideways: the row left
    /// goes to the cell nearest where it is, the hand goes on along `to`
    /// from where that stands, with what it went past that cell.
    fn come_to_row(&mut self, now: f64, to: i32, hand: f64) {
        let (left, cols) = (self.row, self.cols_of(self.row));
        let old = self.along_mut(left);
        let cell = old.at.round().clamp(cols.0 as f64, cols.1 as f64) as i32;
        (old.at, old.shown, old.snap) = (cell as f64, cell as f64, None);
        self.row = to;
        self.base = hand - self.along_mut(to).at;
        let _ = now;
    }

    /// Rows going to where they stand, or settling.
    fn ease(&mut self, now: f64) {
        for along in &mut self.along {
            if let Some(snap) = along.snap {
                along.shown = snap.at(now);
            }
        }
    }

    /// All renderers consume the same serial path, including delayed frames.
    pub fn track_locked(&mut self, now: f64, at: (f64, f64), axis: usize) -> (f64, f64) {
        let cols = self.cols_of(self.row);
        let target = if axis == 0 {
            [at.0, at.1.round().clamp(self.rows.0 as f64, self.rows.1 as f64)]
        } else {
            [(at.0 - self.base).round().clamp(cols.0 as f64, cols.1 as f64) + self.base, at.1]
        };
        let at = self.picture.track(target, axis, now);
        self.active_axis = self.picture.axis();
        self.track(now, (at[0], at[1]))
    }

    /// The hand is at `at` (desktops each way) at `now`: where the picture
    /// is, sideways along the row it is on, and up and down.
    pub fn track(&mut self, now: f64, at: (f64, f64)) -> (f64, f64) {
        self.x.history.add(now, at.0);
        self.y.history.add(now, at.1);
        self.y.shown = shown(at.1, self.rows, self.y.firm);
        let nearest = self.y.shown.round().clamp(self.rows.0 as f64, self.rows.1 as f64) as i32;
        if nearest != self.row && (self.y.shown - self.row as f64).abs() > 0.5 + ROW_SWITCH {
            self.come_to_row(now, nearest, at.0);
        }
        let (row, firm) = (self.row, self.x.firm);
        let cols = self.cols_of(row);
        let hand = at.0 - self.base;
        let along = self.along_mut(row);
        (along.at, along.shown, along.snap) = (hand, shown(hand, cols, firm), None);
        self.x.shown = along.shown;
        self.ease(now);
        (self.x.shown, self.y.shown)
    }

    /// Let go of with the hand at `at`: the desktop it settles on, sideways
    /// (along its row) and up and down; settling starts.
    pub fn let_go(&mut self, now: f64, at: (f64, f64)) -> (i32, i32) {
        let vertical = self.active_axis == 1;
        let row = if vertical { landing(at.1, self.y.history.speed(), self.rows) } else { self.y.shown.round().clamp(self.rows.0 as f64, self.rows.1 as f64) as i32 };
        let speed = if vertical { 0.0 } else { self.x.history.speed() };
        let col = if row == self.row {
            if vertical { self.offset(row).round() as i32 } else { landing(at.0 - self.base, speed, self.cols_of(row)) }
        } else {
            self.come_to_row(now, row, at.0);
            self.offset(row).round() as i32
        };
        let from = self.offset(row);
        let x = Snap::new(from, col, now, if vertical { 0.0 } else { snap_time(from, col as f64, speed) });
        self.along_mut(row).snap = Some(x);
        self.x.shown = from;
        self.x.snap = Some(x);
        let time = if vertical { snap_time(self.y.shown, row as f64, self.y.history.speed()) } else { 0.0 };
        self.y.snap = Some(Snap::new(self.y.shown, row, now, time));
        (col, row)
    }

    /// Where the picture is while settling, and whether it is there.
    pub fn settle(&mut self, now: f64) -> ((f64, f64), bool) {
        let mut done = true;
        for way in [&mut self.x, &mut self.y] {
            if let Some(snap) = way.snap {
                way.shown = snap.at(now);
                done &= snap.done(now);
            }
        }
        self.ease(now);
        ((self.x.shown, self.y.shown), done)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-5
    }

    /// Without notches the picture is where the hand is; with them it holds
    /// back near each desktop and catches up halfway, never going back.
    #[test]
    fn notches_hold_at_each_desktop_and_catch_up_between() {
        for at in [-1.7, -0.2, 0.0, 0.3, 0.5, 1.25, 2.0] {
            assert!(near(notched(at, 0.0), at), "{at}");
        }
        for firm in [0.4, 1.0, 2.0] {
            for at in [-1.0, -0.5, 0.0, 0.5, 1.0, 1.5] {
                assert!(near(notched(at, firm), at), "{firm} {at}");
            }
            // Held back after a desktop, ahead before the next.
            assert!(notched(0.1, firm) < 0.1 && notched(0.9, firm) > 0.9);
            let mut last = f64::MIN;
            for i in -300..=300 {
                let at = notched(i as f64 / 100.0, firm);
                assert!(at >= last - 1e-6, "goes back at {i} with {firm}");
                last = at;
            }
        }
        // Firmer holds back more; the touchpad firmer up and down only.
        assert!(notched(0.15, 1.0) < notched(0.15, 0.4));
        assert_eq!(firmness(100, true, true), 2.0);
        assert_eq!((firmness(100, false, true), firmness(40, true, false)), (1.0, 0.4));
        assert_eq!(firmness(0, true, true), 0.0);
    }

    /// Past the first or last desktop the picture gives a little, never as
    /// much as `GIVE`, and the farther the hand the less it gives for more.
    #[test]
    fn past_the_end_the_picture_gives_a_little() {
        let room = (-1, 2);
        assert!(near(shown(2.0, room, 0.0), 2.0));
        let a = shown(2.1, room, 0.0) - 2.0;
        let b = shown(2.5, room, 0.0) - 2.0;
        let c = shown(50.0, room, 0.0) - 2.0;
        assert!(0.0 < a && a < 0.1 && a < b && b < c && c < GIVE);
        assert!(near(shown(-1.0 - 0.3, room, 0.7), -1.0 - rubber(0.3)));
        assert!(shown(-9.0, room, 0.0) > -1.0 - GIVE);
        // At a lone desktop either way gives.
        assert!(shown(0.4, (0, 0), 1.0) < GIVE && shown(-0.4, (0, 0), 1.0) > -GIVE);
    }

    /// Slowly let go of: the nearest desktop. Flung: on to where the fling
    /// carries, at least the next one, never past the end.
    #[test]
    fn let_go_slowly_goes_to_the_nearest_and_flung_goes_on() {
        let room = (-3, 3);
        assert_eq!(landing(0.4, 0.0, room), 0);
        assert_eq!(landing(0.6, 0.0, room), 1);
        assert_eq!(landing(-0.6, 1.0, room), -1);
        assert_eq!(landing(1.4, -1.0, room), 1);
        // A little flick from where the hold began is the next desktop.
        assert_eq!(landing(0.2, 2.0, room), 1);
        assert_eq!(landing(-0.1, -2.0, room), -1);
        // A fast one carries farther.
        assert_eq!(landing(0.5, 6.0, room), 2);
        assert_eq!(landing(0.5, 30.0, room), 3);
        // Flung back towards where it began from beyond the next.
        assert_eq!(landing(1.6, -2.0, room), 1);
        // Flung back past where it began: the next desktop the other way.
        assert_eq!(landing(0.4, -2.0, room), -1);
        // Past the end and at a lone desktop.
        assert_eq!(landing(3.1, 8.0, room), 3);
        assert_eq!(landing(0.3, 5.0, (0, 0)), 0);
        assert_eq!(landing(-0.3, -5.0, (0, 3)), 0);
    }

    /// Settling eases out over a quarter to two fifths of a second.
    #[test]
    fn settling_eases_out_in_about_a_third_of_a_second() {
        assert_eq!(snap_time(1.0, 1.0, 0.0), 0.0);
        for (from, to, speed) in [(0.4, 0, 0.0), (0.6, 1, 0.0), (0.2, 1, 10.0), (0.9, 1, 50.0), (2.7, 0, -1.0)] {
            let time = snap_time(from, to as f64, speed);
            assert!((SNAP_MIN..=SNAP_MAX).contains(&time), "{from} {to} {speed}: {time}");
        }
        // Fast to go far is shorter than slowly.
        assert!(snap_time(0.7, 1.0, 6.0) < snap_time(0.7, 1.0, 0.0));
        let snap = Snap::new(0.4, 1, 10.0, 0.3);
        assert!(near(snap.at(10.0), 0.4));
        assert!(near(snap.at(10.3), 1.0) && snap.done(10.3) && !snap.done(10.29));
        assert!(near(snap.at(11.0), 1.0));
        let mid = snap.at(10.15);
        assert!(mid > 0.4 + 0.6 * 0.5, "eases out: {mid}");
        for i in 0..=30 {
            let at = snap.at(10.0 + i as f64 * 0.01);
            assert!((0.4 - 1e-9..=1.04).contains(&at));
        }
    }

    /// How fast the hand went is over the last moments only: a hand that
    /// stopped before letting go is no fling.
    #[test]
    fn speed_is_over_the_last_moments() {
        let mut history = History::default();
        assert_eq!(history.speed(), 0.0);
        for i in 0..=10 {
            history.add(i as f64 * 0.016, i as f64 * 0.08);
        }
        assert!((history.speed() - 5.0).abs() < 1e-6);
        for i in 11..=30 {
            history.add(i as f64 * 0.016, 0.8);
        }
        assert!(history.speed().abs() < 1e-9);
    }

    /// The screen recedes about its middle, its neighbours beside it with a
    /// gap, and comes back whole: nothing moves at the whole screen.
    #[test]
    fn the_strip_recedes_about_the_middle() {
        let size = (1000.0, 600.0);
        let screen = [0.0, 0.0, 1000.0, 600.0];
        assert_eq!(on_strip(screen, size, 1.0, (0.0, 0.0)), screen);
        let small = on_strip(screen, size, RECEDE, (0.0, 0.0));
        assert!((small[0] - 500.0 * (1.0 - RECEDE)).abs() < 1e-9 && (small[3] - 300.0 * (1.0 + RECEDE)).abs() < 1e-9);
        let right = on_strip(screen, size, RECEDE, (1.0, 0.0));
        let gap = right[0] - small[2];
        assert!((gap - GAP * 1000.0 * RECEDE).abs() < 1e-9, "{gap}");
        // A desktop next along each way, slantwise too, is clearly in sight:
        // more than a tenth of the screen of it.
        assert!(right[0] < 900.0, "{right:?}");
        let below = on_strip(screen, size, RECEDE, (0.0, 1.0));
        assert!((below[1] - small[3] - GAP * 600.0 * RECEDE).abs() < 1e-9);
        assert!(below[1] < 540.0, "{below:?}");
        let slantwise = on_strip(screen, size, RECEDE, (-1.0, -1.0));
        assert!(slantwise[2] > 100.0 && slantwise[3] > 60.0, "{slantwise:?}");
        // A window keeps its place on its desktop.
        let window = [100.0, 100.0, 300.0, 200.0];
        let drawn = on_strip(window, size, RECEDE, (0.0, 0.0));
        assert!((drawn[0] - (500.0 - 400.0 * RECEDE)).abs() < 1e-9 && (drawn[2] - drawn[0] - 200.0 * RECEDE).abs() < 1e-9);
        assert_eq!(on_strip(window, size, 1.0, (0.0, 0.0)), window);
        assert_eq!(between(window, screen, 0.0), window);
        assert_eq!(between(window, screen, 1.0), screen);
        assert!((scale(0.0) - 1.0).abs() < 1e-12 && (scale(1.0) - RECEDE).abs() < 1e-12);
        assert_eq!((recede(0.0), recede(RECEDE_TIME), come_forward(0.0), come_forward(FORWARD_TIME)), (0.0, 1.0, 1.0, 0.0));
        assert!(forward_after(0.3) < 0.3 && forward_after(0.0) == 0.0);
    }

    /// A step recedes, slides and comes forward in about 0.4 s; another
    /// upon it goes on from where the strip is, still receded.
    #[test]
    fn a_step_recedes_slides_and_comes_forward() {
        let step = Step::new(1.0, (1, 0));
        assert_eq!(step.at(1.0), ((0.0, 0.0), 0.0));
        let ((x, _), receded) = step.at(1.2);
        assert!(x > 1.03 && x < 1.06 && receded == 1.0, "{x} {receded}");
        let ((x, y), receded) = step.at(1.0 + STEP_TIME);
        assert!((x - 1.0).abs() < 1e-9 && y == 0.0 && receded < 1e-9, "{x} {y} {receded}");
        assert!(step.done(1.0 + STEP_TIME) && !step.done(1.3));
        for i in 0..=40 {
            let ((x, _), _) = step.at(1.0 + i as f64 * 0.01);
            assert!((-1e-9..=1.06).contains(&x));
        }
        // Again, halfway: on to the next without coming forward.
        let again = step.then(1.2, (2, 0));
        let (at, receded) = again.at(1.2);
        assert!((at.0 - x_at(&step, 1.2)).abs() < 1e-9 && receded == 1.0);
        for i in 0..20 {
            assert!(again.at(1.2 + i as f64 * 0.01).1 > 0.99, "comes forward between steps at {i}");
        }
        assert!((again.at(1.2 + again.time()).0 .0 - 2.0).abs() < 1e-9);
        fn x_at(step: &Step, now: f64) -> f64 {
            step.at(now).0 .0
        }
    }

    /// A whole hold: slantwise, sideways in a short row, let go of slowly
    /// and flung.
    #[test]
    fn locked_frames_park_the_old_axis_before_a_new_row_moves() {
        let mut f = Follow::new((-2, 0), vec![(0, 2); 3], (0.4, 1.0));
        let mut last = (0.0, 0.0);
        for frame in 0..240 {
            let axis = if frame < 80 || frame >= 160 { 0 } else { 1 };
            let target = if frame < 80 { (0.7, 0.0) } else if frame < 160 { (1.0, -1.0) } else { (1.8, -1.0) };
            let at = f.track_locked(frame as f64 / 120.0, target, axis);
            assert!((at.0 - at.0.round()).abs() < 1e-5 || (at.1 - at.1.round()).abs() < 1e-5, "{frame}: {at:?}");
            if frame > 160 { assert!(f.offset(0).fract().abs() < 1e-5); }
            assert!((at.1 - last.1).abs() < 0.2);
            last = at;
        }
        assert_eq!(f.row, -1);
        assert!(f.offset(-1) > 0.5 && f.offset(-2).abs() < 1e-6);
    }

    /// Back on a row left, the hand goes on along it from where it stood.
    #[test]
    fn a_row_come_back_to_goes_on_from_where_it_stood() {
        let mut follow = Follow::new((0, 1), vec![(0, 4), (0, 4)], (0.0, 0.0));
        let mut now = 0.0;
        for (x, y) in [(1.0, 0.0), (2.0, 0.0), (2.0, 0.7), (2.0, 1.0), (3.0, 1.0), (3.0, 0.3), (3.0, 0.0)] {
            for _ in 0..30 {
                now += 0.016;
                follow.track(now, (x, y));
            }
        }
        assert_eq!(follow.row, 0);
        // The first row stood at 2, the second went one along (to 1).
        assert!((follow.offset(0) - 2.0).abs() < 1e-6 && (follow.offset(1) - 1.0).abs() < 1e-6);
        assert_eq!(follow.let_go(now, (3.0, 0.0)), (2, 0));
    }

    /// Flung down from a row gone along: on to the row below, at the cell
    /// it stands with, not where the hand is along the first.
    #[test]
    fn flung_to_another_row_it_lands_where_that_row_stands() {
        let mut follow = Follow::new((0, 1), vec![(0, 4), (0, 4)], (0.0, 0.0));
        for i in 0..20 {
            follow.track(i as f64 * 0.016, (2.0, 0.0));
        }
        for i in 0..8 {
            follow.track(0.32 + i as f64 * 0.016, (2.0, i as f64 * 0.04));
        }
        follow.active_axis = 1;
        assert_eq!(follow.let_go(0.45, (2.0, 0.3)), (0, 1));
    }

    /// Settling goes on as fast as the hand went, whatever time it takes,
    /// never back and never past the desktop.
    #[test]
    fn settling_begins_at_the_speed_of_the_hand() {
        // Slowly let go of short of the next desktop: begins at the hand's speed.
        let snap = Snap { speed: Some(1.0), ..Snap::new(0.7, 1, 0.0, SNAP_MIN) };
        let speed = (snap.at(1e-6) - snap.at(0.0)) / 1e-6;
        assert!((speed - 1.0).abs() < 0.01, "{speed}");
        assert!((snap.at(SNAP_MIN) - 1.0).abs() < 1e-9 && snap.done(SNAP_MIN + 1e-9));
    }

    /// A step upon a step goes on at the pace that one had.
    #[test]
    fn a_step_upon_a_step_keeps_its_pace() {
        let step = Step::new(0.0, (1, 0));
        let now = 0.10;
        let pace = |step: &Step, now: f64| (step.at(now + 1e-6).0 .0 - step.at(now).0 .0) / 1e-6;
        let before = pace(&step, now);
        let again = step.then(now, (2, 0));
        let after = pace(&again, now);
        assert!(before > 1.0 && (after - before).abs() < before * 0.05, "{before} then {after}");
        assert!((again.at(now + again.time()).0 .0 - 2.0).abs() < 1e-9);
    }

    /// The desktop in the middle stands out: as large as it is and lit, the
    /// others smaller and darker; going from one to the next, it goes over
    /// smoothly; whole, nothing stands out.
    #[test]
    fn the_desktop_in_the_middle_stands_out() {
        assert!((emphasis((0.0, 0.0)) - 1.0).abs() < 1e-12);
        for apart in [(1.0, 0.0), (0.0, -1.0), (1.0, 1.0), (-2.0, 0.3)] {
            assert!(emphasis(apart).abs() < 1e-12, "{apart:?}");
        }
        // Halfway the two share it alike; it never jumps.
        assert!((emphasis((0.5, 0.0)) - emphasis((-0.5, 0.0))).abs() < 1e-12);
        let mut last = emphasis((-1.0, 0.0));
        for i in -100..=100 {
            let e = emphasis((i as f64 / 100.0, 0.0));
            assert!((e - last).abs() < 0.05, "{i}");
            last = e;
        }
        let (lift, dark) = standing_out(1.0, 1.0);
        assert!((lift - (1.0 + LIFT)).abs() < 1e-12 && dark.abs() < 1e-12);
        let (lift, dark) = standing_out(0.0, 1.0);
        assert!((lift - 1.0).abs() < 1e-12 && (dark - DIM).abs() < 1e-12);
        let (lift, dark) = standing_out(0.3, 0.0);
        assert!((lift - 1.0).abs() < 1e-12 && dark.abs() < 1e-12);
        // Larger, it stays clear of the desktops beside it.
        let size = (1000.0, 600.0);
        let screen = [0.0, 0.0, 1000.0, 600.0];
        let middle = placed(screen, size, RECEDE, (0.0, 0.0), 1.0 + LIFT);
        let right = placed(screen, size, RECEDE, (1.0, 0.0), 1.0);
        let below = placed(screen, size, RECEDE, (0.0, 1.0), 1.0);
        assert!(middle[2] < right[0] - 5.0 && middle[3] < below[1] - 3.0, "{middle:?} {right:?} {below:?}");
        assert!((middle[0] + middle[2] - 1000.0).abs() < 1e-9, "about its own middle");
        assert_eq!(placed(screen, size, 1.0, (0.0, 0.0), 1.0), screen);
    }

    /// The name of the row goes over to the next one's, the new one coming
    /// from the way the hand goes; back halfway, from as far as it went.
    #[test]
    fn the_row_name_goes_over_smoothly() {
        let mut label = Label::new(0);
        assert_eq!(label.at(0.0), ((0, 1.0, 0.0), None));
        label.show(1.0, 0);
        assert_eq!(label.at(1.0), ((0, 1.0, 0.0), None));
        label.show(1.0, 1);
        let ((row, shown, slide), going) = label.at(1.0);
        assert!(row == 1 && shown.abs() < 1e-9 && slide > 0.0);
        let (was, gone, _) = going.unwrap();
        assert!(was == 0 && (gone - 1.0).abs() < 1e-9);
        let ((_, half, _), going) = label.at(1.0 + LABEL_TIME / 2.0);
        assert!((half - 0.5).abs() < 1e-9 && (going.unwrap().1 - 0.5).abs() < 1e-9);
        // Back to the first halfway: it is as far as it was, coming back.
        label.show(1.0 + LABEL_TIME / 2.0, 0);
        let ((row, shown, slide), _) = label.at(1.0 + LABEL_TIME / 2.0);
        assert!(row == 0 && (shown - 0.5).abs() < 1e-9 && slide < 0.0, "{shown} {slide}");
        let (done, going) = label.at(10.0);
        assert!(done == (0, 1.0, 0.0) && going.is_none());
        let mut last = 0.0;
        for i in 0..=20 {
            let ((_, shown, _), _) = label.at(1.0 + LABEL_TIME / 2.0 + i as f64 * LABEL_TIME / 20.0);
            assert!(shown >= last - 1e-12);
            last = shown;
        }
    }

    /// A hold begun while the last one comes forward goes on receding from
    /// as far as that one had.
    #[test]
    fn a_hold_upon_a_hold_recedes_on_from_where_it_was() {
        assert!((recede_from(0.0, 0.05) - recede(0.05)).abs() < 1e-12);
        assert!((recede_from(0.6, 0.0) - 0.6).abs() < 1e-12);
        assert!((recede_from(0.6, RECEDE_TIME) - 1.0).abs() < 1e-12);
        assert!(recede_from(0.6, 0.03) > 0.6);
    }

    /// In a single step the row the strip is on goes, the others stand.
    #[test]
    fn in_a_step_other_rows_stand() {
        assert!((step_along(0.6, 0.0, 0) - 0.6).abs() < 1e-12);
        assert!(step_along(0.6, 0.0, 1).abs() < 1e-12 && step_along(0.6, 0.0, -1).abs() < 1e-12);
        assert!((step_along(1.0, 0.5, 1) - 0.5).abs() < 1e-12);
    }
}
