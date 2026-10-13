//! Where the hand is along a row, or from row to row: travel one way, and
//! the cell (or row) it comes to.
//!
//! Plain arithmetic with no Windows in it. The cell is read off where the
//! hand is, counted from the cell it set out from, so the same travel is the
//! same cell whatever the pace it came at and however long the app took over
//! the cells before: nothing is counted up to be let go of later.

/// A cell is come to once the hand is this much nearer it than the one
/// before, in tenths of the way between them: a little past halfway, so that
/// a hand resting at the half does not flip to and fro.
const STEP_AT_TENTHS: i32 = 6;
/// How far past the last cell of a row the hand can push, in tenths of a cell.
const WALL_GIVE_TENTHS: i32 = 2;

/// One stretch of sideways travel: from the trigger going down, or from the
/// last change of row, on. Distances are in the mouse's own counts; `step`
/// is how many of them lie between two cells.
#[derive(Clone, Copy, Debug)]
pub struct Slide {
    /// How far the hand is from the cell it set out from, rightwards.
    x: i32,
    /// The cell it has come to, counted from that one likewise.
    cell: i32,
    /// How many cells the row has to the left and to the right of the cell
    /// set out from, once the app has said.
    room: Option<(i32, i32)>,
}

impl Slide {
    pub const fn new() -> Slide {
        Slide { x: 0, cell: 0, room: None }
    }

    pub fn cell(&self) -> i32 {
        self.cell
    }

    /// How far the hand is from the cell it has come to.
    pub fn rest(&self, step: i32) -> i32 {
        self.x - self.cell * step
    }

    /// The app has found how long the row is. A hand that went past an end
    /// of it meanwhile is taken back to that end, to push against.
    pub fn set_room(&mut self, left: i32, right: i32, step: i32) {
        self.room = Some((left, right));
        self.keep_in_row(step);
    }

    fn keep_in_row(&mut self, step: i32) {
        let Some((left, right)) = self.room else { return };
        let give = step * WALL_GIVE_TENTHS / 10;
        self.x = self.x.clamp(-left * step - give, right * step + give);
        self.cell = self.cell.clamp(-left, right);
    }

    /// The hand moved `dx`. With `may_step` the cell follows it: the first
    /// is come to a little past halfway there and each further one a whole
    /// `step` on, so the cell is always the one the hand is nearest to.
    pub fn travel(&mut self, dx: i32, step: i32, may_step: bool) {
        if let Some((left, right)) = self.room {
            // Coming away from the end of the row counts from the last cell
            // itself, not from how far the hand had pushed past it.
            let (first, last) = (-left * step, right * step);
            if self.x > last && dx < 0 {
                self.x = last;
            } else if self.x < first && dx > 0 {
                self.x = first;
            }
        }
        self.x = self.x.saturating_add(dx);
        self.keep_in_row(step);
        while may_step && self.rest(step).abs() * 10 >= step * STEP_AT_TENTHS {
            self.cell += self.rest(step).signum();
        }
    }

    /// A new stretch from the cell this one has come to, the hand as far
    /// on from it as it is now from this one: what it went towards the next
    /// cell is not lost, and no cell come to here is come to again.
    pub fn carried_on(&self, step: i32) -> Slide {
        Slide { x: self.rest(step), cell: 0, room: None }
    }
}

/// Where the hand is both ways in a hold: each way on its own, sideways
/// counting only the hand's sideways travel and up and down only its up and
/// down travel, so a hand going slantwise goes along the row and from row to
/// row at once. A new row begins a new stretch along it (`Slide::carried_on`).
#[derive(Clone, Copy, Debug)]
pub struct Plane {
    pub cols: Slide,
    pub rows: Slide,
}

/// What one movement came to: another row, another cell along the row.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Moved {
    pub row: bool,
    pub cell: bool,
}

impl Plane {
    pub const fn new() -> Plane {
        Plane { cols: Slide::new(), rows: Slide::new() }
    }

    /// The hand moved (`dx`, `dy`). Up and down is taken first, so that
    /// sideways travel coming with a change of row goes on in the new row.
    pub fn travel(&mut self, dx: i32, dy: i32, step: i32) -> Moved {
        let row_before = self.rows.cell();
        self.rows.travel(dy, step, true);
        let row = self.rows.cell() != row_before;
        if row {
            self.cols = self.cols.carried_on(step);
        }
        let cell_before = self.cols.cell();
        self.cols.travel(dx, step, true);
        Moved { row, cell: self.cols.cell() != cell_before }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn after(moves: impl IntoIterator<Item = i32>) -> i32 {
        let mut slide = Slide::new();
        for dx in moves {
            slide.travel(dx, 260, true);
        }
        slide.cell()
    }

    /// The cell depends on how far the hand went and on nothing else: in one
    /// sweep, count by count, or wavering on the way, the same travel is the
    /// same cell, each cell is as far on as the last, and what is left over
    /// from one cell goes towards the next.
    #[test]
    fn the_same_travel_is_the_same_cell_however_it_comes() {
        for (travel, cell) in [(155, 0), (156, 1), (415, 1), (416, 2), (675, 2), (676, 3), (700, 3)] {
            assert_eq!(after([travel]), cell, "{travel} in one sweep");
            assert_eq!(after(std::iter::repeat(1).take(travel as usize)), cell, "{travel} count by count");
            assert_eq!(after([-travel]), -cell, "{travel} leftwards");
        }
        let wavering = (0..34).flat_map(|_| [30, -10]).chain([20]);
        assert_eq!(after(wavering), 3);
        // Back again: a cell is left as it was come to, a little past halfway.
        assert_eq!(after([416, -51]), 2);
        assert_eq!(after([416, -52]), 1);
    }

    /// A hand flung past the end of the row before the app could say where
    /// the row ends is where it would have been had that been known all
    /// along: on the last cell, never on one asked for and then taken back.
    #[test]
    fn the_end_of_the_row_is_the_same_whenever_it_is_learnt() {
        let mut known = Slide::new();
        known.set_room(0, 1, 260);
        known.travel(700, 260, true);
        let mut late = Slide::new();
        late.travel(700, 260, true);
        assert_eq!(late.cell(), 3);
        late.set_room(0, 1, 260);
        for slide in [&mut known, &mut late] {
            assert_eq!((slide.cell(), slide.rest(260)), (1, 52));
            slide.travel(-155, 260, true);
            assert_eq!(slide.cell(), 1);
            slide.travel(-1, 260, true);
            assert_eq!(slide.cell(), 0);
        }
        // Turning back before the end was known asks for no cell to the left.
        let mut turned = Slide::new();
        turned.travel(700, 260, true);
        turned.travel(-200, 260, true);
        turned.set_room(0, 1, 260);
        assert_eq!(turned.cell(), 1);
    }

    /// A hand going slantwise goes from row to row and along the row at
    /// once, each as far as it would going that way alone; a hand going one
    /// way, wavering a little, moves the other way not at all.
    #[test]
    fn each_way_follows_the_hand_on_its_own() {
        // Two cells right and two rows down, in small slantwise movements.
        let mut slant = Plane::new();
        let (mut rows, mut cells) = (0, 0);
        for _ in 0..104 {
            let moved = slant.travel(5, 5, 260);
            rows += moved.row as i32;
            cells += moved.cell as i32;
        }
        assert_eq!(slant.rows.cell(), 2);
        assert_eq!(rows, 2);
        // The new rows began new stretches; what was gone along each one and
        // towards the next cell was kept, so the cells add up to the travel.
        assert_eq!(cells, 2);
        // The last row was come to with the hand past the first cell: the
        // stretch along it began one cell on.
        assert_eq!((slant.cols.cell(), slant.cols.rest(260)), (1, 0));
        // Straight down, wavering sideways: rows only.
        let mut down = Plane::new();
        for i in 0..70 {
            assert!(!down.travel(if i % 2 == 0 { 30 } else { -30 }, 10, 260).cell);
        }
        assert_eq!((down.rows.cell(), down.cols.cell()), (3, 0));
        // Straight across: cells only.
        let mut across = Plane::new();
        for _ in 0..70 {
            assert!(!across.travel(10, 0, 260).row);
        }
        assert_eq!((across.rows.cell(), across.cols.cell()), (0, 3));
    }
}
