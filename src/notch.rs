//! A display position in cells, shared by every view of the hand.
//! Strength 0..1 uses t - k sin(2πt)/(2π). Above 1, compose the full
//! notch with another of strength k-1 (up to 2), keeping every slope ≥ 0
//! while widening the hold near each integer. Travel and selection stay raw.

pub const DEFAULT_X: u32 = 40;
/// A touchpad's vertical 100% composes two full notches; the mouse uses one.
pub const TOUCHPAD_VERTICAL_SCALE: u32 = 2;
pub const MAX_STRENGTH: f32 = 2.0;

/// Periodic over the whole line, including negative cells and wall give.
/// Strength is bounded to 0..2; integers stay fixed at every strength.
pub fn position(p: f32, strength: f32) -> f32 {
    let strength = strength.max(0.0).min(MAX_STRENGTH);
    let whole = p.floor();
    let mut t = p - whole;
    let bend = |t: f32, k: f32| t - k * (std::f32::consts::TAU * t).sin() / std::f32::consts::TAU;
    t = bend(t, strength.min(1.0));
    if strength > 1.0 {
        t = bend(t, strength - 1.0);
    }
    whole + t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_symmetry_identity_and_monotonicity() {
        for k in [0.0, 0.4, 1.0, 1.5, 2.0] {
            assert_eq!(position(0.0, k), 0.0);
            assert_eq!(position(1.0, k), 1.0);
            let mut previous = position(-2.2, k);
            for i in -2200..=3200 {
                let p = i as f32 / 1000.0;
                let shown = position(p, k);
                assert!(shown >= previous - 1e-6, "p={p}, k={k}");
                assert!((position(1.0 - p, k) - (1.0 - shown)).abs() < 1e-6);
                if k == 0.0 {
                    assert!((shown - p).abs() < 1e-6);
                }
                previous = shown;
            }
        }
    }

    #[test]
    fn periodic_boundaries_are_continuous_with_the_expected_slope() {
        let h = 0.001;
        for (k, slope) in [(0.0, 1.0), (0.4, 0.6), (1.0, 0.0), (1.5, 0.0), (2.0, 0.0)] {
            for n in -3..=3 {
                let p = n as f32;
                assert_eq!(position(p, k), p);
                for side in [-1.0, 1.0] {
                    let delta = position(p + side * h, k) - p;
                    assert!(delta.abs() <= h * 1.001);
                    assert!((delta / (side * h) - slope).abs() < 0.001, "p={p}, k={k}");
                }
            }
        }
        assert!(position(0.2, 2.0) < position(0.2, 1.0));
    }
}
