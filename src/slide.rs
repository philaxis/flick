//! Where the hand is along a row, or from row to row: travel one way, and
//! the cell (or row) it comes to.
//!
//! Plain arithmetic with no Windows in it. Sideways the cell is read off
//! where the hand is, counted from the cell it set out from, so the same
//! travel is the same cell whatever the pace it came at and however long the
//! app took over the cells before. Up and down goes by notches instead, as a
//! mouse wheel does (`Detent`), but on the board, where it follows the hand
//! as sideways does (`Rows::Linear`).

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

/// Opposite travel, in tenths of a row's distance, that lets go of what was
/// gathered towards the next row: less is a hand wavering, and only takes
/// that much back.
const TURN_TENTHS: i32 = 1;
/// How much of what goes past a row is kept towards the next, at most, in
/// tenths of a row's distance: enough that a steady hand's rows come evenly
/// spaced, never so much that one fling goes two rows.
const CARRY_TENTHS: i32 = 5;
/// How far the minimap leans towards the next row, at most, before the hand
/// gets there, in rows.
pub const LEAN: f32 = 0.12;

/// Up and down in a hold, the way a mouse wheel goes by its notches: travel
/// is gathered, and a row is come to each time it reaches a whole `step`, no
/// sooner. A row at a time: what went past it is kept towards the next, but
/// never a whole row's worth. Turning back lets go of what was gathered, so a
/// hand going back and forth does not take the row to and fro; at the first
/// or the last row nothing is gathered towards the wall.
#[derive(Clone, Copy, Debug)]
pub struct Detent {
    /// Travel gathered towards the next row, downwards.
    gathered: i32,
    /// Travel since against what was gathered.
    back: i32,
    /// The row come to, counted from the one the hold began on.
    row: i32,
    /// Rows above and below the first, once the app has said.
    room: Option<(i32, i32)>,
}

impl Detent {
    pub const fn new() -> Detent {
        Detent { gathered: 0, back: 0, row: 0, room: None }
    }

    pub fn cell(&self) -> i32 {
        self.row
    }

    /// Travel gathered towards the next row (negative upwards).
    pub fn gathered(&self) -> i32 {
        self.gathered
    }

    /// How far the minimap leans towards the next row, in rows.
    pub fn lean(&self, step: i32) -> f32 {
        self.gathered as f32 / step.max(1) as f32 * LEAN
    }

    pub fn set_room(&mut self, up: i32, down: i32) {
        self.room = Some((up, down));
        self.row = self.row.clamp(-up, down);
        self.keep_off_walls();
    }

    fn keep_off_walls(&mut self) {
        let Some((up, down)) = self.room else { return };
        if (self.gathered < 0 && self.row <= -up) || (self.gathered > 0 && self.row >= down) {
            self.gathered = 0;
        }
    }

    /// The hand moved `dy` (downwards). True when that came to another row.
    pub fn travel(&mut self, dy: i32, step: i32) -> bool {
        let step = step.max(1);
        if self.gathered != 0 && dy.signum() == -self.gathered.signum() {
            self.back = self.back.saturating_add(dy.abs());
            if self.back * 10 >= step * TURN_TENTHS {
                (self.gathered, self.back) = (0, 0);
            } else if dy.abs() >= self.gathered.abs() {
                self.gathered = 0;
            } else {
                self.gathered += dy;
            }
            return false;
        }
        self.back = 0;
        self.gathered = self.gathered.saturating_add(dy);
        self.keep_off_walls();
        if self.gathered.abs() < step {
            return false;
        }
        let way = self.gathered.signum();
        self.row += way;
        let carry = step * CARRY_TENTHS / 10;
        self.gathered = way * (self.gathered.abs() - step).min(carry);
        self.keep_off_walls();
        true
    }
}

/// Up and down in a hold: by notches (`Detent`), or, with the board open,
/// following the hand as sideways does (`Slide`), so the selection goes
/// over the map as far as the hand goes.
#[derive(Clone, Copy, Debug)]
pub enum Rows {
    Notched(Detent),
    Linear(Slide),
}

impl Rows {
    pub fn cell(&self) -> i32 {
        match self {
            Rows::Notched(detent) => detent.cell(),
            Rows::Linear(slide) => slide.cell(),
        }
    }

    /// Travel towards the next row not yet come to (negative upwards).
    pub fn gathered(&self, step: i32) -> i32 {
        match self {
            Rows::Notched(detent) => detent.gathered(),
            Rows::Linear(slide) => slide.rest(step),
        }
    }

    /// How far the minimap leans towards the next row, in rows.
    pub fn lean(&self, step: i32) -> f32 {
        match self {
            Rows::Notched(detent) => detent.lean(step),
            Rows::Linear(slide) => slide.rest(step) as f32 / step.max(1) as f32,
        }
    }

    pub fn set_room(&mut self, up: i32, down: i32, step: i32) {
        match self {
            Rows::Notched(detent) => detent.set_room(up, down),
            Rows::Linear(slide) => slide.set_room(up, down, step),
        }
    }

    /// The hand moved `dy` (downwards). True when that came to another row.
    pub fn travel(&mut self, dy: i32, step: i32) -> bool {
        match self {
            Rows::Notched(detent) => detent.travel(dy, step),
            Rows::Linear(slide) => {
                let before = slide.cell();
                slide.travel(dy, step.max(1), true);
                slide.cell() != before
            }
        }
    }
}

/// Where the hand is both ways in a hold: each way on its own, sideways
/// counting only the hand's sideways travel and up and down only its up and
/// down travel, so a hand going slantwise goes along the row and from row to
/// row at once. Sideways the cell follows the hand (`Slide`); up and down
/// goes by notches (`Detent`), or follows it too on the board (`Rows`). A
/// new row begins a new stretch along it (`Slide::carried_on`).
#[derive(Clone, Copy, Debug)]
pub struct Plane {
    pub cols: Slide,
    pub rows: Rows,
}

/// What one movement came to: another row, another cell along the row.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Moved {
    pub row: bool,
    pub cell: bool,
}

impl Plane {
    pub const fn new() -> Plane {
        Plane { cols: Slide::new(), rows: Rows::Notched(Detent::new()) }
    }

    /// A hold begun with the board open: up and down follows the hand.
    pub const fn linear() -> Plane {
        Plane { cols: Slide::new(), rows: Rows::Linear(Slide::new()) }
    }

    /// The hand moved (`dx`, `dy`). Up and down is taken first, so that
    /// sideways travel coming with a change of row goes on in the new row.
    /// `step`: how far the hand goes for one cell, and for one row.
    pub fn travel(&mut self, dx: i32, dy: i32, (step_x, step_y): (i32, i32)) -> Moved {
        let row = self.rows.travel(dy, step_y);
        if row {
            self.cols = self.cols.carried_on(step_x);
        }
        let cell_before = self.cols.cell();
        self.cols.travel(dx, step_x, true);
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

    /// Up and down goes by notches: short of a whole row's distance the row
    /// stays, at it exactly one row comes, and one fling, however far, is
    /// one row. Turning back lets go of what was gathered; sideways is not
    /// touched by any of it.
    #[test]
    fn rows_come_by_notches() {
        let steps = (260, 400);
        let mut plane = Plane::new();
        assert_eq!(plane.travel(0, 399, steps), Moved::default());
        assert!(plane.travel(0, 1, steps).row);
        assert_eq!(plane.rows.cell(), 1);
        let mut flung = Detent::new();
        assert!(flung.travel(5000, 400));
        assert_eq!(flung.cell(), 1);
        assert!(!flung.travel(199, 400), "what a fling went past one row is not a second");
        // Back and forth short of a row: back to the gathering from nothing.
        let mut turned = Detent::new();
        turned.travel(350, 400);
        turned.travel(-40, 400);
        assert_eq!(turned.gathered(), 0);
        assert!(!turned.travel(350, 400));
        // A waver smaller than that only takes itself back.
        turned.travel(-20, 400);
        assert_eq!(turned.gathered(), 330);
        // No gathering towards a wall, nor past it.
        let mut wall = Detent::new();
        wall.set_room(0, 1);
        assert!(!wall.travel(-900, 400));
        assert_eq!((wall.cell(), wall.gathered()), (0, 0));
        assert!(wall.travel(900, 400));
        assert_eq!((wall.cell(), wall.gathered()), (1, 0));
        // Slantwise: rows by notches, cells along each row as the hand goes.
        let mut slant = Plane::new();
        let (mut rows, mut cells) = (0, 0);
        for _ in 0..160 {
            let moved = slant.travel(5, 5, steps);
            rows += moved.row as i32;
            cells += moved.cell as i32;
        }
        assert_eq!((rows, slant.rows.gathered(steps.1)), (2, 0));
        assert_eq!(cells, 3);
        // Straight down wavering sideways: rows only; straight across: cells only.
        let mut down = Plane::new();
        for i in 0..90 {
            assert!(!down.travel(if i % 2 == 0 { 30 } else { -30 }, 10, steps).cell);
        }
        assert_eq!((down.rows.cell(), down.cols.cell()), (2, 0));
        let mut across = Plane::new();
        for _ in 0..70 {
            assert!(!across.travel(10, 0, steps).row);
        }
        assert_eq!((across.rows.cell(), across.cols.cell()), (0, 3));
    }

    /// On the board up and down follows the hand as sideways does: a row a
    /// little past halfway there, one fling as many rows as it went, and
    /// back again as soon as the hand is nearer the row before.
    #[test]
    fn on_the_board_rows_follow_the_hand() {
        let steps = (260, 400);
        let mut plane = Plane::linear();
        assert!(!plane.travel(0, 239, steps).row);
        assert!(plane.travel(0, 1, steps).row);
        assert_eq!(plane.rows.cell(), 1);
        let mut flung = Plane::linear();
        assert!(flung.travel(0, 1250, steps).row);
        assert_eq!(flung.rows.cell(), 3);
        assert!(flung.travel(0, -300, steps).row);
        assert_eq!(flung.rows.cell(), 2);
        // Against the last row the hand pushes only a little past it.
        let mut wall = Plane::linear();
        wall.rows.set_room(0, 1, steps.1);
        wall.travel(0, 5000, steps);
        assert_eq!(wall.rows.cell(), 1);
        assert!(wall.travel(0, -320, steps).row);
        assert_eq!(wall.rows.cell(), 0);
    }
}
