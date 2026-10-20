//! Where a pinned window is among the windows of the cell it shows on.
//!
//! A window pinned to every shelf or to its own is in front of a cell's own
//! windows or not, as the user left it: brought in front of them on a cell
//! it is "raised", and stays in front of the windows of every cell it goes
//! to; another of the cell's windows brought over it, it is not, and goes
//! right under the front window of every cell it goes to (in front, on a
//! cell with none). What Flick itself brings to the front (focusing a
//! cell come to) leaves this as it was. Windows always on top are another
//! matter, above all of these as they are.
//!
//! A window just pinned is raised, and stays so wherever Flick takes it
//! until the user brings another window to the front.
//!
//! Plain ordering with no Windows in it.

/// Whether a pinned `window` is raised, on a cell whose windows are in
/// `stack` (topmost first, each with whether it is the cell's own, not
/// pinned): in front of all of its own. With none, as it `was`.
pub fn judged<T: PartialEq>(was: bool, window: T, stack: &[(T, bool)]) -> bool {
    let Some(own) = stack.iter().position(|(_, own)| *own) else { return was };
    stack.iter().position(|(other, _)| *other == window).map_or(was, |at| at < own)
}

/// The pinned windows raised, and those of them pinned since the user last
/// brought a window to the front: raised whatever is over them.
pub struct Raised<T> {
    raised: Vec<T>,
    fresh: Vec<T>,
}

impl<T: Copy + PartialEq> Raised<T> {
    pub fn new() -> Self {
        Self { raised: Vec::new(), fresh: Vec::new() }
    }

    pub fn contains(&self, window: T) -> bool {
        self.raised.contains(&window)
    }

    fn set(&mut self, window: T, raised: bool) {
        self.raised.retain(|other| *other != window);
        if raised {
            self.raised.push(window);
        }
    }

    /// `window` was pinned, or pinned another way: raised.
    pub fn pinned(&mut self, window: T) {
        self.set(window, true);
        self.fresh.retain(|other| *other != window);
        self.fresh.push(window);
    }

    pub fn unpinned(&mut self, window: T) {
        self.set(window, false);
        self.fresh.retain(|other| *other != window);
    }

    /// The user brought `window` to the front (not Flick): the ones pinned
    /// before are judged by where they are from now on.
    pub fn brought(&mut self, window: T) {
        self.fresh.retain(|fresh| *fresh == window);
    }

    /// Whether `window` is raised, on a cell whose windows are `stack`
    /// (`judged`).
    pub fn judged(&self, window: T, stack: &[(T, bool)]) -> bool {
        self.fresh.contains(&window) || judged(self.contains(window), window, stack)
    }

    /// Notes whether `window` is raised, the cell it is on about to be left.
    pub fn judge(&mut self, window: T, stack: &[(T, bool)]) {
        let raised = self.judged(window, stack);
        self.set(window, raised);
    }

    /// The windows just pinned, to be put in front again after Flick has
    /// focused another.
    pub fn fresh(&self) -> impl Iterator<Item = T> + '_ {
        self.fresh.iter().copied()
    }

    pub fn retain(&mut self, mut keep: impl FnMut(T) -> bool) {
        self.raised.retain(|window| keep(*window));
        self.fresh.retain(|window| keep(*window));
    }
}

/// Where a pinned window goes on a cell come to whose own windows are
/// `own` (topmost first).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Spot<T> {
    /// In front of them all.
    Front,
    /// Right under this one, the front one.
    Under(T),
}

pub fn spot<T: Copy>(raised: bool, own: &[T]) -> Spot<T> {
    match own.first() {
        Some(&front) if !raised => Spot::Under(front),
        _ => Spot::Front,
    }
}

/// What a window of the picture of the cells (`glide`) is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    /// One of the windows of this cell.
    Own((i32, i32)),
    /// A pinned window where it is among the others, seen on every cell it
    /// passes: raised or not.
    Pinned(bool),
}

/// The order of the windows of the picture, given in their order (bottom
/// first), for each cell to show its pinned windows as it will once come
/// to: the raised ones in front of all, the others right under its front
/// window. On the cell the picture begins on (`origin`) those are under
/// what is in front of them now, the cell as it is.
pub fn pictured<T: Copy>(now: &[(T, Item)], origin: (i32, i32)) -> Vec<T> {
    let under = |at: usize| matches!(now[at].1, Item::Pinned(false));
    let highest = (0..now.len()).rev().find(|&at| under(at));
    let over = |at: usize| match (now[at].1, highest) {
        (Item::Own(cell), Some(highest)) if cell == origin => at > highest,
        (Item::Own(cell), Some(_)) => !now[at + 1..].iter().any(|(_, item)| *item == Item::Own(cell)),
        _ => false,
    };
    let base = (0..now.len()).filter(|&at| matches!(now[at].1, Item::Own(_)) && !over(at));
    let fronts = (0..now.len()).filter(|&at| over(at));
    let raised = (0..now.len()).filter(|&at| now[at].1 == Item::Pinned(true));
    base.chain((0..now.len()).filter(|&at| under(at))).chain(fronts).chain(raised).map(|at| now[at].0).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cell's windows, topmost first, after `window` is put at `spot`.
    fn put(stack: &[(char, bool)], window: char, spot: Spot<char>) -> Vec<(char, bool)> {
        let mut stack: Vec<_> = stack.iter().copied().filter(|(w, _)| *w != window).collect();
        let at = match spot {
            Spot::Front => 0,
            Spot::Under(front) => stack.iter().position(|(w, _)| *w == front).unwrap() + 1,
        };
        stack.insert(at, (window, false));
        stack
    }

    /// Flick comes to a cell of own windows `own` (topmost first) with the
    /// pinned `P`, puts it where it goes, and focuses the cell's front window.
    fn come(raised: bool, own: &[char]) -> Vec<(char, bool)> {
        let stack: Vec<_> = own.iter().map(|&w| (w, true)).collect();
        let mut stack = put(&stack, 'P', spot(raised, own));
        let front = stack.remove(0);
        stack.insert(0, front);
        stack
    }

    #[test]
    fn a_window_the_user_raised_is_in_front_on_every_cell() {
        // Clicked on cell A, P went over A's windows.
        let raised = judged(false, 'P', &[('P', false), ('a', true), ('b', true)]);
        assert!(raised);
        assert_eq!(spot(raised, &['c', 'd']), Spot::Front);
        assert_eq!(come(raised, &['c', 'd']), [('P', false), ('c', true), ('d', true)]);
    }

    #[test]
    fn under_another_window_it_is_second_on_every_cell() {
        // a was clicked on cell A and went over P.
        let raised = judged(true, 'P', &[('a', true), ('P', false), ('b', true)]);
        assert!(!raised);
        assert_eq!(spot(raised, &['c', 'd']), Spot::Under('c'));
        assert_eq!(come(raised, &['c', 'd']), [('c', true), ('P', false), ('d', true)]);
        // On a cell with nothing of its own, it simply shows.
        assert_eq!(spot::<char>(raised, &[]), Spot::Front);
    }

    #[test]
    fn flick_going_from_cell_to_cell_leaves_it_as_it_was() {
        for raised in [false, true] {
            let mut now = raised;
            for own in [&['c', 'd'][..], &['e'], &[], &['f', 'g', 'h']] {
                // Come to the cell, its front window focused, then left.
                now = judged(now, 'P', &come(now, own));
                assert_eq!(now, raised);
            }
        }
        // Nothing of the cell's own to judge by: as it was.
        assert!(judged(true, 'P', &[('P', false), ('Q', false)]));
        assert!(!judged(false, 'P', &[('Q', false), ('P', false)]));
    }

    #[test]
    fn a_window_just_pinned_is_raised_until_the_user_brings_another_over_it() {
        let mut raised = Raised::new();
        raised.pinned('P');
        assert!(raised.contains('P'));
        // Pinned from the board, which then focused a's window of the cell
        // gone to, over P: Flick's doing, P stays raised.
        let stack = [('a', true), ('P', false), ('b', true)];
        raised.judge('P', &stack);
        assert!(raised.contains('P'));
        // Flick going from cell to cell leaves it raised.
        for own in [&['c', 'd'][..], &['e'], &[]] {
            let stack = come(raised.contains('P'), own);
            raised.judge('P', &stack);
            assert!(raised.contains('P'));
        }
        // P itself brought to the front changes nothing.
        raised.brought('P');
        raised.judge('P', &stack);
        assert!(raised.contains('P'));
        // The user brought a over it.
        raised.brought('a');
        raised.judge('P', &stack);
        assert!(!raised.contains('P'));
        assert_eq!(spot(raised.contains('P'), &['c', 'd']), Spot::Under('c'));
        // Pinned another way, raised again; unpinned, forgotten.
        raised.pinned('P');
        raised.judge('P', &stack);
        assert!(raised.contains('P'));
        raised.unpinned('P');
        assert!(!raised.contains('P'));
        assert_eq!(raised.fresh().count(), 0);
    }

    #[test]
    fn the_picture_shows_each_cell_as_it_will_be() {
        use Item::*;
        let (a, b) = ((0, 0), (0, 1));
        // Bottom first: on A, a2 under P under a1; on B, b2 under b1.
        let now = [('b', Own(b)), ('B', Own(b)), ('x', Own(a)), ('P', Pinned(false)), ('y', Own(a))];
        // P over b, under B, B's front; on A where it is.
        assert_eq!(pictured(&now, a), ['b', 'x', 'P', 'B', 'y']);
        // Raised, it is in front of all.
        let now = [('b', Own(b)), ('P', Pinned(true)), ('B', Own(b)), ('x', Own(a))];
        assert_eq!(pictured(&now, a), ['b', 'B', 'x', 'P']);
        // With nothing pinned the order is as it is.
        let now = [('B', Own(b)), ('x', Own(a)), ('b', Own(b))];
        assert_eq!(pictured(&now, a), ['B', 'x', 'b']);
    }
}
