//! The 2D layout laid over Windows' flat list of virtual desktops.
//!
//! A row is a shelf; its cells are that shelf's screens. Rows may have
//! different lengths. Cells are identified by the desktop GUID (as a string),
//! so this module has no Windows dependency.

use crate::usage::{self, CellUsage};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

pub type CellId = String;

/// How many windows of a cell are remembered.
pub const MEMORY_MOST: usize = 12;
/// How long the memory of a cell no longer in the grid is kept (a week).
pub const MEMORY_KEEP_SECS: u64 = 7 * 24 * 3600;
/// Windows of these are not remembered: started again, Explorer opens only
/// its home folder, and a store app needs its own id (`remembered`).
const NOT_REMEMBERED: [&str; 2] = ["explorer.exe", "applicationframehost.exe"];

/// A window that was in a cell: enough to start its app again.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AppMemory {
    /// Full path of the executable.
    pub exe: String,
    /// Command line beyond the exe; empty for now.
    #[serde(default)]
    pub args: String,
    /// For the hint line; may be empty.
    #[serde(default)]
    pub title: String,
    /// The app's name as people know it.
    #[serde(default)]
    pub name: String,
}

impl AppMemory {
    /// The executable's file name, lower case ("chrome.exe"), as
    /// `vd::exe_name` has it.
    pub fn app(&self) -> String {
        exe_file(&self.exe)
    }
}

/// The file name of an executable's path, lower case.
pub fn exe_file(path: &str) -> String {
    path.rsplit(['\\', '/']).next().unwrap_or_default().to_lowercase()
}

/// The windows a cell had, to bring back after a restart.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CellMemory {
    /// Unix seconds when the cell last had live windows.
    pub taken: u64,
    /// One entry per window, topmost first, at most `MEMORY_MOST`.
    pub apps: Vec<AppMemory>,
}

/// A live window as remembering sees it.
pub struct Seen {
    /// Full path of its executable, when it could be read.
    pub exe: Option<String>,
    pub title: String,
    pub name: String,
    /// Pinned to every shelf or to its own: not a cell's.
    pub pinned: bool,
}

/// What of a cell's windows (topmost first) is remembered: those of an app
/// that can be started again by its executable, not pinned ones.
pub fn remembered(windows: impl IntoIterator<Item = Seen>) -> Vec<AppMemory> {
    windows
        .into_iter()
        .filter(|w| !w.pinned)
        .filter_map(|w| {
            let exe = w.exe?;
            (!NOT_REMEMBERED.contains(&exe_file(&exe).as_str())).then(|| AppMemory { exe, args: String::new(), title: w.title, name: w.name })
        })
        .collect()
}

/// The remembered apps not alive again: `memory`'s windows less, app by
/// app, as many as `live` (executable file names) has of each.
pub fn ghosts<'a>(memory: &'a CellMemory, live: &[String]) -> Vec<&'a AppMemory> {
    let mut left: Vec<&String> = live.iter().collect();
    memory
        .apps
        .iter()
        .filter(|app| {
            let name = app.app();
            match left.iter().position(|l| l.eq_ignore_ascii_case(&name)) {
                Some(at) => {
                    left.remove(at);
                    false
                }
                None => true,
            }
        })
        .collect()
}

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

    /// Keeps only the cells `keep` holds to. Where the one the row was left
    /// on goes, the row stays turned about there: on the kept cell before
    /// it, or else the one after.
    fn retain(&mut self, keep: impl Fn(&CellId) -> bool) {
        if let Some(at) = self.last.as_ref().and_then(|id| self.cells.iter().position(|c| c == id)) {
            if !keep(&self.cells[at]) {
                self.last = self.cells[..at].iter().rev().chain(&self.cells[at + 1..]).find(|c| keep(c)).cloned();
            }
        }
        self.cells.retain(|c| keep(c));
    }
}

/// What a row is called: its name, or its number while the user has not
/// given it one.
pub fn row_title(name: &str, index: usize) -> String {
    if name.is_empty() {
        format!("Shelf {}", index + 1)
    } else {
        name.to_owned()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Grid {
    pub rows: Vec<Row>,
    /// Decayed active-stay credits, keyed by desktop id.
    #[serde(default)]
    pub usage: BTreeMap<CellId, CellUsage>,
    /// Individual live windows, shared with the board but never saved.
    #[serde(skip)]
    pub window_usage: BTreeMap<isize, usage::Score>,
    /// Executable names (lower case) whose windows follow the user from cell
    /// to cell inside whichever row they are in.
    #[serde(default)]
    pub follow_apps: Vec<String>,
    /// Cells the user marked to stand out in the map and the minimap.
    #[serde(default)]
    pub emphasised: Vec<CellId>,
    /// Windows this app put above the others (handle and executable name),
    /// so that it takes back only what it did itself, also after a restart.
    #[serde(default)]
    pub kept_on_top: Vec<(isize, String)>,
    /// Of those, the ones put up as they were pinned, and pinned ones the
    /// user took down (`pin::PinTop`).
    #[serde(default)]
    pub pin_top: crate::pin::PinTop,
    /// The window showing how Flick is used was closed once: it is not
    /// shown by itself again.
    #[serde(default)]
    pub guide_seen: bool,
    /// The board was opened once: what can be done there is no longer
    /// said at its bottom unless asked for (?).
    #[serde(default)]
    pub board_hints_seen: bool,
    /// What was in each cell, to bring back after a restart. Kept for
    /// cells that went (`sync` leaves it) until `prune_memory`.
    #[serde(default)]
    pub memory: BTreeMap<CellId, CellMemory>,
}

impl Grid {
    /// Only completing the first guide earns the tray-location notification.
    pub fn complete_guide(&mut self) -> bool {
        !std::mem::replace(&mut self.guide_seen, true)
    }

    /// Drops removed desktops and credits too old to affect the display.
    pub fn prune_usage(&mut self, now: u64) {
        self.usage.retain(|id, usage| {
            self.rows.iter().any(|row| row.cells.contains(id))
                && usage.visits.value(now, usage::CELL_HALF_LIFE) > 0.0
        });
    }

    /// Takes `apps` (topmost first) as what cell `id` holds. Says whether
    /// that is other than it was.
    pub fn remember(&mut self, id: &str, mut apps: Vec<AppMemory>, now: u64) -> bool {
        apps.truncate(MEMORY_MOST);
        let memory = self.memory.entry(id.to_owned()).or_default();
        memory.taken = now;
        let changed = memory.apps != apps;
        memory.apps = apps;
        changed
    }

    pub fn forget(&mut self, ids: &[CellId]) {
        for id in ids {
            self.memory.remove(id);
        }
    }

    /// Drops the memory of cells no longer in the grid once a week old.
    pub fn prune_memory(&mut self, now: u64) {
        let rows = &self.rows;
        self.memory.retain(|id, memory| {
            rows.iter().any(|row| row.cells.contains(id)) || now.saturating_sub(memory.taken) <= MEMORY_KEEP_SECS
        });
    }

    /// Brings the memory up to what the cells hold now (`live`, each cell's
    /// remembered windows): a cell with windows is remembered as it is,
    /// one without keeps what it had. A cell still waiting for its apps to
    /// come back (`ghosting`) keeps its memory until every one has, and
    /// then waits no more. Says whether any memory changed.
    pub fn update_memory(&mut self, live: &BTreeMap<CellId, Vec<AppMemory>>, ghosting: &mut HashSet<CellId>, now: u64) -> bool {
        let mut changed = false;
        let ids: Vec<CellId> = self.rows.iter().flat_map(|row| row.cells.iter().cloned()).collect();
        for id in ids {
            let apps = live.get(&id).cloned().unwrap_or_default();
            if ghosting.contains(&id) {
                let names: Vec<String> = apps.iter().map(AppMemory::app).collect();
                if self.memory.get(&id).is_some_and(|memory| !ghosts(memory, &names).is_empty()) {
                    continue;
                }
                ghosting.remove(&id);
            }
            if !apps.is_empty() {
                changed |= self.remember(&id, apps, now);
            }
        }
        self.prune_memory(now);
        changed
    }

    /// Brightness uses the square root of the share of the highest score.
    pub fn cell_usage(&self, id: &str, now: u64) -> f32 {
        let maximum = self.usage.values().map(|cell| cell.visits.value(now, usage::CELL_HALF_LIFE)).fold(0.0, f64::max);
        let score = self.usage.get(id).map_or(0.0, |cell| cell.visits.value(now, usage::CELL_HALF_LIFE));
        usage::normalised(score, maximum).sqrt()
    }

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

    /// The cell `by` cells to the right of `origin` in its row (to the left
    /// when negative), or the end of the row where it is not that long; and
    /// how many cells the row has to the left and to the right of `origin`.
    pub fn along(&self, origin: &str, by: i32) -> Option<(Pos, (usize, usize))> {
        let from = self.find(origin)?;
        let last = self.rows[from.row].cells.len() - 1;
        let col = (from.col as i32).saturating_add(by).clamp(0, last as i32) as usize;
        Some((Pos { row: from.row, col }, (from.col, last - from.col)))
    }

    /// The row `by` rows below row `origin` (above when negative), or the
    /// first or last row where there are not that many, at the cell a move
    /// there lands on (`Row::anchor`); and how many rows there are above and
    /// below `origin`.
    pub fn across(&self, origin: usize, by: i32) -> Option<(Pos, (usize, usize))> {
        let last = self.rows.len().checked_sub(1)?;
        let origin = origin.min(last);
        let row = (origin as i32).saturating_add(by).clamp(0, last as i32) as usize;
        Some((Pos { row, col: self.rows[row].anchor() }, (origin, last - origin)))
    }

    /// The cell `col` cells along and `row` rows down from `from` (up and
    /// to the left when negative), as a picture of a hold counts them:
    /// along `from`'s row from `from`, along another from the cell a move
    /// there lands on (`Row::anchor`).
    pub fn counted(&self, from: Pos, (col, row): (i32, i32)) -> Option<Pos> {
        let at = usize::try_from(from.row as i32 + row).ok()?;
        let line = self.rows.get(at)?;
        let reference = if row == 0 { from.col } else { line.anchor() };
        let col = usize::try_from(reference as i32 + col).ok().filter(|&col| col < line.cells.len())?;
        Some(Pos { row: at, col })
    }

    /// The rows a hold from `from` goes to, and each one's cells, as
    /// `counted` counts them: only its own row, or its own cell of each,
    /// the ways (`sideways`, `across`) it does not go.
    pub fn reach(&self, from: Pos, sideways: bool, across: bool) -> ((i32, i32), Vec<(i32, i32)>) {
        let last = self.rows.len() as i32 - 1;
        let rows = if across { (-(from.row as i32), last - from.row as i32) } else { (0, 0) };
        let cols = (rows.0..=rows.1)
            .map(|row| {
                let line = &self.rows[(from.row as i32 + row) as usize];
                let reference = if row == 0 { from.col } else { line.anchor() } as i32;
                if sideways { (-reference, line.cells.len() as i32 - 1 - reference) } else { (0, 0) }
            })
            .collect();
        (rows, cols)
    }

    /// The rows a hold from `from` turned stay turned, as a cube's do: each
    /// is left on the cell `turns` has for it, counted as `counted` does.
    /// True when one changed.
    pub fn keep_turns(&mut self, from: Pos, turns: &[(i32, i32)]) -> bool {
        let cells: Vec<CellId> =
            turns.iter().filter(|turn| turn.0 != 0).filter_map(|&turn| self.id_at(self.counted(from, turn)?).cloned()).collect();
        let before: Vec<usize> = self.rows.iter().map(Row::anchor).collect();
        for id in &cells {
            self.visit(id);
        }
        self.rows.iter().map(Row::anchor).ne(before)
    }

    /// The cell a hold from `from` ends on, `to` counted as `counted` does
    /// before the rows it turned (`turns`) are kept turned: counted after,
    /// a turned row's cells would be counted twice. True when a row changed.
    pub fn land(&mut self, from: Pos, to: (i32, i32), turns: &[(i32, i32)]) -> (Option<CellId>, bool) {
        let id = self.counted(from, to).and_then(|at| self.id_at(at).cloned());
        (id, self.keep_turns(from, turns))
    }

    pub fn visit(&mut self, id: &str) {
        if let Some(pos) = self.find(id) {
            self.rows[pos.row].last = Some(id.to_owned());
        }
    }

    /// Moves a cell to `index` of `row`, where `index` counts the row's cells
    /// without the moved one. A row left empty disappears.
    pub fn move_cell(&mut self, id: &str, row: usize, index: usize) {
        let Some(from) = self.find(id) else { return };
        if row >= self.rows.len() {
            return;
        }
        if from.row == row {
            self.rows[row].cells.remove(from.col);
        } else {
            self.rows[from.row].retain(|c| c != id);
        }
        let cells = &mut self.rows[row].cells;
        cells.insert(index.min(cells.len()), id.to_owned());
        self.rows.retain(|r| !r.cells.is_empty());
    }

    /// Moves row `from` to before row `at`, where `at` counts the rows
    /// without the moved one.
    pub fn move_row(&mut self, from: usize, at: usize) {
        if from < self.rows.len() {
            let row = self.rows.remove(from);
            self.rows.insert(at.min(self.rows.len()), row);
        }
    }

    /// Gives every row without a name one: "Shelf N" with the lowest
    /// number no row has yet. Rows still called by the name an earlier
    /// version gave them ("Workspace N", "워크스페이스 N") become "Shelf N".
    /// Returns whether any name changed.
    pub fn name_unnamed(&mut self) -> bool {
        let mut changed = false;
        for row in &mut self.rows {
            let old = ["Workspace ", "워크스페이스 "].iter().find_map(|p| row.name.strip_prefix(p));
            if let Some(n) = old.filter(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())) {
                row.name = format!("Shelf {n}");
                changed = true;
            }
        }
        for r in 0..self.rows.len() {
            if self.rows[r].name.trim().is_empty() {
                let name = (1..).find(|n| self.rows.iter().all(|row| row.name != format!("Shelf {n}"))).map(|n| format!("Shelf {n}"));
                self.rows[r].name = name.unwrap_or_default();
                changed = true;
            }
        }
        changed
    }

    /// Moves a cell into a new row inserted before row `at`.
    pub fn move_cell_to_new_row(&mut self, id: &str, at: usize) {
        let Some(from) = self.find(id) else { return };
        self.rows[from.row].retain(|c| c != id);
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
            r.retain(|c| c != id);
        }
        self.rows.retain(|r| !r.cells.is_empty());
        self.emphasised.retain(|c| c != id);
        self.usage.remove(id);
    }

    pub fn is_emphasised(&self, id: &str) -> bool {
        self.emphasised.iter().any(|c| c == id)
    }

    /// Marks a cell to stand out, or takes the mark off again.
    pub fn toggle_emphasis(&mut self, id: &str) {
        if self.is_emphasised(id) {
            self.emphasised.retain(|c| c != id);
        } else if self.find(id).is_some() {
            self.emphasised.push(id.to_owned());
        }
    }

    /// Reconciles the grid with the desktops that actually exist. Desktops that
    /// vanished are dropped; ones created outside this app (Task View,
    /// Win+Ctrl+D) join the end of the current row. Returns whether anything
    /// changed.
    pub fn sync(&mut self, actual: &[CellId], current: &str) -> bool {
        let before = self.clone();
        for r in &mut self.rows {
            r.retain(|c| actual.contains(c));
        }
        self.rows.retain(|r| !r.cells.is_empty());
        self.emphasised.retain(|c| actual.contains(c));
        self.usage.retain(|id, _| actual.contains(id));

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

    #[test]
    fn cell_brightness_uses_the_same_decayed_credits_across_rows() {
        let mut g = grid(&[&["a"], &["b", "unused"]]);
        assert_eq!(g.cell_usage("a", 100), 0.0);
        g.usage.entry("a".into()).or_default().visits.record(100, 1.0, usage::CELL_HALF_LIFE);
        g.usage.entry("b".into()).or_default().visits.record(100, 1.0, usage::CELL_HALF_LIFE);
        g.usage.get_mut("b").unwrap().visits.record(100, 1.0, usage::CELL_HALF_LIFE);
        assert!((g.cell_usage("a", 100) - 0.5_f32.sqrt()).abs() < 0.0001);
        assert_eq!(g.cell_usage("b", 100), 1.0);
        assert_eq!(g.cell_usage("unused", 100), 0.0);
        g.usage.get_mut("b").unwrap().visits.record(100, 2.0, usage::CELL_HALF_LIFE);
        assert_eq!(g.cell_usage("a", 100), 0.5);
        assert_eq!(g.cell_usage("b", 100), 1.0);
        assert_eq!(g.cell_usage("a", 100 + usage::CELL_HALF_LIFE), 0.5);
        let before = g.usage.clone();
        g.visit("a");
        assert_eq!(g.usage, before, "board selection only changes the row anchor");
    }

    #[test]
    fn usage_prunes_removed_cells_and_old_credits() {
        let mut g = grid(&[&["a", "b"]]);
        for id in ["a", "b", "gone"] {
            let cell = g.usage.entry(id.into()).or_default();
            cell.visits.record(100, 1.0, usage::CELL_HALF_LIFE);
        }
        g.prune_usage(100);
        assert_eq!(g.usage.len(), 2);
        let mut old = g.clone();
        old.prune_usage(100 + 21 * usage::CELL_HALF_LIFE);
        assert!(old.usage.is_empty());
        g.remove("b");
        assert!(!g.usage.contains_key("b"));
        g.sync(&["other".into()], "other");
        assert!(g.usage.is_empty());
    }

    #[test]
    fn names_from_earlier_versions_become_shelves() {
        let mut g = grid(&[&["a"], &["b"], &["c"], &["d"]]);
        g.rows[0].name = "Workspace 1".into();
        g.rows[1].name = "워크스페이스 3".into();
        g.rows[2].name = "Workspace of mine".into();
        assert!(g.name_unnamed());
        let names: Vec<_> = g.rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["Shelf 1", "Shelf 3", "Workspace of mine", "Shelf 2"]);
        assert!(!g.name_unnamed());
    }

    #[test]
    fn a_shelf_turned_in_a_hold_stays_turned_once_left() {
        // From shelf 3 up to shelf 2, two cells along it, back down to shelf 3.
        let mut g = grid(&[&["a"], &["b0", "b1", "b2", "b3"], &["c0", "c1"]]);
        g.visit("c1");
        let from = g.find("c1").unwrap();
        let mut hand = crate::follow::Follow::new((-2, 0), vec![(0, 0), (0, 3), (-1, 0)]);
        assert_eq!(hand.finish_path(&[(0.0, -1.0, 1), (2.0, -1.0, 0), (2.0, 0.0, 1)]), (0, 0));
        assert!(g.keep_turns(from, &hand.turns()));
        // Shelf 2 is shown and gone to at the cell it was turned to; shelf 3 is where it was.
        assert_eq!(g.counted(from, (0, -1)), Some(Pos { row: 1, col: 2 }));
        assert_eq!(g.across(2, -1).unwrap().0, Pos { row: 1, col: 2 });
        assert_eq!(g.rows[2].anchor(), 1);
    }

    #[test]
    fn down_a_shelf_and_along_it_lands_where_the_hand_let_go() {
        let mut g = grid(&[&["a"], &["b0", "b1", "b2"]]);
        let from = g.find("a").unwrap();
        for (along, cell) in [(1, "b1"), (1, "b2"), (-2, "b0")] {
            let (id, _) = g.land(from, (along, 1), &[(0, 0), (along, 1)]);
            assert_eq!(id.as_deref(), Some(cell));
            assert_eq!(g.rows[1].anchor(), g.find(cell).unwrap().col);
        }
    }

    fn app(exe: &str) -> AppMemory {
        AppMemory { exe: format!("C:\\Apps\\{exe}"), args: String::new(), title: String::new(), name: String::new() }
    }

    fn names(apps: &[&AppMemory]) -> Vec<String> {
        apps.iter().map(|a| a.app()).collect()
    }

    #[test]
    fn ghosts_are_the_remembered_apps_not_alive_counted_per_exe() {
        let memory = CellMemory { taken: 1, apps: vec![app("chrome.exe"), app("Code.exe"), app("chrome.exe"), app("chrome.exe")] };
        let live = vec!["chrome.exe".to_owned()];
        assert_eq!(names(&ghosts(&memory, &live)), ["code.exe", "chrome.exe", "chrome.exe"]);
        let all = vec!["chrome.exe".to_owned(), "code.exe".into(), "chrome.exe".into(), "chrome.exe".into(), "other.exe".into()];
        assert!(ghosts(&memory, &all).is_empty());
        assert_eq!(ghosts(&memory, &[]).len(), 4);
    }

    #[test]
    fn state_without_memory_still_loads_and_saves_memory_back() {
        let old = r#"{"rows":[{"name":"Work","cells":["a"]}],"guide_seen":true}"#;
        let mut g: Grid = serde_json::from_str(old).unwrap();
        assert!(g.memory.is_empty());
        g.remember("a", vec![app("code.exe")], 5);
        let saved = serde_json::to_string(&g).unwrap();
        assert!(saved.contains("\"memory\""));
        let back: Grid = serde_json::from_str(&saved).unwrap();
        assert_eq!(back.memory["a"].apps, [app("code.exe")]);
        assert_eq!(back.memory["a"].taken, 5);
    }

    #[test]
    fn memory_of_vanished_cells_is_kept_a_week() {
        let mut g = grid(&[&["a", "b"]]);
        g.remember("a", vec![app("code.exe")], 100);
        g.remember("b", vec![app("code.exe")], 100);
        g.sync(&["a".into()], "a");
        assert!(g.memory.contains_key("b"), "sync leaves the memory of a cell that went");
        let day = 24 * 3600;
        let mut later = g.clone();
        later.prune_memory(100 + 6 * day);
        assert!(later.memory.contains_key("b"));
        g.prune_memory(100 + 8 * day);
        assert!(!g.memory.contains_key("b"));
        assert!(g.memory.contains_key("a"), "a cell still in the grid keeps its memory");
    }

    #[test]
    fn remember_caps_a_cell_at_twelve_windows_topmost_first() {
        let mut g = grid(&[&["a"]]);
        let apps: Vec<AppMemory> = (0..15).map(|i| app(&format!("app{i}.exe"))).collect();
        assert!(g.remember("a", apps, 1));
        let kept = &g.memory["a"].apps;
        assert_eq!(kept.len(), MEMORY_MOST);
        assert_eq!(kept[0].app(), "app0.exe");
        assert_eq!(kept[11].app(), "app11.exe");
        assert!(!g.remember("a", (0..15).map(|i| app(&format!("app{i}.exe"))).collect(), 2), "the same windows again change nothing");
        assert_eq!(g.memory["a"].taken, 2);
    }

    #[test]
    fn a_ghosting_cell_keeps_its_memory_until_every_ghost_is_back() {
        let mut g = grid(&[&["a", "b", "c"]]);
        g.remember("a", vec![app("code.exe"), app("wt.exe")], 1);
        g.remember("c", vec![app("chrome.exe")], 1);
        let mut ghosting: HashSet<CellId> = ["a".to_owned(), "c".to_owned()].into();
        // Started with Windows, a messenger comes up in cell a before the rest.
        let mut live = BTreeMap::from([("a".to_owned(), vec![app("chat.exe"), app("code.exe")]), ("b".to_owned(), vec![app("notes.exe")])]);
        assert!(g.update_memory(&live, &mut ghosting, 2));
        assert_eq!(g.memory["a"].apps, [app("code.exe"), app("wt.exe")], "not taken over while apps are still to come back");
        assert_eq!(g.memory["b"].apps, [app("notes.exe")], "a cell not waiting is remembered as it is");
        assert_eq!(g.memory["c"].apps, [app("chrome.exe")], "an empty cell keeps what it had");
        assert!(ghosting.contains("a") && ghosting.contains("c"));
        live.insert("a".into(), vec![app("chat.exe"), app("wt.exe"), app("code.exe")]);
        g.update_memory(&live, &mut ghosting, 3);
        assert!(!ghosting.contains("a"), "every app back, the cell waits no more");
        assert_eq!(g.memory["a"].apps, live["a"]);
        assert!(ghosting.contains("c"));
    }

    #[test]
    fn remembering_skips_pinned_explorer_and_store_hosts() {
        let seen = |exe: Option<&str>, pinned: bool| Seen { exe: exe.map(str::to_owned), title: "t".into(), name: "n".into(), pinned };
        let kept = remembered([
            seen(Some("C:\\Windows\\Explorer.EXE"), false),
            seen(Some("C:\\Windows\\System32\\ApplicationFrameHost.exe"), false),
            seen(Some("C:\\Apps\\code.exe"), true),
            seen(None, false),
            seen(Some("C:\\Apps\\wt.exe"), false),
        ]);
        assert_eq!(kept, [AppMemory { exe: "C:\\Apps\\wt.exe".into(), args: String::new(), title: "t".into(), name: "n".into() }]);
    }

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
        g.rows[0].cells.insert(0, "a0".into());
        assert_eq!(go(&mut g, "b1", Dir::Up).as_deref(), Some("a2"));

        let actual: Vec<String> = ["a0", "a1", "b1", "new"].iter().map(|s| s.to_string()).collect();
        assert!(g.sync(&actual, "b1"));
        assert_eq!(g.rows[1].cells, ["b1", "new"]);
        // The cell it was left on gone, the shelf stays turned about there.
        assert_eq!(go(&mut g, "b1", Dir::Up).as_deref(), Some("a1"));
    }

    /// Board moves save each shelf's anchor even while Windows stays on
    /// the original cell. Reading desktops again must leave them there.
    #[test]
    fn anchors_survive_saving_and_refreshing_on_a_different_current_cell() {
        let mut g = grid(&[&["a1", "a2", "a3"], &["b1", "b2"], &["c1"]]);
        g.visit("a2");
        g.visit("b2");
        g.visit("a3");
        let saved = serde_json::to_string(&g).unwrap();
        let mut restored: Grid = serde_json::from_str(&saved).unwrap();
        let actual: Vec<_> = ["a1", "a2", "a3", "b1", "b2", "c1"].into_iter().map(str::to_owned).collect();
        assert!(!restored.sync(&actual, "a1"));
        assert_eq!(restored.rows.iter().map(Row::anchor).collect::<Vec<_>>(), [2, 1, 0]);
        assert_eq!(restored.id_at(restored.across(0, 1).unwrap().0).map(String::as_str), Some("b2"));
        assert_eq!(restored.id_at(restored.across(1, -1).unwrap().0).map(String::as_str), Some("a3"));
        let before = restored.clone();
        restored.visit("gone");
        assert_eq!(restored, before);
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

    /// Removing a shelf: each of its cells sends its windows to the cell
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

#[cfg(test)]
mod onboarding_tests {
    #[test]
    fn guide_completion_is_only_announced_once_even_after_restart() {
        let mut grid = super::Grid::default();
        assert!(grid.complete_guide());
        assert!(!grid.complete_guide());
        let mut restored: super::Grid = serde_json::from_str(&serde_json::to_string(&grid).unwrap()).unwrap();
        assert!(!restored.complete_guide());
    }
}
