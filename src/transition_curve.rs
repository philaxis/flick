//! How a desktop moves when the user goes to the next one: it recedes, the
//! strip of desktops slides, and the one arrived at comes forward again.
//!
//! Plain numbers with no Windows in them, shared by everything that shows
//! the move (the board's preview area, and a full-screen overlay). Times are
//! in seconds from any fixed instant the caller picks; positions are in
//! whatever unit the caller slides by (cells, or pixels).
//!
//! * A discrete move (a key or a click, one cell) takes `STEP` seconds. How
//!   far back the desktop is (its `depth`, 0 in front to 1 receded) rises
//!   and falls in a bell over the whole of it, while the position eases from
//!   `SLIDE_START` to `SLIDE_END` ([`step_at`]).
//! * A move made while one is under way carries on from where the strip is
//!   and how fast it goes, still receded ([`Glide::retarget`],
//!   [`Recede::step`]): nothing jumps and nothing stops in between.
//! * A hold recedes in `HOLD_RECEDE` seconds and the strip follows the hand
//!   one to one ([`Recede::hold`]); let go, it snaps to a cell in `SNAP`
//!   seconds (where to is [`predict`]ed from the hand's speed) and comes
//!   forward in `ARRIVE` ([`Recede::release`]).

/// How large a receded desktop is, of its full size.
pub const RECEDE_SCALE: f32 = 0.9;
/// A whole discrete move: receding, sliding and coming forward.
pub const STEP: f32 = 0.4;
/// When the slide of a discrete move begins and ends, from its start.
pub const SLIDE_START: f32 = 0.05;
pub const SLIDE_END: f32 = 0.35;
/// How long a discrete move takes to recede, and to come forward again.
pub const STEP_RECEDE: f32 = 0.15;
/// How long a hold takes to recede.
pub const HOLD_RECEDE: f32 = 0.12;
/// How long the strip takes to settle on a cell after the hand let go.
pub const SNAP: f32 = crate::axis::SETTLE_TIME as f32;
/// How long the desktop takes to come forward after a hold.
pub const ARRIVE: f32 = 0.15;
/// How far ahead the hand's speed is carried when it lets go, in seconds.
pub const PROJECTION: f32 = 0.12;

/// 0 to 1 with no speed at either end.
pub fn ease(u: f32) -> f32 {
    let u = u.clamp(0.0, 1.0);
    u * u * (3.0 - 2.0 * u)
}

/// The `u` that `ease` takes to `value`.
fn unease(value: f32) -> f32 {
    if value <= 0.0 {
        return 0.0;
    }
    if value >= 1.0 {
        return 1.0;
    }
    let (mut low, mut high) = (0.0, 1.0);
    for _ in 0..24 {
        let middle = (low + high) / 2.0;
        if ease(middle) < value { low = middle } else { high = middle }
    }
    (low + high) / 2.0
}

/// The size of a desktop `depth` receded.
pub fn scale(depth: f32) -> f32 {
    1.0 - (1.0 - RECEDE_SCALE) * depth.clamp(0.0, 1.0)
}

/// A single discrete move `t` seconds in: its scale, and how much of the way
/// it has slid (0 to 1).
pub fn step_at(t: f32) -> (f32, f32) {
    let mut recede = Recede::default();
    recede.step(0.0, SLIDE_END);
    (scale(recede.depth(t)), 1.0 - Glide::start(1.0, 0.0, false).at(t))
}

/// Where a strip let go of at `position`, moving at `velocity` (per second),
/// would come to: the caller rounds it to a cell and keeps it in bounds.
pub fn predict(position: f32, velocity: f32) -> f32 {
    position + velocity * PROJECTION
}

/// How far one thing still is from where it is going, along one axis, and
/// how that shrinks to nothing: a cubic from its distance and speed at
/// `start` to rest after `duration`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glide {
    from: f32,
    velocity: f32,
    start: f32,
    duration: f32,
}

impl Default for Glide {
    fn default() -> Self {
        Glide { from: 0.0, velocity: 0.0, start: 0.0, duration: SLIDE_END - SLIDE_START }
    }
}

impl Glide {
    /// `distance` away at `now`, at rest: it starts moving `SLIDE_START`
    /// later, as a discrete move does, unless `at_once`.
    pub fn start(distance: f32, now: f32, at_once: bool) -> Glide {
        let start = if at_once { now } else { now + SLIDE_START };
        Glide { from: distance, velocity: 0.0, start, duration: SLIDE_END - SLIDE_START }
    }

    /// What is left at `t`.
    pub fn at(&self, t: f32) -> f32 {
        let s = (t - self.start) / self.duration;
        if s <= 0.0 {
            return self.from + self.velocity * (t - self.start);
        }
        if s >= 1.0 - 1e-6 {
            return 0.0;
        }
        if self.from.abs() < 1e-6 { return 0.0; }
        self.from * (1.0 - crate::axis::settle_with_speed(s as f64, (-self.velocity * self.duration / self.from) as f64) as f32)
    }

    /// How fast what is left changes at `t`, per second.
    pub fn velocity(&self, t: f32) -> f32 {
        let s = (t - self.start) / self.duration;
        if s <= 0.0 {
            return self.velocity;
        }
        if s >= 1.0 - 1e-6 {
            return 0.0;
        }
        if self.from.abs() < 1e-6 { return 0.0; }
        -self.from / self.duration * crate::axis::settle_speed(s as f64, (-self.velocity * self.duration / self.from) as f64) as f32
    }

    /// When it is done.
    pub fn end(&self) -> f32 {
        self.start + self.duration
    }

    pub fn settled(&self, t: f32) -> bool {
        t >= self.end()
    }

    /// The target moved `jump` further away at `now` (or what is left was
    /// given to it at once, by a hand that let go): it carries on from where
    /// it is and as fast as it goes, and arrives `duration` later.
    pub fn retarget(&mut self, jump: f32, now: f32, duration: f32) {
        let (left, velocity) = (self.at(now), self.velocity(now));
        let moving = !self.settled(now) && (left != 0.0 || velocity != 0.0);
        *self = if moving || velocity != 0.0 {
            Glide { from: left + jump, velocity, start: now, duration }
        } else {
            Glide { duration, ..Glide::start(left + jump, now, false) }
        };
    }

    /// Like `retarget`, from a hand that was moving at `velocity` and has
    /// just let go `distance` away: it starts at once.
    pub fn release(&mut self, distance: f32, velocity: f32, now: f32) {
        let left = self.at(now);
        *self = Glide { from: left + distance, velocity: self.velocity(now) + velocity, start: now, duration: SNAP };
    }
}

/// How far back the desktop is: rising from `rise_from` over `rise`
/// seconds, falling over `fall` seconds to be in front at `fall_to`; the
/// lower of the two.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Recede {
    rise_from: f32,
    rise: f32,
    fall_to: f32,
    fall: f32,
}

impl Default for Recede {
    /// In front.
    fn default() -> Self {
        Recede { rise_from: f32::INFINITY, rise: STEP_RECEDE, fall_to: f32::NEG_INFINITY, fall: STEP_RECEDE }
    }
}

impl Recede {
    pub fn depth(&self, t: f32) -> f32 {
        let rising = ease((t - self.rise_from) / self.rise);
        let falling = ease((self.fall_to - t) / self.fall);
        rising.min(falling)
    }

    /// In front at `t` and from then on.
    pub fn settled(&self, t: f32) -> bool {
        t >= self.fall_to || t <= self.rise_from
    }

    /// Recedes from how far back it is at `now`, never jumping: the rise
    /// goes on as it was, or starts over from there over `rise` seconds.
    fn rise_from(&mut self, now: f32, rise: f32) {
        let depth = self.depth(now);
        let rising = ease((now - self.rise_from) / self.rise) <= ease((self.fall_to - now) / self.fall);
        if !(rising && depth > 0.0 && self.rise == rise) {
            self.rise_from = now - unease(depth) * rise;
            self.rise = rise;
        }
    }

    /// A discrete move whose slide ends at `slide_end`: back as soon as it
    /// can be and in front again `STEP - SLIDE_END` after the slide.
    pub fn step(&mut self, now: f32, slide_end: f32) {
        self.rise_from(now, STEP_RECEDE);
        self.fall_to = slide_end + (STEP - SLIDE_END);
        self.fall = STEP_RECEDE;
    }

    /// The hand took hold: back in `HOLD_RECEDE` and staying back.
    pub fn hold(&mut self, now: f32) {
        self.rise_from(now, HOLD_RECEDE);
        self.fall_to = f32::INFINITY;
        self.fall = ARRIVE;
    }

    /// The hand let go and the strip settles at `snap_end`: it comes forward
    /// in `ARRIVE` seconds, setting off as the snap ends.
    pub fn release(&mut self, now: f32, snap_end: f32) {
        // Never further forward at once than it is.
        let depth = self.depth(now);
        self.fall = ARRIVE;
        self.fall_to = (snap_end + ARRIVE * 0.5).max(now + unease(depth) * ARRIVE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: f32 = 1.0 / 240.0;

    fn times(until: f32) -> impl Iterator<Item = f32> {
        (0..=(until / FRAME).ceil() as usize).map(|i| i as f32 * FRAME)
    }

    /// A discrete move starts and ends full size where it was and where it
    /// went, shrinks to no less than the receded size, and only ever slides
    /// forward, inside its window of time.
    #[test]
    fn a_step_starts_and_ends_in_front_and_slides_one_way() {
        assert_eq!(step_at(0.0), (1.0, 0.0));
        assert_eq!(step_at(STEP), (1.0, 1.0));
        assert_eq!(step_at(SLIDE_START).1, 0.0);
        assert_eq!(step_at(SLIDE_END).1, 1.0);
        let (mut last, mut smallest) = (0.0, 1.0f32);
        for t in times(STEP) {
            let (size, slid) = step_at(t);
            assert!((RECEDE_SCALE - 1e-6..=1.0).contains(&size), "{t}: {size}");
            assert!((-1e-6..=1.06).contains(&slid), "{t}: {slid} after {last}");
            last = slid;
            smallest = smallest.min(size);
        }
        assert!((smallest - RECEDE_SCALE).abs() < 1e-4, "{smallest}");
        // Back before it slides much, in front only once it is there.
        assert!(step_at(SLIDE_START + 0.05).0 < 0.95 && step_at(SLIDE_START + 0.05).1 < 0.7);
        assert!((step_at(SLIDE_END - 0.1).0 - RECEDE_SCALE).abs() < 1e-4);
        assert!((step_at(STEP / 2.0).0 - RECEDE_SCALE).abs() < 1e-4);
    }

    /// The speed of the slide never jumps, nor does the size.
    #[test]
    fn a_step_has_no_jumps_in_place_speed_or_size() {
        let glide = Glide::start(1.0, 0.0, false);
        for t in times(STEP) {
            let (a, b) = (glide.at(t), glide.at(t + FRAME));
            assert!((a - b).abs() <= glide.velocity(t).abs().max(glide.velocity(t + FRAME).abs()) * FRAME * 1.5 + 1e-5, "{t}");
            assert!((glide.velocity(t) - glide.velocity(t + FRAME)).abs() < 5.0, "{t}");
            assert!((step_at(t).0 - step_at(t + FRAME).0).abs() <= 1.5 * (1.0 - RECEDE_SCALE) / STEP_RECEDE * FRAME * 1.01, "{t}");
        }
    }

    /// Moves made one after another while the first is under way: the strip
    /// goes on from where it is at the speed it has, stays back, and ends
    /// at the last cell asked for, in front.
    #[test]
    fn moves_in_a_row_carry_on_without_a_jump_and_stay_back() {
        for gap in [0.06, 0.12, 0.2, 0.3, 0.38] {
            // Positions in cells; the target is the last move's cell.
            let (mut glide, mut recede, mut target) = (Glide::start(1.0, 0.0, false), Recede::default(), 1.0);
            recede.step(0.0, glide.end());
            let mut previous: Option<(f32, f32, f32)> = None;
            let mut seconds = 0.0;
            for t in times(1.5) {
                if (t - gap).abs() < FRAME / 2.0 || (t - 2.0 * gap).abs() < FRAME / 2.0 {
                    glide.retarget(1.0, t, SLIDE_END - SLIDE_START);
                    recede.step(t, glide.end());
                    target += 1.0;
                }
                let at = target - glide.at(t);
                let (velocity, depth) = (-glide.velocity(t), recede.depth(t));
                if let Some((was, speed, size)) = previous {
                    assert!((at - was).abs() <= speed.abs().max(velocity.abs()) * FRAME * 1.5 + 1e-4, "gap {gap} at {t}: {was} to {at}");
                    assert!((velocity - speed).abs() < 10.0, "gap {gap} at {t}: speed {speed} to {velocity}");
                    assert!((depth - size).abs() < 0.05, "gap {gap} at {t}: depth {size} to {depth}");
                }
                assert!(at <= target + 0.15, "gap {gap} at {t}: past the end {at}");
                // Back the whole time moves come in quick succession.
                if gap <= 0.2 && t > STEP_RECEDE && t < 2.0 * gap + SLIDE_START {
                    assert!(depth > 0.99, "gap {gap} at {t}: came forward between moves ({depth})");
                }
                previous = Some((at, velocity, depth));
                seconds = t;
            }
            assert_eq!(target - glide.at(seconds), 3.0);
            assert_eq!(recede.depth(seconds), 0.0);
        }
    }

    /// A hold recedes in its time and stays back however long; let go, the
    /// strip settles where it was sent and comes forward, with no jump.
    #[test]
    fn a_hold_recedes_follows_and_comes_forward_after_the_snap() {
        let mut recede = Recede::default();
        recede.hold(1.0);
        assert_eq!(recede.depth(1.0), 0.0);
        assert!((recede.depth(1.0 + HOLD_RECEDE) - 1.0).abs() < 1e-5);
        assert_eq!(recede.depth(30.0), 1.0);
        // Let go at 0.4 of a cell, moving on at 3 cells a second: it goes to the next cell.
        let (position, speed) = (0.4, 3.0);
        assert_eq!(predict(position, speed).round(), 1.0);
        let mut glide = Glide::default();
        glide.release(position - 1.0, speed, 2.0);
        recede.release(2.0, glide.end());
        assert_eq!(glide.at(2.0), position - 1.0);
        assert_eq!(-glide.velocity(2.0), -speed);
        assert_eq!(recede.depth(2.0), 1.0);
        assert!(recede.depth(glide.end() - ARRIVE * 0.5 - 1e-3) > 0.99);
        assert_eq!(recede.depth(glide.end() + ARRIVE), 0.0);
        assert_eq!(glide.at(glide.end()), 0.0);
        let mut last = recede.depth(2.0);
        for t in times(0.6).map(|t| 2.0 + t) {
            assert!(recede.depth(t) <= last + 1e-6);
            last = recede.depth(t);
        }
        // A hold let go before it was all the way back falls from where it was.
        let mut recede = Recede::default();
        recede.hold(0.0);
        let half = recede.depth(HOLD_RECEDE / 2.0);
        recede.release(HOLD_RECEDE / 2.0, HOLD_RECEDE / 2.0);
        assert!((recede.depth(HOLD_RECEDE / 2.0) - half).abs() < 1e-4);
        // Let go all the way back with nothing left to snap: no jump forward either.
        let mut recede = Recede::default();
        recede.hold(0.0);
        recede.release(1.0, 1.0);
        assert_eq!(recede.depth(1.0), 1.0);
        assert!(recede.depth(1.0 + FRAME) > 0.98);
    }

    /// A move that comes as the last one is coming forward goes back from
    /// there rather than jumping back.
    #[test]
    fn a_move_while_coming_forward_goes_back_smoothly() {
        let mut recede = Recede::default();
        recede.step(0.0, SLIDE_END);
        let now = STEP - 0.05;
        let before = recede.depth(now);
        recede.step(now, now + SLIDE_END - SLIDE_START);
        assert!((recede.depth(now) - before).abs() < 1e-4);
        assert!(recede.depth(now + STEP_RECEDE) > 0.99);
        for t in times(STEP) {
            assert!((0.0..=1.0).contains(&recede.depth(now + t)));
            assert!((RECEDE_SCALE..=1.0).contains(&scale(recede.depth(now + t))));
        }
    }
}
