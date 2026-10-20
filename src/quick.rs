//! How far the screen recedes as the hand goes (`depth`, 0 to 1), as a
//! marking menu shows itself only to a hand that hesitates: a hand sure
//! of its way keeps the screen whole and the desktop it goes to slides in
//! (or comes at once, `QuickFlick::Instant`), and stays sure through a
//! short slowing or stop between cells; once it is plainly unsure (still
//! or slow a while, turning back, going the other way) the screen goes all
//! the way back at once, not by halves, and stays so until let go: a hand
//! gone unsure does not get the screen whole again by going on quickly.
//! In between only for a moment, while a hand just set off is not yet
//! either. A hold begun just after one let go of sure is sure from the
//! start; one let go of unsure carries nothing over.
//!
//! The judging (`Judge`) only reads the hand; the cells gone to are still
//! the hand's latches (`follow::Follow`), switched to once, on letting go.
use crate::{config::QuickFlick, transition_curve::Spring};
use std::sync::atomic::{AtomicU8, Ordering};

/// How far the hand goes before it counts as on its way, in cells.
pub const START: f64 = 0.02;
/// Not set off this soon after the hold begins (s), the hand is unsure.
pub const ONSET_LIMIT: f64 = 0.15;
/// Just set off, for this long (s), it is taken as sure.
pub const DECIDE: f64 = 0.07;
/// At this speed or faster (cells a second) the hand is sure of its way;
/// sure, it stays so until it goes slower than `KEEP` for `AMBIGUOUS`
/// (s): a short slowing or a stop between cells does not count. Not yet
/// sure, as long as it goes slower than `SURE_SPEED` for `DECIDE` more:
/// only a real flick is sure.
pub const SURE_SPEED: f64 = 5.0;
pub const KEEP: f64 = 2.5;
pub const AMBIGUOUS: f64 = 0.2;
/// Not yet sure and this slow or slower, it is unsure at once.
pub const SLOW: f64 = 2.0;
/// Turning back this far (cells) from the farthest it went is unsure.
pub const BACK: f64 = 0.15;
/// The hand's speed is taken over this long (s).
const WINDOW: f64 = 0.05;
/// The depth follows what the hand says as a low-pass filter, quickest at
/// first (ease-out), settling in about three times these (s): going all
/// the way back once the hand is unsure, in about 120 ms; forward at once
/// otherwise. In between only on the way.
pub const RECEDE: f64 = 0.04;
pub const RETURN: f64 = 0.025;
/// A hold begun this soon (s) after one let go of sure is sure from the start.
pub const CARRY: f64 = 1.0;
/// The slide's spring: settles a cell in about 150 ms, overshooting under 1%.
const OMEGA: f64 = 30.0;
const ZETA: f64 = 0.85;
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
    /// Since when it has gone slower than it must to stay as it is.
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
        if speed >= SURE_SPEED { (self.mind, self.slow_since) = (Mind::Sure, None); return; }
        let (sure, onset) = (self.mind == Mind::Sure, self.onset.map(|(onset, ..)| onset));
        match onset {
            None if !sure => { if now > ONSET_LIMIT { self.mind = Mind::Unsure; } return; }
            // Just set off, it is sure enough, unless it was slow to.
            Some(onset) if !sure && now - onset <= DECIDE => { if onset > ONSET_LIMIT { self.mind = Mind::Unsure; } return; }
            _ => {}
        }
        // Plainly slow, or slower than it must be long enough: unsure.
        if !sure && speed < SLOW { self.mind = Mind::Unsure; return; }
        let (slow, ambiguous) = if sure { (KEEP, AMBIGUOUS) } else { (SURE_SPEED, DECIDE) };
        if speed >= slow { self.slow_since = None; return; }
        let since = *self.slow_since.get_or_insert(now);
        if now - since >= ambiguous { self.mind = Mind::Unsure; }
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
        Some(Quick { instant, judge, along: [Spring::default(); 2], depth: 0.0, last: None, cell: (0, 0), came: f64::NEG_INFINITY })
    }

    /// Let go of at `now`; let go of sure, the next hold soon after is sure
    /// from the start.
    pub fn release(&mut self, now: f64) {
        self.judge.release(now);
        if let Ok(mut end) = SURE_END.lock() {
            *end = (self.judge.mind == Mind::Sure).then(std::time::Instant::now);
        }
    }

    /// The picture at `now`, the hand's latches aiming at `aim` (cells
    /// along and down, leaning as the hand does) on `cell`; `axis` the
    /// hand's locked axis.
    pub fn look(&mut self, now: f64, aim: (f64, f64), cell: (i32, i32), axis: usize) -> Look {
        let now = now.max(self.last.unwrap_or(now));
        let dt = self.last.replace(now).map_or(0.0, |last| now - last);
        self.judge.check(now);
        self.depth = approach(self.depth, self.judge.wanted(), dt);
        let aim = [aim.0, aim.1];
        for (spring, aim) in self.along.iter_mut().zip(aim) {
            spring.advance(aim, dt, OMEGA, ZETA);
            if spring.settled(aim) { *spring = Spring { p: aim, v: 0.0 }; }
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
        Quick { instant: false, judge: Judge::default(), along: [Spring::default(); 2], depth: 0.0, last: None, cell: (0, 0), came: 0.0 }
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
                let depth = quick.look(t, (x(t).round(), 0.0), (x(t).round() as i32, 0), 0).depth;
                frames.push((t, depth, quick.judge.mind));
                frame += 1.0 / 120.0;
            }
            t += dt;
        }
        frames
    }

    fn deepest(frames: &[(f64, f64, Mind)]) -> f64 { frames.iter().map(|f| f.1).fold(0.0, f64::max) }

    #[test]
    fn a_quick_sure_move_keeps_the_screen_whole_on_mouse_and_touchpad() {
        for dt in [0.001, 0.008] {
            let mut quick = fresh();
            let frames = run(&mut quick, |t| 6.0 * (t - 0.05).clamp(0.0, 0.4), dt, 0.0, 0.5);
            assert!(deepest(&frames) < 0.05, "every {dt} s: {frames:?}");
            // Stopping, then letting go soon, stays whole.
            quick.release(0.52);
            assert!(quick.look(0.6, (2.0, 0.0), (2, 0), 0).depth < 0.05);
        }
    }

    #[test]
    fn going_on_quickly_stays_whole_through_a_short_slowing_or_stop() {
        // Quick, then slow for 0.15 s, then quick again; and the same with a stop.
        for between in [0.5, 0.0] {
            let x = move |t: f64| 6.0 * t.min(0.25) + between * (t.min(0.4) - 0.25).max(0.0) + 6.0 * (t - 0.4).max(0.0);
            let mut quick = fresh();
            let frames = run(&mut quick, x, 0.001, 0.0, 0.8);
            assert!(deepest(&frames) < 0.05, "{between}: {frames:?}");
        }
    }

    #[test]
    fn stopping_a_while_turning_back_or_changing_axis_goes_all_the_way_back_at_once() {
        // Quick, then still from 0.25 s on.
        let mut quick = fresh();
        let frames = run(&mut quick, |t| 6.0 * t.min(0.25), 0.001, 0.0, 0.8);
        let at = |t: f64| frames.iter().find(|f| f.0 >= t).unwrap();
        assert!(at(0.42).1 < 0.05);
        let unsure = frames.iter().find(|f| f.2 == Mind::Unsure).unwrap().0;
        assert!((0.45..=0.52).contains(&unsure), "{unsure}");
        assert!(at(unsure + 0.12).1 > 0.93, "{:?}", at(unsure + 0.12));
        // Receding begins at once and has no jump.
        assert!(frames.windows(2).all(|w| w[1].1 - w[0].1 < 0.2));
        // Turning back, or going the other way, at once.
        let mut quick = fresh();
        run(&mut quick, |t| 6.0 * t, 0.001, 0.0, 0.2);
        quick.judge.feed(0.21, (0.9, 0.0), 0);
        assert_eq!((quick.judge.mind, quick.judge.wanted()), (Mind::Unsure, 1.0));
        let mut quick = fresh();
        run(&mut quick, |t| 6.0 * t, 0.001, 0.0, 0.2);
        quick.judge.feed(0.21, (0.8, 0.1), 1);
        assert_eq!((quick.judge.mind, quick.judge.axis()), (Mind::Unsure, Some(1)));
    }

    #[test]
    fn once_unsure_it_stays_receded_until_let_go() {
        // Still a while, then quick again: all the way back, and kept so.
        let x = |t: f64| 6.0 * t.min(0.25) + 6.0 * (t - 0.8).max(0.0);
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
        let frames = run(&mut next, |t| 6.0 * (t - 0.05).clamp(0.0, 0.4), 0.001, 0.0, 0.5);
        assert!(deepest(&frames) < 0.05, "{frames:?}");
    }

    #[test]
    fn a_middling_hand_does_not_stay_half_receded() {
        // Three cells a second, steadily, is precise: in between only for a moment, then all the way.
        let mut quick = fresh();
        let frames = run(&mut quick, |t| 3.0 * (t - 0.05).max(0.0), 0.001, 0.0, 1.2);
        let unsure = frames.iter().find(|f| f.2 == Mind::Unsure).unwrap().0;
        assert!(unsure <= 0.05 + 2.0 * DECIDE + 0.02, "{unsure}");
        assert!(frames.iter().filter(|f| f.0 >= unsure).all(|f| f.2 == Mind::Unsure));
        assert!(frames.iter().find(|f| f.0 >= unsure + 0.12).unwrap().1 > 0.93);
        let halfway = frames.iter().filter(|f| (0.3..0.7).contains(&f.1)).count() as f64 / 120.0;
        assert!(halfway < 0.2, "{halfway} s half receded");
        assert!(frames.last().unwrap().1 > 0.99);
        // Slower than two cells a second, precise as soon as it is set off.
        let mut quick = fresh();
        let frames = run(&mut quick, |t| 1.5 * (t - 0.05).max(0.0), 0.001, 0.0, 0.4);
        let unsure = frames.iter().find(|f| f.2 == Mind::Unsure).unwrap().0;
        assert!(unsure <= 0.05 + DECIDE + 0.03, "{unsure}");
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
        assert!(frames.iter().filter(|f| f.0 >= 0.05 + DECIDE + 0.01).all(|f| f.2 == Mind::Sure));
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
    fn the_slide_takes_about_150_ms_a_cell_and_hardly_overshoots() {
        let mut quick = fresh();
        let (mut most, mut near) = (0.0f64, None);
        for frame in 0..=60 {
            let now = frame as f64 / 120.0;
            let look = quick.look(now, (1.0, 0.0), (1, 0), 0);
            most = most.max(look.at);
            if near.is_none() && (look.at - 1.0).abs() < 0.03 { near = Some(now); }
        }
        assert!(most < 1.02, "overshoot {most}");
        assert!(near.is_some_and(|t| (0.1..=0.18).contains(&t)), "{near:?}");
    }
}
