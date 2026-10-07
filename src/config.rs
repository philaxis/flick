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

const DEFAULT_CONFIG: &str = r#"# Flick 설정. 저장하면 바로 적용됩니다.
# 트리거, 각도, 거리는 트레이 아이콘 우클릭 → "설정…" 창에서도 바꿉니다.

# 누르고 있는 동안 마우스를 움직이면 칸을 이동하는 버튼. 쉼표로 여러 개를 함께 쓸 수 있습니다.
#   마우스: "xbutton1"(뒤로), "xbutton2"(앞으로), "middle"(휠 버튼)
#   키보드: "capslock", "scrolllock", "pause", "apps", "ralt", "rctrl", "f1" ~ "f24"
#   동시 누르기: "w+e+r"처럼 +로 묶은 키들을 한 번에 누르고 있는 동안 (하나만 쓸 수 있음)
#   예: trigger = "xbutton2, w+e+r"
trigger = "xbutton2"

# 한 칸 넘어가는 데 필요한 이동 거리(픽셀). 세로를 더 길게 두면 실수로 워크스페이스가 바뀌지 않습니다.
step_x = 260
step_y = 320

# 위아래는 빠르게 휙 밀 때마다 워크스페이스 하나씩 넘어갑니다(휙-휙은 둘, 손을 천천히 되돌리는 움직임은 무시).
# false로 하면 좌우처럼 민 거리만큼 계속 넘어갑니다.
vertical_sticky = true

# 위아래로 치는 각도: 똑바로 위(아래)에서 좌우로 이 각도 안쪽으로 밀어야 위아래입니다. 나머지는 좌우입니다.
vertical_angle = 26.6

# 새 칸과 새 워크스페이스는 전체 보기(트리거를 눌렀다 떼기)에서 마우스로 만듭니다.
# 여기에 1 이상을 넣으면 격자 가장자리에서 그 횟수만큼 더 밀 때도 만들어집니다. 0이면 만들지 않습니다.
edge_create_pushes = 0

# 제스처 중에 이 키를 같이 누르고 있으면 지금 활성 창을 들고 이동합니다. "shift", "ctrl", "alt"
carry_modifier = "shift"

# Ctrl+Alt+Win+방향키로 이동 (Shift를 더하면 창을 들고 이동), Ctrl+Alt+Win+P로 활성 창 고정 메뉴
hotkeys = true

# 이 시간(분) 동안 가지 않은 워크스페이스의 앱 메모리를 윈도우가 압축/페이지아웃하게 합니다.
# 돌아가면 잠깐 버벅일 수 있습니다. 0이면 쓰지 않습니다.
sleep_after_minutes = 0
"#;

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct Config {
    pub trigger: String,
    pub step_x: i32,
    pub step_y: i32,
    pub vertical_sticky: bool,
    /// Degrees either side of straight up or down within which a movement
    /// counts as vertical.
    pub vertical_angle: f32,
    pub edge_create_pushes: u32,
    pub carry_modifier: String,
    pub hotkeys: bool,
    pub sleep_after_minutes: u32,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            trigger: "xbutton2".into(),
            step_x: 260,
            step_y: 320,
            vertical_sticky: true,
            // 2:1, vertical to horizontal.
            vertical_angle: 26.6,
            edge_create_pushes: 0,
            carry_modifier: "shift".into(),
            hotkeys: true,
            sleep_after_minutes: 0,
        }
    }
}

/// The narrowest and the widest `vertical_angle` made use of.
pub const VERTICAL_ANGLES: (f32, f32) = (10.0, 70.0);

impl Config {
    /// How many times more vertical than horizontal a movement must be to
    /// count as vertical.
    pub fn vertical_bias(&self) -> f32 {
        1.0 / self.vertical_angle.clamp(VERTICAL_ANGLES.0, VERTICAL_ANGLES.1).to_radians().tan()
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

    /// Changing a setting from the settings window rewrites the user's file:
    /// their comments and other values must survive, a key that only starts
    /// like the one being set must not be mistaken for it, and a key missing
    /// from an older file must be added.
    #[test]
    fn setting_a_value_keeps_the_rest_of_the_file() {
        let file = "# mine\ntrigger_note = 1\ntrigger = \"xbutton2\"  # forward\n\nstep_x = 260\n";
        let changed = with_value(file, "trigger", "\"f13\"");
        assert_eq!(changed, "# mine\ntrigger_note = 1\ntrigger = \"f13\"\n\nstep_x = 260\n");
        let added = with_value(&changed, "vertical_angle", "30");
        assert_eq!(added, format!("{changed}vertical_angle = 30\n"));
        let config: Config = toml::from_str(&added).unwrap();
        assert_eq!((config.trigger.as_str(), config.step_x, config.vertical_angle), ("f13", 260, 30.0));
    }
}
