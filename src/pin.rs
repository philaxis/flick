//! The same exclusive window/app pin choices in the board and tray.
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pin { Off, Row, Everywhere }
impl Pin {
    pub fn from_flags(row: bool, all: bool) -> Self {
        if all { Self::Everywhere } else if row { Self::Row } else { Self::Off }
    }
    pub fn next(self) -> Self {
        match self { Self::Off => Self::Row, Self::Row => Self::Everywhere, Self::Everywhere => Self::Off }
    }
    pub fn choose(self, choice: Self) -> Self {
        if self == choice { Self::Off } else { choice }
    }
    pub fn flags(self) -> (bool, bool) { (self == Self::Row, self == Self::Everywhere) }
    pub fn label(self) -> &'static str {
        let words = match self {
            Self::Off => ("Off", "끄기", "关闭"),
            Self::Row => ("This shelf", "이 셸프", "本架子"),
            Self::Everywhere => ("All shelves", "모든 셸프", "所有架子"),
        };
        #[cfg(windows)]
        { crate::i18n::text(words.0, words.1, words.2) }
        #[cfg(not(windows))]
        { words.0 }
    }
}
#[derive(Clone, Copy, Default)]
pub struct PinState {
    pub row_window: bool,
    pub row_app: bool,
    pub all_window: bool,
    pub all_app: bool,
}
impl PinState {
    // A window-wide All shelves pin can override an app's This shelf pin.
    pub fn inherited(self) -> bool { self.all_app || (self.row_app && !self.all_window) }
    pub fn window_change(self, requested: Pin) -> Option<Pin> {
        (!self.inherited()).then_some(requested)
    }
}
/// Windows Flick put on top as they were pinned (`by_pin`), taken down again
/// as they are unpinned, and pinned ones the user took down since
/// (`declined`), left down. Kept with the grid, so after a restart too.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PinTop {
    #[serde(default)]
    pub by_pin: Vec<isize>,
    #[serde(default)]
    pub declined: Vec<isize>,
}
impl PinTop {
    /// `window`'s pin went from `was` to `now`, it being on top or not: to
    /// be put on top (`Some(true)`), taken down (`Some(false)`) or left.
    /// Pinned, or pinned another way, it goes on top unless it is already or
    /// the user took it down; unpinned, it comes down if pinning put it up.
    pub fn pin_changed(&mut self, window: isize, was: Pin, now: Pin, topmost: bool) -> Option<bool> {
        if now == Pin::Off {
            self.declined.retain(|w| *w != window);
            let put = self.by_pin.contains(&window);
            self.by_pin.retain(|w| *w != window);
            return (put && topmost).then_some(false);
        }
        (now != was && !topmost && !self.declined.contains(&window)).then_some(true)
    }
    /// Pinning put `window` on top.
    pub fn put_up(&mut self, window: isize) {
        self.by_pin.retain(|w| *w != window);
        self.by_pin.push(window);
    }
    /// The user put `window` on top or took it down: theirs from now on.
    pub fn set_by_user(&mut self, window: isize, on: bool, pinned: bool) {
        self.by_pin.retain(|w| *w != window);
        self.declined.retain(|w| *w != window);
        if !on && pinned {
            self.declined.push(window);
        }
    }
}
/// A window property is destroyed with the window, even if its HWND is reused.
pub fn same_window(saved: isize, property: isize) -> bool { saved != 0 && saved == property }
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn board_and_menu_use_one_exclusive_cycle() {
        let mut pin = Pin::Off;
        for expected in [Pin::Row, Pin::Everywhere, Pin::Off] {
            pin = pin.next();
            assert_eq!(pin, expected);
            let (row, all) = pin.flags();
            assert!(!(row && all));
            assert_eq!(Pin::from_flags(row, all), pin);
            assert_eq!(Pin::Off.choose(pin), pin);
            assert_eq!(pin.choose(pin), Pin::Off);
        }
        assert_eq!(Pin::Everywhere.choose(Pin::Row), Pin::Row);
        assert_eq!(Pin::Row.choose(Pin::Everywhere), Pin::Everywhere);
    }
    #[test]
    fn one_window_cannot_remove_an_app_pin() {
        for state in [PinState { row_app: true, ..Default::default() }, PinState { all_app: true, ..Default::default() }] {
            assert_eq!(state.window_change(Pin::Off), None);
            assert!(state.inherited());
        }
        assert_eq!(PinState::default().window_change(Pin::Off), Some(Pin::Off));
        let override_pin = PinState { row_app: true, all_window: true, ..Default::default() };
        assert_eq!(override_pin.window_change(Pin::Off), Some(Pin::Off));
        assert!(override_pin.row_app); // Removing the window override leaves the app's shelf pin alone.
    }
    #[test]
    fn pinning_puts_on_top_and_unpinning_takes_down_only_its_own() {
        let mut top = PinTop::default();
        assert_eq!(top.pin_changed(1, Pin::Off, Pin::Row, false), Some(true));
        top.put_up(1);
        assert_eq!(top.pin_changed(1, Pin::Row, Pin::Everywhere, true), None);
        assert_eq!(top.pin_changed(1, Pin::Everywhere, Pin::Off, true), Some(false));
        assert_eq!(top.pin_changed(1, Pin::Off, Pin::Off, false), None);
        // On top before it was pinned: left so as it is unpinned.
        assert_eq!(top.pin_changed(2, Pin::Off, Pin::Everywhere, true), None);
        assert_eq!(top.pin_changed(2, Pin::Everywhere, Pin::Off, true), None);
        // Taken down by the user while pinned: stays down, also pinned another way.
        assert_eq!(top.pin_changed(3, Pin::Off, Pin::Row, false), Some(true));
        top.put_up(3);
        top.set_by_user(3, false, true);
        assert_eq!(top.pin_changed(3, Pin::Row, Pin::Everywhere, false), None);
        assert_eq!(top.pin_changed(3, Pin::Everywhere, Pin::Off, false), None);
        assert_eq!(top.pin_changed(3, Pin::Off, Pin::Row, false), Some(true));
        // Put back up by the user while pinned: theirs, left up as it is unpinned.
        top.put_up(3);
        top.set_by_user(3, false, true);
        top.set_by_user(3, true, true);
        assert_eq!(top.pin_changed(3, Pin::Row, Pin::Off, true), None);
        assert_eq!(top, PinTop::default());
    }
    #[test]
    fn reused_handles_do_not_inherit_the_old_window() {
        assert!(same_window(7, 7));
        assert!(!same_window(7, 0));
        assert!(!same_window(7, 8));
        assert!(!same_window(0, 0));
    }
}
