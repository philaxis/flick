//! Variant F: a short elastic pull commits a cell; damped springs carry the picture.
pub const COMMIT: f64 = 1.0;
pub const EDGE: f64 = 0.15;
pub const PULL: f64 = 0.2;
pub const HOLD: f64 = 0.25;
pub const SLIDE_START: f32 = 0.0;
pub const SLIDE_END: f32 = 0.65;
pub const SNAP: f32 = SLIDE_END;

pub fn scale(depth: f32) -> f32 { 1.0 - 0.3 * depth.clamp(0.0, 1.0) }

static COMMIT_SPEED: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
pub fn configure(speed: crate::config::CommitSpeed) {
    COMMIT_SPEED.store(speed as u8, std::sync::atomic::Ordering::Relaxed);
}
fn commit_omega(vertical: bool) -> f64 {
    use crate::config::CommitSpeed;
    let speed = match COMMIT_SPEED.load(std::sync::atomic::Ordering::Relaxed) {1=>CommitSpeed::Gentle,2=>CommitSpeed::Slow,_=>CommitSpeed::Default};
    speed.omega(vertical)
}

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
    pub fn commit(&mut self, target: f64, dt: f64, vertical: bool) {
        self.advance(target, dt, commit_omega(vertical), 0.78);
        if self.settled(target) { self.p = target; self.v = 0.0; }
    }
    pub fn position(&mut self, target: f64, dt: f64, vertical: bool) {
        let (w, z) = if vertical { (32.0, 0.78) } else { (35.0, 0.78) };
        self.advance(target, dt, w, z);
        if self.settled(target) { self.p = target; self.v = 0.0; }
    }
}

/// How fast a picture recedes as it moves, and comes forward again.
pub const RECEDE_RATE: f64 = 24.0;
pub const RETURN_RATE: f64 = 22.0;

/// The depth (0 whole, 1 receded) going toward receded (`deep`) or whole
/// for `dt`. A spring setting off from rest lingers near where it was; this
/// one leaves at least as fast as an exponential approach (95% receded in
/// 0.125 s, whole again in 0.136 s) and is exact from there, so a late
/// frame keeps the same curve.
pub fn deepen(depth: &mut Spring, deep: bool, dt: f64) {
    launch(depth, deep);
    let (target, rate) = if deep { (1.0, RECEDE_RATE) } else { (0.0, RETURN_RATE) };
    depth.advance(target, dt, rate, 1.0);
}

/// At least an exponential approach's speed; one already faster keeps it,
/// and stays faster as it goes, so this changes nothing a second time.
fn launch(depth: &mut Spring, deep: bool) {
    depth.v = if deep { depth.v.max(RECEDE_RATE * (1.0 - depth.p)) } else { depth.v.min(-RETURN_RATE * depth.p) };
}

/// The speed of `depth` kicked `by` deeper, on top of how it leaves (`deepen`).
pub fn kicked(depth: Spring, by: f64) -> f64 {
    let mut depth = depth;
    launch(&mut depth, true);
    depth.v + by
}

/// How far past the last cell the picture goes for a hand `over` (cells)
/// past it: heavier the farther, never `EDGE`.
pub fn give(over: f64) -> f64 { over.signum() * EDGE * over.abs() / (over.abs() + EDGE) }

/// Distances are normalized by the configured Distance: one full unit commits.
#[derive(Clone, Copy, Debug, Default)]
pub struct Latch { pub cell: i32, pub hand: f64, last: f64, held: bool, hold_origin: f64 }
impl Latch {
    pub fn track(&mut self, raw: f64, room: (i32, i32)) -> i32 {
        let delta = raw - self.last;
        self.last = raw;
        // Turning inward at an edge starts there, without paying back overscroll.
        if (self.cell == room.0 && self.hand < 0.0 && delta > 0.0)
            || (self.cell == room.1 && self.hand > 0.0 && delta < 0.0) { self.hand = 0.0; self.hold_origin = 0.0; }
        self.hand += delta;
        let before = self.cell;
        while self.hand.abs() >= COMMIT - 1e-9 {
            let dir = self.hand.signum() as i32;
            let next = (self.cell + dir).clamp(room.0, room.1);
            if next == self.cell { break; }
            self.cell = next;
            self.hand -= dir as f64;
        }
        if self.cell != before { self.held = true; self.hold_origin = self.hand; }
        self.cell - before
    }
    pub fn rebase(&mut self, raw: f64) { self.last = raw; self.hand = 0.0; self.held = true; self.hold_origin = 0.0; }
    pub fn target(&self, room: (i32, i32)) -> f64 {
        let hand = self.hand - self.hold_origin;
        let hand = if self.held { hand.signum() * (hand.abs() - HOLD).max(0.0) / (1.0 - HOLD) } else { hand };
        let pull = if (self.cell == room.0 && hand < 0.0) || (self.cell == room.1 && hand > 0.0) {
            give(hand)
        } else { PULL * (hand / PULL).tanh() };
        self.cell as f64 + pull
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Glide { state: Spring, start: f32, pub vertical: bool, commit: bool }
impl Glide {
    pub fn start(distance: f32, now: f32, _at_once: bool) -> Self {
        Self { state: Spring { p: distance as f64, v: 0.0 }, start: now, vertical: false, commit: true }
    }
    fn sample(&self, t: f32) -> Spring {
        let mut state = self.state;
        if self.commit { state.commit(0.0, (t-self.start).max(0.0) as f64,self.vertical); }
        else { state.position(0.0, (t-self.start).max(0.0) as f64,self.vertical); }
        state
    }
    pub fn at(&self, t: f32) -> f32 { self.sample(t).p as f32 }
    pub fn velocity(&self, t: f32) -> f32 { self.sample(t).v as f32 }
    pub fn end(&self) -> f32 { self.start + SLIDE_END }
    pub fn settled(&self, t: f32) -> bool { self.sample(t).settled(0.0) }
    pub fn retarget(&mut self, jump: f32, now: f32, _duration: f32) {
        self.state = self.sample(now);
        if self.state.settled(0.0) { self.commit = false; }
        self.state.p += jump as f64;
        self.start = now;
    }
    pub fn committed(&mut self) { self.commit = true; }
    pub fn following(&mut self) { self.commit = false; }
    pub fn release(&mut self, distance: f32, velocity: f32, now: f32) {
        self.retarget(distance, now, SNAP);
        self.state.v += velocity as f64;
        self.commit = true;
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
        if rising > 0.0 { deepen(&mut state, true, rising as f64); }
        if t > self.fall { deepen(&mut state, false, (t - self.fall.max(self.start)) as f64); }
        state
    }
    pub fn depth(&self, t: f32) -> f32 { self.sample(t).p.clamp(0.0, 1.0) as f32 }
    pub fn settled(&self, t: f32) -> bool { t >= self.fall && self.sample(t).settled(0.0) }
    pub fn kick(&mut self, now: f32, velocity: f64) {
        let now=now.max(self.start);
        self.state = self.sample(now); self.start = now; self.state.v = kicked(self.state, velocity);
    }
    pub fn moving(&mut self, now: f32, moving: bool) {
        let now=now.max(self.start);
        deepen(&mut self.state, moving, (now - self.start).max(0.0) as f64);
        self.start = now;
        self.fall = if moving { f32::INFINITY } else { now };
    }
    pub fn step(&mut self, now: f32, end: f32) { self.kick(now, 6.0); self.fall = end.max(self.start); }
    pub fn hold(&mut self, now: f32) { self.kick(now, 0.0); self.fall = f32::INFINITY; }
    pub fn release(&mut self, now: f32, end: f32) { self.kick(now, 0.0); self.fall = end.max(self.start); }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn optional_commit_speeds_reduce_the_60_hz_step_without_changing_the_target() {
        let mut peaks=Vec::new();
        for mode in [crate::config::CommitSpeed::Default,crate::config::CommitSpeed::Gentle,crate::config::CommitSpeed::Slow] {
            let mut s=Spring::default(); let mut peak=0f64;
            for _ in 0..120 { let old=s.p; s.advance(1.0,1.0/60.0,mode.omega(false),0.78);peak=peak.max((s.p-old).abs()); }
            assert!(s.settled(1.0));peaks.push(peak);
        }
        assert!((peaks[0]-0.240406).abs()<0.000001);
        assert!(peaks[0]>peaks[1] && peaks[1]>peaks[2]);
    }
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
            assert!((one.p - 1.0).abs() < 0.003);
            let v = one.v; one.position(2.0, 0.0, vertical); assert!((one.v - v).abs() < 1e-9);
        }
    }
    #[test]
    fn committed_hold_is_flat_continuous_and_keeps_distance() {
        for direction in [-1.0, 1.0] {
            let mut l = Latch::default();
            l.track(direction, (-4, 4));
            for by in [0.01, 0.1, 0.249999, 0.25, -0.1, -0.25] {
                l.track(direction + by, (-4, 4));
                assert!((l.target((-4, 4)) - direction).abs() < 1e-9);
            }
            l.track(direction + 0.250001, (-4, 4));
            assert!((l.target((-4, 4)) - direction).abs() < 0.000002);
            assert_eq!(l.track(direction * 1.999, (-4, 4)), 0);
            assert_eq!(l.track(direction * 2.0, (-4, 4)), direction as i32);
        }
    }
    #[test]
    fn a_commit_at_the_last_cell_also_holds_before_its_existing_edge_curve() {
        let mut l = Latch::default();
        l.track(1.0, (0, 1));
        l.track(1.2, (0, 1));
        assert!((l.target((0, 1)) - 1.0).abs() < 1e-9);
        l.track(1.5, (0, 1));
        assert!(l.target((0, 1)) > 1.0 && l.target((0, 1)) < 1.15);
        assert_eq!(l.track(0.5, (0, 1)), -1);
        let mut edge = Latch::default();
        edge.track(-0.1, (0, 1));
        assert!((edge.target((0, 1)) + EDGE * 0.1 / (0.1 + EDGE)).abs() < 1e-9);
    }
    #[test]
    fn a_coalesced_commit_holds_from_the_sample_without_adding_to_distance() {
        let mut l = Latch::default();
        assert_eq!(l.track(1.4, (-4, 4)), 1);
        for raw in [1.4, 1.5, 1.2, 1.65] {
            l.track(raw, (-4, 4));
            assert!((l.target((-4, 4)) - 1.0).abs() < 1e-9);
        }
        assert_eq!(l.track(2.0, (-4, 4)), 1);
        assert!((l.target((-4, 4)) - 2.0).abs() < 1e-9);
    }
    #[test]
    fn commit_overshoot_stays_below_three_percent_and_settles_in_two_hundred_ms() {
        for vertical in [false, true] {
            let mut spring = Spring::default();
            let mut peak = 0.0_f64;
            for frame in 1..=1000 {
                spring.position(1.0, 0.001, vertical);
                peak = peak.max(spring.p);
                if frame >= 200 { assert!((spring.p - 1.0).abs() <= 0.03); }
            }
            assert!(peak > 1.0 && peak <= 1.03, "{peak}");
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
