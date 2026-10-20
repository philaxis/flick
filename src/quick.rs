//! How far the screen recedes as the hand goes (`depth`, 0 to 1): a hand
//! sure of its way keeps the screen whole and the desktop it goes to
//! slides in (or comes at once, `QuickFlick::Instant`); the slower and the
//! less sure it goes, the farther the screen recedes, all the way to the
//! zoomed-out picture once it stops, turns back or goes the other way. In
//! between, as a marking menu does with a hand half sure, the screen stays
//! receded part of the way and slides on so.
//!
//! The judging (`Judge`) only reads the hand; the cells gone to are still
//! the hand's latches (`follow::Follow`), switched to once, on letting go.
use crate::{config::QuickFlick, transition_curve::Spring};
use std::sync::atomic::{AtomicU8, Ordering};

/// How far the hand goes before it counts as on its way, in cells.
pub const START: f64 = 0.02;
/// Not set off this soon after the hold begins (s), the hand is unsure.
pub const ONSET_LIMIT: f64 = 0.5;
/// Just set off, for this long (s), it is taken as sure.
pub const DECIDE: f64 = 0.15;
/// At this speed or slower (cells a second) the screen recedes all the
/// way; at `SURE_SPEED` or faster it stays whole; in between, part way.
pub const SLOW: f64 = 1.0;
pub const SURE_SPEED: f64 = 3.0;
/// Turning back this far (cells) from the farthest it went, or going
/// the other way, recedes all the way for this long (s).
pub const BACK: f64 = 0.15;
pub const DOUBT: f64 = 0.15;
/// The hand's speed is taken over this long (s).
const WINDOW: f64 = 0.05;
/// Let go of this soon after slowing (s), it is judged as it went before.
const LATE: f64 = 0.1;
/// The depth follows what the hand says as a low-pass filter: going back
/// at once, quickest at first, settling in about three times this (s);
/// coming forward again a little more slowly, so a hand slowing for a
/// moment does not flicker the picture.
pub const RECEDE: f64 = 0.06;
pub const RETURN: f64 = 0.1;
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

/// How far the screen should recede for a hand going `speed` cells a second.
pub fn wanted(speed: f64) -> f64 {
    1.0 - ((speed - SLOW) / (SURE_SPEED - SLOW)).clamp(0.0, 1.0)
}

/// The depth `depth` following `wanted` over `dt` s: no jump, receding
/// quickest at first (ease-out), exact whatever the frame rate.
pub fn approach(depth: f64, wanted: f64, dt: f64) -> f64 {
    let tau = if wanted > depth { RECEDE } else { RETURN };
    let next = wanted + (depth - wanted) * (-dt.max(0.0) / tau).exp();
    if (next - wanted).abs() < 0.002 { wanted } else { next }
}

/// What the hand's way says of the hold, from its samples (time from the
/// hold's start, cells along and down, the locked axis) and the time.
#[derive(Clone, Debug, Default)]
pub struct Judge {
    /// When it set off, along which axis, which way.
    onset: Option<(f64, usize, f64)>,
    /// The farthest it went its way.
    far: f64,
    /// Until when it is in doubt (turned back, went the other way).
    doubt: f64,
    /// Its way so far over the last `WINDOW` and `LATE`, and one more.
    recent: Vec<(f64, f64)>,
    /// Let go of, and how far it wanted the screen to recede then.
    released: Option<f64>,
}

impl Judge {
    /// From here on its way is `axis`, `sign`, starting at `t`, `gone` along it.
    fn anchor(&mut self, t: f64, axis: usize, sign: f64, gone: f64) {
        self.onset = Some((self.onset.map_or(t, |(onset, ..)| onset), axis, sign));
        (self.far, self.recent) = (gone, vec![(t, gone)]);
    }

    pub fn feed(&mut self, t: f64, at: (f64, f64), axis: usize) {
        if self.released.is_some() { return; }
        let along = if axis == 0 { at.0 } else { at.1 };
        let Some((_, way, sign)) = self.onset else {
            if along.abs() >= START {
                self.onset = Some((t, axis, along.signum()));
                self.recent = vec![(t, along.abs())];
                self.far = along.abs();
                // Setting off late is a doubt of its own (`wanted_at`).
            }
            return;
        };
        if axis != way {
            self.doubt = t + DOUBT;
            return self.anchor(t, axis, along.signum(), along.abs());
        }
        let gone = sign * along;
        if gone < self.far - BACK {
            self.doubt = t + DOUBT;
            return self.anchor(t, way, -sign, -gone);
        }
        self.far = self.far.max(gone);
        self.recent.push((t, gone));
        let keep = self.recent.iter().rposition(|(when, _)| *when <= t - WINDOW - LATE).unwrap_or(0);
        self.recent.drain(..keep);
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

    /// How far the hand wants the screen to recede at `now`.
    pub fn wanted_at(&self, now: f64) -> f64 {
        if let Some(wanted) = self.released { return wanted; }
        match self.onset {
            None => if now > ONSET_LIMIT { 1.0 } else { 0.0 },
            Some((onset, ..)) if onset > ONSET_LIMIT && now - onset <= DECIDE => 1.0,
            Some(_) if now < self.doubt => 1.0,
            Some((onset, ..)) if now - onset <= DECIDE => 0.0,
            Some(_) => wanted(self.speed(now)),
        }
    }

    /// Let go of: as it is, from now on (just set off counts as sure, and
    /// so does letting go just after stopping, as a flick does).
    pub fn release(&mut self, now: f64) {
        if self.released.is_none() { self.released = Some(self.wanted_at(now).min(self.wanted_at(now - LATE))); }
    }

    /// The axis it goes along.
    pub fn axis(&self) -> Option<usize> { self.onset.map(|(_, axis, _)| axis) }
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
        Some(Quick { instant, judge: Judge::default(), along: [Spring::default(); 2], depth: 0.0, last: None, cell: (0, 0), came: f64::NEG_INFINITY })
    }

    /// The picture at `now`, the hand's latches aiming at `aim` (cells
    /// along and down, leaning as the hand does) on `cell`; `axis` the
    /// hand's locked axis.
    pub fn look(&mut self, now: f64, aim: (f64, f64), cell: (i32, i32), axis: usize) -> Look {
        let now = now.max(self.last.unwrap_or(now));
        let dt = self.last.replace(now).map_or(0.0, |last| now - last);
        self.depth = approach(self.depth, self.judge.wanted_at(now), dt);
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
        let wanted = self.judge.wanted_at(now);
        Look { axis, at, depth: self.depth, flash, settled: self.judge.released.is_some() && still && self.depth == wanted }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> Quick {
        Quick { instant: false, judge: Judge::default(), along: [Spring::default(); 2], depth: 0.0, last: None, cell: (0, 0), came: 0.0 }
    }

    /// A hand along x at `x(t)` (cells), sampled every `dt` until `until`,
    /// drawn every 120th of a second: the depth of each frame.
    fn run(quick: &mut Quick, x: impl Fn(f64) -> f64, dt: f64, from: f64, until: f64) -> Vec<f64> {
        let mut depths = Vec::new();
        let (mut t, mut frame) = (from, from);
        while t <= until + 1e-9 {
            quick.judge.feed(t, (x(t), 0.0), 0);
            if t >= frame {
                depths.push(quick.look(t, (x(t).round(), 0.0), (x(t).round() as i32, 0), 0).depth);
                frame += 1.0 / 120.0;
            }
            t += dt;
        }
        depths
    }

    #[test]
    fn a_quick_sure_move_keeps_the_screen_whole_on_mouse_and_touchpad() {
        for dt in [0.001, 0.008] {
            let mut quick = fresh();
            let depths = run(&mut quick, |t| 4.0 * (t - 0.05).clamp(0.0, 0.45), dt, 0.0, 0.5);
            assert!(depths.iter().all(|d| *d < 0.05), "every {dt} s: {depths:?}");
            // Stopping, then letting go soon, stays whole.
            quick.judge.release(0.52);
            assert!(quick.look(0.6, (2.0, 0.0), (2, 0), 0).depth < 0.05);
        }
    }

    #[test]
    fn the_hand_s_speed_sets_how_far_the_screen_recedes_between_the_two() {
        assert_eq!((wanted(0.5), wanted(SLOW), wanted(2.0), wanted(SURE_SPEED), wanted(9.0)), (1.0, 1.0, 0.5, 0.0, 0.0));
        // Half sure: three quarters as large, sliding on.
        let mut quick = fresh();
        let depths = run(&mut quick, |t| 2.0 * (t - 0.05).max(0.0), 0.001, 0.0, 1.0);
        let last = *depths.last().unwrap();
        assert!((last - 0.5).abs() < 0.05, "{last}");
        assert!((crate::follow::scale(last) - 0.75).abs() < 0.03);
        // Slow, or held without moving: all the way.
        let mut quick = fresh();
        let depths = run(&mut quick, |t| 0.6 * t, 0.001, 0.0, 1.0);
        assert!(*depths.last().unwrap() > 0.98);
        let mut quick = fresh();
        let depths = run(&mut quick, |_| 0.0, 0.001, 0.0, 1.0);
        assert!(depths[..55].iter().all(|d| *d == 0.0) && *depths.last().unwrap() > 0.98);
    }

    #[test]
    fn hesitating_recedes_at_once_without_a_jump_and_going_on_comes_forward() {
        // Quick, stops at 0.25 s, holds still, sets off again at 0.6 s.
        let x = |t: f64| 4.0 * t.min(0.25) + 4.0 * (t - 0.6).max(0.0);
        let mut quick = fresh();
        let depths = run(&mut quick, x, 0.001, 0.0, 1.0);
        let frame = |t: f64| (t * 120.0).round() as usize;
        assert!(depths[frame(0.25)] < 0.05);
        // Receding starts within a few frames, quickest first, and never jumps.
        assert!(depths[frame(0.25) + 6] > 0.3, "{:?}", &depths[frame(0.25)..frame(0.4)]);
        let steps: Vec<f64> = depths.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(steps.iter().all(|s| s.abs() < 0.16), "{steps:?}");
        assert!(depths[frame(0.55)] > 0.95);
        assert!(*depths.last().unwrap() < 0.05);
        // Turning back, or going the other way, recedes all the way for a moment.
        let mut quick = fresh();
        run(&mut quick, |t| 4.0 * t, 0.001, 0.0, 0.2);
        quick.judge.feed(0.21, (0.6, 0.0), 0);
        assert_eq!(quick.judge.wanted_at(0.22), 1.0);
        let mut quick = fresh();
        run(&mut quick, |t| 4.0 * t, 0.001, 0.0, 0.2);
        quick.judge.feed(0.21, (0.8, 0.1), 1);
        assert_eq!(quick.judge.wanted_at(0.22), 1.0);
        assert_eq!(quick.judge.axis(), Some(1));
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
