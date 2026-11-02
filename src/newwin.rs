//! Where new windows go: rules stacked one by one by the user. A rule says
//! that the new windows a window of one program opens (its title holding a
//! word) are put on a cell and a monitor. The newest rule that fits wins.
//! New windows of the same program as the one they came from are left
//! alone, as are new windows no rule fits.

use crate::grid::CellId;
use serde::{Deserialize, Serialize};

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
}
