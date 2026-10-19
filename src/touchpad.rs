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
    /// How far the hand is from the last step each way, in steps, for the
    /// screen following it (`Rails::lean`).
    pub lean: (f64, f64),
    pub axis: Option<usize>,
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
            event.lean = self.rails.lean();
            event.axis = self.rails.axis();
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

/// Movement needed to commit a cell in either direction, in millimetres.
pub const STEP_MM: f64 = 12.0;
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
/// Locks the physical pad path before deriving steps or visible lean.
#[derive(Debug)]
pub struct Rails {
    step: [f64; 2],
    lock: crate::axis::Lock,
    counts: [i32; 2],
}

impl Default for Rails {
    fn default() -> Self { Self::new([STEP_MM; 2]) }
}

impl Rails {
    pub fn new(step: [f64; 2]) -> Self {
        let mut lock = crate::axis::Lock::default();
        lock.f_mode = true;
        Self { step, lock, counts: [0; 2] }
    }

    pub fn lean(&self) -> (f64, f64) {
        (self.lock.at[0] - self.counts[0] as f64, self.lock.at[1] - self.counts[1] as f64)
    }

    pub fn progress(&self) -> (f64, f64) { self.lean() }

    pub fn axis(&self) -> Option<usize> { self.lock.axis }

    pub fn travel(&mut self, delta: (f64, f64), ms: u32) -> Vec<Dir> {
        self.lock.travel([delta.0, delta.1], self.step, 3.0, ms);
        let mut steps = Vec::new();
        for axis in 0..2 {
            let rest = self.lock.at[axis] - self.counts[axis] as f64;
            let count = (rest.abs() + 1e-9).floor() as i32;
            let way = if rest >= 0.0 { 1 } else { -1 };
            for _ in 0..count {
                self.counts[axis] += way;
                steps.push(match (axis, way) { (0, 1) => Dir::Right, (0, _) => Dir::Left, (1, 1) => Dir::Down, _ => Dir::Up });
            }
        }
        steps
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
    #[test]
    fn rails_share_the_dead_zone_and_keep_the_raw_origin_across_turns() {
        let mut rails = Rails::new([10.0; 2]);
        for ms in 0..5 { assert!(rails.travel((0.4, 0.1), ms * 8).is_empty()); assert_eq!(rails.lean(), (0.0, 0.0)); }
        let steps = drive(&mut rails, &mut 40, line((0.0, 0.0), (35.0, 4.0), 0.5));
        assert_eq!(steps, vec![Dir::Right; 3]);
        assert_eq!(rails.lean().1, 0.0);
        let steps = drive(&mut rails, &mut 1000, line((35.0, 4.0), (35.0, -36.0), 0.5));
        assert!(steps.contains(&Dir::Up));
        let x = rails.lean().0;
        rails.travel((0.0, -0.5), 2000);
        assert!((rails.lean().0 - x).abs() < 1e-9);
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
        assert_eq!(e, Event { axis: Some(0), ..Event::default() });
        let previous = g.update(&hand(2, 6.0), 300);
        assert_eq!(previous.delta, (1.0, 0.0));
        let rebased = g.update(&hand(1, 9.0), 400);
        assert_eq!(rebased.delta, (0.0, 0.0));
        assert_eq!(rebased.lean, previous.lean);
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
