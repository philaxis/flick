//! Decayed credits for active stays: saved cells and runtime window handles.

use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

pub const CELL_HALF_LIFE: u64 = 3 * 24 * 60 * 60;
pub const WINDOW_HALF_LIFE: u64 = 60 * 60;
pub const CELL_TAU: f64 = 5.0 * 60.0;
pub const WINDOW_TAU: f64 = 2.0 * 60.0;
const LEAST: f64 = 0.000001;

pub fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Score {
    pub score: f64,
    /// Unix seconds when credits were last added.
    pub updated: u64,
}

impl Score {
    /// Decayed credits without changing history on reads. A clock going
    /// backwards must not make a score grow.
    pub fn value(&self, now: u64, half_life: u64) -> f64 {
        if !self.score.is_finite() || self.score <= 0.0 {
            return 0.0;
        }
        let value = self.score * 0.5_f64.powf(now.saturating_sub(self.updated) as f64 / half_life as f64);
        if value < LEAST { 0.0 } else { value }
    }

    pub fn record(&mut self, now: u64, credit: f64, half_life: u64) {
        self.score = self.value(now, half_life) + credit;
        self.updated = now;
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CellUsage {
    /// Decayed active-stay credits; the name preserves older state files.
    pub visits: Score,
}

/// A stay earns at most one credit, however long it lasts.
pub fn credit(active: f64, tau: f64) -> f64 {
    if active < 3.0 { 0.0 } else { -(-active / tau).exp_m1() }
}

/// Only actual arrivals and foreground changes start stays. Queued cells and
/// board selections never reach `switch`. Time is sampled on the config timer.
#[derive(Default)]
pub struct Stay<T> {
    pub current: Option<T>,
    active: f64,
    ticked: u64,
}

impl<T: PartialEq> Stay<T> {
    /// Ends the previous stay once; duplicate observations keep it intact.
    pub fn switch(&mut self, current: Option<T>, millis: u64) -> Option<(T, f64)> {
        if self.current == current {
            return None;
        }
        let previous = self.current.take().map(|key| (key, self.active));
        self.current = current;
        self.active = 0.0;
        self.ticked = millis;
        previous
    }

    /// A failed input query is idle too. Advance the clock even while idle,
    /// so a later input cannot claim that time.
    pub fn tick(&mut self, millis: u64, input_age: Option<u32>) -> bool {
        let elapsed = millis.saturating_sub(self.ticked);
        self.ticked = millis;
        if self.current.is_some() && input_age.is_some_and(|age| age <= 60_000) {
            self.active += elapsed as f64 / 1000.0;
            return elapsed > 0;
        }
        false
    }

    pub fn credit(&self, tau: f64) -> f64 {
        credit(self.active, tau)
    }
}

/// Area factor: no history keeps equal sizes, otherwise unused windows get
/// the floor and the most used window gets the ceiling.
pub fn preview_factor(score: f64, maximum: f64) -> f32 {
    if maximum <= 0.0 { 1.0 } else { 0.7 + 0.8 * normalised(score, maximum) }
}

/// A share of the highest score, bounded even for values outside that range.
pub fn normalised(score: f64, maximum: f64) -> f32 {
    if !score.is_finite() || !maximum.is_finite() || maximum <= 0.0 {
        0.0
    } else {
        (score / maximum).clamp(0.0, 1.0) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credit_curve_is_bounded_for_even_a_ten_hour_stay() {
        for tau in [CELL_TAU, WINDOW_TAU] {
            for active in [0.0, 1.0, 2.999] { assert_eq!(credit(active, tau), 0.0); }
            assert!(credit(3.0, tau) > 0.0);
            assert!((credit(tau, tau) - (1.0 - (-1.0_f64).exp())).abs() < 1e-12);
            let mut stay = Stay::default();
            stay.switch(Some(1), 0);
            for millis in (2000..=36_000_000).step_by(2000) {
                stay.tick(millis, Some(0));
                assert!(stay.credit(tau) <= 1.0);
            }
            let (_, active) = stay.switch(None, 36_000_000).unwrap();
            assert!(credit(active, tau) > 0.999999);
            assert!(credit(active, tau) <= 1.0);
        }
    }

    #[test]
    fn idle_ticks_and_absent_foregrounds_accrue_nothing() {
        let mut stay = Stay::default();
        stay.switch(Some(1), 0);
        assert!(!stay.tick(2000, Some(60_001)));
        assert!(!stay.tick(4000, None));
        assert_eq!(stay.active, 0.0);
        assert!(stay.tick(6000, Some(60_000)));
        assert_eq!(stay.active, 2.0);
        stay.switch(None, 6000);
        assert!(!stay.tick(8000, Some(0)));
        stay.switch(Some(2), 9000);
        stay.tick(10_000, Some(0));
        assert_eq!(stay.active, 1.0, "time before becoming foreground is excluded");
    }

    #[test]
    fn stays_survive_duplicate_observations_and_snapshots() {
        let mut stay = Stay::default();
        stay.switch(Some("a"), 0);
        stay.tick(4000, Some(0));
        assert!(stay.switch(Some("a"), 4000).is_none());
        let history = Score::default();
        for _ in 0..10 {
            let mut saved = history.clone();
            saved.record(100, stay.credit(CELL_TAU), CELL_HALF_LIFE);
            assert_eq!(saved.score, credit(4.0, CELL_TAU));
        }
        assert_eq!(history.score, 0.0);
        // Only settled arrivals are supplied: intermediate cells have no stay.
        let (previous, active) = stay.switch(Some("destination"), 5000).unwrap();
        assert_eq!((previous, active), ("a", 4.0));
        assert_eq!(stay.current, Some("destination"));
        assert_eq!(stay.credit(CELL_TAU), 0.0);
    }

    #[test]
    fn cells_decay_over_three_days_and_windows_over_one_hour() {
        for half_life in [CELL_HALF_LIFE, WINDOW_HALF_LIFE] {
            let mut score = Score::default();
            score.record(100, 1.0, half_life);
            assert_eq!(score.value(100 + half_life, half_life), 0.5);
            let mut restored: Score = serde_json::from_str(&serde_json::to_string(&score).unwrap()).unwrap();
            restored.record(100 + half_life, 1.0, half_life);
            assert_eq!(restored.score, 1.5);
            assert_eq!(restored.value(0, half_life), 1.5);
            assert_eq!(score.value(100 + 21 * half_life, half_life), 0.0);
            assert_eq!(Score { score: -1.0, updated: 0 }.value(0, half_life), 0.0);
        }
        assert_eq!(WINDOW_HALF_LIFE, 3600);
        let mut score = Score::default();
        score.record(100, credit(3.0, CELL_TAU), CELL_HALF_LIFE);
        assert!(score.value(100, CELL_HALF_LIFE) > 0.0);
    }

    #[test]
    fn area_factors_are_bounded_and_each_side_uses_the_square_root() {
        assert_eq!(preview_factor(0.0, 0.0), 1.0);
        for (score, expected) in [(0.0, 0.7), (5.0, 1.1), (10.0, 1.5), (20.0, 1.5), (-1.0, 0.7)] {
            let factor = preview_factor(score, 10.0);
            assert!((factor - expected).abs() < 0.0001);
            let side = factor.sqrt();
            assert!((side * side - expected).abs() < 0.0001);
        }
        assert_eq!(normalised(0.0, 0.0), 0.0);
        assert_eq!(normalised(5.0, 10.0), 0.5);
        assert_eq!(normalised(20.0, 10.0), 1.0);
    }
}
