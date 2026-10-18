//! Three-finger gestures in centimetres; Windows report decoding lives separately.

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
}

#[derive(Default)]
pub struct Gesture {
    previous: Vec<Contact>,
    landed: Option<u32>,
    held: bool,
    blocked: bool,
    path: f64,
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
            event.began = true;
        } else if self.held {
            event.delta = delta;
        }
        self.previous = contacts.to_vec();
        event
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
            self.scan = Some(scan);
            self.expected = count;
            self.records.clear();
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
