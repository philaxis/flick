//! How far the screen recedes as the hand goes (`depth`, 0 to 1), as a
//! marking menu shows itself only to a hand that hesitates: a hand that
//! flicks (a stroke set off sharply) and lets go soon (within `QUICK_MOST`
//! of setting off) means one cell, no more: the screen stays whole and
//! still, and let go of, the desktop next to it its way is there at once
//! (`QuickFlick::Instant` darkening it for a moment), however short or
//! long, slow or fast the stroke went. Every other hand is taken as
//! unsure, soon (still, set off slowly, held on, turning back, going the
//! other way): the screen goes all the way back at once, not by halves,
//! and stays so until let go: a hand gone unsure does not get the screen
//! whole again by going on quickly.
//!
//! The hand is read in cells, which each device counts in its own units
//! (mouse counts, touchpad steps of the pad's own size, `input`), so a
//! flick is judged alike on either.
//!
//! The judging (`Judge`) only reads the hand. Whole, the screen stays on
//! the cell it set off from, giving only as the latches do when pushed
//! past the row's end (`Follow::past`), and let go of sure it is on the
//! next cell its way (`Judge::step`, `Follow::let_go_one`), or comes back
//! onto the last one at the row's end; unsure, it eases to where the hand
//! is and the cells gone to are the hand's latches (`follow::Follow`),
//! switched to once, on letting go.
use crate::{config::QuickFlick, transition_curve::Spring};
use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};

/// How far the hand goes before it counts as on its way, in cells.
pub const START: f64 = 0.02;
/// Not set off this soon after the hold begins (s), the hand is unsure.
pub const ONSET_LIMIT: f64 = 0.25;
/// A stroke is a flick when, within `DECIDE` (s) of setting off, it gets
/// as fast as `PEAK` (cells a second) for a moment: a sharp start, not a
/// speed kept up. Not so, it is unsure.
pub const DECIDE: f64 = 0.12;
pub const PEAK: f64 = 2.5;
/// Sure and not let go of this long (s) after setting off, it is unsure;
/// `experimental.flick_ms` in config.toml sets it (`quick_most`).
pub const QUICK_MOST: f64 = 0.35;
/// Of `quick_most`, how much passes with the screen whole before it starts
/// to recede towards the depth it has once unsure: none before, so that a
/// flick does not see it, and in between more and more, slowly at first,
/// so that a hand held on goes from a flick to a precise hold with no step
/// in the picture.
const LEAN_FROM: f64 = 0.6;
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
/// Let go of unsure, the screen lands as a critically damped spring of
/// this (rad/s): from half a cell still in about 0.1 s, never past the cell.
const LAND: f64 = 75.0;
/// `Instant`: how dark a desktop comes in, and how soon it is clear (s).
const FLASH: (f64, f64) = (0.45, 0.12);

static MODE: AtomicU8 = AtomicU8::new(QuickFlick::Slide as u8);
static QUICK_MOST_MS: AtomicU32 = AtomicU32::new((QUICK_MOST * 1000.0) as u32);

pub fn configure(mode: QuickFlick, flick_ms: u32) {
    MODE.store(mode as u8, Ordering::Relaxed);
    QUICK_MOST_MS.store(flick_ms.clamp(80, 2000), Ordering::Relaxed);
}

/// Not set off this soon after the hold begins (s), the hand is unsure:
/// `ONSET_LIMIT`, or less with a shorter flick time, a flick being a
/// click with a stroke in it.
fn onset_limit() -> f64 {
    ONSET_LIMIT.min(quick_most())
}

/// How long after setting off a flick may be held on (s).
pub fn quick_most() -> f64 {
    QUICK_MOST_MS.load(Ordering::Relaxed) as f64 / 1000.0
}

/// How far a hand still taken for a flick `held` s after setting off wants
/// the screen to recede: nothing until `LEAN_FROM` of `quick_most`, then
/// rising flat at first and gently into all the way at `quick_most`, where
/// it is unsure.
fn lean(held: f64) -> f64 {
    let most = quick_most();
    let t = ((held - LEAN_FROM * most) / ((1.0 - LEAN_FROM) * most)).clamp(0.0, 1.0);
    t * t * t * (t * (6.0 * t - 15.0) + 10.0)
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
    /// Its way so far over the last `WINDOW`, and one more.
    recent: Vec<(f64, f64)>,
    /// Let go of, and how far it wanted the screen to recede then.
    released: Option<f64>,
    /// How far it wants the screen to recede while not yet unsure (`lean`).
    lean: f64,
}

impl Judge {
    /// From here on its way is `axis`, `sign`, starting at `t`, `gone` along
    /// it, and it is unsure (for good, this hold).
    fn anchor(&mut self, t: f64, axis: usize, sign: f64, gone: f64) {
        self.onset = Some((self.onset.map_or(t, |(onset, ..)| onset), axis, sign));
        (self.far, self.recent, self.mind) = (gone, vec![(t, gone)], Mind::Unsure);
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
            (Mind::Deciding, None) => if now > onset_limit() { self.mind = Mind::Unsure; },
            // Set off sharply, a flick; slow to set off, or not sharp soon, unsure.
            (Mind::Deciding, Some(onset)) => {
                if onset > onset_limit() || (speed < PEAK && now - onset > DECIDE) { self.mind = Mind::Unsure; }
                else if speed >= PEAK { self.mind = Mind::Sure; }
            }
            // Held on, it is no flick.
            (_, onset) => if onset.is_some_and(|onset| now - onset > quick_most()) { self.mind = Mind::Unsure; },
        }
        self.lean = self.onset.map_or(0.0, |(onset, ..)| lean(now - onset));
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
        match (self.mind, self.released) {
            (Mind::Unsure, _) => 1.0,
            // Let go of as a flick, the screen comes whole again as it lands.
            (_, Some(_)) => 0.0,
            _ => self.lean,
        }
    }

    /// Let go of: as it is, from now on (not yet sure, the screen whole
    /// and the latches' cell).
    pub fn release(&mut self, now: f64) {
        if self.released.is_some() { return; }
        self.check(now);
        self.released = Some(self.wanted());
    }

    /// The axis it goes along.
    pub fn axis(&self) -> Option<usize> { self.onset.map(|(_, axis, _)| axis) }

    /// Which way along it, 1 or -1 (0 not yet set off).
    pub fn way(&self) -> f64 { self.onset.map_or(0.0, |(.., sign)| sign) }

    /// Where a hold let go of goes from the cell it set off from (cells
    /// along and down): sure, a cell its way; else nowhere of its own.
    pub fn step(&self) -> (f64, f64) {
        let Some((_, axis, sign)) = self.onset.filter(|_| self.mind == Mind::Sure) else { return (0.0, 0.0) };
        if axis == 0 { (sign, 0.0) } else { (0.0, sign) }
    }
}

/// The speed a landing `off` (cells) from its cell sets off at, the hand
/// going `speed`: as fast towards it as can still stop on it (`LAND`), and
/// not away.
fn launch(off: f64, speed: f64) -> f64 {
    if speed * off >= 0.0 { 0.0 } else { speed.clamp(-LAND * off.abs(), LAND * off.abs()) }
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
    /// The cell it set off from, whether the screen was last kept on it,
    /// the hand not yet unsure, and what of it the picture still owes as
    /// it goes to the hand.
    from: Option<(i32, i32)>,
    still: bool,
    ease: [Spring; 2],
    /// Kept on it, whether it shows the hand past the end there.
    edge: [bool; 2],
    /// How far past the end the hand last showed (`Follow::past`), what
    /// of it the picture still owes as the hand turns back (`Latch` drops
    /// it at once), and whether it was let go of there.
    past: [f64; 2],
    back: [Spring; 2],
    rebound: bool,
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
        Some(Quick { instant, judge: Judge::default(), along: [Spring::default(); 2], landing: false, from: None, still: false, ease: [Spring::default(); 2], edge: [false; 2], past: [0.0; 2], back: [Spring::default(); 2], rebound: false, depth: 0.0, last: None, cell: (0, 0), came: f64::NEG_INFINITY })
    }

    /// Let go of at `now` (as `follow::Follow::land` lets its judge go).
    #[cfg(test)]
    pub fn release(&mut self, now: f64) { self.judge.release(now); }

    /// How far past the end the hand shows (`Follow::past`) for the next
    /// `look`: turned back from there the picture eases in as the latches
    /// do (`Spring::position`) rather than jumping.
    pub fn past_end(&mut self, past: (f64, f64)) {
        for (way, past) in [past.0, past.1].into_iter().enumerate() {
            if past.abs() < self.past[way].abs() { self.back[way].p += self.past[way] - past; }
            self.past[way] = past;
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
            // Whole, on the cell it set off from, or past the end there as
            // far as the latches give; unsure, where the hand is, eased to
            // from where it was kept.
            let still = self.judge.mind != Mind::Unsure;
            let from = self.from.get_or_insert(cell);
            let (held, from) = ([held.0, held.1], [from.0 as f64, from.1 as f64]);
            for (way, (back, ease)) in self.back.iter_mut().zip(&mut self.ease).enumerate() {
                back.position(0.0, dt, way == 1);
                ease.position(0.0, dt, way == 1);
            }
            for way in 0..2 {
                // Past the end only pushed out from it, and while it still owes some of it.
                let out = (held[way] - self.past[way] - from[way]).abs() < 1e-9 && self.past[way] != 0.0;
                self.edge[way] = out || self.edge[way] && self.back[way].p != 0.0;
                let kept = if self.edge[way] { from[way] + self.past[way] } else { from[way] };
                if self.still && !still { self.ease[way].p = kept - held[way]; }
                if still && !self.edge[way] { self.back[way] = Spring::default(); }
                let (back, ease) = (self.back[way], self.ease[way]);
                let at = if still { kept } else { held[way] };
                self.along[way] = Spring { p: at + back.p + ease.p, v: back.v + ease.v };
            }
            self.still = still;
            if self.judge.released.is_some() && !self.landing {
                self.landing = true;
                // Let go of past the end, it comes back as the latches do;
                // whole, it is on its cell at once; unsure, off as fast as
                // the hand went.
                self.rebound = if still { self.edge.contains(&true) } else { self.past.iter().chain(self.back.iter().map(|back| &back.p)).any(|&off| off.abs() > 1e-9) };
                if !self.rebound && still {
                    self.along = [Spring { p: aim[0], v: 0.0 }, Spring { p: aim[1], v: 0.0 }];
                } else if let (Some(way), false) = (self.judge.axis(), self.rebound) {
                    self.along[way].v = launch(self.along[way].p - aim[way], self.judge.way() * self.judge.speed(now));
                }
            }
        } else {
            for (way, (spring, aim)) in self.along.iter_mut().zip(aim).enumerate() {
                if self.rebound { spring.commit(aim, dt, way == 1); } else { spring.advance(aim, dt, LAND, 1.0); }
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
    use crate::transition_curve::{give, EDGE};

    fn fresh() -> Quick {
        Quick { instant: false, judge: Judge::default(), along: [Spring::default(); 2], landing: false, from: None, still: false, ease: [Spring::default(); 2], edge: [false; 2], past: [0.0; 2], back: [Spring::default(); 2], rebound: false, depth: 0.0, last: None, cell: (0, 0), came: 0.0 }
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

    /// The hand at `x(t)` until `end` on a row of `cols`, let go of as
    /// `glide` lets go: the cell gone to and what the hand was taken to be.
    fn let_go(x: &dyn Fn(f64) -> f64, end: f64, cols: (i32, i32)) -> ((i32, i32), Mind) {
        let mut quick = fresh();
        let mut hand = crate::follow::Follow::new((0, 0), vec![cols]);
        let mut t = 0.0;
        while t <= end + 1e-9 {
            hand.feed(Some(&mut quick.judge), t, (x(t), 0.0), 0);
            if quick.judge.mind == Mind::Sure { assert!(hand.held_as(true).0.abs() <= 1.0 + EDGE); }
            t += 0.001;
        }
        let (to, _) = hand.land(Some(&mut quick.judge), t, (x(t), 0.0));
        (to, quick.judge.mind)
    }

    /// The hand going from 0 to `to` (cells) at `speed`, set off at 0.05 s.
    fn go(to: f64, speed: f64) -> impl Fn(f64) -> f64 {
        move |t: f64| to.signum() * (speed * (t - 0.05).max(0.0)).min(to.abs())
    }

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
            let frames = run(&mut quick, |t| stroke(t, 0.05, 0.6, 0.06), dt, 0.0, 0.25);
            assert!(stroke(0.25, 0.05, 0.6, 0.06) / 0.2 < 3.0 && deepest(&frames) < 0.05, "every {dt} s: {frames:?}");
            assert_eq!(quick.judge.mind, Mind::Sure);
            // Stopping, then letting go soon, stays whole.
            quick.release(0.27);
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
        // Two cells a second, steadily, and slower: in between only for a moment, then all the way.
        for speed in [2.0, 1.0] {
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
    fn held_still_it_is_precise_within_a_third_of_a_second() {
        let mut quick = fresh();
        let frames = run(&mut quick, |_| 0.0, 0.001, 0.0, 1.0);
        let unsure = frames.iter().find(|f| f.2 == Mind::Unsure).unwrap().0;
        assert!(unsure <= 0.3, "{unsure}");
        // Whole until then, then all the way back at once, ease-out and without a jump.
        assert!(frames.iter().filter(|f| f.0 < unsure).all(|f| f.1 == 0.0));
        assert!(frames.iter().find(|f| f.0 >= unsure + 0.12).unwrap().1 > 0.93);
        assert!(frames.windows(2).all(|w| w[1].1 - w[0].1 < 0.21));
        assert!(frames.last().unwrap().1 > 0.98);
    }

    #[test]
    fn a_flick_let_go_soon_goes_one_cell_however_far_the_hand_went() {
        // A tenth of a cell at eight a second, or two cells at twenty: a cell, either way.
        assert_eq!(let_go(&go(0.1, 8.0), 0.07, (-3, 3)), ((1, 0), Mind::Sure));
        assert_eq!(let_go(&go(-0.1, 8.0), 0.07, (-3, 3)), ((-1, 0), Mind::Sure));
        assert_eq!(let_go(&go(2.0, 20.0), 0.16, (-3, 3)), ((1, 0), Mind::Sure));
        assert_eq!(let_go(&go(2.0, 20.0), 0.25, (-3, 3)), ((1, 0), Mind::Sure));
        // At the row's end, back on the last cell.
        assert_eq!(let_go(&go(0.6, 20.0), 0.1, (-3, 0)), ((0, 0), Mind::Sure));
    }

    #[test]
    fn a_flick_held_on_is_precise_and_lands_on_the_latches() {
        let x = go(2.7, 6.0);
        let mut quick = fresh();
        let frames = run(&mut quick, &x, 0.001, 0.0, 0.6);
        let unsure = frames.iter().find(|f| f.2 == Mind::Unsure).unwrap().0;
        assert!(frames.iter().any(|f| f.2 == Mind::Sure) && (0.05 + quick_most()..=0.05 + quick_most() + 0.01).contains(&unsure), "{unsure}");
        let mut hand = crate::follow::Follow::new((0, 0), vec![(-3, 3)]);
        let mut t = 0.0;
        while t <= 0.6 {
            hand.track_locked(t, (x(t), 0.0), 0);
            t += 0.001;
        }
        let latched = hand.clone().let_go(0.6, (x(0.6), 0.0));
        assert!(latched.0 >= 2, "{latched:?}");
        assert_eq!(let_go(&x, 0.6, (-3, 3)), (latched, Mind::Unsure));
    }

    #[test]
    fn stopping_a_while_turning_back_or_changing_axis_goes_all_the_way_back_at_once() {
        // A flick, then held on.
        let mut quick = fresh();
        let frames = run(&mut quick, |t| stroke(t, 0.05, 1.5, 0.04), 0.001, 0.0, 0.8);
        let at = |t: f64| frames.iter().find(|f| f.0 >= t).unwrap();
        assert!(at(0.25).1 < 0.05);
        let unsure = frames.iter().find(|f| f.2 == Mind::Unsure).unwrap().0;
        assert!((0.05 + quick_most()..=0.07 + quick_most()).contains(&unsure), "{unsure}");
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
        // Let go unsure, the next hold starts afresh.
        quick.release(0.61);
        assert_eq!(quick.judge.mind, Mind::Unsure);
        let mut next = fresh();
        let frames = run(&mut next, |t| stroke(t, 0.05, 1.0, 0.04), 0.001, 0.0, 0.25);
        assert!(deepest(&frames) < 0.05, "{frames:?}");
    }

    #[test]
    fn whole_the_screen_stays_on_its_cell_and_let_go_is_on_the_next_at_once() {
        // Held, sure or not yet: on the cell it set off from, wherever the hand is.
        let x = |t: f64| stroke(t, 0.02, 1.2, 0.05);
        let mut quick = fresh();
        let mut hand = crate::follow::Follow::new((0, 0), vec![(-1, 3)]);
        let mut t = 0.0;
        while t < 0.06 {
            hand.track_locked(t, (x(t), 0.0), 0);
            quick.judge.feed(t, (x(t), 0.0), 0);
            let held = if quick.judge.mind == Mind::Sure { hand.within_a_cell(hand.held()) } else { hand.held() };
            let look = quick.look(t, hand.aim(), Some(held), hand.selected(), 0);
            assert_eq!((look.at, look.depth), (0.0, 0.0), "{t}: for {}", x(t));
            t += 1.0 / 120.0;
        }
        assert_eq!(quick.judge.mind, Mind::Sure);
        // Let go of: on the next cell that very frame, at rest.
        quick.release(t);
        let held = hand.within_a_cell(hand.held());
        let to = hand.let_go_one(t, (x(t), 0.0), quick.judge.step());
        assert_eq!(to, (1, 0));
        let look = quick.look(t, hand.aim(), Some(held), to, 0);
        assert!(look.at == 1.0 && look.settled, "{look:?}");
        assert_eq!(quick.look(t + 1.0 / 120.0, hand.aim(), None, to, 0).at, 1.0);
        // Gone unsure, to where the hand is without a jump.
        let mut quick = fresh();
        let mut hand = crate::follow::Follow::new((0, 0), vec![(0, 3)]);
        let x = go(2.0, 1.5);
        let (mut t, mut last) = (0.0, 0.0f64);
        while t < 0.6 {
            hand.track_locked(t, (x(t), 0.0), 0);
            quick.judge.feed(t, (x(t), 0.0), 0);
            let at = quick.look(t, hand.aim(), Some(hand.held()), hand.selected(), 0).at;
            assert!((at - last).abs() < 0.12, "{t}: {last} to {at}");
            last = at;
            t += 1.0 / 120.0;
        }
        assert_eq!(quick.judge.mind, Mind::Unsure);
        assert!((last - hand.held().0).abs() < 0.01, "{last} for {:?}", hand.held());
        // At the row's end, held only as far as the latches give there.
        let mut hand = crate::follow::Follow::new((0, 0), vec![(-1, 0)]);
        hand.track_locked(0.0, (0.3, 0.0), 0);
        assert_eq!(hand.held(), (give(0.3), 0.0));
    }

    #[test]
    fn past_the_end_the_hand_gives_as_the_latches_do_and_lets_go_back_onto_the_last_cell() {
        // Each row its own end, and the rows theirs, heavier the farther and never a cell's EDGE.
        let mut hand = crate::follow::Follow::new((0, 1), vec![(0, 3), (0, 1)]);
        hand.track_locked(0.0, (0.5, 0.0), 0);
        assert_eq!(hand.held(), (0.5, 0.0));
        hand.track_locked(0.01, (3.3, 0.0), 0);
        assert!((hand.held().0 - 3.0 - give(0.3)).abs() < 1e-9 && (hand.past().0 - give(0.3)).abs() < 1e-9 && hand.past().1 == 0.0);
        hand.track_locked(0.02, (90.0, 0.0), 0);
        assert!(hand.held().0 > 3.14 && hand.held().0 < 3.0 + EDGE);
        let mut hand = crate::follow::Follow::new((0, 1), vec![(0, 3), (0, 1)]);
        hand.track_locked(0.0, (0.0, 1.0), 1);
        hand.track_locked(0.01, (1.4, 1.0), 0);
        assert!((hand.held().0 - 1.0 - give(0.4)).abs() < 1e-9, "{:?}", hand.held());
        let mut hand = crate::follow::Follow::new((0, 1), vec![(0, 3), (0, 1)]);
        hand.track_locked(0.0, (0.0, -0.2), 1);
        assert!((hand.held().1 + give(0.2)).abs() < 1e-9);

        // Turned back there, the picture eases in rather than jumping.
        let mut quick = fresh();
        let mut hand = crate::follow::Follow::new((0, 0), vec![(-1, 0)]);
        let x = |t: f64| if t < 0.1 { 1.5 * t / 0.1 } else { 1.5 - (t - 0.1) };
        let (mut t, mut last) = (0.0, 0.0f64);
        while t < 0.3 {
            hand.track_locked(t, (x(t), 0.0), 0);
            quick.past_end(hand.past());
            let at = quick.look(t, hand.aim(), Some(hand.held()), hand.selected(), 0).at;
            if t > 0.1 { assert!((at - last).abs() < 0.05, "{t}: {last} to {at}"); }
            last = at;
            t += 1.0 / 120.0;
        }

        // Let go past the end, flicked or not, back onto it as a latch's commit
        // comes back (`Spring::commit`), never past the end beyond.
        for speed in [1.0, 18.0] {
            let mut quick = fresh();
            let mut hand = crate::follow::Follow::new((0, 0), vec![(-3, 0)]);
            let x = |t: f64| speed * t;
            let mut t = 0.0;
            while t < 0.1 {
                hand.track_locked(t, (x(t), 0.0), 0);
                quick.judge.feed(t, (x(t), 0.0), 0);
                quick.past_end(hand.past());
                let at = quick.look(t, hand.aim(), Some(hand.held()), hand.selected(), 0).at;
                assert!(t < 0.02 || at > 0.0, "{speed} {t}: {at}");
                t += 1.0 / 120.0;
            }
            quick.release(t);
            let (held, past) = (hand.held(), hand.past());
            let to = if quick.judge.mind == Mind::Sure { hand.let_go_one(t, (x(t), 0.0), quick.judge.step()) } else { hand.let_go(t, (x(t), 0.0)) };
            assert_eq!((to, quick.judge.mind == Mind::Sure), ((0, 0), speed > PEAK));
            quick.past_end(past);
            let mut look = quick.look(t, hand.aim(), Some(held), to, 0);
            let mut commit = Spring { p: look.at, v: 0.0 };
            let mut most = look.at;
            for frame in 1..=48 {
                look = quick.look(t + frame as f64 / 120.0, hand.aim(), None, to, 0);
                commit.commit(0.0, 1.0 / 120.0, false);
                assert!((look.at - commit.p).abs() < 1e-6);
                most = most.max(look.at);
            }
            assert!(most < EDGE && look.settled && look.at == 0.0, "{speed}: {most} {look:?}");
        }
    }
}
