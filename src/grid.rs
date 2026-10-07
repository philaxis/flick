//! The 2D layout laid over Windows' flat list of virtual desktops.
//!
//! A row is a workspace; its cells are that workspace's screens. Rows may have
//! different lengths. Cells are identified by the desktop GUID (as a string),
//! so this module has no Windows dependency.

use serde::{Deserialize, Serialize};

pub type CellId = String;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pos {
    pub row: usize,
    pub col: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Row {
    #[serde(default)]
    pub name: String,
    pub cells: Vec<CellId>,
    /// The cell this row was last left on. Stored by id so that inserting,
    /// removing or reordering cells cannot make it point at the wrong one.
    #[serde(default)]
    pub last: Option<CellId>,
}

impl Row {
    /// A new, unnamed row holding one cell.
    pub fn with_cell(id: CellId) -> Row {
        Row { name: String::new(), cells: vec![id], last: None }
    }

    /// The column a vertical move into this row lands on: the cell it was
    /// last left on, or the first one if it was never visited.
    pub fn anchor(&self) -> usize {
        self.last.as_ref().and_then(|id| self.cells.iter().position(|c| c == id)).unwrap_or(0)
    }
}

/// What a row is called: its name, or its number while the user has not
/// given it one.
pub fn row_title(name: &str, index: usize) -> String {
    if name.is_empty() {
        format!("워크스페이스 {}", index + 1)
    } else {
        name.to_owned()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Grid {
    pub rows: Vec<Row>,
    /// Cells created by pushing past an edge that have not held a window yet.
    /// They are removed again when left empty.
    #[serde(default)]
    pub ephemeral: Vec<CellId>,
    /// Executable names (lower case) whose windows follow the user from cell
    /// to cell inside whichever row they are in.
    #[serde(default)]
    pub follow_apps: Vec<String>,
}

impl Grid {
    pub fn find(&self, id: &str) -> Option<Pos> {
        self.rows.iter().enumerate().find_map(|(row, r)| {
            r.cells.iter().position(|c| c == id).map(|col| Pos { row, col })
        })
    }

    pub fn id_at(&self, pos: Pos) -> Option<&CellId> {
        self.rows.get(pos.row)?.cells.get(pos.col)
    }

    /// Where a move from `from` lands, or `None` at the edge of the grid.
    /// Vertical moves go to the target row's `anchor`.
    pub fn target(&self, from: Pos, dir: Dir) -> Option<Pos> {
        let row = self.rows.get(from.row)?;
        match dir {
            Dir::Left => from.col.checked_sub(1).map(|col| Pos { row: from.row, col }),
            Dir::Right => (from.col + 1 < row.cells.len()).then(|| Pos { row: from.row, col: from.col + 1 }),
            Dir::Up | Dir::Down => {
                let r = if dir == Dir::Up { from.row.checked_sub(1)? } else { from.row + 1 };
                Some(Pos { row: r, col: self.rows.get(r)?.anchor() })
            }
        }
    }

    pub fn visit(&mut self, id: &str) {
        if let Some(pos) = self.find(id) {
            self.rows[pos.row].last = Some(id.to_owned());
        }
    }

    /// Inserts a newly created cell next to `from` in direction `dir`:
    /// sideways it joins the same row, vertically it starts a new row.
    pub fn insert_beside(&mut self, from: Pos, dir: Dir, id: CellId) {
        match dir {
            Dir::Left => self.rows[from.row].cells.insert(from.col, id),
            Dir::Right => self.rows[from.row].cells.insert(from.col + 1, id),
            Dir::Up => self.rows.insert(from.row, Row::with_cell(id)),
            Dir::Down => self.rows.insert(from.row + 1, Row::with_cell(id)),
        }
    }

    /// Moves a cell to `index` of `row`, where `index` counts the row's cells
    /// without the moved one. A row left empty disappears.
    pub fn move_cell(&mut self, id: &str, row: usize, index: usize) {
        let Some(from) = self.find(id) else { return };
        if row >= self.rows.len() {
            return;
        }
        self.rows[from.row].cells.remove(from.col);
        let cells = &mut self.rows[row].cells;
        cells.insert(index.min(cells.len()), id.to_owned());
        self.rows.retain(|r| !r.cells.is_empty());
    }

    /// Moves a cell into a new row inserted before row `at`.
    pub fn move_cell_to_new_row(&mut self, id: &str, at: usize) {
        let Some(from) = self.find(id) else { return };
        self.rows[from.row].cells.remove(from.col);
        let at = at.min(self.rows.len());
        self.rows.insert(at, Row::with_cell(id.to_owned()));
        self.rows.retain(|r| !r.cells.is_empty());
    }

    /// How far `row` is drawn shifted right, in cells: rows are shifted so
    /// that their anchors sit in one common column.
    fn shift(&self, row: usize) -> i64 {
        let lead = self.rows.iter().map(Row::anchor).max().unwrap_or(0);
        self.rows.get(row).map_or(0, |r| (lead - r.anchor()) as i64)
    }

    /// The cell outside `pos`'s row that is closest to it as the grid is
    /// drawn: a neighbouring row before a farther one, then the smallest
    /// sideways distance, the row above before the row below, the left cell
    /// before the right. This is where a row's windows go when it is removed.
    pub fn nearest_in_other_rows(&self, pos: Pos) -> Option<&CellId> {
        let x = pos.col as i64 + self.shift(pos.row);
        self.rows
            .iter()
            .enumerate()
            .filter(|(r, _)| *r != pos.row)
            .flat_map(|(r, row)| {
                let shift = self.shift(r);
                row.cells.iter().enumerate().map(move |(c, id)| {
                    let (rows_apart, cells_apart) = ((r as i64 - pos.row as i64).abs(), (c as i64 + shift - x).abs());
                    ((rows_apart, cells_apart, r > pos.row, c), id)
                })
            })
            .min_by_key(|(key, _)| *key)
            .map(|(_, id)| id)
    }

    pub fn remove(&mut self, id: &str) {
        for r in &mut self.rows {
            r.cells.retain(|c| c != id);
        }
        self.rows.retain(|r| !r.cells.is_empty());
        self.ephemeral.retain(|c| c != id);
    }

    /// Reconciles the grid with the desktops that actually exist. Desktops that
    /// vanished are dropped; ones created outside this app (Task View,
    /// Win+Ctrl+D) join the end of the current row. Returns whether anything
    /// changed.
    pub fn sync(&mut self, actual: &[CellId], current: &str) -> bool {
        let before = self.clone();
        for r in &mut self.rows {
            r.cells.retain(|c| actual.contains(c));
        }
        self.rows.retain(|r| !r.cells.is_empty());
        self.ephemeral.retain(|c| actual.contains(c));

        let unknown: Vec<CellId> = actual.iter().filter(|id| self.find(id).is_none()).cloned().collect();
        if !unknown.is_empty() {
            // `current` may itself be one of the unknown desktops; then they all
            // go to the first row.
            let row = self.find(current).map(|p| p.row).unwrap_or(0);
            if self.rows.is_empty() {
                self.rows.push(Row::default());
            }
            self.rows[row].cells.extend(unknown);
        }
        *self != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(rows: &[&[&str]]) -> Grid {
        Grid {
            rows: rows
                .iter()
                .map(|cells| Row { cells: cells.iter().map(|c| c.to_string()).collect(), ..Row::default() })
                .collect(),
            ..Grid::default()
        }
    }

    fn go(g: &mut Grid, from: &str, dir: Dir) -> Option<String> {
        let to = g.id_at(g.target(g.find(from)?, dir)?)?.clone();
        g.visit(&to);
        Some(to)
    }

    /// Rows of unequal length: going down lands on the first column of a row
    /// never visited, and coming back returns to the column that was left,
    /// not to the same column index.
    #[test]
    fn vertical_moves_use_each_rows_remembered_column() {
        let mut g = grid(&[&["a1", "a2", "a3"], &["b1", "b2"]]);
        g.visit("a3");
        assert_eq!(go(&mut g, "a3", Dir::Down).as_deref(), Some("b1"));
        assert_eq!(go(&mut g, "b1", Dir::Right).as_deref(), Some("b2"));
        assert_eq!(go(&mut g, "b2", Dir::Up).as_deref(), Some("a3"));
        assert_eq!(go(&mut g, "a3", Dir::Down).as_deref(), Some("b2"));
        assert_eq!(g.target(g.find("b2").unwrap(), Dir::Right), None);
        assert_eq!(g.target(g.find("b2").unwrap(), Dir::Down), None);
    }

    /// The remembered column must survive cells being inserted before it and
    /// must fall back to the first column when that desktop is deleted.
    #[test]
    fn remembered_column_follows_the_desktop_not_the_index() {
        let mut g = grid(&[&["a1", "a2"], &["b1"]]);
        g.visit("a2");
        g.insert_beside(Pos { row: 0, col: 0 }, Dir::Left, "a0".into());
        assert_eq!(go(&mut g, "b1", Dir::Up).as_deref(), Some("a2"));

        let actual: Vec<String> = ["a0", "a1", "b1", "new"].iter().map(|s| s.to_string()).collect();
        assert!(g.sync(&actual, "b1"));
        assert_eq!(g.rows[1].cells, ["b1", "new"]);
        assert_eq!(go(&mut g, "b1", Dir::Up).as_deref(), Some("a0"));
    }

    /// Dragging a cell: the drop index is counted without the dragged cell, so
    /// a move to the right inside the same row must not land one too far, and
    /// a row emptied by the move must vanish.
    #[test]
    fn moving_cells_between_and_within_rows() {
        let mut g = grid(&[&["a", "b", "c"], &["d"]]);
        g.move_cell("a", 0, 1);
        assert_eq!(g.rows[0].cells, ["b", "a", "c"]);
        g.move_cell("d", 0, 3);
        assert_eq!(g.rows.len(), 1);
        assert_eq!(g.rows[0].cells, ["b", "a", "c", "d"]);
        g.move_cell_to_new_row("a", 0);
        assert_eq!(g.rows[0].cells, ["a"]);
        assert_eq!(g.rows[1].cells, ["b", "c", "d"]);
    }

    /// Removing a workspace: each of its cells sends its windows to the cell
    /// drawn closest to it, a neighbouring row winning over a nearer cell
    /// two rows away and the row above winning a tie.
    #[test]
    fn a_removed_rows_cells_go_to_the_nearest_cell() {
        let mut g = grid(&[&["a1", "a2", "a3"], &["b1", "b2"], &["c1"]]);
        // Rows are drawn lined up on their last-visited cells: a3, b1 and c1 share a column.
        g.visit("a3");
        g.visit("b1");
        let nearest = |g: &Grid, id: &str| g.nearest_in_other_rows(g.find(id).unwrap()).cloned();
        assert_eq!(nearest(&g, "b1").as_deref(), Some("a3"));
        assert_eq!(nearest(&g, "b2").as_deref(), Some("a3"));
        assert_eq!(nearest(&g, "c1").as_deref(), Some("b1"));
        assert_eq!(nearest(&g, "a1").as_deref(), Some("b1"));
    }
}
