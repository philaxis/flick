//! The order of the windows kept above the others.
//!
//! Windows puts whichever such window was clicked last above all of them.
//! Flick's own are to stay above every one, and of a user's windows, one
//! pinned to every desktop above one pinned to its row, and that above one
//! only kept on top. So once a window is clicked the windows of the kinds
//! above its own are put back over it, each kind keeping its own order:
//! within a kind, the one clicked last stays on top.
//!
//! Plain ordering with no Windows in it.

/// What a window kept above the others is, from the top kind down.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// Flick's own: the board, shown over everything while it is up.
    Flick,
    /// Pinned to every desktop.
    All,
    /// Pinned to its row, following the user from cell to cell.
    Row,
    /// Not pinned, only kept on top from its tile.
    Top,
}

/// The order windows should be in, topmost first, given the order they are
/// in now (topmost first): kinds above kinds, and within a kind the order
/// as it is, which Windows keeps by last click.
pub fn stacked<T: Copy>(now: &[(T, Kind)]) -> Vec<T> {
    let mut order = now.to_vec();
    order.sort_by_key(|(_, kind)| *kind); // stable: keeps the order within a kind
    order.into_iter().map(|(window, _)| window).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_pinned_everywhere_stays_over_one_pinned_to_its_row() {
        // B (row) was clicked after A (everywhere), so Windows put B on top.
        assert_eq!(stacked(&[('B', Kind::Row), ('A', Kind::All)]), ['A', 'B']);
        // Of two pinned to their row, the one clicked last (C) stays on top;
        // Flick's board over all of them.
        assert_eq!(
            stacked(&[('C', Kind::Row), ('A', Kind::All), ('F', Kind::Flick), ('B', Kind::Row)]),
            ['F', 'A', 'C', 'B']
        );
    }
}
