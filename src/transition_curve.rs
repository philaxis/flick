//! Variant F: a short elastic pull commits a cell; damped springs carry the picture.
pub const COMMIT: f64 = 1.0;
pub const EDGE: f64 = 0.15;
pub const PULL: f64 = 0.2;
pub const SLIDE_START: f32 = 0.0;
pub const SLIDE_END: f32 = 0.65;
pub const SNAP: f32 = SLIDE_END;

pub fn scale(depth: f32) -> f32 { 1.0 - 0.3 * depth.clamp(0.0, 1.0) }

/// Exact spring integration keeps the same curve when a frame is late.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Spring { pub p: f64, pub v: f64 }
impl Spring {
    pub fn advance(&mut self, target: f64, dt: f64, omega: f64, zeta: f64) {
        let t = dt.max(0.0);
        let x = self.p - target;
        let decay = (-zeta * omega * t).exp();
        let (p, v) = if zeta >= 1.0 {
            let b = self.v + omega * x;
            (decay * (x + b * t), decay * (self.v - omega * b * t))
        } else {
            let w = omega * (1.0 - zeta * zeta).sqrt();
            let (s, c) = (w * t).sin_cos();
            (decay * (x * c + (self.v + zeta * omega * x) / w * s),
             decay * (self.v * c - (zeta * omega * self.v + omega * omega * x) / w * s))
        };
        self.p = target + p;
        self.v = v;
    }
    pub fn settled(&self, target: f64) -> bool { (self.p - target).abs() < 0.0005 && self.v.abs() < 0.005 }
    pub fn position(&mut self, target: f64, dt: f64, vertical: bool) {
        let (w, z) = if vertical { (15.0, 0.65) } else { (17.0, 0.62) };
        self.advance(target, dt, w, z);
        if self.settled(target) { self.p = target; self.v = 0.0; }
    }
}

/// Distances are normalized by the configured Distance: one full unit commits.
#[derive(Clone, Copy, Debug, Default)]
pub struct Latch { pub cell: i32, pub hand: f64, last: f64 }
impl Latch {
    pub fn track(&mut self, raw: f64, room: (i32, i32)) -> i32 {
        let delta = raw - self.last;
        self.last = raw;
        // Turning inward at an edge starts there, without paying back overscroll.
        if (self.cell == room.0 && self.hand < 0.0 && delta > 0.0)
            || (self.cell == room.1 && self.hand > 0.0 && delta < 0.0) { self.hand = 0.0; }
        self.hand += delta;
        let before = self.cell;
        while self.hand.abs() >= COMMIT - 1e-9 {
            let dir = self.hand.signum() as i32;
            let next = (self.cell + dir).clamp(room.0, room.1);
            if next == self.cell { break; }
            self.cell = next;
            self.hand -= dir as f64;
        }
        self.cell - before
    }
    pub fn rebase(&mut self, raw: f64) { self.last = raw; self.hand = 0.0; }
    pub fn target(&self, room: (i32, i32)) -> f64 {
        let pull = if (self.cell == room.0 && self.hand < 0.0) || (self.cell == room.1 && self.hand > 0.0) {
            self.hand.signum() * EDGE * self.hand.abs() / (self.hand.abs() + EDGE)
        } else { PULL * (self.hand / PULL).tanh() };
        self.cell as f64 + pull
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Glide { state: Spring, start: f32, pub vertical: bool }
impl Glide {
    pub fn start(distance: f32, now: f32, _at_once: bool) -> Self {
        Self { state: Spring { p: distance as f64, v: 0.0 }, start: now, vertical: false }
    }
    fn sample(&self, t: f32) -> Spring {
        let mut state = self.state;
        state.position(0.0, (t - self.start).max(0.0) as f64, self.vertical);
        state
    }
    pub fn at(&self, t: f32) -> f32 { self.sample(t).p as f32 }
    pub fn velocity(&self, t: f32) -> f32 { self.sample(t).v as f32 }
    pub fn end(&self) -> f32 { self.start + SLIDE_END }
    pub fn settled(&self, t: f32) -> bool { self.sample(t).settled(0.0) }
    pub fn retarget(&mut self, jump: f32, now: f32, _duration: f32) {
        self.state = self.sample(now);
        self.state.p += jump as f64;
        self.start = now;
    }
    pub fn release(&mut self, distance: f32, velocity: f32, now: f32) {
        self.retarget(distance, now, SNAP);
        self.state.v += velocity as f64;
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Recede { state: Spring, start: f32, fall: f32 }
impl Default for Recede {
    fn default() -> Self { Self { state: Spring::default(), start: 0.0, fall: 0.0 } }
}
impl Recede {
    fn sample(&self, t: f32) -> Spring {
        let mut state = self.state;
        let rising = t.min(self.fall).max(self.start) - self.start;
        if rising > 0.0 { state.advance(1.0, rising as f64, 22.0, 1.0); }
        if t > self.fall { state.advance(0.0, (t - self.fall.max(self.start)) as f64, 14.0, 1.0); }
        state
    }
    pub fn depth(&self, t: f32) -> f32 { self.sample(t).p.clamp(0.0, 1.0) as f32 }
    pub fn settled(&self, t: f32) -> bool { t >= self.fall && self.sample(t).settled(0.0) }
    pub fn kick(&mut self, now: f32, velocity: f64) {
        self.state = self.sample(now); self.start = now; self.state.v += velocity;
    }
    pub fn moving(&mut self, now: f32, moving: bool) {
        self.state.advance(if moving { 1.0 } else { 0.0 }, (now - self.start).max(0.0) as f64, if moving { 22.0 } else { 14.0 }, 1.0);
        self.start = now;
        self.fall = if moving { f32::INFINITY } else { now };
    }
    pub fn step(&mut self, now: f32, end: f32) { self.kick(now, 6.0); self.fall = end; }
    pub fn hold(&mut self, now: f32) { self.kick(now, 0.0); self.fall = f32::INFINITY; }
    pub fn release(&mut self, now: f32, end: f32) { self.kick(now, 0.0); self.fall = end.max(now); }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn distance_is_the_full_commit_threshold_and_edges_release_inward() {
        let mut l = Latch::default();
        assert_eq!(l.track(0.99, (0, 4)), 0);
        assert_eq!(l.track(1.0, (0, 4)), 1);
        assert_eq!(l.track(1.01, (0, 4)), 0);
        assert_eq!(l.track(2.0, (0, 4)), 1);
        assert_eq!(l.track(1.99, (0, 4)), 0);
        assert_eq!(l.track(1.0, (0, 4)), -1);
        assert!((l.target((0, 4)) - l.cell as f64).abs() <= PULL);
        l.track(90.0, (0, 4));
        assert!(l.target((0, 4)) < 4.15);
        assert_eq!(l.track(89.0, (0, 4)), -1);
    }
    #[test]
    fn exact_spring_preserves_speed_and_is_independent_of_frame_rate() {
        for vertical in [false, true] {
            let mut one = Spring::default(); let mut many = one;
            one.position(1.0, 0.3, vertical);
            for _ in 0..30 { many.position(1.0, 0.01, vertical); }
            assert!((one.p - many.p).abs() < 1e-9 && (one.v - many.v).abs() < 1e-9);
            assert!(one.p > 1.0 && one.p < 1.09);
            let v = one.v; one.position(2.0, 0.0, vertical); assert!((one.v - v).abs() < 1e-9);
        }
    }
    #[test]
    fn depth_kicks_do_not_jump_or_shrink_below_seventy_percent() {
        let mut d = Recede::default(); d.step(0.0, 0.5);
        let before = d.depth(0.1); d.kick(0.1, 5.0);
        assert!((d.depth(0.1) - before).abs() < 1e-6);
        for i in 0..200 { assert!((0.7..=1.0).contains(&scale(d.depth(i as f32 / 100.0)))); }
        assert!(d.settled(2.0));
    }
}
