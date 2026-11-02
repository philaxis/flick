//! Where new windows go: rules stacked one by one by the user. A rule says
//! that the new windows a window of one program opens (its title holding a
//! word) are put on a cell and a monitor. The newest rule that fits wins.
//! New windows of the same program as the one they came from are left
//! alone, as are new windows no rule fits.

use crate::grid::CellId;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

/// One rule, as made from a window's tile on the board.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewWindowRule {
    /// Executable name, lower case, of the window new ones come from.
    pub exe: String,
    /// A word its title must hold, compared without case. Empty fits any
    /// title.
    #[serde(default)]
    pub title_word: String,
    pub cell: RuleCell,
    /// The monitor, by the id of its device (stable across plugging it out
    /// and in again). While no such monitor is there, the rule sleeps.
    pub monitor: String,
    /// For a fixed cell only: the user goes there with the new window.
    /// Otherwise the user stays and the keyboard stays with the window the
    /// new one came from.
    #[serde(default)]
    pub follow: bool,
}

/// The cell a rule puts new windows on.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuleCell {
    /// Whichever cell the window they came from is on.
    Source,
    /// Always this cell (a desktop id). While it is gone, the rule sleeps.
    Fixed(CellId),
}

/// The rule for a new window of `new_exe` that came from a window of
/// `source_exe` titled `source_title`: the newest that fits, none for a
/// window of the same program. Executable names are lower case.
pub fn find<'a>(rules: &'a [NewWindowRule], source_exe: &str, source_title: &str, new_exe: &str) -> Option<&'a NewWindowRule> {
    if source_exe == new_exe {
        return None;
    }
    let title = source_title.to_lowercase();
    rules
        .iter()
        .rev()
        .find(|rule| rule.exe == source_exe && title.contains(&rule.title_word.to_lowercase()))
}

/// How long after a window was in front it can still be the window a new
/// one came from.
pub const SOURCE_WINDOW: Duration = Duration::from_secs(2);

/// A window that was in front, and when it came there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrontWindow {
    pub hwnd: isize,
    pub since: Instant,
}

/// The window a new window `new` came from: the last one in `history`
/// other than `new`, while it is the foreground window still or left the
/// front (when the next one came there) no longer than `SOURCE_WINDOW` ago.
/// A window in front for an hour before a link in it was clicked counts.
/// `foreground` is the window with the keyboard now, 0 for none.
pub fn source_window(history: &[FrontWindow], new: isize, foreground: isize, now: Instant) -> Option<FrontWindow> {
    let at = history.iter().rposition(|entry| entry.hwnd != new)?;
    let entry = history[at];
    let left = history.get(at + 1).map(|next| next.since);
    (entry.hwnd == foreground || left.is_some_and(|left| now.saturating_duration_since(left) <= SOURCE_WINDOW)).then_some(entry)
}

/// The window in front just before `hwnd` last came to the front, skipping
/// other entries for `hwnd` itself.
pub fn previous_window(history: &[FrontWindow], hwnd: isize) -> Option<FrontWindow> {
    let last = history.iter().rposition(|entry| entry.hwnd == hwnd)?;
    history[..last].iter().rev().find(|entry| entry.hwnd != hwnd).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(exe: &str, word: &str, monitor: &str) -> NewWindowRule {
        NewWindowRule { exe: exe.into(), title_word: word.into(), cell: RuleCell::Source, monitor: monitor.into(), follow: false }
    }

    #[test]
    fn newest_fitting_rule_wins_and_same_program_is_left_alone() {
        let rules = [rule("orca.exe", "", "A"), rule("orca.exe", "flick", "B"), rule("code.exe", "", "C")];
        assert_eq!(find(&rules, "orca.exe", "Flick — Orca", "chrome.exe").map(|r| r.monitor.as_str()), Some("B"));
        assert_eq!(find(&rules, "orca.exe", "mulbit — Orca", "chrome.exe").map(|r| r.monitor.as_str()), Some("A"));
        assert_eq!(find(&rules, "orca.exe", "Flick — Orca", "orca.exe"), None);
        assert_eq!(find(&rules, "explorer.exe", "Downloads", "chrome.exe"), None);
    }

    fn entry(hwnd: isize, since: std::time::Instant) -> FrontWindow {
        FrontWindow { hwnd, since }
    }

    #[test]
    fn the_source_is_the_window_before_the_new_one_while_it_is_recent() {
        let now = std::time::Instant::now();
        let recent = now.checked_sub(Duration::from_millis(500)).unwrap();
        let old = now.checked_sub(Duration::from_secs(5)).unwrap();
        // In front long before, the new window came to the front just now.
        assert_eq!(source_window(&[entry(1, old), entry(2, recent)], 2, 2, now).map(|entry| entry.hwnd), Some(1));
        // Still in front is enough however long ago it came there.
        assert_eq!(source_window(&[entry(1, old)], 4, 1, now).map(|entry| entry.hwnd), Some(1));
        // Left the front long ago: there is no source.
        assert_eq!(source_window(&[entry(1, old), entry(2, old)], 2, 2, now), None);
    }

    #[test]
    fn previous_window_skips_repeats_of_the_window_that_came_to_the_front() {
        let now = std::time::Instant::now();
        let history = [entry(1, now), entry(2, now), entry(1, now)];
        assert_eq!(previous_window(&history, 1).map(|entry| entry.hwnd), Some(2));
        assert_eq!(previous_window(&history, 9), None);
    }
}
