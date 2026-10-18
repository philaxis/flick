//! Decaying visit counts, saved by cell and app identity rather than window handle.

use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, time::{SystemTime, UNIX_EPOCH}};

/// Three days, in seconds: old habits gradually give way to new ones.
const HALF_LIFE: u64 = 3 * 24 * 60 * 60;
const LEAST: f64 = 0.01;

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Score {
    pub score: f64,
    pub updated: u64,
}

impl Score {
    /// The count as it is now, without a write on every read. A clock going
    /// backwards must not make a count grow.
    pub fn value(&self, now: u64) -> f64 {
        if !self.score.is_finite() || self.score <= 0.0 {
            return 0.0;
        }
        let value = self.score * 0.5_f64.powf(now.saturating_sub(self.updated) as f64 / HALF_LIFE as f64);
        if value < LEAST { 0.0 } else { value }
    }

    pub fn record(&mut self, now: u64) {
        self.score = self.value(now) + 1.0;
        self.updated = now;
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CellUsage {
    pub visits: Score,
    pub windows: BTreeMap<String, Score>,
}

/// A missing history leaves the old sizes alone; otherwise unused apps get
/// the floor and the most used app the ceiling.
pub fn preview_factor(score: f64, maximum: f64) -> f32 {
    if maximum <= 0.0 { 1.0 } else { 0.7 + 0.8 * normalised(score, maximum) }
}

/// A share of the highest count, bounded even if a caller supplies a value
/// outside that range.
pub fn normalised(score: f64, maximum: f64) -> f32 {
    if !score.is_finite() || !maximum.is_finite() || maximum <= 0.0 {
        0.0
    } else {
        (score / maximum).clamp(0.0, 1.0) as f32
    }
}

/// Matches the board's app grouping, without persisting a window handle.
pub fn app_key(icon: u64, exe: &str, name: &str) -> Option<String> {
    if icon != 0 {
        Some(format!("icon:{icon}"))
    } else {
        let name = if exe.is_empty() { name } else { exe };
        (!name.is_empty()).then(|| format!("app:{}", name.to_lowercase()))
    }
}

/// Only runtime state: repeated notifications of the same foreground window
/// within a second are not another use.
#[derive(Default)]
pub struct Foreground {
    last: Option<(isize, u64)>,
}

impl Foreground {
    pub fn record(&mut self, window: isize, millis: u64) -> bool {
        if self.last.is_some_and(|(last, at)| last == window && millis.saturating_sub(at) < 1000) {
            return false;
        }
        self.last = Some((window, millis));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visits_decay_and_resume_after_a_restart() {
        let mut score = Score::default();
        score.record(100);
        assert_eq!(score.value(100 + HALF_LIFE), 0.5);
        let mut restored: Score = serde_json::from_str(&serde_json::to_string(&score).unwrap()).unwrap();
        restored.record(100 + HALF_LIFE);
        assert_eq!(restored.score, 1.5);
        assert_eq!(restored.value(0), 1.5);
        assert_eq!(score.value(100 + 7 * HALF_LIFE), 0.0);
        assert_eq!(Score { score: -1.0, updated: 0 }.value(0), 0.0);
    }

    #[test]
    fn normalisation_and_preview_sizes_are_bounded() {
        assert_eq!(preview_factor(0.0, 0.0), 1.0);
        assert_eq!(preview_factor(0.0, 10.0), 0.7);
        assert!((preview_factor(5.0, 10.0) - 1.1).abs() < 0.0001);
        assert_eq!(preview_factor(10.0, 10.0), 1.5);
        assert_eq!(preview_factor(20.0, 10.0), 1.5);
        assert_eq!(preview_factor(-1.0, 10.0), 0.7);
        assert_eq!(normalised(0.0, 0.0), 0.0);
        assert_eq!(normalised(5.0, 10.0), 0.5);
        assert_eq!(normalised(20.0, 10.0), 1.0);
    }

    #[test]
    fn identities_and_debounce_do_not_depend_on_saved_handles() {
        assert_eq!(app_key(9, "a.exe", "A"), app_key(9, "b.exe", "B"));
        assert_eq!(app_key(0, "A.EXE", "A"), Some("app:a.exe".into()));
        assert_eq!(app_key(0, "", "App"), Some("app:app".into()));
        assert_eq!(app_key(0, "", ""), None);
        let mut foreground = Foreground::default();
        assert!(foreground.record(1, 0));
        assert!(!foreground.record(1, 999));
        assert!(foreground.record(1, 1000));
        assert!(foreground.record(2, 1001));
    }
}
