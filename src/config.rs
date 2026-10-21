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

# Which version of this file it is, for bringing older ones up to date. Leave it as it is.
config_version = 3

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

# The small map of the grid in the middle of the screen while moving. The screen itself shows the
# desktops around while it follows the hand, so it is off unless turned on.
minimap = false

# How window pictures move. "auto": the normal view places them at fractional pixels in one
# DirectComposition commit on Windows 11 22H2..25H2 builds, falling back to the public thumbnails;
# the board keeps the public path. "off": public thumbnails everywhere. "all": the board too
# (experimental: its redrawn surface can miss refreshes). Takes effect when Flick starts.
compositor = "auto"

# Lets Windows compress or page out the memory of apps in a shelf not visited for this long (minutes).
# Coming back may stutter for a moment. 0 turns it off.
sleep_after_minutes = 0

# Each device on its own. _x is sideways (along a shelf), _y is up and down (from shelf to shelf).
#   step: movement needed to commit the next cell, after the axis dead zone; the full Distance is the threshold.
#   follow: variant F commits after the full Distance, pulls at most 20%, and springs into the next cell.
#     The picture recedes to 70%; Windows switches once when the hand lets go, without fling prediction.
#     Off switches Windows immediately at the same full-distance threshold.
#   notch_x/y: legacy values accepted for compatibility; variant F uses its fixed curve.
#   reverse: with true the desktops go the other way from the hand, as if pushing the screen like scrolling content on a touchpad; with false they follow it.
#     Keyboard shortcuts are not affected.

[mouse]
# Distances in raw mouse counts, not screen pixels; physical travel depends on mouse DPI.
# follow_x/y also animate keyboard desktop shortcuts.
step_x = 220
step_y = 220
notch_x = 40
notch_y = 100
follow_x = true
follow_y = true
reverse = false

[touchpad]
# Three-finger precision touchpad trigger (Windows three-finger swipes and taps must be Nothing).
three_finger = false
# Distances in millimetres on the pad.
step_x = 12.0
step_y = 12.0
notch_x = 40
notch_y = 100
follow_x = true
follow_y = true
reverse = true

[experimental]
# Visual commit/settle only: "default" (35/32), "gentle" (28/25), "slow" (24/24).
commit_speed = "default"
# Prototype: two faint DWM trails, costs three thumbnails per source; reopen the board.
motion_blur = false
# A quick, sure move keeps the picture whole, through short slowings; a
# hesitating one zooms all the way out at once: "slide" (the next desktop
# slides in), "instant" (comes at once, with a short fade), or "off" (always
# zoomed out).
quick_flick = "slide"
# How long after setting off (milliseconds) a move may still be a flick of
# one cell, however far it goes; held on longer it is a precise hold. The
# picture starts to draw back gently from 60% of it, so that the one goes
# into the other with no jump. Shorter makes flicks rarer. Also in Settings.
flick_ms = 150
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
    /// Which version of the file it is (`CONFIG_VERSION`); none before 2,
    /// read as 0 (a new config is of this version).
    #[serde(default)]
    pub config_version: u32,
    pub trigger: String,
    pub carry_modifier: String,
    pub hotkeys: bool,
    pub sleep_after_minutes: u32,
    /// The minimap while moving (`overlay`).
    pub minimap: bool,
    /// Which views place window pictures through `composition`.
    pub compositor: Compositor,
    pub mouse: Mouse,
    pub touchpad: Touchpad,
    pub experimental: Experimental,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Compositor { #[default] Auto, Off, All }

/// Opt-in visual experiments never change the distance or commit rules.
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(default)]
pub struct Experimental { pub commit_speed: CommitSpeed, pub motion_blur: bool, pub quick_flick: QuickFlick, pub flick_ms: u32 }
impl Default for Experimental {
    fn default() -> Self {
        Experimental {
            commit_speed: CommitSpeed::default(),
            motion_blur: false,
            quick_flick: QuickFlick::default(),
            // A flick is a click with a stroke in it: shorter than
            // `quick::QUICK_MOST`, which the judge's tests are made for.
            flick_ms: 150,
        }
    }
}
/// How a quick, sure move is shown (`quick`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QuickFlick { Off, #[default] Slide, Instant }
#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CommitSpeed { #[default] Default, Gentle, Slow }
impl CommitSpeed {
    pub fn omega(self, vertical: bool) -> f64 {
        match (self, vertical) { (Self::Default,false)=>35.0, (Self::Default,true)=>32.0,
            (Self::Gentle,false)=>28.0, (Self::Gentle,true)=>25.0, (Self::Slow,_)=>24.0 }
    }
}

/// The mouse (and a keyboard trigger, whose hand is on the mouse). `_x` is
/// sideways along a row, `_y` up and down from row to row.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(default)]
pub struct Mouse {
    /// Raw mouse counts for one cell and one shelf; physical travel depends on mouse DPI.
    pub step_x: i32,
    pub step_y: i32,
    /// Legacy notch values, retained for config compatibility.
    pub notch_x: u32,
    pub notch_y: u32,
    /// The screen moving along with the hand between desktops, the desktop
    /// changing once, on letting go (`follow`).
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
            config_version: CONFIG_VERSION,
            trigger: "xbutton2".into(),
            carry_modifier: "shift".into(),
            hotkeys: true,
            sleep_after_minutes: 0,
            minimap: false,
            compositor: Compositor::Auto,
            mouse: Mouse::default(),
            touchpad: Touchpad::default(),
            experimental: Experimental::default(),
        }
    }
}

impl Default for Mouse {
    fn default() -> Self {
        Mouse { step_x: 220, step_y: 220, notch_x: 40, notch_y: 100, follow_x: true, follow_y: true, reverse: false }
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

/// The version of the config file this one writes (`Config::config_version`).
/// 3: Distance is the full commit threshold; old distances reset once to 220 counts / 12 mm.
/// 2: sideways notches of 0 in a file of no version are the template of
/// 0.6.0-pre.1, not the user's choice, and are read as 40 (`migrated`).
pub const CONFIG_VERSION: u32 = 3;

/// Reads a config file. Keys of earlier versions (`LEGACY`) are read as the
/// ones that took their place, unless the file gives those too.
#[cfg(test)]
fn parse(text: &str) -> Result<Config, toml::de::Error> {
    Ok(migrated(text)?.0)
}

/// `text` (a config file) read, and brought up to `CONFIG_VERSION`: the
/// config, and the file as it is to be written again when that changed it.
/// 0.6.0-pre.1 wrote sideways notches of 0 into every new file, with no
/// version, so a 0 there is taken for 40 once; the version written with it,
/// a 0 set from then on stays.
fn migrated(text: &str) -> Result<(Config, Option<String>), toml::de::Error> {
    let mut config = parse_as_is(text)?;
    if config.config_version >= CONFIG_VERSION {
        return Ok((config, None));
    }
    let mut file = text.to_owned();
    if config.config_version < 2 {
        for key in ["mouse.notch_x", "touchpad.notch_x"] {
            let notch = if key.starts_with("mouse") { &mut config.mouse.notch_x } else { &mut config.touchpad.notch_x };
            if *notch == 0 {
                *notch = 40;
                file = with_value(&file, key, "40");
            }
        }
    }
    (config.mouse.step_x, config.mouse.step_y) = (220, 220);
    (config.touchpad.step_x, config.touchpad.step_y) = (12.0, 12.0);
    for (key, value) in [("mouse.step_x", "220"), ("mouse.step_y", "220"), ("touchpad.step_x", "12.0"), ("touchpad.step_y", "12.0")] {
        file = with_value(&file, key, value);
    }
    config.config_version = CONFIG_VERSION;
    Ok((config, Some(versioned(&file))))
}

/// `text` with the version of the file set to `CONFIG_VERSION`.
fn versioned(text: &str) -> String {
    with_value(text, "config_version", &CONFIG_VERSION.to_string())
}

fn parse_as_is(text: &str) -> Result<Config, toml::de::Error> {
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
        Ok(text) => match migrated(&text) {
            Ok((config, rewritten)) => {
                if let Some(rewritten) = rewritten {
                    let _ = fs::write(&path, rewritten);
                }
                (config, None)
            }
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
    // A value set in a file of no version yet is the user's: versioned, a
    // sideways notch of 0 set now is not taken for the old template's.
    let text = match migrated(&text) {
        Ok((_, Some(updated))) => updated,
        _ => text,
    };
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
    fn visual_experiments_are_off_unless_explicitly_selected() {
        let default: Config=toml::from_str("").unwrap();
        assert!(!default.experimental.motion_blur);assert_eq!(default.experimental.commit_speed.omega(false),35.0);
        let opt: Config=toml::from_str("[experimental]\ncommit_speed='gentle'\nmotion_blur=true").unwrap();
        assert_eq!(opt.experimental.commit_speed.omega(true),25.0);assert!(opt.experimental.motion_blur);
        assert!(toml::from_str::<Config>("[experimental]\ncommit_speed='unknown'").is_err());
        assert_eq!(default.experimental.quick_flick,QuickFlick::Slide);
        assert_eq!(toml::from_str::<Config>("[experimental]\nquick_flick='instant'").unwrap().experimental.quick_flick,QuickFlick::Instant);
        assert_eq!(default.compositor,Compositor::Auto);
        assert_eq!(toml::from_str::<Config>("compositor='off'").unwrap().compositor,Compositor::Off);
    }
    #[test]
    fn new_notches_default_to_forty_but_saved_off_stays_off() {
        for config in [Config::default(), parse(DEFAULT_CONFIG).unwrap(), parse("").unwrap(), parse(OLD_CONFIG).unwrap()] {
            assert_eq!((config.mouse.notch_x, config.touchpad.notch_x), (40, 40));
        }
        // A deliberate Off in a versioned file stays off.
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

    /// The file 0.6.0-pre.1 wrote: no version, sideways notches of 0.
    const PRE1_CONFIG: &str = "trigger = \"xbutton2\"\nhotkeys = true\n\n[mouse]\nstep_x = 260\nnotch_x = 0\nnotch_y = 100\n\
        follow_x = true\n\n[touchpad]\nthree_finger = true\nnotch_x = 0\nnotch_y = 100\n";

    /// Its sideways notches of 0 are taken for 40 once, and the file is
    /// rewritten with the version; a 0 set after that is the user's.
    #[test]
    fn the_pre1_template_notches_are_brought_up_once() {
        let (config, rewritten) = migrated(PRE1_CONFIG).unwrap();
        assert_eq!((config.mouse.notch_x, config.touchpad.notch_x, config.config_version), (40, 40, CONFIG_VERSION));
        let rewritten = rewritten.expect("the file is brought up to date");
        assert!(rewritten.contains("three_finger = true") && rewritten.contains("step_x = 220"), "{rewritten}");
        let (again, none) = migrated(&rewritten).unwrap();
        assert_eq!((again.mouse.notch_x, again.touchpad.notch_x), (40, 40));
        assert!(none.is_none(), "only once");
        // Set to 0 afterwards (as `set_value` writes it): stays 0.
        let off = with_value(&rewritten, "touchpad.notch_x", "0");
        let (off, none) = migrated(&off).unwrap();
        assert_eq!((off.mouse.notch_x, off.touchpad.notch_x), (40, 0));
        assert!(none.is_none());
        // Other notches are left alone, and a new file needs nothing.
        let firm = PRE1_CONFIG.replacen("notch_x = 0", "notch_x = 70", 1);
        let (firm, _) = migrated(&firm).unwrap();
        assert_eq!((firm.mouse.notch_x, firm.touchpad.notch_x), (70, 40));
        assert!(migrated(DEFAULT_CONFIG).unwrap().1.is_none());
        let unversioned_off = with_value("", "mouse.notch_x", "0");
        assert_eq!(parse_as_is(&versioned(&unversioned_off)).unwrap().mouse.notch_x, 0);
        assert_eq!(parse(&unversioned_off).unwrap().mouse.notch_x, 40);
    }

    /// The minimap is off unless turned on, old files too.
    #[test]
    fn the_minimap_is_off_unless_turned_on() {
        assert!(!Config::default().minimap);
        assert!(!parse(DEFAULT_CONFIG).unwrap().minimap);
        assert!(!parse(OLD_CONFIG).unwrap().minimap && !parse(PRE1_CONFIG).unwrap().minimap);
        assert!(parse(&with_value(DEFAULT_CONFIG, "minimap", "true")).unwrap().minimap);
    }

    /// A file of the version before each device had its own settings.
    const OLD_CONFIG: &str = "trigger = \"xbutton2\"\nthree_finger_touchpad = true\nmouse_reverse = true\n\
        touchpad_reverse = false\n# mine\nstep_x = 300\nstep_y = 500\nvertical_sticky = false\nhotkeys = false\n";

    /// Its keys are read as the mouse's (and the touchpad's switches); the
    /// touchpad's distances, which it had none of, are the defaults.
    #[test]
    fn an_old_config_is_read_as_the_mouse_settings() {
        let config = parse_as_is(OLD_CONFIG).unwrap();
        let mouse = Mouse { step_x: 300, step_y: 500, notch_y: 0, reverse: true, ..Mouse::default() };
        assert_eq!(config.mouse, mouse);
        assert_eq!(config.touchpad, Touchpad { three_finger: true, reverse: false, ..Touchpad::default() });
        assert!(!config.hotkeys);
        assert_eq!(parse("vertical_sticky = true").unwrap().mouse.notch_y, 100);
        // Where both are given, the new key is the one.
        let both = parse_as_is("step_x = 300\n[mouse]\nstep_x = 200\n").unwrap();
        assert_eq!((both.mouse.step_x, both.mouse.step_y), (200, 220));
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
            let config = parse(&versioned(&text)).unwrap_or_else(|e| panic!("{e}\n{text}"));
            assert_eq!((config.mouse, config.touchpad), (mouse, touchpad), "{text}");
            let top: toml::Table = toml::from_str(&text).unwrap();
            for (old, ..) in LEGACY {
                assert!(!top.contains_key(old), "{old} left in\n{text}");
            }
        }
        // The rest of an old file stays, and a top-level key goes before the tables.
        let text = with_value(&with_value(OLD_CONFIG, "mouse.step_x", "200"), "carry_modifier", "\"ctrl\"");
        assert!(text.contains("# mine\nstep_y = 500\n"));
        let config = parse(&versioned(&text)).unwrap();
        assert_eq!((config.mouse.step_x, config.mouse.step_y, config.carry_modifier.as_str()), (200, 500, "ctrl"));
        let text = with_value(DEFAULT_CONFIG, "hotkeys", "false");
        assert!(!parse(&text).unwrap().hotkeys);
        assert!(text.find("hotkeys = false").unwrap() < text.find("[mouse]").unwrap());
    }

    #[test]
    fn distance_migration_runs_once_and_preserves_later_choices() {
        let old = "config_version = 2\nminimap = true\n[mouse]\nstep_x = 260\nstep_y = 400\nnotch_x = 0\n[touchpad]\nstep_x = 17.0\nstep_y = 12.0\n";
        let (c, text) = migrated(old).unwrap();
        assert_eq!((c.mouse.step_x, c.mouse.step_y, c.mouse.notch_x), (220, 220, 0));
        assert!((c.touchpad.step_x - 12.0).abs() < 1e-9 && (c.touchpad.step_y - 12.0).abs() < 1e-9 && c.minimap);
        let text = with_value(&with_value(&text.unwrap(), "mouse.step_x", "170"), "touchpad.step_y", "6.5");
        let (c, rewrite) = migrated(&text).unwrap();
        assert_eq!((c.mouse.step_x, c.mouse.step_y), (170, 220));
        assert!((c.touchpad.step_y - 6.5).abs() < 1e-9 && rewrite.is_none());
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
        assert_eq!((config.trigger.as_str(), config.mouse.step_x), ("f13", 220));
    }
}
