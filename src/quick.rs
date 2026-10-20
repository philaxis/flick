//! A quick flick: a hold that sets off at once, sure of its way, skips the
//! zoomed-out picture. As a marking menu does, the picture shows only when
//! the hand hesitates: the screen stays whole and the desktop the hand
//! goes to slides in (or comes at once, `QuickFlick::Instant`), until the
//! hand slows, stops, turns back or goes the other way, when the receded
//! picture blends in from where things are (`Quick::look`).
//!
//! The judging (`Judge`) only reads the hand; the cells gone to are still
//! the hand's latches (`follow::Follow`), switched to once, on letting go.
use crate::{config::QuickFlick, transition_curve::Spring};
use std::sync::atomic::{AtomicU8, Ordering};

/// How far the hand goes before it counts as on its way, in cells.
pub const START: f64 = 0.02;
/// The hand must set off this soon after the hold begins (s)...
pub const ONSET_LIMIT: f64 = 0.5;
/// ...and go this far along one axis (cells)...
pub const SURE: f64 = 0.3;
/// ...this soon after setting off (s), to be sure of its way.
pub const DECIDE: f64 = 0.15;
/// Slower than this (cells a second)...
pub const SLOW: f64 = 1.0;
/// ...for this long (s) is a hesitation, as is a stop.
pub const PAUSE: f64 = 0.15;
/// Coming back this far (cells) from the farthest it went is a hesitation.
pub const BACK: f64 = 0.15;
/// The hand's speed is taken over this long (s).
const WINDOW: f64 = 0.05;
/// From the quick picture to the receded one (s).
pub const BLEND: f64 = 0.18;
/// The slide's spring: settles a cell in about 150 ms, overshooting under 1%.
const OMEGA: f64 = 30.0;
const ZETA: f64 = 0.85;
/// `Instant`: how dark a desktop comes in, and how soon it is clear (s).
const FLASH: (f64, f64) = (0.45, 0.12);

static MODE: AtomicU8 = AtomicU8::new(QuickFlick::Slide as u8);

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

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Verdict {
    /// Not yet sure.
    Deciding,
    /// Sure of its way.
    Quick,
    /// Hesitated, at this time: the receded picture from then on.
    Hesitant(f64),
}

/// What the hand's way says of the hold, from its samples (time from the
/// hold's start, cells along and down, the locked axis) and the time.
#[derive(Clone, Debug)]
pub struct Judge {
    verdict: Verdict,
    /// When it set off, along which axis, which way.
    onset: Option<(f64, usize, f64)>,
    /// The farthest it went its way.
    far: f64,
    /// The last time it went at least `SLOW`.
    brisk: f64,
    /// Its way so far over the last `WINDOW` and one more.
    recent: Vec<(f64, f64)>,
    released: bool,
}

impl Default for Judge {
    fn default() -> Self {
        Self { verdict: Verdict::Deciding, onset: None, far: 0.0, brisk: 0.0, recent: Vec::new(), released: false }
    }
}

impl Judge {
    fn hesitate(&mut self, t: f64) {
        if !matches!(self.verdict, Verdict::Hesitant(_)) { self.verdict = Verdict::Hesitant(t); }
    }

    pub fn feed(&mut self, t: f64, at: (f64, f64), axis: usize) {
        if self.released || matches!(self.verdict, Verdict::Hesitant(_)) { return; }
        let Some((onset, way, sign)) = self.onset else {
            let along = if axis == 0 { at.0 } else { at.1 };
            if along.abs() >= START {
                if t > ONSET_LIMIT { return self.hesitate(t); }
                self.onset = Some((t, axis, along.signum()));
                self.brisk = t;
                self.feed(t, at, axis);
            }
            return;
        };
        if axis != way { return self.hesitate(t); }
        let gone = sign * if way == 0 { at.0 } else { at.1 };
        if gone < self.far - BACK { return self.hesitate(t); }
        self.far = self.far.max(gone);
        self.recent.push((t, gone));
        let keep = self.recent.iter().rposition(|(when, _)| *when <= t - WINDOW).unwrap_or(0);
        self.recent.drain(..keep);
        let (since, from) = self.recent[0];
        if t - since >= 0.01 && (gone - from) / (t - since) >= SLOW { self.brisk = t; }
        if self.verdict == Verdict::Deciding && self.far >= SURE && t - onset <= DECIDE { self.verdict = Verdict::Quick; }
        self.check(t);
    }

    /// Time going by without the hand: waiting too long is a hesitation.
    pub fn check(&mut self, now: f64) {
        if self.released || matches!(self.verdict, Verdict::Hesitant(_)) { return; }
        match self.onset {
            None if now > ONSET_LIMIT => self.hesitate(now),
            Some((onset, ..)) if self.verdict == Verdict::Deciding && now - onset > DECIDE => self.hesitate(now),
            Some(_) if now - self.brisk >= PAUSE => self.hesitate(now),
            _ => {}
        }
    }

    /// Let go of: as it is, from now on (not yet sure counts as sure).
    pub fn release(&mut self, now: f64) {
        self.check(now);
        self.released = true;
    }

    /// The axis it goes along.
    pub fn axis(&self) -> Option<usize> { self.onset.map(|(_, axis, _)| axis) }
}

/// How the quick picture is drawn at a moment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    /// The axis that moves and where along it the screen is.
    pub axis: usize,
    pub at: f64,
    /// 0 quick, 1 the receded picture, in between as it blends in.
    pub mix: f64,
    /// How dark every desktop is as one comes in at once (`Instant`).
    pub flash: f64,
    /// At rest where it was let go.
    pub settled: bool,
}

/// A hold's quick picture: where the screen is along the hand's axis.
#[derive(Clone, Debug)]
pub struct Quick {
    instant: bool,
    pub judge: Judge,
    along: [Spring; 2],
    last: Option<f64>,
    /// The cell last shown and when it came (`Instant`).
    cell: (i32, i32),
    came: f64,
}

impl Quick {
    /// `None` when switched off.
    pub fn new() -> Option<Quick> {
        let instant = match mode() {
            QuickFlick::Off => return None,
            QuickFlick::Slide => false,
            QuickFlick::Instant => true,
        };
        Some(Quick { instant, judge: Judge::default(), along: [Spring::default(); 2], last: None, cell: (0, 0), came: f64::NEG_INFINITY })
    }

    /// Whether the receded picture is all there is from now on.
    pub fn gone(&self, now: f64) -> bool { self.mix(now) >= 1.0 }

    pub fn mix(&self, now: f64) -> f64 {
        match self.judge.verdict {
            Verdict::Hesitant(since) => crate::follow::smoothstep((now - since) / BLEND),
            _ => 0.0,
        }
    }

    /// The picture at `now`, the hand's latches aiming at `aim` (cells
    /// along and down, leaning as the hand does) on `cell`; `axis` the
    /// hand's locked axis.
    pub fn look(&mut self, now: f64, aim: (f64, f64), cell: (i32, i32), axis: usize) -> Look {
        self.judge.check(now);
        let now = now.max(self.last.unwrap_or(now));
        let dt = self.last.replace(now).map_or(0.0, |last| now - last);
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
        Look { axis, at, mix: self.mix(now), flash, settled: self.judge.released && still }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Samples every `dt` of a hand at `speed` cells a second along x from
    /// `from` s for `time` s, then still; judged at `until`.
    fn run(judge: &mut Judge, from: f64, speed: f64, time: f64, dt: f64, until: f64) {
        let mut t = 0.0;
        while t <= until + 1e-9 {
            let x = speed * (t - from).clamp(0.0, time);
            if t <= from + time + 1e-9 { judge.feed(t, (x, 0.0), 0); }
            judge.check(t);
            t += dt;
        }
    }

    #[test]
    fn a_quick_sure_move_is_a_quick_flick_on_mouse_and_touchpad() {
        for dt in [0.001, 0.008] {
            let mut judge = Judge::default();
            run(&mut judge, 0.05, 4.0, 0.45, dt, 0.5);
            assert_eq!(judge.verdict, Verdict::Quick, "every {dt} s");
            // Stopping, then letting go soon, is still sure.
            judge.release(0.6);
            assert_eq!(judge.verdict, Verdict::Quick);
        }
    }

    #[test]
    fn a_slow_or_late_start_shows_the_receded_picture() {
        let mut judge = Judge::default();
        run(&mut judge, 0.05, 1.0, 1.0, 0.001, 0.4);
        assert!(matches!(judge.verdict, Verdict::Hesitant(t) if (t - 0.22).abs() < 0.01));
        let mut judge = Judge::default();
        run(&mut judge, 0.7, 6.0, 0.5, 0.001, 0.9);
        assert!(matches!(judge.verdict, Verdict::Hesitant(_)));
        // Held without moving.
        let mut judge = Judge::default();
        judge.check(0.6);
        assert!(matches!(judge.verdict, Verdict::Hesitant(_)));
    }

    #[test]
    fn pausing_turning_back_or_changing_axis_in_a_quick_flick_hesitates() {
        // Stops after 0.2 s and holds on.
        let mut judge = Judge::default();
        run(&mut judge, 0.0, 4.0, 0.2, 0.001, 0.34);
        assert_eq!(judge.verdict, Verdict::Quick);
        judge.check(0.351);
        assert!(matches!(judge.verdict, Verdict::Hesitant(t) if (t - 0.35).abs() < 0.01));
        // Turns back.
        let mut judge = Judge::default();
        run(&mut judge, 0.0, 4.0, 0.2, 0.001, 0.2);
        judge.feed(0.21, (0.6, 0.0), 0);
        assert!(matches!(judge.verdict, Verdict::Hesitant(_)));
        // Goes the other way.
        let mut judge = Judge::default();
        run(&mut judge, 0.0, 4.0, 0.2, 0.001, 0.2);
        judge.feed(0.21, (0.8, 0.1), 1);
        assert!(matches!(judge.verdict, Verdict::Hesitant(_)));
    }

    #[test]
    fn the_slide_takes_about_150_ms_a_cell_and_hardly_overshoots() {
        let mut quick = Quick { instant: false, judge: Judge::default(), along: [Spring::default(); 2], last: None, cell: (0, 0), came: 0.0 };
        let (mut most, mut near) = (0.0f64, None);
        for frame in 0..=60 {
            let now = frame as f64 / 120.0;
            let look = quick.look(now, (1.0, 0.0), (1, 0), 0);
            most = most.max(look.at);
            if near.is_none() && (look.at - 1.0).abs() < 0.03 { near = Some(now); }
        }
        assert!(most < 1.02, "overshoot {most}");
        assert!(near.is_some_and(|t| (0.1..=0.18).contains(&t)), "{near:?}");
        // Hesitating blends the receded picture in from there.
        quick.judge.hesitate(0.5);
        assert_eq!(quick.mix(0.5), 0.0);
        assert!(quick.mix(0.5 + BLEND / 2.0) > 0.3 && !quick.gone(0.6));
        assert!(quick.gone(0.5 + BLEND));
    }
}
