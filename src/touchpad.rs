//! Three-finger gestures in centimetres; Windows report decoding lives separately.

use std::sync::atomic::{AtomicU32, Ordering};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Contact {
    pub id: u32,
    pub confident: bool,
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Default, PartialEq)]
pub struct Event {
    pub began: bool,
    pub delta: (f64, f64),
    pub ended: Option<bool>, // whether the release is a tap
    /// Cells and rows the hold travelled, in order (`Rails`).
    pub steps: Vec<Dir>,
    /// How far towards the next step each way, in steps (`Rails::progress`).
    pub progress: (f64, f64),
}

#[derive(Default)]
pub struct Gesture {
    previous: Vec<Contact>,
    landed: Option<u32>,
    held: bool,
    blocked: bool,
    path: f64,
    rails: Rails,
}

impl Gesture {
    /// Cancellation must not start another hold until the hand leaves the pad.
    pub fn cancel(&mut self) {
        self.held = false;
        self.blocked = true;
    }

    pub fn update(&mut self, contacts: &[Contact], ms: u32) -> Event {
        let mut event = Event::default();
        if contacts.is_empty() {
            if self.held {
                event.ended = Some(
                    self.landed
                        .is_some_and(|start| ms.wrapping_sub(start) <= 250)
                        && self.path < 0.2,
                );
            }
            *self = Self::default();
            return event;
        }
        self.landed.get_or_insert(ms);
        // Match identities, not report slots. A different set starts a new baseline.
        let same = contacts.len() == self.previous.len()
            && contacts
                .iter()
                .all(|c| self.previous.iter().any(|p| p.id == c.id));
        let mut delta = (0.0, 0.0);
        let mut farthest: f64 = 0.0;
        for contact in contacts {
            if let Some(old) = self.previous.iter().find(|p| p.id == contact.id) {
                let (dx, dy) = (contact.x - old.x, contact.y - old.y);
                farthest = farthest.max(dx.hypot(dy));
                if same {
                    delta.0 += dx / contacts.len() as f64;
                    delta.1 += dy / contacts.len() as f64;
                }
            }
        }
        // Re-basing navigation must not hide travel from tap classification.
        self.path += farthest;
        if !self.held
            && !self.blocked
            && contacts.len() == 3
            && contacts.iter().all(|c| c.confident)
        {
            self.held = true;
            self.rails = Rails::new(step_mm());
            event.began = true;
        } else if self.held {
            event.delta = delta;
            event.steps = self.rails.travel((delta.0 * 10.0, delta.1 * 10.0), ms);
            event.progress = self.rails.progress();
        }
        self.previous = contacts.to_vec();
        event
    }
}

/// One cell or row of a touchpad hold. Screen directions: down is +y on the pad.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

/// Pad travel for one step either way, in mm (the 1 cm a step always was), unless set otherwise.
pub const STEP_MM: f64 = 10.0;
/// Pad travel for one step sideways and up and down as set (`Config::touchpad`), in hundredths of
/// a mm; none set is `STEP_MM`.
static STEP_SET: [AtomicU32; 2] = [AtomicU32::new(0), AtomicU32::new(0)];

/// Sets the pad travel for one step sideways and up and down, from the next hold on.
pub fn set_step_mm(x: f64, y: f64) {
    for (set, mm) in STEP_SET.iter().zip([x, y]) {
        set.store((mm.max(1.0) * 100.0).round() as u32, Ordering::Relaxed);
    }
}

fn step_mm() -> [f64; 2] {
    [0, 1].map(|axis| match STEP_SET[axis].load(Ordering::Relaxed) {
        0 => STEP_MM,
        hundredths => hundredths as f64 / 100.0,
    })
}
/// Travel from where the hold began before its way is judged: well under a step, so none is lost.
pub const JUDGE_MM: f64 = 3.0;
// Moving along a row is easy and changing row is deliberate: an unclear start goes across, and
// leaving a rail for up and down takes longer and straighter travel than leaving it for across.
/// A start at most this steep (degrees from across, about 1.1 across for 1 up or down) goes across.
pub const X_RAIL_MAX_DEG: f64 = 42.0;
/// A start this much more up and down than across (about 63° steep) goes up and down.
pub const Y_RAIL_RATIO: f64 = 2.0;
/// Starts between `X_RAIL_MAX_DEG` and this are meant slantwise and both ways count; steeper
/// ones short of `Y_RAIL_RATIO` go across.
pub const SLANT_MAX_DEG: f64 = 55.0;
/// Width of the channel round a rail across: up and down drift within it is a crooked finger path.
/// It is forgotten after every channel of straight travel, so slow drift never adds up.
pub const X_CHANNEL_MM: f64 = 9.0;
/// The same round a rail up and down: narrower, so turning across from it comes sooner.
pub const Y_CHANNEL_MM: f64 = 4.0;
/// Out of the channel, the recent movement must be within this of up and down to switch to it...
pub const Y_SWITCH_DEG: f64 = 20.0;
/// ...and within this of across to switch to that.
pub const X_SWITCH_DEG: f64 = 35.0;
/// Going slantwise, the recent movement held within this of across...
pub const X_RELOCK_DEG: f64 = 20.0;
/// ...or of up and down...
pub const Y_RELOCK_DEG: f64 = 12.0;
/// ...for this long locks onto that axis again.
pub const RELOCK_MS: u32 = 150;
/// Recent movement fades by this per ms (libinput's 0.97^ms: about a 100 ms window).
pub const DECAY_PER_MS: f64 = 0.97;
/// A gap longer than this between reports forgets the recent movement.
pub const RECENT_TIMEOUT_MS: u32 = 100;
/// Recent movement shorter than this has no direction to speak of.
pub const RECENT_MIN_MM: f64 = 0.5;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Mode {
    /// Not yet `JUDGE_MM` from where the hold began.
    #[default]
    Judging,
    /// Locked to one axis: 0 sideways, 1 up and down.
    Rail(usize),
    /// Both ways count on their own.
    Free,
}

/// Turns a touchpad hold's travel (pad mm, no acceleration) into steps, keeping a hand that means
/// to go straight on one axis however crooked its fingers go. After Chromium's snap scroll
/// channel and libinput's fading direction vector for scroll axis locking.
///
/// Each axis gathers travel and steps once it reaches a whole step (`STEP_MM` unless set), keeping
/// what goes past.
/// Turning back has to gather a whole step from nothing again, so a hand resting on the edge of a
/// step does not take it to and fro.
#[derive(Debug)]
pub struct Rails {
    /// Travel for one step on each axis (x, y), in mm.
    step: [f64; 2],
    mode: Mode,
    /// Travel gathered towards the next step on each axis (x, y).
    gathered: [f64; 2],
    /// While on a rail: sideways travel since the channel last forgot it, and straight travel since.
    off: f64,
    along: f64,
    /// Recent movement, fading (`DECAY_PER_MS`).
    recent: [f64; 2],
    last_ms: Option<u32>,
    /// Going slantwise: the axis recent movement keeps close to, and since when.
    near: Option<(usize, u32)>,
}

impl Default for Rails {
    fn default() -> Self {
        Rails::new([STEP_MM; 2])
    }
}

impl Rails {
    /// `step`: travel for one step sideways and up and down, in mm.
    pub fn new(step: [f64; 2]) -> Rails {
        Rails {
            step,
            mode: Mode::default(),
            gathered: [0.0; 2],
            off: 0.0,
            along: 0.0,
            recent: [0.0; 2],
            last_ms: None,
            near: None,
        }
    }

    /// How far towards the next step each way, in steps; nothing on the axis a rail ignores.
    pub fn progress(&self) -> (f64, f64) {
        let [x, y] = [self.gathered[0] / self.step[0], self.gathered[1] / self.step[1]];
        match self.mode {
            Mode::Judging => (0.0, 0.0),
            Mode::Rail(0) => (x, 0.0),
            Mode::Rail(_) => (0.0, y),
            Mode::Free => (x, y),
        }
    }

    /// The hand moved `delta` mm (down positive) at `ms`; the steps that came of it.
    pub fn travel(&mut self, delta: (f64, f64), ms: u32) -> Vec<Dir> {
        let d = [delta.0, delta.1];
        let fade = match self.last_ms.map(|last| ms.wrapping_sub(last)) {
            Some(dt) if dt <= RECENT_TIMEOUT_MS => DECAY_PER_MS.powi(dt as i32),
            _ => 0.0,
        };
        self.last_ms = Some(ms);
        self.recent = [self.recent[0] * fade + d[0], self.recent[1] * fade + d[1]];
        let mut steps = Vec::new();
        match self.mode {
            Mode::Judging => {
                self.gathered = [self.gathered[0] + d[0], self.gathered[1] + d[1]];
                let [x, y] = self.gathered.map(f64::abs);
                if x.hypot(y) < JUDGE_MM {
                    return steps;
                }
                let steep = y.atan2(x).to_degrees();
                self.mode = if y >= x * Y_RAIL_RATIO {
                    Mode::Rail(1)
                } else if steep > X_RAIL_MAX_DEG && steep < SLANT_MAX_DEG {
                    Mode::Free
                } else {
                    Mode::Rail(0)
                };
                if let Mode::Rail(axis) = self.mode {
                    self.off = self.gathered[1 - axis];
                    self.gathered[1 - axis] = 0.0;
                    self.along = self.gathered[axis].abs();
                }
                for axis in 0..2 {
                    self.step(axis, 0.0, &mut steps);
                }
            }
            Mode::Rail(axis) => {
                let side = 1 - axis;
                self.off += d[side];
                let (channel, switch) = if axis == 0 { (X_CHANNEL_MM, Y_SWITCH_DEG) } else { (Y_CHANNEL_MM, X_SWITCH_DEG) };
                if self.off.abs() > channel && self.within(side, switch) {
                    // An L: the new way gathers from nothing, so the turn itself is no step.
                    self.mode = Mode::Rail(side);
                    self.gathered = [0.0; 2];
                    (self.off, self.along) = (0.0, 0.0);
                    return steps;
                }
                self.along += d[axis].abs();
                if self.along >= channel {
                    (self.off, self.along) = (0.0, 0.0);
                }
                self.step(axis, d[axis], &mut steps);
            }
            Mode::Free => {
                for axis in 0..2 {
                    self.step(axis, d[axis], &mut steps);
                }
                let near = (0..2).find(|&axis| self.within(axis, [X_RELOCK_DEG, Y_RELOCK_DEG][axis]));
                match (near, self.near) {
                    (Some(axis), Some((was, since))) if axis == was => {
                        if ms.wrapping_sub(since) >= RELOCK_MS {
                            self.mode = Mode::Rail(axis);
                            self.gathered[1 - axis] = 0.0;
                            (self.off, self.along) = (0.0, 0.0);
                            self.near = None;
                        }
                    }
                    (Some(axis), _) => self.near = Some((axis, ms)),
                    (None, _) => self.near = None,
                }
            }
        }
        steps
    }

    /// Recent movement is within `degrees` of `axis`, either way along it.
    fn within(&self, axis: usize, degrees: f64) -> bool {
        let (a, b) = (self.recent[axis].abs(), self.recent[1 - axis].abs());
        a.hypot(b) >= RECENT_MIN_MM && b <= a * degrees.to_radians().tan()
    }

    fn step(&mut self, axis: usize, d: f64, steps: &mut Vec<Dir>) {
        // What a step leaves over lies its own way, so turning back passes nothing before a whole step.
        let step = self.step[axis];
        let g = &mut self.gathered[axis];
        *g += d;
        while g.abs() >= step {
            let way = g.signum();
            *g -= way * step;
            steps.push(match (axis, way > 0.0) {
                (0, false) => Dir::Left,
                (0, true) => Dir::Right,
                (_, false) => Dir::Up,
                (_, true) => Dir::Down,
            });
        }
    }
}

/// HID unit exponent is a signed four-bit integer; linear SI is cm, English is inches.
pub fn cm_per_unit(
    logical: (i32, i32),
    physical: (i32, i32),
    units: u32,
    exponent: u32,
    fallback_cm: f64,
) -> f64 {
    let logical_span = (logical.1 as f64 - logical.0 as f64).max(1.0);
    let exp = ((exponent as i8) << 4) >> 4;
    let factor = match units {
        0x11 => 1.0,
        0x13 => 2.54,
        _ => 0.0,
    };
    let cm = (physical.1 as f64 - physical.0 as f64) * 10f64.powi(exp as i32) * factor;
    if (1.0..=100.0).contains(&cm) {
        cm / logical_span
    } else {
        fallback_cm / logical_span
    }
}

/// A hybrid frame can span reports; its zero-count continuations are not releases.
#[derive(Default)]
pub struct Frame {
    scan: Option<u32>,
    expected: usize,
    records: Vec<(u32, Option<Contact>)>,
}

impl Frame {
    pub fn push(
        &mut self,
        scan: u32,
        count: usize,
        records: &[(u32, Option<Contact>)],
    ) -> Option<Vec<Contact>> {
        if count > 10 {
            *self = Self::default();
            return None;
        }
        if count > 0 {
            // Per-contact reports may repeat the frame's count; new ids in the same scan continue it.
            let continues = self.scan == Some(scan)
                && self.expected == count
                && records
                    .iter()
                    .all(|(id, _)| !self.records.iter().any(|(seen, _)| seen == id));
            if !continues {
                self.scan = Some(scan);
                self.expected = count;
                self.records.clear();
            }
        } else if self.scan != Some(scan) {
            *self = Self::default();
            return records.iter().all(|(_, c)| c.is_none()).then(Vec::new);
        }
        for record in records {
            if self.records.len() >= self.expected {
                break;
            }
            if !self.records.iter().any(|(id, _)| *id == record.0) {
                self.records.push(*record);
            }
        }
        if self.expected == 0 || self.records.len() != self.expected {
            return None;
        }
        let contacts = self.records.iter().filter_map(|(_, c)| *c).collect();
        *self = Self::default();
        Some(contacts)
    }
}

/// Descriptors that put an unsigned maximum in a too-short item read back negative.
pub fn logical_range(min: i32, max: i32, bits: u16) -> (i32, i32) {
    if max >= min {
        return (min, max);
    }
    let mask = if (1..32).contains(&bits) { (1u32 << bits) - 1 } else { u32::MAX >> 1 };
    let max = (max as u32 & mask) as i32;
    if max > min.max(0) { (min.max(0), max) } else { (0, mask as i32) }
}

/// Why Windows' own three-finger gestures still act, or None when both are Nothing.
/// The basic choice is authoritative; only Custom (an unknown value) defers to the advanced mappings.
pub fn windows_gesture_conflict(
    slide: Option<u32>,
    tap: Option<u32>,
    advanced: [Option<u32>; 5],
) -> Option<&'static str> {
    // Missing global values use Windows defaults, which enable gestures.
    let custom_off = |mappings: &[Option<u32>]| mappings.iter().all(|value| *value == Some(0));
    let slide_on = match slide {
        Some(0) => false,
        None | Some(1..=3) => true,
        Some(_) => !custom_off(&advanced[..4]),
    };
    let tap_on = match tap {
        Some(0) => false,
        None | Some(1..=4) => true,
        Some(_) => !custom_off(&advanced[4..]),
    };
    match (slide_on, tap_on) {
        (true, true) => Some("three-finger swipes and taps are on"),
        (true, false) => Some("three-finger swipes are on"),
        (false, true) => Some("three-finger taps are on"),
        (false, false) => None,
    }
}

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::*;

#[cfg(test)]
mod tests {
    use super::*;
    fn hand(n: u32, x: f64) -> Vec<Contact> {
        (0..n)
            .map(|id| Contact {
                id,
                confident: true,
                x: x + id as f64,
                y: 0.0,
            })
            .collect()
    }

    /// Feeds the hand through `points` (mm), one report every 8 ms from `ms`.
    fn drive(rails: &mut Rails, ms: &mut u32, points: impl IntoIterator<Item = (f64, f64)>) -> Vec<Dir> {
        let mut last: Option<(f64, f64)> = None;
        let mut steps = Vec::new();
        for p in points {
            if let Some(l) = last {
                *ms += 8;
                steps.extend(rails.travel((p.0 - l.0, p.1 - l.1), *ms));
            }
            last = Some(p);
        }
        steps
    }
    fn line(from: (f64, f64), to: (f64, f64), mm_per_report: f64) -> Vec<(f64, f64)> {
        let n = ((to.0 - from.0).hypot(to.1 - from.1) / mm_per_report).ceil().max(1.0) as usize;
        (0..=n)
            .map(|i| {
                let t = i as f64 / n as f64;
                (from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t)
            })
            .collect()
    }
    fn count(steps: &[Dir], dir: Dir) -> usize {
        steps.iter().filter(|s| **s == dir).count()
    }
    #[test]
    fn straight_down_with_a_crooked_finger_path_takes_no_cell_sideways() {
        // Wavering 4 mm either way, at several paces and wavelengths, drifting off sideways too.
        for (wave, drift, pace) in [(40.0, 0.0, 0.5), (24.0, 0.0, 0.3), (40.0, 12.0, 0.8), (60.0, -15.0, 0.4)] {
            let mut rails = Rails::default();
            let mut ms = 0;
            let path = (0..=(61.0 / pace) as usize).map(|i| {
                let y = i as f64 * pace;
                (4.0 * ((std::f64::consts::TAU * y / wave).cos() - 1.0) + drift * y / 60.0, y)
            });
            let steps = drive(&mut rails, &mut ms, path);
            assert_eq!(steps, vec![Dir::Down; 6], "wave {wave} drift {drift} pace {pace}");
            assert_eq!(rails.progress().0, 0.0, "a rail does not lean sideways");
        }
        // Up and across alike: the axes are the same.
        let mut rails = Rails::default();
        let path = (0..=122).map(|i| (-(i as f64) * 0.5, 4.0 * ((i as f64 * 0.5 / 40.0 * std::f64::consts::TAU).cos() - 1.0)));
        assert_eq!(drive(&mut rails, &mut 0, path), vec![Dir::Left; 6]);
    }
    /// Each way has its own distance per step, set in mm.
    #[test]
    fn each_way_steps_at_its_own_distance() {
        let mut rails = Rails::new([5.0, 20.0]);
        let mut ms = 0;
        assert_eq!(drive(&mut rails, &mut ms, line((0.0, 0.0), (22.0, 0.0), 0.5)), vec![Dir::Right; 4]);
        assert!((rails.progress().0 - 0.4).abs() < 1e-9);
        let mut rails = Rails::new([5.0, 20.0]);
        let steps = drive(&mut rails, &mut ms, line((0.0, 0.0), (0.0, 50.0), 0.5));
        assert_eq!(steps, vec![Dir::Down; 2]);
        assert!((rails.progress().1 - 0.5).abs() < 1e-9);
    }
    #[test]
    fn slantwise_goes_both_ways_and_steady_straight_travel_locks_again() {
        let mut rails = Rails::default();
        let mut ms = 0;
        let steps = drive(&mut rails, &mut ms, line((0.0, 0.0), (35.0, -35.0), 0.5));
        assert_eq!((count(&steps, Dir::Right), count(&steps, Dir::Up), steps.len()), (3, 3, 6));
        // Near 45° only: 50° steep still counts both ways.
        let mut steep = Rails::default();
        let steps = drive(&mut steep, &mut 0, line((0.0, 0.0), (30.0, 35.8), 0.5));
        assert_eq!((count(&steps, Dir::Right), count(&steps, Dir::Down)), (3, 3));
        // Then straight on rightwards for longer than RELOCK_MS: a rail again, wavering is ignored.
        let more = drive(&mut rails, &mut ms, line((35.0, -35.0), (55.0, -35.0), 0.5));
        assert_eq!(more, vec![Dir::Right; 2]);
        let wobble = (0..=60).map(|i| (55.0 + i as f64 * 0.5, -35.0 + 4.0 * (i as f64 * 0.5 / 30.0 * std::f64::consts::TAU).sin()));
        assert_eq!(drive(&mut rails, &mut ms, wobble), vec![Dir::Right; 3]);
    }
    #[test]
    fn an_unclear_start_goes_along_the_row_and_only_a_clear_one_changes_row() {
        // (angle from across, rights, downs): a 40° slant is across alone; so is 58°, short of
        // the up-and-down ratio; 65° is up and down alone; 45° and 50° count both ways.
        for (deg, rights, downs) in [(0.0, 5, 0), (30.0, 4, 0), (40.0, 3, 0), (45.0, 3, 3), (50.0, 3, 3), (58.0, 2, 0), (65.0, 0, 4), (90.0, 0, 5)] {
            let rad = f64::to_radians(deg);
            let to = (50.0 * rad.cos(), 50.0 * rad.sin());
            let mut rails = Rails::default();
            let steps = drive(&mut rails, &mut 0, line((0.0, 0.0), to, 0.5));
            assert_eq!((count(&steps, Dir::Right), count(&steps, Dir::Down), steps.len()), (rights, downs, rights + downs), "{deg}°");
        }
        // A 40° slant across wavering up and down besides still changes no row.
        let mut rails = Rails::default();
        let path = (0..=140).map(|i| {
            let x = i as f64 * 0.5;
            (x, x * 0.84 + 3.0 * ((x / 25.0 * std::f64::consts::TAU).cos() - 1.0))
        });
        let steps = drive(&mut rails, &mut 0, path);
        assert_eq!((count(&steps, Dir::Right), count(&steps, Dir::Down)), (7, 0));
    }
    #[test]
    fn an_l_switches_axis_without_counting_the_turn() {
        let mut rails = Rails::default();
        let mut ms = 0;
        let down = drive(&mut rails, &mut ms, line((0.0, 0.0), (0.0, 34.0), 0.5));
        assert_eq!(down, vec![Dir::Down; 3]);
        // Out of the narrow channel the remaining 4 mm down is dropped and across gathers from nothing.
        let across = drive(&mut rails, &mut ms, line((0.0, 34.0), (-25.0, 34.0), 0.5));
        assert_eq!(across, vec![Dir::Left; 2]);
        assert_eq!(rails.progress().1, 0.0);
        // Back to up and down the same way, only out of the wider channel round across.
        let up = drive(&mut rails, &mut ms, line((-25.0, 34.0), (-25.0, 14.0), 0.5));
        assert_eq!(up, vec![Dir::Up]);
        let mut late = Rails::default();
        drive(&mut late, &mut 0, line((0.0, 0.0), (30.0, 0.0), 0.5));
        assert!(drive(&mut late, &mut 100, line((30.0, 0.0), (30.0, 9.0), 0.5)).is_empty());
        // Still across: further along the row steps there.
        assert_eq!(drive(&mut late, &mut 200, line((30.0, 9.0), (40.0, 9.0), 0.5)), vec![Dir::Right]);
    }
    #[test]
    fn a_hand_resting_on_the_edge_of_a_step_does_not_take_it_back() {
        let mut rails = Rails::default();
        let mut ms = 0;
        let mut path = line((0.0, 0.0), (0.0, 10.0), 0.5);
        for _ in 0..20 {
            path.extend([(0.3, 8.5), (-0.2, 11.0), (0.0, 9.0), (0.1, 10.5)]);
        }
        assert_eq!(drive(&mut rails, &mut ms, path), vec![Dir::Down]);
        // Back the other way: a whole step from where the step was, not from what it left over.
        assert!(drive(&mut rails, &mut ms, line((0.1, 10.5), (0.0, 0.6), 0.5)).is_empty());
        assert_eq!(drive(&mut rails, &mut ms, line((0.0, 0.6), (0.0, -0.1), 0.5)), vec![Dir::Up]);
    }
    #[test]
    fn short_travel_is_judged_before_it_counts_and_one_cm_is_one_step() {
        let mut rails = Rails::default();
        assert!(rails.travel((1.0, 2.0), 0).is_empty());
        assert_eq!(rails.progress(), (0.0, 0.0));
        assert!(rails.travel((0.0, 7.0), 8).is_empty());
        assert_eq!(rails.progress(), (0.0, 0.9));
        assert_eq!(rails.travel((0.0, 1.0), 16), vec![Dir::Down]);
        // One report going more than a step at once takes every step it went.
        let mut fling = Rails::default();
        assert_eq!(fling.travel((25.0, 1.0), 0), vec![Dir::Right; 2]);
        // The gesture hands its hold's travel over in mm.
        let mut g = Gesture::default();
        g.update(&hand(3, 0.0), 0);
        assert_eq!(g.update(&hand(3, 1.0), 8).steps, vec![Dir::Right]);
        let progress = g.update(&hand(3, 1.05), 16).progress;
        assert!((progress.0 - 0.05).abs() < 1e-9 && progress.1 == 0.0, "{progress:?}");
    }
    #[test]
    fn tap_and_small_travel() {
        for (travel, time, tap) in [
            (0.0, 250, true),
            (0.19, 200, true),
            (0.2, 200, false),
            (0.0, 251, false),
        ] {
            let mut g = Gesture::default();
            assert!(g.update(&hand(3, 0.0), 0).began);
            g.update(&hand(3, travel), 100);
            assert_eq!(g.update(&[], time).ended, Some(tap));
        }
    }
    #[test]
    fn tap_duration_uses_input_timestamps_including_tick_wrap() {
        let mut g = Gesture::default();
        g.update(&hand(3, 0.0), u32::MAX - 100);
        g.update(&hand(3, 0.01), u32::MAX - 50);
        assert_eq!(g.update(&[], 20).ended, Some(true));
    }
    #[test]
    fn hold_lasts_through_two_and_one_finger_and_rebases() {
        let mut g = Gesture::default();
        g.update(&hand(3, 0.0), 0);
        assert_eq!(g.update(&hand(3, 1.0), 100).delta, (1.0, 0.0));
        let mut two = hand(2, 5.0);
        two.reverse();
        let e = g.update(&two, 200);
        assert_eq!(e, Event::default());
        assert_eq!(g.update(&hand(2, 6.0), 300).delta, (1.0, 0.0));
        assert_eq!(g.update(&hand(1, 9.0), 400), Event::default());
        assert_eq!(g.update(&hand(1, 10.0), 500).delta, (1.0, 0.0));
        assert_eq!(g.update(&[], 600).ended, Some(false));
    }
    #[test]
    fn low_confidence_cannot_start_a_hold_or_end_one_before_tip_up() {
        let mut g = Gesture::default();
        let mut contacts = hand(3, 0.0);
        contacts[0].confident = false;
        assert!(!g.update(&contacts, 0).began);
        contacts[0].confident = true;
        assert!(g.update(&contacts, 10).began);
        contacts.iter_mut().for_each(|c| c.confident = false);
        assert_eq!(g.update(&contacts, 20), Event::default());
        assert_eq!(g.update(&[], 30).ended, Some(true));
    }
    #[test]
    fn lifting_fingers_or_one_wandering_finger_does_not_hide_travel() {
        let mut g = Gesture::default();
        g.update(&hand(3, 0.0), 0);
        assert_eq!(g.update(&hand(2, 1.0), 100), Event::default());
        assert_eq!(g.update(&[], 150).ended, Some(false));
        let mut g = Gesture::default();
        g.update(&hand(3, 0.0), 0);
        let mut fingers = hand(3, 0.0);
        fingers[0].x += 0.3;
        g.update(&fingers, 100);
        assert_eq!(g.update(&[], 150).ended, Some(false));
    }
    #[test]
    fn landing_time_and_cancellation() {
        let mut g = Gesture::default();
        g.update(&hand(1, 0.0), 0);
        g.update(&hand(3, 0.0), 200);
        assert_eq!(g.update(&[], 300).ended, Some(false));
        g.update(&hand(3, 0.0), 400);
        g.cancel();
        assert_eq!(g.update(&hand(3, 1.0), 500), Event::default());
        assert_eq!(g.update(&[], 600), Event::default());
        assert!(g.update(&hand(3, 0.0), 700).began);
    }
    #[test]
    fn one_cm_is_one_step_with_both_row_modes() {
        let cm = cm_per_unit((0, 4000), (0, 4000), 0x11, 0xE, 10.0);
        assert_eq!(cm, 0.01);
        assert!((cm_per_unit((0, 1000), (0, 4000), 0x13, 0xD, 10.0) - 0.01016).abs() < 1e-10);
        assert_eq!(cm_per_unit((0, 1000), (0, 0), 0, 0, 10.0), 0.01);
        for mut plane in [crate::slide::Plane::new(), crate::slide::Plane::linear()] {
            plane.travel(
                (100.0 * cm * 260.0).round() as i32,
                (100.0 * cm * 400.0).round() as i32,
                (260, 400),
            );
            assert_eq!((plane.cols.cell(), plane.rows.cell()), (1, 1));
        }
    }
    #[test]
    fn hybrid_frame_recovers_from_scan_rollover_and_ignores_orphan_active_reports() {
        let mut f = Frame::default();
        let c = hand(3, 0.0);
        assert_eq!(f.push(10, 0, &[(0, Some(c[0]))]), None);
        assert_eq!(f.push(65535, 3, &[(0, Some(c[0]))]), None);
        assert_eq!(f.push(0, 1, &[(0, Some(c[0]))]), Some(vec![c[0]]));
        assert_eq!(f.push(1, 3, &[(0, Some(c[0])), (1, Some(c[1]))]), None);
        assert_eq!(f.push(1, 0, &[(2, Some(c[2])), (0, None)]), Some(c));
        assert_eq!(f.push(2, 1, &[(0, None), (0, None)]), Some(vec![]));
    }
    #[test]
    fn per_contact_reports_with_a_repeated_count_complete_one_frame() {
        let mut f = Frame::default();
        let c = hand(3, 0.0);
        assert_eq!(f.push(7, 3, &[(0, Some(c[0]))]), None);
        assert_eq!(f.push(7, 3, &[(1, Some(c[1]))]), None);
        assert_eq!(f.push(7, 3, &[(2, Some(c[2]))]), Some(c.clone()));
        // A repeated id or a new scan starts the next frame instead of joining a stale one.
        assert_eq!(f.push(8, 3, &[(0, Some(c[0]))]), None);
        assert_eq!(f.push(8, 3, &[(0, Some(c[0]))]), None);
        assert_eq!(f.push(9, 2, &[(1, Some(c[1]))]), None);
        assert_eq!(f.push(9, 2, &[(2, None)]), Some(vec![c[1]]));
        // Per-contact count-first frames (count only on the first report) also complete.
        assert_eq!(f.push(10, 3, &[(0, Some(c[0]))]), None);
        assert_eq!(f.push(10, 0, &[(1, Some(c[1]))]), None);
        assert_eq!(f.push(10, 0, &[(2, Some(c[2]))]), Some(c.clone()));
        let mut g = Gesture::default();
        assert!(g.update(&c, 0).began);
    }
    #[test]
    fn parallel_reports_skip_unused_zero_slots() {
        let mut f = Frame::default();
        let c = hand(3, 0.0);
        let mut slots: Vec<_> = c.iter().map(|c| (c.id, Some(*c))).collect();
        slots.extend([(0, None), (0, None)]);
        assert_eq!(f.push(1, 3, &slots), Some(c));
    }
    #[test]
    fn negative_logical_maximum_reads_as_unsigned() {
        assert_eq!(logical_range(0, 4095, 12), (0, 4095));
        assert_eq!(logical_range(0, -1, 16), (0, 0xFFFF));
        assert_eq!(logical_range(0, -2, 12), (0, 0xFFE));
        assert_eq!(logical_range(-100, 100, 16), (-100, 100));
        assert_eq!(logical_range(0, -1, 32), (0, i32::MAX));
    }
    #[test]
    fn basic_nothing_wins_over_stale_advanced_mappings() {
        let none = [Some(0); 5];
        let stale = [Some(5), Some(6), Some(7), Some(8), Some(2)];
        assert_eq!(windows_gesture_conflict(Some(0), Some(0), none), None);
        assert_eq!(windows_gesture_conflict(Some(0), Some(0), stale), None);
        assert_eq!(windows_gesture_conflict(Some(0), Some(0), [None; 5]), None);
        assert!(windows_gesture_conflict(None, Some(0), none).is_some());
        assert!(windows_gesture_conflict(Some(2), Some(0), none).is_some());
        assert_eq!(windows_gesture_conflict(Some(0), Some(1), none), Some("three-finger taps are on"));
        // Custom choices defer to the advanced mappings, which must all exist and be Nothing.
        assert_eq!(windows_gesture_conflict(Some(0xFFFF), Some(0xFFFF), none), None);
        assert!(windows_gesture_conflict(Some(0xFFFF), Some(0), stale).is_some());
        assert!(windows_gesture_conflict(Some(0xFFFF), Some(0), [None; 5]).is_some());
    }
    #[test]
    fn hybrid_frames_include_lift_records_and_ignore_zero_continuations() {
        let mut f = Frame::default();
        let c = hand(3, 0.0);
        assert_eq!(f.push(1, 3, &[(0, Some(c[0]))]), None);
        assert_eq!(f.push(1, 0, &[(1, Some(c[1]))]), None);
        assert_eq!(f.push(1, 0, &[(2, Some(c[2]))]), Some(c.clone()));
        assert_eq!(
            f.push(2, 3, &[(0, None), (1, Some(c[1])), (2, None)]),
            Some(vec![c[1]])
        );
        assert_eq!(f.push(3, 1, &[(1, None)]), Some(vec![]));
        assert_eq!(f.push(4, 0, &[]), Some(vec![]));
        assert_eq!(f.push(5, 3, &[(0, Some(c[0]))]), None);
        assert_eq!(f.push(6, 1, &[(1, Some(c[1]))]), Some(vec![c[1]]));
    }
}
