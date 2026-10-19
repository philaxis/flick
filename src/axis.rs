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
        let turning = if self.f_mode { self.off.abs() >= dead && delta[side].abs() > delta[axis].abs() * 1.3 }
            else { self.off.abs() >= step[side] * 0.5 && self.recent[side].abs() > self.recent[axis].abs() * 2.0 };
        if turning {
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
    fn row_commit_accepts_a_right_turn_after_one_small_dead_zone() {
        let mut lock = Lock { f_mode: true, ..Lock::default() };
        let mut picture = crate::follow::Follow::new((0, 2), vec![(0, 2); 3]);
        lock.travel([0.0, 12.0], [220.0; 2], 12.0, 0);
        lock.travel([0.0, 220.0], [220.0; 2], 12.0, 10);
        picture.track_locked(0.01, (lock.at[0], lock.at[1]), 1);
        lock.travel([12.0, 0.0], [220.0; 2], 12.0, 20);
        assert_eq!(lock.axis, Some(0));
        lock.travel([220.0, 0.0], [220.0; 2], 12.0, 30);
        assert_eq!(picture.finish_path(&[(lock.at[0], lock.at[1], 0)]), (1, 1));
    }
}
