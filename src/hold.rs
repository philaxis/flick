//! What is being held: which triggers are down, and whether keys going down
//! are a chord or ordinary typing.
//!
//! Plain state with no Windows in it. The hooks in `input` feed it events and
//! carry out what it answers.

/// One thing that can hold the gesture.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Source {
    XButton1,
    XButton2,
    Middle,
    Key(u32),
    Chord,
}

/// The triggers that are down right now. The gesture lasts while any of them
/// is, so letting go of one does not end a hold kept by another.
#[derive(Default)]
pub struct Held(Vec<Source>);

impl Held {
    pub fn any(&self) -> bool {
        !self.0.is_empty()
    }

    /// Returns true when nothing was held before: the gesture begins.
    pub fn press(&mut self, source: Source) -> bool {
        let began = self.0.is_empty();
        if !self.0.contains(&source) {
            self.0.push(source);
        }
        began
    }

    /// Returns true when this was the last one held: the gesture ends.
    pub fn release(&mut self, source: Source) -> bool {
        let before = self.any();
        self.0.retain(|held| *held != source);
        before && !self.any()
    }

    pub fn has(&self, source: Source) -> bool {
        self.0.contains(&source)
    }

    /// Forgets everything; returns whether anything was held.
    pub fn clear(&mut self) -> bool {
        !std::mem::take(&mut self.0).is_empty()
    }
}

/// What to do about a key event.
#[derive(Debug, Default, PartialEq)]
pub struct Verdict {
    /// Keep the event from the apps.
    pub swallow: bool,
    /// Key events to give the apps instead, in order: (key, is a release).
    pub replay: Vec<(u32, bool)>,
    /// The chord went down (true) or came up (false) as a trigger.
    pub trigger: Option<bool>,
    /// Why a partial chord became typing, for the regular log.
    pub abandoned: Option<Abandoned>,
}

/// A partial chord that was given back to the apps as typing.
#[derive(Debug, PartialEq)]
pub struct Abandoned {
    pub arrived: Vec<u32>,
    pub missing: Vec<u32>,
    pub waited: u32,
    pub why: &'static str,
}

/// Tells a chord (several keys pressed together) from typing.
///
/// Physical keys must overlap and arrive together. Mouse software injects
/// keys more slowly, sometimes releasing one before sending the next: those
/// events wait longer, in order, until the chord completes or is replayed.
#[derive(Default)]
pub struct Chord {
    keys: Vec<u32>,
    pending: Vec<(u32, bool)>,
    /// Pending keys that have not come up yet.
    down: Vec<u32>,
    /// Keys still down that went to the apps as typing.
    typed: Vec<u32>,
    /// Keys still down after the chord fired; their events are dropped.
    fired: Vec<u32>,
    began: u32,
    injected: bool,
}

impl Chord {
    pub fn new(keys: Vec<u32>) -> Chord {
        Chord { keys, ..Chord::default() }
    }

    pub fn has(&self, key: u32) -> bool {
        self.keys.contains(&key)
    }

    /// The chord went down as the trigger and this key of it has not come up.
    pub fn holds(&self, key: u32) -> bool {
        self.fired.contains(&key)
    }

    /// Raw input can finish draining a physical chord after the hold ends,
    /// even when the keyboard hook missed its releases.
    pub fn draining(&self) -> bool {
        !self.fired.is_empty()
    }

    /// A hardware release must not cancel a chord sent by mouse software.
    pub fn raw_up(&mut self, key: u32) -> bool {
        if self.injected || !self.holds(key) {
            return false;
        }
        self.fired.retain(|k| *k != key);
        true
    }

    /// Forgets a chord whose release could not be heard.
    pub fn let_go(&mut self) {
        self.fired.clear();
    }

    pub fn waiting(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Milliseconds left, counted from the first event, even across the
    /// Windows uptime counter wrapping. Repeats never extend the wait.
    pub fn remaining(&self, now: u32) -> u32 {
        // Two letters typed next to each other need a shorter wait.
        // Mouse software may send the chord slowly, so give it half a second.
        let window: u32 = if self.injected { 500 } else if self.keys.len() <= 2 { 40 } else { 100 };
        window.saturating_sub(now.wrapping_sub(self.began))
    }

    /// A key event, with Windows' event time and whether software sent it.
    pub fn key(&mut self, key: u32, up: bool, injected: bool, now: u32) -> Verdict {
        // Timers are low priority: enforce the deadline here too, before a
        // late key can complete a chord whose timer has not been dispatched.
        let expired = self.timeout(now);
        let mut verdict = self.event(key, up, injected, now);
        if !expired.replay.is_empty() {
            let mut replay = expired.replay;
            if !verdict.swallow {
                replay.push((key, up));
                verdict.swallow = true;
            }
            replay.append(&mut verdict.replay);
            verdict.replay = replay;
            verdict.abandoned = expired.abandoned;
        }
        verdict
    }

    fn event(&mut self, key: u32, up: bool, injected: bool, now: u32) -> Verdict {
        let swallow = Verdict { swallow: true, ..Verdict::default() };
        if !self.has(key) {
            if !self.waiting() {
                return Verdict::default();
            }
            let mut verdict = self.flush(now, "another key arrived");
            verdict.replay.push((key, up));
            return verdict;
        }
        if self.fired.contains(&key) {
            if !up {
                return swallow;
            }
            self.fired.retain(|k| *k != key);
            return Verdict { trigger: Some(false), ..swallow };
        }
        if self.typed.contains(&key) {
            if up {
                self.typed.retain(|k| *k != key);
            }
            return Verdict::default();
        }
        // Do not combine a person's typing with a software button press.
        if self.waiting() && injected != self.injected {
            let mut verdict = self.flush(now, "the input source changed");
            if !up {
                self.typed.push(key);
            } else {
                self.typed.retain(|k| *k != key);
            }
            verdict.replay.push((key, up));
            return verdict;
        }
        if self.down.contains(&key) {
            if !up {
                return swallow;
            }
            self.pending.push((key, true));
            self.down.retain(|k| *k != key);
            if !injected {
                return self.flush(now, "a key came up before the rest arrived");
            }
            return swallow;
        }
        if up {
            return Verdict::default();
        }
        if !self.typed.is_empty() {
            self.typed.push(key);
            return Verdict::default();
        }
        // After firing, drain the whole chord before accepting another one.
        if !self.fired.is_empty() {
            self.fired.push(key);
            return swallow;
        }
        if !self.waiting() {
            self.began = now;
            self.injected = injected;
        }
        self.pending.push((key, false));
        self.down.push(key);
        if self.keys.iter().all(|k| self.pending.contains(&(*k, false))) {
            self.pending.clear();
            self.fired = std::mem::take(&mut self.down);
            return Verdict { trigger: Some(true), ..swallow };
        }
        swallow
    }

    /// Gives up only once the wait is over. A queued timer from an earlier
    /// chord must not cut the next chord's wait short.
    pub fn timeout(&mut self, now: u32) -> Verdict {
        if self.waiting() && self.remaining(now) == 0 {
            self.flush(now, "the wait ran out")
        } else {
            Verdict::default()
        }
    }

    /// Gives all held-back events to the apps, keeping down/up order and
    /// leaving only keys still down classified as typing until their release.
    pub fn flush(&mut self, now: u32, why: &'static str) -> Verdict {
        if !self.waiting() {
            return Verdict::default();
        }
        let arrived: Vec<u32> = self.keys.iter().copied().filter(|key| self.pending.contains(&(*key, false))).collect();
        let missing = self.keys.iter().copied().filter(|key| !arrived.contains(key)).collect();
        let abandoned = Some(Abandoned { arrived, missing, waited: now.wrapping_sub(self.began), why });
        self.typed.append(&mut self.down);
        Verdict { swallow: true, replay: std::mem::take(&mut self.pending), trigger: None, abandoned }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: u32 = 0x54;
    const Y: u32 = 0x59;
    const CTRL: u32 = 0xA2;
    const ALT: u32 = 0xA4;
    const F13: u32 = 0x7C;

    #[test]
    fn injected_keys_can_arrive_slowly_in_any_order() {
        for keys in [[CTRL, ALT, F13], [CTRL, F13, ALT], [ALT, CTRL, F13],
            [ALT, F13, CTRL], [F13, CTRL, ALT], [F13, ALT, CTRL]] {
            let mut chord = Chord::new(vec![CTRL, ALT, F13]);
            assert!(chord.key(keys[0], false, true, 0).swallow);
            assert_eq!(chord.timeout(100), Verdict::default());
            assert!(chord.key(keys[1], false, true, 150).swallow);
            assert_eq!(chord.key(keys[2], false, true, 450).trigger, Some(true));
            for key in keys {
                assert_eq!(chord.key(key, false, true, 900).trigger, None, "a repeat began another hold");
            }
            assert_eq!(chord.key(keys[0], true, true, 1000).trigger, Some(false));
        }
        let mut pair = Chord::new(vec![T, Y]);
        pair.key(T, false, true, 0);
        assert_eq!(pair.key(Y, false, true, 200).trigger, Some(true));
    }

    #[test]
    fn physical_keys_keep_the_short_deadline_even_if_the_timer_is_late() {
        for (keys, window) in [(vec![T, Y], 40), (vec![CTRL, ALT, F13], 100)] {
            let mut chord = Chord::new(keys.clone());
            chord.key(keys[0], false, false, 0);
            assert_eq!(chord.timeout(window - 1), Verdict::default());
            let late = chord.key(keys[1], false, false, window);
            assert_eq!(late.trigger, None);
            assert_eq!(late.replay, [(keys[0], false), (keys[1], false)]);
            assert_eq!(late.abandoned.unwrap().waited, window);
            assert_eq!(chord.key(keys[0], true, false, window + 1), Verdict::default());
        }
    }

    #[test]
    fn only_injected_keys_may_be_released_before_the_chord_completes() {
        let mut chord = Chord::new(vec![CTRL, ALT, F13]);
        chord.key(CTRL, false, true, 0);
        assert!(chord.key(CTRL, true, true, 10).replay.is_empty());
        chord.key(ALT, false, true, 150);
        chord.key(ALT, true, true, 160);
        assert_eq!(chord.key(F13, false, true, 300).trigger, Some(true));
        assert!(!chord.holds(CTRL));
        assert!(chord.holds(F13));
        assert_eq!(chord.key(F13, true, true, 310).trigger, Some(false));

        let mut physical = Chord::new(vec![T, Y]);
        physical.key(T, false, false, 0);
        let released = physical.key(T, true, false, 10);
        assert_eq!(released.replay, [(T, false), (T, true)]);
        assert_eq!(released.abandoned.unwrap().why, "a key came up before the rest arrived");
        assert_eq!(physical.key(Y, false, false, 20).trigger, None);
    }

    #[test]
    fn an_incomplete_injected_sequence_replays_downs_and_ups_in_order() {
        let mut chord = Chord::new(vec![CTRL, ALT, F13]);
        chord.key(CTRL, false, true, 0);
        chord.key(CTRL, true, true, 10);
        chord.key(ALT, false, true, 150);
        let timeout = chord.timeout(500);
        assert_eq!(timeout.replay, [(CTRL, false), (CTRL, true), (ALT, false)]);
        assert_eq!(timeout.abandoned, Some(Abandoned {
            arrived: vec![CTRL, ALT], missing: vec![F13], waited: 500, why: "the wait ran out",
        }));
        assert_eq!(chord.key(ALT, false, true, 510), Verdict::default());
        assert_eq!(chord.key(F13, false, true, 520), Verdict::default());
        assert_eq!(chord.key(ALT, true, true, 530), Verdict::default());
        assert_eq!(chord.key(F13, true, true, 540), Verdict::default());
    }

    #[test]
    fn repeats_do_not_extend_the_wait_and_early_timers_do_not_shorten_it() {
        let mut chord = Chord::new(vec![T, Y]);
        chord.key(T, false, true, 0);
        assert!(chord.key(T, false, true, 400).swallow);
        assert_eq!(chord.remaining(400), 100);
        assert_eq!(chord.timeout(499), Verdict::default());
        let late = chord.key(Y, false, true, 500);
        assert_eq!(late.trigger, None);
        assert_eq!(late.replay, [(T, false), (Y, false)]);
        chord.key(T, true, true, 501);
        chord.key(Y, true, true, 502);
        chord.key(T, false, true, 600);
        assert_eq!(chord.timeout(601), Verdict::default(), "an old timer ended a new wait");
        assert_eq!(chord.key(Y, false, true, 700).trigger, Some(true));
    }

    #[test]
    fn another_key_or_input_source_keeps_typing_in_order() {
        for injected in [false, true] {
            let mut chord = Chord::new(vec![CTRL, F13]);
            chord.key(CTRL, false, injected, 0);
            let typing = chord.key(T, false, injected, 10);
            assert_eq!(typing.replay, [(CTRL, false), (T, false)]);
            assert_eq!(typing.abandoned.unwrap().missing, [F13]);
            assert_eq!(chord.key(F13, false, injected, 20), Verdict::default());
            assert_eq!(chord.key(CTRL, true, injected, 30), Verdict::default());
        }
        let mut chord = Chord::new(vec![CTRL, F13]);
        chord.key(CTRL, false, false, 0);
        let mixed = chord.key(F13, false, true, 10);
        assert_eq!(mixed.trigger, None);
        assert_eq!(mixed.replay, [(CTRL, false), (F13, false)]);
        assert_eq!(mixed.abandoned.unwrap().why, "the input source changed");
    }

    #[test]
    fn a_repressed_key_does_not_rearm_a_chord_that_has_not_drained() {
        let mut chord = Chord::new(vec![T, Y]);
        chord.key(T, false, true, 0);
        assert_eq!(chord.key(Y, false, true, 1).trigger, Some(true));
        assert_eq!(chord.key(T, true, true, 2).trigger, Some(false));
        assert_eq!(chord.key(T, false, true, 3).trigger, None);
        assert!(!chord.waiting());
        assert_eq!(chord.key(Y, false, true, 4).trigger, None);
        chord.key(T, true, true, 5);
        chord.key(Y, true, true, 6);
        chord.key(T, false, true, 7);
        assert_eq!(chord.key(Y, false, true, 8).trigger, Some(true));
    }

    #[test]
    fn raw_releases_drain_only_physical_chords_one_key_at_a_time() {
        let mut chord = Chord::new(vec![T, Y]);
        chord.key(T, false, false, 0);
        chord.key(Y, false, false, 1);
        assert!(chord.raw_up(T));
        assert!(chord.draining());
        assert!(!chord.raw_up(T));
        assert!(chord.raw_up(Y));
        assert!(!chord.draining());
        chord.key(T, false, true, 10);
        assert_eq!(chord.key(Y, false, true, 11).trigger, Some(true));
        assert!(!chord.raw_up(T), "a physical key cancelled a software hold");
        assert!(chord.holds(T));
        assert_eq!(chord.key(T, true, true, 12).trigger, Some(false));
    }

    #[test]
    fn chord_deadlines_work_across_windows_uptime_wrapping() {
        let mut chord = Chord::new(vec![T, Y]);
        chord.key(T, false, true, u32::MAX - 99);
        assert_eq!(chord.remaining(0), 400);
        assert_eq!(chord.timeout(399), Verdict::default());
        assert_eq!(chord.timeout(400).abandoned.unwrap().waited, 500);
    }

    /// A chord key held down on its own is typing: its auto-repeat must reach
    /// the app at once, pressing the other chord key later must not fire the
    /// chord, and the app must see the key come up again (or it stays stuck
    /// down). Pressed together, the keys still are the trigger.
    #[test]
    fn a_chord_key_held_alone_stays_an_ordinary_key() {
        let mut chord = Chord::new(vec![T, Y]);
        assert!(chord.key(T, false, false, 50).swallow);
        assert_eq!(chord.timeout(90).replay, [(T, false)]);

        assert_eq!(chord.key(T, false, false, 100), Verdict::default(), "auto-repeat is held back again");
        assert!(!chord.waiting());
        assert_eq!(chord.key(Y, false, false, 100), Verdict::default(), "a late second key fired the chord");
        assert_eq!(chord.key(T, true, false, 100), Verdict::default(), "the release never reaches the app");
        assert_eq!(chord.key(Y, true, false, 100), Verdict::default());

        assert!(chord.key(Y, false, false, 100).swallow);
        assert_eq!(chord.key(T, false, false, 100), Verdict { swallow: true, trigger: Some(true), ..Verdict::default() });
        assert_eq!(chord.key(T, true, false, 100).trigger, Some(false));
    }

    /// Two single-key triggers held at once: letting go of one must not end
    /// the gesture the other still holds.
    #[test]
    fn the_gesture_lasts_until_the_last_trigger_key_is_released() {
        let (f13, f14) = (Source::Key(0x7C), Source::Key(0x7D));
        let mut held = Held::default();
        assert!(held.press(f13));
        assert!(!held.press(f14));
        assert!(!held.release(f13));
        assert!(held.any());
        assert!(held.release(f14));
    }
}
