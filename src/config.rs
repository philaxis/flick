//! User settings (`config.toml`) and the saved grid (`state.json`), both in
//! `%APPDATA%\Flick`.

use crate::grid::Grid;
use serde::Deserialize;
use std::{fs, io, path::PathBuf, time::SystemTime};

/// The app's display name. Also used for the install folder, the settings
/// folder, the Start menu shortcut and the registry entries; the exe name in
/// Cargo.toml and the icon resource (scripts/make-icon.py) follow it too.
pub const APP_NAME: &str = "Flick";
/// Settings folders of versions under earlier names, newest first.
const OLD_DIRS: [&str; 2] = ["KanKan", "desk2d"];

const DEFAULT_CONFIG: &str = r#"# Flick settings. Saving this file applies them at once.
# Trigger, distances and the rest can also be changed in the window at tray icon → "Settings…".

# The button to hold while moving the mouse to go from cell to cell. Several can be listed with commas.
#   Mouse: "xbutton1" (back), "xbutton2" (forward), "middle" (wheel button)
#   Keyboard: "capslock", "scrolllock", "pause", "apps", "ralt", "rctrl", "f1" to "f24"
#   Chord: keys joined with +, like "w+e+r", held down together (only one chord)
#   Example: trigger = "xbutton2, w+e+r"
trigger = "xbutton2"

# New cells and new shelves are made only with + in the overview (click the trigger).

# Holding this key during the gesture carries the active window along. "shift", "ctrl", "alt"
carry_modifier = "shift"

# Ctrl+Alt+Win+arrows to move (add Shift to carry the window), Ctrl+Alt+Win+P for the pin menu of the active window
hotkeys = true

# Lets Windows compress or page out the memory of apps in a shelf not visited for this long (minutes).
# Coming back may stutter for a moment. 0 turns it off.
sleep_after_minutes = 0

# Each device on its own. _x is sideways (along a shelf), _y is up and down (from shelf to shelf).
#   step: how far to move for one cell sideways or one shelf up or down. The longer vertical distance keeps you from changing shelf by accident.
#   notch: 0 to 100, how firmly the displayed hand holds near each cell or shelf.
#     0 follows the hand evenly. Sideways selection still changes a little past halfway.
#     Vertical notches keep the whole-distance shelf step and lean then snap.
#     Touchpad vertical strength is doubled: 100 composes two monotone notches.
#   follow: the screen moves along with the hand between desktops (not used yet).
#   reverse: with true the desktops go the other way from the hand, as if pushing the screen like scrolling content on a touchpad; with false they follow it.
#     Keyboard shortcuts are not affected.

[mouse]
# Distances in pixels.
step_x = 260
step_y = 400
notch_x = 40
notch_y = 100
follow_x = true
follow_y = true
reverse = false

[touchpad]
# Three-finger precision touchpad trigger (Windows three-finger swipes and taps must be Nothing).
three_finger = false
# Distances in millimetres on the pad.
step_x = 10.0
step_y = 10.0
notch_x = 40
notch_y = 100
follow_x = true
follow_y = true
reverse = true
"#;

/// Keys of earlier versions, before each device had its own settings: the
/// key, and the table and key it is read as when that one is not given.
const LEGACY: [(&str, &str, &str); 6] = [
    ("step_x", "mouse", "step_x"),
    ("step_y", "mouse", "step_y"),
    ("vertical_sticky", "mouse", "notch_y"),
    ("mouse_reverse", "mouse", "reverse"),
    ("three_finger_touchpad", "touchpad", "three_finger"),
    ("touchpad_reverse", "touchpad", "reverse"),
];

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct Config {
    pub trigger: String,
    /// Accepted from older config files; hover previews appear immediately.
    pub dwell_ms: u32,
    pub carry_modifier: String,
    pub hotkeys: bool,
    pub sleep_after_minutes: u32,
    pub mouse: Mouse,
    pub touchpad: Touchpad,
}

/// The mouse (and a keyboard trigger, whose hand is on the mouse). `_x` is
/// sideways along a row, `_y` up and down from row to row.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(default)]
pub struct Mouse {
    /// Pixels of travel for one cell, and for one row.
    pub step_x: i32,
    pub step_y: i32,
    /// How firmly a cell or row holds, 0 to 100 (`Detent`); 0 follows the
    /// hand evenly (`Slide`). Sideways changes the display, not selection.
    pub notch_x: u32,
    pub notch_y: u32,
    /// The screen moving along with the hand between desktops. Kept, not
    /// acted on yet.
    pub follow_x: bool,
    pub follow_y: bool,
    /// The desktops go against the hand.
    pub reverse: bool,
}

/// Three fingers on a precision touchpad. As `Mouse`, with distances in mm
/// of the pad (`touchpad::Rails`).
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(default)]
pub struct Touchpad {
    pub three_finger: bool,
    pub step_x: f64,
    pub step_y: f64,
    pub notch_x: u32,
    pub notch_y: u32,
    pub follow_x: bool,
    pub follow_y: bool,
    pub reverse: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            trigger: "xbutton2".into(),
            dwell_ms: 500,
            carry_modifier: "shift".into(),
            hotkeys: true,
            sleep_after_minutes: 0,
            mouse: Mouse::default(),
            touchpad: Touchpad::default(),
        }
    }
}

impl Default for Mouse {
    fn default() -> Self {
        Mouse { step_x: 260, step_y: 400, notch_x: 40, notch_y: 100, follow_x: true, follow_y: true, reverse: false }
    }
}

impl Default for Touchpad {
    fn default() -> Self {
        Touchpad {
            three_finger: false,
            step_x: crate::touchpad::STEP_MM,
            step_y: crate::touchpad::STEP_MM,
            notch_x: 40,
            notch_y: 100,
            follow_x: true,
            follow_y: true,
            reverse: true,
        }
    }
}

/// Reads a config file. Keys of earlier versions (`LEGACY`) are read as the
/// ones that took their place, unless the file gives those too.
/// Older templates wrote notch_x = 0 too. There is no record of whether
/// the user chose Off, so preserve explicit zero; only missing keys get 40.
fn parse(text: &str) -> Result<Config, toml::de::Error> {
    let mut file: toml::Table = toml::from_str(text)?;
    for (old, table, key) in LEGACY {
        let Some(value) = file.remove(old) else { continue };
        // Up and down was sticky or not; now it is how firmly.
        let value = match (key, value) {
            ("notch_y", toml::Value::Boolean(sticky)) => toml::Value::Integer(if sticky { 100 } else { 0 }),
            (_, value) => value,
        };
        if let toml::Value::Table(table) = file.entry(table).or_insert_with(|| toml::Table::new().into()) {
            table.entry(key).or_insert(value);
        }
    }
    toml::Value::Table(file).try_into()
}

/// The settings folder. One left by a version under an earlier name is taken
/// over the first time this is asked for.
fn dir() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_default();
    let dir = base.join(APP_NAME);
    if !dir.exists() {
        if let Some(old) = OLD_DIRS.iter().map(|name| base.join(name)).find(|old| old.exists()) {
            let _ = fs::rename(old, &dir);
        }
    }
    dir
}

pub fn config_path() -> PathBuf {
    dir().join("config.toml")
}

fn state_path() -> PathBuf {
    dir().join("state.json")
}

/// Loads the config, writing the commented default file on first run.
/// A file that fails to parse is reported and replaced by defaults in memory only.
pub fn load_config() -> (Config, Option<String>) {
    let path = config_path();
    match fs::read_to_string(&path) {
        Ok(text) => match parse(&text) {
            Ok(config) => (config, None),
            Err(e) => (Config::default(), Some(e.to_string())),
        },
        Err(_) => {
            let _ = fs::create_dir_all(dir());
            let _ = fs::write(&path, DEFAULT_CONFIG);
            (Config::default(), None)
        }
    }
}

/// `text` (a config file) with `key` set to `value`, written as TOML. A
/// key in a table is named with its table, as `mouse.step_x`. Only that
/// key's line changes, so the user's comments and other lines stay; a key
/// the file does not have yet is added at the end of its table (a key of no
/// table before the first table), and a table it does not have at the end.
/// The key of an earlier version it takes the place of (`LEGACY`) goes.
fn with_value(text: &str, key: &str, value: &str) -> String {
    let (table, name) = key.split_once('.').unwrap_or(("", key));
    let line = format!("{name} = {value}");
    let legacy = LEGACY.iter().find(|(_, t, k)| (*t, *k) == (table, name)).map(|(old, ..)| *old);
    let named = |old: &str, key: &str| old.split_once('=').is_some_and(|(name, _)| name.trim() == key);
    // Which table each line is in.
    let header = |old: &str| {
        let old = old.trim();
        (old.starts_with('[') && !old.starts_with("[[")).then(|| old.trim_matches(['[', ']']).trim().to_owned())
    };
    let mut lines: Vec<String> = Vec::new();
    let mut tables: Vec<String> = Vec::new();
    let mut current = String::new();
    for old in text.lines() {
        if let Some(name) = header(old) {
            current = name;
        }
        if current.is_empty() && legacy.is_some_and(|legacy| named(old, legacy)) {
            continue;
        }
        lines.push(old.to_owned());
        tables.push(current.clone());
    }
    let ours = |at: usize| tables[at] == table;
    match (0..lines.len()).find(|&at| ours(at) && header(&lines[at]).is_none() && named(&lines[at], name)) {
        Some(at) => lines[at] = line,
        None => match (0..lines.len()).filter(|&at| ours(at)).last() {
            // After the table's last line that is not blank.
            Some(last) => {
                let first = (0..=last).find(|&at| ours(at)).unwrap_or(0);
                let at = (first..=last).rev().find(|&at| !lines[at].trim().is_empty()).map_or(first, |at| at + 1);
                lines.insert(at, line);
            }
            None if table.is_empty() => lines.insert(0, line),
            None => {
                if lines.last().is_some_and(|last| !last.trim().is_empty()) {
                    lines.push(String::new());
                }
                lines.push(format!("[{table}]"));
                lines.push(line);
            }
        },
    }
    lines.join("\n") + "\n"
}

/// Sets one value in the config file, keeping everything else in it.
pub fn set_value(key: &str, value: &str) -> io::Result<()> {
    let path = config_path();
    let text = fs::read_to_string(&path).unwrap_or_else(|_| DEFAULT_CONFIG.to_owned());
    fs::create_dir_all(dir())?;
    fs::write(path, with_value(&text, key, value))
}

/// When the config file was last written, to notice the user saving it.
pub fn config_modified() -> Option<SystemTime> {
    fs::metadata(config_path()).and_then(|file| file.modified()).ok()
}

/// Loads the saved grid. Without a saved one, or with one that cannot be
/// read (which is reported), the grid starts empty and is filled from the
/// desktops that exist.
pub fn load_grid() -> (Grid, Option<String>) {
    let Ok(text) = fs::read_to_string(state_path()) else { return (Grid::default(), None) };
    match serde_json::from_str(&text) {
        Ok(grid) => (grid, None),
        Err(e) => (Grid::default(), Some(e.to_string())),
    }
}

pub fn save_grid(grid: &Grid) -> io::Result<()> {
    fs::create_dir_all(dir())?;
    fs::write(state_path(), serde_json::to_string_pretty(grid)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_notches_default_to_forty_but_saved_off_stays_off() {
        for config in [Config::default(), parse(DEFAULT_CONFIG).unwrap(), parse("").unwrap(), parse(OLD_CONFIG).unwrap()] {
            assert_eq!((config.mouse.notch_x, config.touchpad.notch_x), (40, 40));
        }
        // Old templates and a deliberate Off are indistinguishable; keep both.
        let old = DEFAULT_CONFIG.replace("notch_x = 40", "notch_x = 0");
        let saved = parse(&old).unwrap();
        assert_eq!((saved.mouse.notch_x, saved.touchpad.notch_x), (0, 0));
        let off = with_value(DEFAULT_CONFIG, "mouse.notch_x", "0");
        assert_eq!(parse(&off).unwrap().mouse.notch_x, 0);
        let sticky = parse("vertical_sticky = false\n[mouse]\nnotch_y = 70\n").unwrap();
        assert_eq!((sticky.mouse.notch_x, sticky.mouse.notch_y), (40, 70));
    }

    #[test]
    fn touchpad_is_opt_in_for_new_and_existing_configs() {
        assert!(!Config::default().touchpad.three_finger);
        assert!(!parse(DEFAULT_CONFIG).unwrap().touchpad.three_finger);
        assert!(!parse("trigger = \"f13\"").unwrap().touchpad.three_finger);
        let enabled = with_value("# keep me\n", "touchpad.three_finger", "true");
        assert!(parse(&enabled).unwrap().touchpad.three_finger);
    }

    /// Configs written before the settings existed keep the mouse as it was
    /// and reverse the touchpad; each can be changed on its own.
    #[test]
    fn mouse_follows_the_hand_and_touchpad_is_reversed_by_default() {
        let reverse = |c: Config| (c.mouse.reverse, c.touchpad.reverse);
        assert_eq!(reverse(Config::default()), (false, true));
        assert_eq!(reverse(parse(DEFAULT_CONFIG).unwrap()), (false, true));
        assert_eq!(reverse(parse("three_finger_touchpad = true").unwrap()), (false, true));
        let mouse = with_value(DEFAULT_CONFIG, "mouse.reverse", "true");
        assert_eq!(reverse(parse(&mouse).unwrap()), (true, true));
        let touchpad = with_value(DEFAULT_CONFIG, "touchpad.reverse", "false");
        assert_eq!(reverse(parse(&touchpad).unwrap()), (false, false));
    }

    /// A file of the version before each device had its own settings.
    const OLD_CONFIG: &str = "trigger = \"xbutton2\"\nthree_finger_touchpad = true\nmouse_reverse = true\n\
        touchpad_reverse = false\n# mine\nstep_x = 300\nstep_y = 500\nvertical_sticky = false\nhotkeys = false\n";

    /// Its keys are read as the mouse's (and the touchpad's switches); the
    /// touchpad's distances, which it had none of, are the defaults.
    #[test]
    fn an_old_config_is_read_as_the_mouse_settings() {
        let config = parse(OLD_CONFIG).unwrap();
        let mouse = Mouse { step_x: 300, step_y: 500, notch_y: 0, reverse: true, ..Mouse::default() };
        assert_eq!(config.mouse, mouse);
        assert_eq!(config.touchpad, Touchpad { three_finger: true, reverse: false, ..Touchpad::default() });
        assert!(!config.hotkeys);
        assert_eq!(parse("vertical_sticky = true").unwrap().mouse.notch_y, 100);
        // Where both are given, the new key is the one.
        let both = parse("step_x = 300\n[mouse]\nstep_x = 200\n").unwrap();
        assert_eq!((both.mouse.step_x, both.mouse.step_y), (200, 400));
    }

    /// Every setting of either device written into a file, new or old, is
    /// read back as written, and an old key is replaced by the new one.
    #[test]
    fn device_settings_round_trip() {
        let values: [(&str, &str); 15] = [
            ("mouse.step_x", "320"),
            ("mouse.step_y", "450"),
            ("mouse.notch_x", "30"),
            ("mouse.notch_y", "60"),
            ("mouse.follow_x", "false"),
            ("mouse.follow_y", "false"),
            ("mouse.reverse", "true"),
            ("touchpad.three_finger", "true"),
            ("touchpad.step_x", "12.5"),
            ("touchpad.step_y", "18"),
            ("touchpad.notch_x", "20"),
            ("touchpad.notch_y", "0"),
            ("touchpad.follow_x", "false"),
            ("touchpad.follow_y", "false"),
            ("touchpad.reverse", "false"),
        ];
        let mouse = Mouse { step_x: 320, step_y: 450, notch_x: 30, notch_y: 60, follow_x: false, follow_y: false, reverse: true };
        let touchpad = Touchpad {
            three_finger: true,
            step_x: 12.5,
            step_y: 18.0,
            notch_x: 20,
            notch_y: 0,
            follow_x: false,
            follow_y: false,
            reverse: false,
        };
        for file in [DEFAULT_CONFIG, OLD_CONFIG, "", "# only a comment"] {
            let text = values.iter().fold(file.to_owned(), |text, (key, value)| with_value(&text, key, value));
            let config = parse(&text).unwrap_or_else(|e| panic!("{e}\n{text}"));
            assert_eq!((config.mouse, config.touchpad), (mouse, touchpad), "{text}");
            let top: toml::Table = toml::from_str(&text).unwrap();
            for (old, ..) in LEGACY {
                assert!(!top.contains_key(old), "{old} left in\n{text}");
            }
        }
        // The rest of an old file stays, and a top-level key goes before the tables.
        let text = with_value(&with_value(OLD_CONFIG, "mouse.step_x", "200"), "carry_modifier", "\"ctrl\"");
        assert!(text.contains("# mine\nstep_y = 500\n"));
        let config = parse(&text).unwrap();
        assert_eq!((config.mouse.step_x, config.mouse.step_y, config.carry_modifier.as_str()), (200, 500, "ctrl"));
        let text = with_value(DEFAULT_CONFIG, "hotkeys", "false");
        assert!(!parse(&text).unwrap().hotkeys);
        assert!(text.find("hotkeys = false").unwrap() < text.find("[mouse]").unwrap());
    }

    /// Older state files have no usage field. New credits survive the same
    /// serialization path without changing the grid or its remembered cells.
    #[test]
    fn saved_grid_accepts_old_state_and_round_trips_usage() {
        let old = r#"{"rows":[{"cells":["a"],"last":"a"}],"emphasised":["a"]}"#;
        let mut grid: Grid = serde_json::from_str(old).unwrap();
        assert!(grid.usage.is_empty());
        grid.usage.entry("a".into()).or_default().visits.record(100, 1.0, crate::usage::CELL_HALF_LIFE);
        let previous = r#"{"rows":[{"cells":["a"],"last":"a"}],"usage":{"a":{"visits":{"score":2.0,"updated":100},"windows":{"app:a.exe":{"score":8.0,"updated":100}}}}}"#;
        let previous: Grid = serde_json::from_str(previous).unwrap();
        assert_eq!(previous.usage["a"].visits.score, 2.0);
        assert!(!serde_json::to_string(&previous).unwrap().contains("windows"));
        let saved = serde_json::to_string_pretty(&grid).unwrap();
        assert_eq!(serde_json::from_str::<Grid>(&saved).unwrap(), grid);
        grid.window_usage.entry(42).or_default().record(100, 1.0, crate::usage::WINDOW_HALF_LIFE);
        let saved = serde_json::to_string(&grid).unwrap();
        assert!(!saved.contains("window_usage"));
        assert!(serde_json::from_str::<Grid>(&saved).unwrap().window_usage.is_empty());
    }

    /// Changing a setting from the settings window rewrites the user's file:
    /// their comments and other values must survive, a key that only starts
    /// like the one being set must not be mistaken for it, and a key missing
    /// from an older file must be added. A key no longer used, left in an
    /// older file, must not stop the file from being read.
    #[test]
    fn setting_a_value_keeps_the_rest_of_the_file() {
        let file = "# mine\ntrigger_note = 1\ntrigger = \"xbutton2\"  # forward\n\nstep_x = 260\n";
        let changed = with_value(file, "trigger", "\"f13\"");
        assert_eq!(changed, "# mine\ntrigger_note = 1\ntrigger = \"f13\"\n\nstep_x = 260\n");
        let added = with_value(&changed, "dwell_ms", "300");
        assert_eq!(added, format!("{changed}dwell_ms = 300\n"));
        let config = parse(&format!("{added}vertical_angle = 26.6\n")).unwrap();
        assert_eq!((config.trigger.as_str(), config.mouse.step_x, config.dwell_ms), ("f13", 260, 300));
    }
}
