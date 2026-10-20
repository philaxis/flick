//! How far the screen recedes as the hand goes (`depth`, 0 to 1), as a
//! marking menu shows itself only to a hand that hesitates: a hand that
//! flicks (a stroke set off sharply, whatever its length or how it ends)
//! keeps the screen whole and the desktop it goes to slides in (or comes
//! at once, `QuickFlick::Instant`); flicking again soon after a stroke
//! ends, it stays so, each stroke going as far as it went, and let go
//! still flung very fast, a cell farther (`Judge::coast`). Every other hand is
//! taken as unsure, soon (still, set off slowly, stopped a while, turning
//! back, going the other way): the screen goes all the way back at once,
//! not by halves, and stays so until let go: a hand gone unsure does not
//! get the screen whole again by going on quickly. A hold begun just
//! after one let go of sure is sure from the start; one let go of unsure
//! carries nothing over.
//!
//! The hand is read in cells, which each device counts in its own units
//! (mouse counts, touchpad steps of the pad's own size, `input`), so a
//! flick is judged alike on either.
//!
//! The judging (`Judge`) only reads the hand. Whole, the screen is where
//! the hand is, cell for cell, as it goes (`follow::Follow::held`), and let
//! go of sure it lands on the cell nearest where it was going, coast and
//! all (`Follow::let_go_near`); unsure, the cells gone to are the hand's
//! latches (`follow::Follow`), switched to once, on letting go.
use crate::{config::QuickFlick, transition_curve::Spring};
use std::sync::atomic::{AtomicU8, Ordering};

/// How far the hand goes before it counts as on its way, in cells.
pub const START: f64 = 0.02;
/// Not set off this soon after the hold begins (s), the hand is unsure.
pub const ONSET_LIMIT: f64 = 0.15;
/// A stroke is a flick when, within `DECIDE` (s) of setting off, it gets
/// as fast as `PEAK` (cells a second) for a moment: a sharp start, not a
/// speed kept up. Not so, it is unsure.
pub const DECIDE: f64 = 0.08;
pub const PEAK: f64 = 5.0;
/// Sure, a stroke goes on as long as it goes `KEEP` or faster; slower,
/// it has ended, and another flick (as fast as `PEAK` again) must come
/// within `PAUSE` (s) of that, or the hand is unsure.
pub const KEEP: f64 = 2.5;
pub const PAUSE: f64 = 0.25;
/// Let go of sure, it goes as far as the hand went; only still going as
/// fast as `FLING` (cells a second) does it go on `COAST_MOST` more, a cell.
pub const FLING: f64 = 12.0;
pub const COAST_MOST: f64 = 1.0;
/// Turning back this far (cells) from the farthest it went is unsure.
pub const BACK: f64 = 0.15;
/// The hand's speed is taken over this long (s).
const WINDOW: f64 = 0.03;
/// The depth follows what the hand says as a low-pass filter, quickest at
/// first (ease-out), settling in about three times these (s): going all
/// the way back once the hand is unsure, in about 120 ms; forward at once
/// otherwise. In between only on the way.
pub const RECEDE: f64 = 0.04;
pub const RETURN: f64 = 0.025;
/// A hold begun this soon (s) after one let go of sure is sure from the start.
pub const CARRY: f64 = 1.0;
/// Let go of, the screen lands as a critically damped spring of this
/// (rad/s): from half a cell still in about 0.1 s, never past the cell.
const LAND: f64 = 75.0;
/// `Instant`: how dark a desktop comes in, and how soon it is clear (s).
const FLASH: (f64, f64) = (0.45, 0.12);

static MODE: AtomicU8 = AtomicU8::new(QuickFlick::Slide as u8);
/// When the last hold let go of sure ended, if one did.
static SURE_END: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);

pub fn configure(mode: QuickFlick) {
    MODE.store(mode as u8, Ordering::Relaxed);
}

pub fn mode() -> QuickFlick {
    match MODE.load(Ordering::Relaxed) {
        0 => QuickFlick::Off,
        2 => QuickFlick::Instant,
        _ => QuickFlick::Slide,
    }
}

/// The depth `depth` following `wanted` over `dt` s: no jump, quickest
/// at first (ease-out), exact whatever the frame rate.
pub fn approach(depth: f64, wanted: f64, dt: f64) -> f64 {
    let tau = if wanted > depth { RECEDE } else { RETURN };
    let next = wanted + (depth - wanted) * (-dt.max(0.0) / tau).exp();
    if (next - wanted).abs() < 0.002 { wanted } else { next }
}

/// What the hand is taken to be: the screen whole (and while it is not
/// yet either), or all the way receded.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Mind {
    #[default]
    Deciding,
    Sure,
    Unsure,
}

/// What the hand's way says of the hold, from its samples (time from the
/// hold's start, cells along and down, the locked axis) and the time.
#[derive(Clone, Debug, Default)]
pub struct Judge {
    pub mind: Mind,
    /// When it set off, along which axis, which way.
    onset: Option<(f64, usize, f64)>,
    /// The farthest it went its way.
    far: f64,
    /// Sure, since when its stroke has ended (gone slower than `KEEP`).
    slow_since: Option<f64>,
    /// Its way so far over the last `WINDOW`, and one more.
    recent: Vec<(f64, f64)>,
    /// Let go of, and how far it wanted the screen to recede then.
    released: Option<f64>,
}

impl Judge {
    /// Sure from the start: a hold soon after one let go of sure.
    pub fn sure() -> Judge { Judge { mind: Mind::Sure, ..Judge::default() } }

    /// From here on its way is `axis`, `sign`, starting at `t`, `gone` along
    /// it, and it is unsure (for good, this hold).
    fn anchor(&mut self, t: f64, axis: usize, sign: f64, gone: f64) {
        self.onset = Some((self.onset.map_or(t, |(onset, ..)| onset), axis, sign));
        (self.far, self.recent) = (gone, vec![(t, gone)]);
        (self.mind, self.slow_since) = (Mind::Unsure, None);
    }

    pub fn feed(&mut self, t: f64, at: (f64, f64), axis: usize) {
        if self.released.is_some() { return; }
        let along = if axis == 0 { at.0 } else { at.1 };
        let Some((_, way, sign)) = self.onset else {
            if along.abs() >= START {
                self.onset = Some((t, axis, along.signum()));
                self.recent = vec![(t, along.abs())];
                self.far = along.abs();
            }
            return self.check(t);
        };
        if axis != way {
            return self.anchor(t, axis, along.signum(), along.abs());
        }
        let gone = sign * along;
        if gone < self.far - BACK {
            return self.anchor(t, way, -sign, -gone);
        }
        self.far = self.far.max(gone);
        self.recent.push((t, gone));
        let keep = self.recent.iter().rposition(|(when, _)| *when <= t - WINDOW).unwrap_or(0);
        self.recent.drain(..keep);
        self.check(t);
    }

    /// What the hand is at `now`, the time going by with it or without it;
    /// once unsure, it stays so for the rest of the hold.
    pub fn check(&mut self, now: f64) {
        if self.released.is_some() || self.mind == Mind::Unsure { return; }
        let speed = self.speed(now);
        match (self.mind, self.onset.map(|(onset, ..)| onset)) {
            (Mind::Deciding, None) => if now > ONSET_LIMIT { self.mind = Mind::Unsure; },
            // Set off sharply, a flick; slow to set off, or not sharp soon, unsure.
            (Mind::Deciding, Some(onset)) => {
                if onset > ONSET_LIMIT || (speed < PEAK && now - onset > DECIDE) { self.mind = Mind::Unsure; }
                else if speed >= PEAK { self.mind = Mind::Sure; }
            }
            _ => {
                // Its stroke going on, or ended and the next as sharp soon enough.
                let going = if self.slow_since.is_some() { PEAK } else { KEEP };
                if speed >= going { self.slow_since = None; return; }
                let since = *self.slow_since.get_or_insert(now);
                if now - since >= PAUSE { self.mind = Mind::Unsure; }
            }
        }
    }

    /// The hand's speed its way at `now`, falling off while it stays still.
    pub fn speed(&self, now: f64) -> f64 {
        let Some(&(last, gone)) = self.recent.iter().rev().find(|(when, _)| *when <= now) else { return 0.0 };
        let from = self.recent.iter().rev().find(|(when, _)| *when <= now - WINDOW).or(self.recent.first());
        let Some(&(since, start)) = from else { return 0.0 };
        let span = (now - since).max(last - since);
        if span < 0.01 { return 0.0; }
        ((gone - start) / span).max(0.0)
    }

    /// How far the hand wants the screen to recede (as last `check`ed).
    pub fn wanted(&self) -> f64 {
        if self.mind == Mind::Unsure { 1.0 } else { 0.0 }
    }

    /// Let go of: as it is, from now on (not yet sure counts as sure, as
    /// does letting go just after stopping, as a flick does).
    pub fn release(&mut self, now: f64) {
        if self.released.is_some() { return; }
        self.check(now);
        if self.mind == Mind::Deciding { self.mind = Mind::Sure; }
        self.released = Some(self.wanted());
    }

    /// The axis it goes along.
    pub fn axis(&self) -> Option<usize> { self.onset.map(|(_, axis, _)| axis) }

    /// Which way along it, 1 or -1 (0 not yet set off).
    pub fn way(&self) -> f64 { self.onset.map_or(0.0, |(.., sign)| sign) }

    /// How much farther than the hand (cells along and down) a hold let go
    /// of sure at `now` goes: a cell flung as fast as `FLING`, else none.
    pub fn coast(&self, now: f64) -> (f64, f64) {
        let Some((_, axis, sign)) = self.onset.filter(|_| self.mind == Mind::Sure) else { return (0.0, 0.0) };
        let on = if self.speed(now) >= FLING { sign * COAST_MOST } else { 0.0 };
        if axis == 0 { (on, 0.0) } else { (0.0, on) }
    }
}

/// The speed a landing `off` (cells) from its cell sets off at, the hand
/// going `speed`: as fast towards it as can still stop on it (`LAND`), and
/// not away.
fn launch(off: f64, speed: f64) -> f64 {
    if speed * off >= 0.0 { 0.0 } else { speed.clamp(-LAND * off.abs(), LAND * off.abs()) }
}

/// Whether a hold begun `since` s after one let go of sure is sure from
/// the start.
pub fn carried(since: Option<f64>) -> bool {
    since.is_some_and(|since| (0.0..=CARRY).contains(&since))
}

/// How the picture is drawn at a moment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    /// The axis that moves and where along it the screen is.
    pub axis: usize,
    pub at: f64,
    /// How far the screen recedes, of all the way (`Follow::depth`): 0
    /// whole, 1 the zoomed-out picture.
    pub depth: f64,
    /// How dark every desktop is as one comes in at once (`Instant`).
    pub flash: f64,
    /// At rest where it was let go.
    pub settled: bool,
}

/// A hold's picture as the hand goes: where the screen is along the
/// hand's axis, and how far it recedes.
#[derive(Clone, Debug)]
pub struct Quick {
    instant: bool,
    pub judge: Judge,
    along: [Spring; 2],
    /// Let go of, and set off landing from where the hand was.
    landing: bool,
    depth: f64,
    last: Option<f64>,
    /// The cell last shown and when it came (`Instant`).
    cell: (i32, i32),
    came: f64,
}

impl Quick {
    /// `None` when switched off: the screen recedes all the way.
    pub fn new() -> Option<Quick> {
        let instant = match mode() {
            QuickFlick::Off => return None,
            QuickFlick::Slide => false,
            QuickFlick::Instant => true,
        };
        let since = SURE_END.lock().ok().and_then(|end| end.map(|end| end.elapsed().as_secs_f64()));
        let judge = if carried(since) { Judge::sure() } else { Judge::default() };
        Some(Quick { instant, judge, along: [Spring::default(); 2], landing: false, depth: 0.0, last: None, cell: (0, 0), came: f64::NEG_INFINITY })
    }

    /// Let go of at `now`; let go of sure, the next hold soon after is sure
    /// from the start.
    pub fn release(&mut self, now: f64) {
        self.judge.release(now);
        if let Ok(mut end) = SURE_END.lock() {
            *end = (self.judge.mind == Mind::Sure).then(std::time::Instant::now);
        }
    }

    /// The picture at `now`, the hand `held` where it is (cells along and
    /// down, `Follow::held`) while it holds and as it lets go, and the
    /// cell gone to `aim` (`Follow::aim`) on `cell`; `axis` the hand's
    /// locked axis.
    pub fn look(&mut self, now: f64, aim: (f64, f64), held: Option<(f64, f64)>, cell: (i32, i32), axis: usize) -> Look {
        let now = now.max(self.last.unwrap_or(now));
        let dt = self.last.replace(now).map_or(0.0, |last| now - last);
        self.judge.check(now);
        self.depth = approach(self.depth, self.judge.wanted(), dt);
        let aim = [aim.0, aim.1];
        if let Some(held) = held {
            // Where the hand is, no later; let go of, off as fast as it went.
            self.along = [Spring { p: held.0, v: 0.0 }, Spring { p: held.1, v: 0.0 }];
            if self.judge.released.is_some() && !self.landing {
                self.landing = true;
                if let Some(way) = self.judge.axis() {
                    self.along[way].v = launch(self.along[way].p - aim[way], self.judge.way() * self.judge.speed(now));
                }
            }
        } else {
            for (spring, aim) in self.along.iter_mut().zip(aim) {
                spring.advance(aim, dt, LAND, 1.0);
                if spring.settled(aim) { *spring = Spring { p: aim, v: 0.0 }; }
            }
        }
        if cell != self.cell { (self.cell, self.came) = (cell, now); }
        let axis = self.judge.axis().unwrap_or(axis);
        let fading = ((now - self.came) / FLASH.1).clamp(0.0, 1.0);
        let (at, flash, still) = if self.instant {
            (if axis == 0 { cell.0 } else { cell.1 } as f64, FLASH.0 * (1.0 - fading), fading >= 1.0)
        } else {
            (self.along[axis].p, 0.0, self.along.iter().zip(aim).all(|(s, aim)| s.settled(aim)))
        };
        let wanted = self.judge.wanted();
        Look { axis, at, depth: self.depth, flash, settled: self.judge.released.is_some() && still && self.depth == wanted }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> Quick {
        Quick { instant: false, judge: Judge::default(), along: [Spring::default(); 2], landing: false, depth: 0.0, last: None, cell: (0, 0), came: 0.0 }
    }

    /// A hand along x at `x(t)` (cells), sampled every `dt` from `from`
    /// until `until`, drawn every 120th of a second: each frame's time,
    /// depth and what the hand was taken to be.
    fn run(quick: &mut Quick, x: impl Fn(f64) -> f64, dt: f64, from: f64, until: f64) -> Vec<(f64, f64, Mind)> {
        let mut frames = Vec::new();
        let (mut t, mut frame) = (from, from);
        while t <= until + 1e-9 {
            quick.judge.feed(t, (x(t), 0.0), 0);
            if t >= frame {
                let depth = quick.look(t, (x(t).round(), 0.0), Some((x(t), 0.0)), (x(t).round() as i32, 0), 0).depth;
                frames.push((t, depth, quick.judge.mind));
                frame += 1.0 / 120.0;
            }
            t += dt;
        }
        frames
    }

    fn deepest(frames: &[(f64, f64, Mind)]) -> f64 { frames.iter().map(|f| f.1).fold(0.0, f64::max) }

    /// A stroke `cells` long set off at `from`, sharp at first and slowing
    /// as a flick does (`tau` s to go most of the way).
    fn stroke(t: f64, from: f64, cells: f64, tau: f64) -> f64 {
        if t < from { 0.0 } else { cells * (1.0 - (-(t - from) / tau).exp()) }
    }

    #[test]
    fn a_sharp_short_stroke_is_a_flick_though_slow_on_average_on_mouse_and_touchpad() {
        for dt in [0.001, 0.008] {
            // Under three cells a second on average, but set off sharply.
            let mut quick = fresh();
            let frames = run(&mut quick, |t| stroke(t, 0.05, 0.6, 0.06), dt, 0.0, 0.3);
            assert!(stroke(0.3, 0.05, 0.6, 0.06) / 0.25 < 2.5 && deepest(&frames) < 0.05, "every {dt} s: {frames:?}");
            assert_eq!(quick.judge.mind, Mind::Sure);
            // Stopping, then letting go soon, stays whole.
            quick.release(0.32);
            assert!(quick.look(0.4, (1.0, 0.0), None, (1, 0), 0).depth < 0.05);
        }
    }

    #[test]
    fn a_long_drag_set_off_slowly_or_a_middling_one_is_precise_soon() {
        // Getting faster and faster, four cells a second half a second on.
        let mut quick = fresh();
        let frames = run(&mut quick, |t| 4.0 * (t - 0.05).max(0.0).powi(2), 0.001, 0.0, 0.8);
        let unsure = frames.iter().find(|f| f.2 == Mind::Unsure).unwrap().0;
        assert!(unsure <= 0.25, "{unsure}");
        assert!(frames.iter().filter(|f| f.0 >= unsure).all(|f| f.2 == Mind::Unsure) && frames.last().unwrap().1 > 0.99);
        // Three cells a second, steadily, and slower: in between only for a moment, then all the way.
        for speed in [3.0, 1.5] {
            let mut quick = fresh();
            let frames = run(&mut quick, |t| speed * (t - 0.05).max(0.0), 0.001, 0.0, 1.0);
            let unsure = frames.iter().find(|f| f.2 == Mind::Unsure).unwrap().0;
            assert!(unsure <= 0.05 + DECIDE + 0.03, "{speed}: {unsure}");
            assert!(frames.iter().find(|f| f.0 >= unsure + 0.12).unwrap().1 > 0.93);
            let halfway = frames.iter().filter(|f| (0.3..0.7).contains(&f.1)).count() as f64 / 120.0;
            assert!(halfway < 0.1, "{halfway} s half receded");
        }
    }

    #[test]
    fn held_still_it_is_precise_within_a_fifth_of_a_second() {
        let mut quick = fresh();
        let frames = run(&mut quick, |_| 0.0, 0.001, 0.0, 1.0);
        let unsure = frames.iter().find(|f| f.2 == Mind::Unsure).unwrap().0;
        assert!(unsure <= 0.2, "{unsure}");
        // Whole until then, then all the way back at once, ease-out and without a jump.
        assert!(frames.iter().filter(|f| f.0 < unsure).all(|f| f.1 == 0.0));
        assert!(frames.iter().find(|f| f.0 >= unsure + 0.12).unwrap().1 > 0.93);
        assert!(frames.windows(2).all(|w| w[1].1 - w[0].1 < 0.21));
        assert!(frames.last().unwrap().1 > 0.98);
    }

    #[test]
    fn a_flick_of_six_cells_a_second_stays_quick() {
        let mut quick = fresh();
        let frames = run(&mut quick, |t| 6.0 * (t - 0.05).max(0.0), 0.001, 0.0, 1.0);
        assert!(deepest(&frames) < 0.05, "{frames:?}");
        assert!(frames.iter().filter(|f| f.0 >= 0.05 + DECIDE).all(|f| f.2 == Mind::Sure));
    }

    #[test]
    fn two_flicks_go_two_cells_whole_all_the_way() {
        let x = |t: f64| stroke(t, 0.05, 1.1, 0.04) + stroke(t, 0.35, 1.1, 0.04);
        let mut quick = fresh();
        let mut hand = crate::follow::Follow::new((0, 0), vec![(0, 4)]);
        let mut t = 0.0;
        while t <= 0.6 {
            hand.track_locked(t, (x(t), 0.0), 0);
            t += 0.001;
        }
        let frames = run(&mut quick, x, 0.001, 0.0, 0.6);
        assert!(deepest(&frames) < 0.05, "{frames:?}");
        quick.release(0.6);
        let coast = quick.judge.coast(0.6);
        assert_eq!(hand.let_go_near(0.6, (x(0.6), 0.0), coast), (2, 0));
    }

    #[test]
    fn a_flick_lands_as_far_as_the_hand_went_past_six_tenths_of_a_cell() {
        let flick = |to: f64, speed: f64| {
            let x = move |t: f64| (speed * (t - 0.05).max(0.0)).min(to);
            let end = 0.05 + to / speed;
            let mut quick = fresh();
            let mut hand = crate::follow::Follow::new((0, 0), vec![(0, 5)]);
            let mut t = 0.0;
            while t <= end + 0.001 {
                hand.track_locked(t, (x(t), 0.0), 0);
                quick.judge.feed(t, (x(t), 0.0), 0);
                t += 0.001;
            }
            quick.release(t);
            hand.let_go_near(t, (x(t), 0.0), quick.judge.coast(t))
        };
        assert_eq!(flick(1.3, 8.0), (1, 0));
        assert_eq!(flick(1.7, 8.0), (2, 0));
        // Let go still going eight cells a second, 1.4 on: the cell it went to.
        assert_eq!(flick(1.4, 8.0), (1, 0));
        // Past six tenths and back to half: still the cell beyond.
        let mut hand = crate::follow::Follow::new((0, 0), vec![(0, 3)]);
        hand.track_locked(0.0, (0.65, 0.0), 0);
        hand.track_locked(0.01, (0.5, 0.0), 0);
        assert_eq!(hand.clone().let_go_near(0.02, (0.5, 0.0), (0.0, 0.0)), (1, 0));
        hand.track_locked(0.02, (0.35, 0.0), 0);
        assert_eq!(hand.let_go_near(0.03, (0.35, 0.0), (0.0, 0.0)), (0, 0));
    }

    #[test]
    fn a_flick_let_go_moving_goes_on_at_most_a_cell() {
        // Let go 1.3 cells on at 20 cells a second: a cell more, no farther.
        let fling = |t: f64| 20.0 * (t - 0.05).max(0.0);
        let mut quick = fresh();
        run(&mut quick, fling, 0.001, 0.0, 0.115);
        quick.release(0.115);
        let coast = quick.judge.coast(0.115);
        assert!((coast.0 - COAST_MOST).abs() < 1e-9 && coast.1 == 0.0, "{coast:?}");
        let mut hand = crate::follow::Follow::new((0, 0), vec![(0, 5)]);
        hand.track_locked(0.1, (fling(0.1), 0.0), 0);
        assert_eq!(hand.let_go_near(0.115, (fling(0.115), 0.0), coast), (2, 0));
        // Let go once stopped, as far as it went; let go of unsure, too.
        let mut quick = fresh();
        run(&mut quick, |t| stroke(t, 0.05, 1.3, 0.04), 0.001, 0.0, 0.3);
        quick.release(0.3);
        assert!(quick.judge.coast(0.3).0.abs() < 0.05);
        let mut quick = fresh();
        run(&mut quick, |t| 3.0 * t, 0.001, 0.0, 0.4);
        quick.release(0.4);
        assert_eq!(quick.judge.coast(0.4), (0.0, 0.0));
    }

    #[test]
    fn stopping_a_while_turning_back_or_changing_axis_goes_all_the_way_back_at_once() {
        // A flick, then still.
        let mut quick = fresh();
        let frames = run(&mut quick, |t| stroke(t, 0.05, 1.5, 0.04), 0.001, 0.0, 0.8);
        let at = |t: f64| frames.iter().find(|f| f.0 >= t).unwrap();
        assert!(at(0.25).1 < 0.05);
        let unsure = frames.iter().find(|f| f.2 == Mind::Unsure).unwrap().0;
        assert!((0.3..=0.5).contains(&unsure), "{unsure}");
        assert!(at(unsure + 0.12).1 > 0.93, "{:?}", at(unsure + 0.12));
        // Receding begins at once and has no jump.
        assert!(frames.windows(2).all(|w| w[1].1 - w[0].1 < 0.21));
        // Turning back, or going the other way, at once.
        let mut quick = fresh();
        run(&mut quick, |t| 6.0 * t, 0.001, 0.0, 0.2);
        quick.judge.feed(0.21, (0.9, 0.0), 0);
        assert_eq!((quick.judge.mind, quick.judge.wanted()), (Mind::Unsure, 1.0));
        let mut quick = fresh();
        run(&mut quick, |t| 6.0 * t, 0.001, 0.0, 0.2);
        quick.judge.feed(0.21, (1.2, 0.1), 1);
        assert_eq!((quick.judge.mind, quick.judge.axis()), (Mind::Unsure, Some(1)));
    }

    #[test]
    fn once_unsure_it_stays_receded_until_let_go() {
        // Still a while, then a flick again: all the way back, and kept so.
        let x = |t: f64| stroke(t, 0.05, 1.0, 0.04) + stroke(t, 0.8, 1.0, 0.04);
        let mut quick = fresh();
        let frames = run(&mut quick, x, 0.001, 0.0, 1.4);
        let unsure = frames.iter().find(|f| f.2 == Mind::Unsure).unwrap().0;
        assert!(frames.iter().filter(|f| f.0 >= unsure + 0.12).all(|f| f.1 > 0.93 && f.2 == Mind::Unsure), "{frames:?}");
        // Turned back, then quick on: still all the way.
        let mut quick = fresh();
        run(&mut quick, |t| 6.0 * t, 0.001, 0.0, 0.2);
        quick.judge.feed(0.21, (0.9, 0.0), 0);
        let frames = run(&mut quick, |t| 0.9 - 6.0 * (t - 0.21), 0.001, 0.22, 0.6);
        assert!(frames.last().unwrap().1 > 0.99 && frames.last().unwrap().2 == Mind::Unsure);
        // Let go unsure, it is not carried (`Quick::release`): the next hold starts afresh.
        quick.release(0.61);
        assert_eq!(quick.judge.mind, Mind::Unsure);
        let mut next = fresh();
        let frames = run(&mut next, |t| stroke(t, 0.05, 1.0, 0.04), 0.001, 0.0, 0.25);
        assert!(deepest(&frames) < 0.05, "{frames:?}");
    }

    #[test]
    fn a_hold_soon_after_a_sure_one_is_sure_from_the_start() {
        assert!(carried(Some(0.4)) && !carried(Some(CARRY + 0.1)) && !carried(None));
        let mut quick = Quick { judge: Judge::sure(), ..fresh() };
        // Setting off slowly does not recede it at once.
        let frames = run(&mut quick, |t| 1.5 * t, 0.001, 0.0, 0.18);
        assert!(frames[0].1 == 0.0 && deepest(&frames) < 0.05, "{frames:?}");
    }

    #[test]
    fn whole_the_screen_is_where_the_hand_is_and_lands_on_the_nearest_cell_in_a_tenth_of_a_second() {
        // Held: cell for cell where the hand is, from its first move.
        let x = |t: f64| stroke(t, 0.02, 1.2, 0.05);
        let mut quick = fresh();
        let mut hand = crate::follow::Follow::new((0, 0), vec![(0, 3)]);
        let mut t = 0.0;
        while t < 0.06 {
            hand.track_locked(t, (x(t), 0.0), 0);
            quick.judge.feed(t, (x(t), 0.0), 0);
            let look = quick.look(t, hand.aim(), Some(hand.held()), hand.selected(), 0);
            assert!((look.at - x(t)).abs() < 1e-9, "{t}: {} for {}", look.at, x(t));
            t += 1.0 / 120.0;
        }
        // Let go still moving, short of a cell and past six tenths of it:
        // to it, at once, never past it.
        quick.release(t);
        let held = hand.held();
        let to = hand.let_go_near(t, (x(t), 0.0), quick.judge.coast(t));
        assert_eq!(to, (1, 0));
        let mut look = quick.look(t, hand.aim(), Some(held), to, 0);
        assert_eq!(look.at, held.0);
        let (from, mut most, mut near) = (t, 0.0f64, None);
        for frame in 1..=60 {
            let now = from + frame as f64 / 120.0;
            look = quick.look(now, hand.aim(), None, to, 0);
            most = most.max(look.at);
            if near.is_none() && (look.at - 1.0).abs() < 0.005 { near = Some(now - from); }
        }
        assert!(most <= 1.01 && look.settled, "overshoot {most}");
        assert!(near.is_some_and(|t| t <= 0.1), "{near:?}");
        // Still, half a cell on: there in about a tenth of a second too; short of half, back.
        let mut quick = fresh();
        quick.release(0.0);
        quick.look(0.0, (1.0, 0.0), Some((0.5, 0.0)), (1, 0), 0);
        let near = (1..60).map(|f| f as f64 / 120.0).find(|&now| (quick.look(now, (1.0, 0.0), None, (1, 0), 0).at - 1.0).abs() < 0.005);
        assert!(near.is_some_and(|t| t <= 0.1), "{near:?}");
        let mut hand = crate::follow::Follow::new((0, 0), vec![(0, 3)]);
        hand.track_locked(0.0, (0.4, 0.0), 0);
        assert_eq!(hand.let_go_near(0.01, (0.4, 0.0), (0.0, 0.0)), (0, 0));
        // Coasting far, no farther than the cell beyond the hand.
        let mut hand = crate::follow::Follow::new((0, 0), vec![(0, 3)]);
        hand.track_locked(0.0, (0.9, 0.0), 0);
        assert_eq!(hand.let_go_near(0.01, (0.9, 0.0), (COAST_MOST, 0.0)), (1, 0));
        // At the row's end, held there.
        let mut hand = crate::follow::Follow::new((0, 0), vec![(-1, 0)]);
        hand.track_locked(0.0, (0.3, 0.0), 0);
        assert_eq!(hand.held(), (0.0, 0.0));
    }
}
