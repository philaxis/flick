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
}

/// Tells a chord (several keys pressed together) from typing.
///
/// The first keys of a chord cannot be told from typing, so they are held
/// back while `waiting`. If the rest follow before `timeout` the chord acts
/// as the trigger and nothing is typed; otherwise the keys are replayed in
/// order and stay ordinary keys until they are let go.
#[derive(Default)]
pub struct Chord {
    keys: Vec<u32>,
    /// Keys held back, waiting to see whether the chord completes.
    pending: Vec<u32>,
    /// Keys still down that went to the apps as typing.
    typed: Vec<u32>,
    /// Keys still down after the chord fired; their events are dropped.
    fired: Vec<u32>,
}

impl Chord {
    pub fn new(keys: Vec<u32>) -> Chord {
        Chord { keys, ..Chord::default() }
    }

    pub fn has(&self, key: u32) -> bool {
        self.keys.contains(&key)
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// Keys are being held back; `timeout` is due when the wait, counted
    /// from the first of them, runs out.
    pub fn waiting(&self) -> bool {
        !self.pending.is_empty()
    }

    /// An event of one of the chord's keys.
    pub fn key(&mut self, key: u32, up: bool) -> Verdict {
        let swallow = Verdict { swallow: true, ..Verdict::default() };
        if self.fired.contains(&key) {
            if !up {
                return swallow; // auto-repeat while the chord is held
            }
            // The chord is over as soon as one of its keys comes up.
            self.fired.retain(|k| *k != key);
            return Verdict { trigger: Some(false), ..swallow };
        }
        if self.typed.contains(&key) {
            if up {
                self.typed.retain(|k| *k != key);
            }
            return Verdict::default();
        }
        if self.pending.contains(&key) {
            if !up {
                return swallow; // auto-repeat while waiting
            }
            // Released before the chord was complete: it was typing.
            let mut replay = self.give_up();
            self.typed.retain(|k| *k != key);
            replay.push((key, true));
            return Verdict { replay, ..swallow };
        }
        if up {
            return Verdict::default();
        }
        if !self.typed.is_empty() {
            // Pressed while another of the chord's keys is down as typing:
            // not together, so this is typing too.
            self.typed.push(key);
            return Verdict::default();
        }
        self.pending.push(key);
        if self.keys.iter().all(|k| self.pending.contains(k)) {
            self.fired = std::mem::take(&mut self.pending);
            return Verdict { trigger: Some(true), ..swallow };
        }
        swallow
    }

    /// An event of any other key. Coming in between, it shows that what was
    /// held back was typing; that is replayed together with this event so
    /// the order is kept.
    pub fn other_key(&mut self, key: u32, up: bool) -> Verdict {
        if !self.waiting() {
            return Verdict::default();
        }
        let mut replay = self.give_up();
        replay.push((key, up));
        Verdict { swallow: true, replay, trigger: None }
    }

    /// The rest of the chord did not follow in time. Returns the key events
    /// to give the apps.
    pub fn timeout(&mut self) -> Vec<(u32, bool)> {
        self.give_up()
    }

    /// What was held back is typing after all: its presses, to be replayed.
    fn give_up(&mut self) -> Vec<(u32, bool)> {
        let keys = std::mem::take(&mut self.pending);
        self.typed.extend(&keys);
        keys.into_iter().map(|key| (key, false)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: u32 = 0x54;
    const Y: u32 = 0x59;

    /// A chord key held down on its own is typing: its auto-repeat must reach
    /// the app at once, pressing the other chord key later must not fire the
    /// chord, and the app must see the key come up again (or it stays stuck
    /// down). Pressed together, the keys still are the trigger.
    #[test]
    fn a_chord_key_held_alone_stays_an_ordinary_key() {
        let mut chord = Chord::new(vec![T, Y]);
        assert!(chord.key(T, false).swallow);
        assert_eq!(chord.timeout(), [(T, false)]);

        assert_eq!(chord.key(T, false), Verdict::default(), "auto-repeat is held back again");
        assert!(!chord.waiting());
        assert_eq!(chord.key(Y, false), Verdict::default(), "a late second key fired the chord");
        assert_eq!(chord.key(T, true), Verdict::default(), "the release never reaches the app");
        assert_eq!(chord.key(Y, true), Verdict::default());

        assert!(chord.key(Y, false).swallow);
        assert_eq!(chord.key(T, false), Verdict { swallow: true, replay: vec![], trigger: Some(true) });
        assert_eq!(chord.key(T, true).trigger, Some(false));
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
