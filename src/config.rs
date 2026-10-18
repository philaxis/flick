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
# Trigger and distances can also be changed in the window at tray icon → "Settings…".

# The button to hold while moving the mouse to go from cell to cell. Several can be listed with commas.
#   Mouse: "xbutton1" (back), "xbutton2" (forward), "middle" (wheel button)
#   Keyboard: "capslock", "scrolllock", "pause", "apps", "ralt", "rctrl", "f1" to "f24"
#   Chord: keys joined with +, like "w+e+r", held down together (only one chord)
#   Example: trigger = "xbutton2, w+e+r"
trigger = "xbutton2"

# How far to move (pixels) to go one cell sideways (step_x) and one shelf up or down (step_y). The longer vertical distance keeps you from changing shelf by accident.
step_x = 260
step_y = 400

# Up and down, each quick flick goes one shelf (two flicks go two; slowly bringing the hand back is ignored).
# With false it keeps going by the distance moved, like sideways.
vertical_sticky = true

# New cells and new shelves are made only with + in the overview (click the trigger).

# Holding this key during the gesture carries the active window along. "shift", "ctrl", "alt"
carry_modifier = "shift"

# Ctrl+Alt+Win+arrows to move (add Shift to carry the window), Ctrl+Alt+Win+P for the pin menu of the active window
hotkeys = true

# Lets Windows compress or page out the memory of apps in a shelf not visited for this long (minutes).
# Coming back may stutter for a moment. 0 turns it off.
sleep_after_minutes = 0
"#;

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct Config {
    pub trigger: String,
    pub step_x: i32,
    pub step_y: i32,
    pub vertical_sticky: bool,
    /// Accepted from older config files; hover previews appear immediately.
    pub dwell_ms: u32,
    pub carry_modifier: String,
    pub hotkeys: bool,
    pub sleep_after_minutes: u32,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            trigger: "xbutton2".into(),
            step_x: 260,
            step_y: 400,
            vertical_sticky: true,
            dwell_ms: 500,
            carry_modifier: "shift".into(),
            hotkeys: true,
            sleep_after_minutes: 0,
        }
    }
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
        Ok(text) => match toml::from_str::<Config>(&text) {
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

/// `text` (a config file) with `key` set to `value`, written as TOML. Only
/// that key's line changes, so the user's comments and other lines stay; a
/// key the file does not have yet is added at the end.
fn with_value(text: &str, key: &str, value: &str) -> String {
    let line = format!("{key} = {value}");
    let is_key = |old: &str| old.split_once('=').is_some_and(|(name, _)| name.trim() == key);
    let mut lines: Vec<&str> = text.lines().collect();
    match lines.iter().position(|old| is_key(old)) {
        Some(at) => lines[at] = &line,
        None => lines.push(&line),
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

    /// Older state files have no usage field. New counts survive the same
    /// serialization path without changing the grid or its remembered cells.
    #[test]
    fn saved_grid_accepts_old_state_and_round_trips_usage() {
        let old = r#"{"rows":[{"cells":["a"],"last":"a"}],"emphasised":["a"]}"#;
        let mut grid: Grid = serde_json::from_str(old).unwrap();
        assert!(grid.usage.is_empty());
        grid.usage.entry("a".into()).or_default().visits.record(100);
        grid.usage.get_mut("a").unwrap().windows.entry("app:a.exe".into()).or_default().record(100);
        let saved = serde_json::to_string_pretty(&grid).unwrap();
        assert_eq!(serde_json::from_str::<Grid>(&saved).unwrap(), grid);
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
        let config: Config = toml::from_str(&format!("{added}vertical_angle = 26.6\n")).unwrap();
        assert_eq!((config.trigger.as_str(), config.step_x, config.dwell_ms), ("f13", 260, 300));
    }
}
