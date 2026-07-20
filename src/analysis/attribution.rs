//! Channel attribution for compound tune steps: chassis and gearing changes
//! leave fingerprints in different places, so a compound step's lap-time delta
//! can be *partially* attributed by where on the route the time moved.
//!
//! Chassis (roll stiffness) effects live in cornering segments — bins where
//! the baseline ran the tires at a meaningful share of the slip limit. Gearing
//! effects live in straights/acceleration zones. The split is a heuristic, not
//! a measurement: corner-exit traction is shared between both (a diff/gearing
//! change alters wheelspin out of corners), so attributed conclusions are
//! capped at Medium confidence and always carry the split as evidence.

use super::profile::StintProfile;

/// Mean combined-slip level (1.0 = grip limit) above which a bin counts as
/// cornering. Calibrated on the real McLaren F1 session: 33% of bins sit
/// below 0.2 (clear straights); >=0.4 classes a stable 55% of lap time as
/// cornering across stints.
pub const CORNER_SLIP_THRESHOLD: f32 = 0.4;

#[derive(Debug, Clone, Copy)]
pub struct Attribution {
    /// Time delta summed over cornering bins (chassis-dominated road).
    pub corner_delta_s: f32,
    /// Time delta summed over straight bins (gearing-dominated road).
    pub straight_delta_s: f32,
    /// Share of the baseline's lap time classed as cornering.
    pub corner_share: f32,
}

/// Split a comparison's per-bin time delta into cornering vs straight road.
/// Bins are classed by the BASELINE profile's slip levels — the change being
/// judged must not reclassify the road it is judged on.
pub fn split_delta(baseline: &StintProfile, bin_delta_s: &[f32]) -> Attribution {
    let (mut corner, mut straight) = (0.0f32, 0.0f32);
    let (mut t_corner, mut t_total) = (0.0f32, 0.0f32);
    for (i, d) in bin_delta_s.iter().enumerate() {
        let bin = &baseline.composite.bins[i];
        let slip = (bin.slip_front + bin.slip_rear) / 2.0;
        if slip >= CORNER_SLIP_THRESHOLD {
            corner += d;
            t_corner += bin.time_s;
        } else {
            straight += d;
        }
        t_total += bin.time_s;
    }
    Attribution {
        corner_delta_s: corner,
        straight_delta_s: straight,
        corner_share: if t_total > 0.0 { t_corner / t_total } else { 0.0 },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::profile::{build_composite, BinStats, LapProfile};

    fn profile_with_slip(speeds_slips: &[(f32, f32)]) -> StintProfile {
        let bins: Vec<BinStats> = speeds_slips
            .iter()
            .map(|(v, slip)| BinStats {
                time_s: 10.0 / v,
                speed_avg: *v,
                slip_front: *slip,
                slip_rear: *slip,
                samples: 5,
                ..Default::default()
            })
            .collect();
        let lap = LapProfile {
            lap_number: 1,
            time_s: bins.iter().map(|b| b.time_s).sum(),
            standing_start: false,
            bins,
        };
        let shared = lap.bins.len();
        let laps = vec![lap];
        StintProfile {
            shared_bins: shared,
            composite: build_composite(&laps, shared),
            best_lap_time_s: laps[0].time_s,
            standing_start_only: false,
            car_ordinal: 1,
            laps,
        }
    }

    /// Time lost in high-slip bins lands in the corner bucket; time moved in
    /// low-slip bins lands in the straight bucket.
    #[test]
    fn delta_splits_by_baseline_slip() {
        // 10 corner bins (slip 0.8) then 10 straight bins (slip 0.1).
        let mut road = vec![(30.0, 0.8); 10];
        road.extend(vec![(60.0, 0.1); 10]);
        let baseline = profile_with_slip(&road);

        let mut delta = vec![0.03f32; 10]; // corners: lost time
        delta.extend(vec![-0.01f32; 10]); // straights: gained time
        let a = split_delta(&baseline, &delta);
        assert!((a.corner_delta_s - 0.3).abs() < 1e-4, "{}", a.corner_delta_s);
        assert!((a.straight_delta_s + 0.1).abs() < 1e-4, "{}", a.straight_delta_s);
        // Corners are slow bins: they dominate lap time here.
        assert!(a.corner_share > 0.6, "{}", a.corner_share);
    }
}
