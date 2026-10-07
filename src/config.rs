//! User settings (`config.toml`) and the saved grid (`state.json`), both in
//! `%APPDATA%\KanKan`.

use crate::grid::Grid;

/// The app's display name. Also used for the install folder, the settings
/// folder, the Start menu shortcut and the registry entries; the exe name in
/// Cargo.toml and the icon resource (scripts/make-icon.py) follow it too.
pub const APP_NAME: &str = "KanKan";
/// Settings folder of versions before the app had a name.
const OLD_DIR: &str = "desk2d";
use serde::Deserialize;
use std::{fs, path::PathBuf};

const DEFAULT_CONFIG: &str = r#"# KanKan 설정. 저장한 뒤 트레이 아이콘 메뉴의 "설정 다시 읽기"를 누르면 적용됩니다.

# 누르고 있는 동안 마우스를 움직이면 칸을 이동하는 버튼.
#   마우스: "xbutton1"(뒤로), "xbutton2"(앞으로), "middle"(휠 버튼)
#   키보드: "capslock", "scrolllock", "pause", "apps", "ralt", "rctrl", "f13" ~ "f24"
trigger = "xbutton2"

# 한 칸 넘어가는 데 필요한 이동 거리(픽셀). 세로를 더 길게 두면 실수로 행이 바뀌지 않습니다.
step_x = 140
step_y = 180

# 격자 가장자리에서 이 횟수만큼 더 밀면 새 칸/새 행을 만듭니다. 0이면 만들지 않습니다.
edge_create_pushes = 2

# 제스처 중에 이 키를 같이 누르고 있으면 지금 활성 창을 들고 이동합니다. "shift", "ctrl", "alt"
carry_modifier = "shift"

# Ctrl+Alt+Win+방향키로 이동 (Shift를 더하면 창을 들고 이동), Ctrl+Alt+Win+P로 활성 창 고정 메뉴
hotkeys = true

# 이 시간(분) 동안 가지 않은 행의 앱 메모리를 윈도우가 압축/페이지아웃하게 합니다.
# 돌아가면 잠깐 버벅일 수 있습니다. 0이면 쓰지 않습니다.
sleep_after_minutes = 0
"#;

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct Config {
    pub trigger: String,
    pub step_x: i32,
    pub step_y: i32,
    pub edge_create_pushes: u32,
    pub carry_modifier: String,
    pub hotkeys: bool,
    pub sleep_after_minutes: u32,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            trigger: "xbutton2".into(),
            step_x: 140,
            step_y: 180,
            edge_create_pushes: 2,
            carry_modifier: "shift".into(),
            hotkeys: true,
            sleep_after_minutes: 0,
        }
    }
}

fn dir() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_default();
    let dir = base.join(APP_NAME);
    let old = base.join(OLD_DIR);
    if !dir.exists() && old.exists() {
        let _ = fs::rename(old, &dir);
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

pub fn load_grid() -> Grid {
    fs::read_to_string(state_path())
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

pub fn save_grid(grid: &Grid) {
    let _ = fs::create_dir_all(dir());
    if let Ok(text) = serde_json::to_string_pretty(grid) {
        let _ = fs::write(state_path(), text);
    }
}
