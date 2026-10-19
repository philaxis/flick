//! The screen following the hand between desktops: where the picture is
//! while the hand moves, and the desktop it settles on once let go of.
//!
//! Plain arithmetic with no Windows in it. Each way (sideways, up and down)
//! goes on its own, so a hand going slantwise shows up to four desktops at
//! once. Positions are in desktops from the one the hold began on (positive
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
//! (`Step`). Where each thing is drawn: `on_strip`.

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
pub const SNAP_MIN: f64 = 0.2;
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

pub fn ease_out_cubic(t: f64) -> f64 {
    let t = 1.0 - t.clamp(0.0, 1.0);
    1.0 - t * t * t
}

/// Settling once let go of: from where the picture was to a desktop.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Snap {
    pub from: f64,
    pub to: i32,
    pub began: f64,
    pub time: f64,
}

impl Snap {
    pub fn at(&self, now: f64) -> f64 {
        if self.time <= 0.0 {
            return self.to as f64;
        }
        let t = (now - self.began) / self.time;
        self.from + (self.to as f64 - self.from) * ease_out_cubic(t)
    }

    pub fn done(&self, now: f64) -> bool {
        now - self.began >= self.time
    }
}

/// How large the screen is while the strip slides, and the gap between
/// two desktops on it, as a share of the screen.
pub const RECEDE: f64 = 0.9;
pub const GAP: f64 = 0.03;
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

pub fn ease_in_out_cubic(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 { 4.0 * t * t * t } else { 1.0 - (-2.0 * t + 2.0).powi(3) / 2.0 }
}

/// How far the screen has receded (0 whole, 1 at `RECEDE`) `t` into a hold.
pub fn recede(t: f64) -> f64 {
    smoothstep(t / RECEDE_TIME)
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
}

impl Step {
    pub fn new(now: f64, to: (i32, i32)) -> Step {
        Step { from: (0.0, 0.0), to, began: now, receded: 0.0 }
    }

    /// One more step, to `to`, at `now`: from where this one is.
    pub fn then(&self, now: f64, to: (i32, i32)) -> Step {
        let (at, receded) = self.at(now);
        Step { from: at, to, began: now, receded }
    }

    /// Where the strip is and how far the screen has receded.
    pub fn at(&self, now: f64) -> ((f64, f64), f64) {
        let t = now - self.began;
        // Already receded, the strip slides at once.
        let (start, end) = if self.receded > 0.0 { (0.0, STEP_SLIDE.1 - STEP_SLIDE.0) } else { STEP_SLIDE };
        let slid = ease_in_out_cubic((t - start) / (end - start));
        let at = (self.from.0 + (self.to.0 as f64 - self.from.0) * slid, self.from.1 + (self.to.1 as f64 - self.from.1) * slid);
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

/// Both ways of a hold. Sideways the desktops there are depend on the row
/// (rows differ in length): `cols[row - rows.0]` for each row from
/// `rows.0` to `rows.1`, counted from the one the hold began on.
#[derive(Clone, Debug)]
pub struct Follow {
    pub rows: (i32, i32),
    pub cols: Vec<(i32, i32)>,
    pub x: Way,
    pub y: Way,
}

impl Follow {
    /// `firm`: the notches sideways and up and down (`firmness`).
    pub fn new(rows: (i32, i32), cols: Vec<(i32, i32)>, firm: (f64, f64)) -> Follow {
        let way = |firm| Way { firm, ..Way::default() };
        Follow { rows, cols, x: way(firm.0), y: way(firm.1) }
    }

    /// The desktops along row `row` (counted from the first).
    pub fn cols_of(&self, row: i32) -> (i32, i32) {
        let row = row.clamp(self.rows.0, self.rows.1);
        self.cols.get((row - self.rows.0) as usize).copied().unwrap_or((0, 0))
    }

    /// The hand is at `at` (desktops each way) at `now`: where the picture is.
    pub fn track(&mut self, now: f64, at: (f64, f64)) -> (f64, f64) {
        self.x.history.add(now, at.0);
        self.y.history.add(now, at.1);
        self.y.shown = shown(at.1, self.rows, self.y.firm);
        let row = at.1.round() as i32;
        self.x.shown = shown(at.0, self.cols_of(row), self.x.firm);
        (self.x.shown, self.y.shown)
    }

    /// Let go of with the hand at `at`: the desktop it settles on, sideways
    /// and up and down; settling starts.
    pub fn let_go(&mut self, now: f64, at: (f64, f64)) -> (i32, i32) {
        self.track(now, at);
        let row = landing(at.1, self.y.history.speed(), self.rows);
        let col = landing(at.0, self.x.history.speed(), self.cols_of(row));
        for (way, to) in [(&mut self.x, col), (&mut self.y, row)] {
            let speed = way.history.speed();
            let time = snap_time(way.shown, to as f64, speed);
            way.snap = Some(Snap { from: way.shown, to, began: now, time });
        }
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
        let snap = Snap { from: 0.4, to: 1, began: 10.0, time: 0.3 };
        assert!(near(snap.at(10.0), 0.4));
        assert!(near(snap.at(10.3), 1.0) && snap.done(10.3) && !snap.done(10.29));
        assert!(near(snap.at(11.0), 1.0));
        let mid = snap.at(10.15);
        assert!(mid > 0.4 + 0.6 * 0.5, "eases out: {mid}");
        let mut last = 0.4;
        for i in 0..=30 {
            let at = snap.at(10.0 + i as f64 * 0.01);
            assert!(at >= last - 1e-12);
            last = at;
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
        assert!((small[0] - 50.0).abs() < 1e-9 && (small[3] - 570.0).abs() < 1e-9);
        let right = on_strip(screen, size, RECEDE, (1.0, 0.0));
        let gap = right[0] - small[2];
        assert!((gap - GAP * 1000.0 * RECEDE).abs() < 1e-9, "{gap}");
        // A desktop next along is just in sight at the edge, past the gap.
        assert!(right[0] < 1000.0 && right[0] > 950.0);
        let below = on_strip(screen, size, RECEDE, (0.0, 1.0));
        assert!((below[1] - small[3] - GAP * 600.0 * RECEDE).abs() < 1e-9);
        // A window keeps its place on its desktop.
        let window = [100.0, 100.0, 300.0, 200.0];
        let drawn = on_strip(window, size, RECEDE, (0.0, 0.0));
        assert!((drawn[0] - (500.0 - 400.0 * RECEDE)).abs() < 1e-9 && (drawn[2] - drawn[0] - 200.0 * RECEDE).abs() < 1e-9);
        assert_eq!(on_strip(window, size, 1.0, (0.0, 0.0)), window);
        assert_eq!(between(window, screen, 0.0), window);
        assert_eq!(between(window, screen, 1.0), screen);
        assert_eq!((scale(0.0), scale(1.0)), (1.0, RECEDE));
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
        assert!(x > 0.3 && x < 0.7 && receded == 1.0, "{x} {receded}");
        let ((x, y), receded) = step.at(1.0 + STEP_TIME);
        assert!((x - 1.0).abs() < 1e-9 && y == 0.0 && receded < 1e-9, "{x} {y} {receded}");
        assert!(step.done(1.0 + STEP_TIME) && !step.done(1.3));
        let mut last = 0.0;
        for i in 0..=40 {
            let ((x, _), _) = step.at(1.0 + i as f64 * 0.01);
            assert!(x >= last - 1e-12);
            last = x;
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
    fn a_hold_goes_both_ways_on_its_own() {
        // Rows above and below; the one below is shorter to the right.
        let cols = vec![(-1, 2), (-1, 3), (0, 1)];
        let mut follow = Follow::new((-1, 1), cols.clone(), (0.0, 0.0));
        let at = follow.track(0.0, (0.4, 0.3));
        assert!(near(at.0, 0.4) && near(at.1, 0.3));
        // In the row below the right end is nearer.
        let at = follow.track(0.016, (1.4, 0.7));
        assert!(at.0 > 1.0 && at.0 < 1.0 + GIVE);
        for i in 0..20 {
            follow.track(0.032 + i as f64 * 0.016, (1.4, 0.7));
        }
        let to = follow.let_go(0.4, (1.4, 0.7));
        assert_eq!(to, (1, 1));
        let ((x, y), done) = follow.settle(0.4);
        assert!(!done && x > 1.0 && y < 1.0);
        let ((x, y), done) = follow.settle(0.4 + SNAP_MAX);
        assert!(done && near(x, 1.0) && near(y, 1.0));
        // Flung right in the middle row from a little way: the next desktop.
        let mut flung = Follow::new((-1, 1), cols, (0.24, 1.0));
        for i in 0..8 {
            flung.track(i as f64 * 0.016, (i as f64 * 0.04, 0.0));
        }
        assert_eq!(flung.let_go(0.13, (0.3, 0.0)), (1, 0));
        // Let go of where it began: nothing to settle.
        let mut still = Follow::new((0, 0), vec![(0, 0)], (0.0, 0.0));
        assert_eq!(still.let_go(0.0, (0.0, 0.0)), (0, 0));
        assert!(still.settle(0.0).1);
    }
}
