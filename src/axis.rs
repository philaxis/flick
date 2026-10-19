//! A noisy hand chooses one axis; a turn parks that axis before starting the next.
#[derive(Clone, Copy, Debug)]
pub struct Lock {
    pub axis: Option<usize>,
    pub at: [f64; 2],
    start: [f64; 2],
    off: f64,
    along: f64,
    recent: [f64; 2],
    last: Option<u32>,
    turn: Option<(usize, f64, f64, usize, u32)>,
    pub f_mode: bool,
    row: crate::transition_curve::Latch,
    unlock: bool,
}

impl Default for Lock {
    fn default() -> Self {
        Self { axis: None, at: [0.0; 2], start: [0.0; 2], off: 0.0, along: 0.0, recent: [0.0; 2], last: None, turn: None, f_mode: false, row: crate::transition_curve::Latch::default(), unlock: false }
    }
}

impl Lock {
    /// Physical travel and cell sizes, with a physical dead zone. The first
    /// visible sample is zero; travel used to choose the axis is consumed.
    pub fn travel(&mut self, delta: [f64; 2], step: [f64; 2], dead: f64, ms: u32) -> [f64; 2] {
        let before = self.at;
        if self.unlock {
            self.axis = None; self.start = [0.0; 2]; self.off = 0.0; self.along = 0.0;
            self.recent = [0.0; 2]; self.turn = None; self.unlock = false;
        }
        if let Some((axis, from, to, next, since)) = self.turn {
            let t = (ms.wrapping_sub(since) as f64 / 150.0).clamp(0.0, 1.0);
            self.at[axis] = from + (to - from) * t * t * (3.0 - 2.0 * t);
            if t >= 1.0 {
                self.at[axis] = to;
                self.axis = Some(next);
                self.turn = None;
            }
            return std::array::from_fn(|i| self.at[i] - before[i]);
        }
        let fade = self.last.map_or(0.0, |last| 0.97f64.powf(ms.wrapping_sub(last).min(1000) as f64));
        self.last = Some(ms);
        self.recent = std::array::from_fn(|i| self.recent[i] * fade + delta[i]);
        let Some(axis) = self.axis else {
            self.start = std::array::from_fn(|i| self.start[i] + delta[i]);
            if self.start[0].hypot(self.start[1]) >= dead {
                self.axis = Some(usize::from(self.start[1].abs() > self.start[0].abs() * 1.3));
            }
            return [0.0; 2];
        };
        let side = 1 - axis;
        self.off += delta[side];
        self.along += delta[axis].abs();
        if self.off.abs() >= if self.f_mode { dead } else { step[side] * 0.5 } && self.recent[side].abs() > self.recent[axis].abs() * 2.0 {
            if self.f_mode { self.axis = Some(side); }
            else { self.turn = Some((axis, self.at[axis], self.at[axis].round(), side, ms)); }
            self.off = 0.0;
            self.along = 0.0;
            self.recent = [0.0; 2];
        } else {
            self.at[axis] += delta[axis] / step[axis];
            if self.along >= step[axis] * 0.5 {
                self.off = 0.0;
                self.along = 0.0;
            }
        }
        if self.f_mode && axis == 1 && self.row.track(self.at[1], (i32::MIN + 1, i32::MAX - 1)) != 0 { self.unlock = true; }
        std::array::from_fn(|i| self.at[i] - before[i])
    }
}

/// Only one axis may be between cells, including while catching up with a
/// late first frame or parking the previous axis before a turn.
#[derive(Clone, Copy, Debug, Default)]
pub struct Picture {
    pub at: [f64; 2],
    axis: usize,
    last: Option<f64>,
}

impl Picture {
    pub fn axis(&self) -> usize { self.axis }
    pub fn track(&mut self, target: [f64; 2], axis: usize, now: f64) -> [f64; 2] {
        let dt = self.last.replace(now).map_or(0.0, |last| (now - last).clamp(0.0, 1.0 / 30.0));
        let turning = self.axis != axis;
        let goal = if turning { target[self.axis].round() } else { target[self.axis] };
        let distance = goal - self.at[self.axis];
        // Catch up without skipping the beginning, even after slow thumbnail setup.
        self.at[self.axis] += distance.clamp(-8.0 * dt, 8.0 * dt);
        if turning && distance.abs() < 0.001 {
            self.at[self.axis] = goal;
            self.axis = axis;
        }
        self.at
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dead_zone_and_unclear_diagonals_only_move_sideways() {
        let mut lock = Lock::default();
        for ms in 0..4 { assert_eq!(lock.travel([0.5, 0.5], [10.0; 2], 3.0, ms * 8), [0.0; 2]); }
        for ms in 4..100 { lock.travel([0.5, 0.5], [10.0; 2], 3.0, ms * 8); assert_eq!(lock.at[1], 0.0); }
    }
    #[test]
    fn an_l_parks_before_the_other_axis_moves() {
        let mut lock = Lock::default();
        for ms in 0..34 { lock.travel([0.5, 0.03], [10.0; 2], 3.0, ms * 8); }
        let mut turned = false;
        for ms in 34..100 {
            let by = lock.travel([0.01, -0.5], [10.0; 2], 3.0, ms * 8);
            assert!(by[0].abs() < 1e-9 || by[1].abs() < 1e-9);
            if lock.axis == Some(1) { turned = true; assert!((lock.at[0] - lock.at[0].round()).abs() < 1e-9); }
        }
        assert!(turned && lock.at[1] < -1.0);
    }
    #[test]
    fn ordinary_dragging_tracks_one_to_one_after_the_first_frame() {
        let mut picture = Picture::default();
        assert_eq!(picture.track([0.4, 0.0], 0, 0.0), [0.0; 2]);
        for frame in 1..100 {
            let wanted = 0.4 + frame as f64 / 120.0;
            let at = picture.track([wanted, 0.0], 0, frame as f64 / 120.0);
            if frame >= 8 { assert!((at[0] - wanted).abs() < 1e-9); }
        }
    }
    #[test]
    fn noisy_diagonals_and_turns_never_draw_two_fractional_axes() {
        let mut seed = 42u32;
        let mut lock = Lock::default();
        let mut picture = Picture::default();
        let mut before = [0.0; 2];
        for frame in 0..10000 {
            let mut noise = || { seed = seed.wrapping_mul(1664525).wrapping_add(1013904223); (seed as f64 / u32::MAX as f64 - 0.5) * 0.6 };
            let active = (frame / 120) % 2;
            let mut delta = [noise(), noise()];
            delta[active] += if frame % 480 < 240 { 0.6 } else { -0.6 };
            lock.travel(delta, [10.0, 15.0], 3.0, (frame * 8) as u32);
            let at = picture.track(lock.at, lock.axis.unwrap_or(0), frame as f64 * 0.008);
            assert!((at[0] - at[0].round()).abs() < 1e-9 || (at[1] - at[1].round()).abs() < 1e-9, "{at:?}");
            assert!((at[0] - before[0]).abs() < 1e-9 || (at[1] - before[1]).abs() < 1e-9);
            before = at;
        }
    }
}

/// A small Flick at a confirmed cell: 3.8% overshoot, finished in 300 ms.
pub const DAMPING: f64 = 0.72;
pub const SETTLE_TIME: f64 = 0.30;
const FREQUENCY: f64 = 10.0;
pub fn settle(progress: f64) -> f64 { settle_with_speed(progress, 0.0) }
pub fn settle_with_speed(progress: f64, speed: f64) -> f64 {
    if progress <= 0.0 { return 0.0; }
    if progress >= 1.0 { return 1.0; }
    let root = (1.0 - DAMPING * DAMPING).sqrt();
    let t = FREQUENCY * progress;
    let raw = 1.0 - (-DAMPING * t).exp() * ((root * t).cos() + (DAMPING - speed / FREQUENCY) / root * (root * t).sin());
    let u = ((progress - 0.8) / 0.2).clamp(0.0, 1.0);
    raw + (1.0 - raw) * u * u * (3.0 - 2.0 * u)
}

#[cfg(test)]
mod spring_tests {
    use super::*;
    #[test]
    fn flick_accelerates_then_overshoots_once_and_settles() {
        let samples: Vec<_> = (0..=300).map(|ms| settle(ms as f64 / 300.0)).collect();
        let peak = samples.iter().copied().fold(0.0, f64::max);
        assert!((1.03..=1.06).contains(&peak), "{peak}");
        let first_crossing = samples.iter().position(|x| *x > 1.0).unwrap();
        assert!(samples[..first_crossing].windows(2).all(|pair| pair[1] >= pair[0]));
        assert!(samples[10] - samples[0] < samples[30] - samples[20]);
        assert!(samples[250..].iter().all(|x| (1.0 - x).abs() < 0.005));
        assert_eq!(samples[300], 1.0);
        assert!((0.25..=0.35).contains(&SETTLE_TIME));
    }
}

/// Derivative per unit progress of the same damped step response.
pub fn settle_speed(progress: f64, speed: f64) -> f64 {
    if progress >= 1.0 { return 0.0; }
    let root = (1.0 - DAMPING * DAMPING).sqrt();
    let t = FREQUENCY * progress.max(0.0);
    let raw = 1.0 - (-DAMPING * t).exp() * ((root * t).cos() + (DAMPING - speed / FREQUENCY) / root * (root * t).sin());
    let velocity = (-DAMPING * t).exp() * (speed * (root * t).cos() + (FREQUENCY - DAMPING * speed) / root * (root * t).sin());
    let u = ((progress - 0.8) / 0.2).clamp(0.0, 1.0);
    velocity * (1.0 - u * u * (3.0 - 2.0 * u)) + (1.0 - raw) * 6.0 * u * (1.0 - u) / 0.2
}
