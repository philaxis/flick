//! Bringing a shelf's apps back: starting them one at a time and sending
//! each new window to the cell its app was remembered in.
//!
//! The order and the claiming are worked out here without Windows; the app
//! starts the programs and moves the windows.

use crate::grid::CellId;
use std::collections::{HashSet, VecDeque};

/// Apps are started this far apart, so that their windows come up in the
/// order they were asked for.
pub const LAUNCH_GAP_MS: u64 = 400;
/// An app whose window has not come up by then is given up on.
pub const CLAIM_MS: u64 = 20_000;

/// An app to start for a cell.
#[derive(Clone, Debug, PartialEq)]
pub struct Launch {
    pub cell: CellId,
    pub exe: String,
    pub args: String,
}

/// A started app whose window is still to come up.
#[derive(Clone, Debug, PartialEq)]
pub struct Claim {
    pub cell: CellId,
    /// Executable file name, lower case.
    pub app: String,
    pub since_ms: u64,
}

#[derive(Default)]
pub struct Restore {
    queue: VecDeque<Launch>,
    /// Waiting, in the order they were started.
    claims: Vec<Claim>,
    /// Windows already there when the restore began, or already seen.
    known: HashSet<isize>,
    last_launch_ms: Option<u64>,
}

impl Restore {
    /// Adds apps to start; `known` are the windows already up, never claimed.
    pub fn queue(&mut self, launches: impl IntoIterator<Item = Launch>, known: impl IntoIterator<Item = isize>) {
        self.queue.extend(launches);
        self.known.extend(known);
    }

    /// The next app to start, at most one every `LAUNCH_GAP_MS`. Once it is
    /// taken, its window is waited for.
    pub fn next_launch(&mut self, now_ms: u64) -> Option<Launch> {
        if self.last_launch_ms.is_some_and(|last| now_ms < last + LAUNCH_GAP_MS) {
            return None;
        }
        let launch = self.queue.pop_front()?;
        self.last_launch_ms = Some(now_ms);
        self.claims.push(Claim { cell: launch.cell.clone(), app: crate::grid::exe_file(&launch.exe), since_ms: now_ms });
        Some(launch)
    }

    /// A window (`hwnd`, of executable file name `app`) seen: the cell it
    /// goes to, that of the app's oldest claim, if it is new and one waits.
    pub fn claim(&mut self, hwnd: isize, app: &str) -> Option<CellId> {
        if !self.known.insert(hwnd) {
            return None;
        }
        let at = self.claims.iter().position(|claim| claim.app.eq_ignore_ascii_case(app))?;
        Some(self.claims.remove(at).cell)
    }

    /// The claims given up on.
    pub fn expire(&mut self, now_ms: u64) -> Vec<Claim> {
        let (old, kept) = self.claims.drain(..).partition(|claim| now_ms >= claim.since_ms + CLAIM_MS);
        self.claims = kept;
        old
    }

    /// Nothing left to start or to wait for.
    pub fn is_done(&self) -> bool {
        self.queue.is_empty() && self.claims.is_empty()
    }

    /// Ready for the next restore: the windows seen are forgotten.
    pub fn reset(&mut self) {
        *self = Restore::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launch(cell: &str, exe: &str) -> Launch {
        Launch { cell: cell.into(), exe: format!("C:\\Apps\\{exe}"), args: String::new() }
    }

    #[test]
    fn launches_go_out_one_at_a_time_four_hundred_ms_apart() {
        let mut r = Restore::default();
        r.queue([launch("a", "one.exe"), launch("a", "two.exe"), launch("b", "three.exe")], []);
        assert_eq!(r.next_launch(1000).map(|l| l.exe), Some("C:\\Apps\\one.exe".into()));
        assert_eq!(r.next_launch(1000), None);
        assert_eq!(r.next_launch(1399), None);
        assert_eq!(r.next_launch(1400).map(|l| l.exe), Some("C:\\Apps\\two.exe".into()));
        assert_eq!(r.next_launch(1650), None);
        assert_eq!(r.next_launch(1800).map(|l| l.cell), Some("b".into()));
        assert_eq!(r.next_launch(5000), None);
        assert!(!r.is_done(), "their windows are still waited for");
    }

    #[test]
    fn a_new_window_is_claimed_by_the_oldest_waiting_cell_of_its_app() {
        let mut r = Restore::default();
        r.queue([launch("a", "Code.exe"), launch("b", "code.exe"), launch("c", "wt.exe")], []);
        for t in [0, 400, 800] {
            r.next_launch(t);
        }
        assert_eq!(r.claim(1, "other.exe"), None);
        assert_eq!(r.claim(2, "code.exe").as_deref(), Some("a"));
        assert_eq!(r.claim(2, "code.exe"), None, "a window is claimed once");
        assert_eq!(r.claim(3, "wt.exe").as_deref(), Some("c"));
        assert_eq!(r.claim(4, "code.exe").as_deref(), Some("b"));
        assert!(r.is_done());
    }

    #[test]
    fn claims_expire_after_twenty_seconds_and_restore_ends() {
        let mut r = Restore::default();
        r.queue([launch("a", "slow.exe")], []);
        r.next_launch(100);
        assert!(r.expire(100 + CLAIM_MS - 1).is_empty());
        assert!(!r.is_done());
        let gone = r.expire(100 + CLAIM_MS);
        assert_eq!(gone, [Claim { cell: "a".into(), app: "slow.exe".into(), since_ms: 100 }]);
        assert!(r.is_done());
        assert_eq!(r.claim(9, "slow.exe"), None, "a window coming up late is left where it is");
    }

    #[test]
    fn windows_known_before_the_restore_are_never_claimed() {
        let mut r = Restore::default();
        r.queue([launch("a", "chrome.exe")], [10, 11]);
        r.next_launch(0);
        assert_eq!(r.claim(10, "chrome.exe"), None);
        assert_eq!(r.claim(11, "chrome.exe"), None);
        assert_eq!(r.claim(12, "chrome.exe").as_deref(), Some("a"));
    }
}
