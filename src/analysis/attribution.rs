//! Channel attribution for tune steps: chassis and gearing changes leave
//! fingerprints in different places, so a step's lap-time delta can be
//! *partially* attributed by where on the route the time moved.
//!
//! Two levels, both classified on the BASELINE profile (the change being
//! judged must not reclassify the road it is judged on):
//!
//! - corner vs straight: bins where the baseline ran the tires at a
//!   meaningful share of the slip limit are cornering road.
//! - entry vs exit within each corner: split at the corner's minimum-speed
//!   bin (the apex). Phase location is the driver-mask-proof fingerprint —
//!   adapting around a bad setting converts its behaviour into time, and the
//!   time still lands in the phase the setting hurts (diff decel / brake
//!   balance -> entry; diff accel -> exit; roll/aero -> throughout).
//!
//! The split is a heuristic, not a measurement: corner-exit traction is
//! shared between chassis and driveline, so attributed conclusions are capped
//! at Medium confidence and always carry the split as evidence.

use super::profile::StintProfile;

/// Mean combined-slip level (1.0 = grip limit) above which a bin counts as
/// cornering. Calibrated on the real McLaren F1 session: 33% of bins sit
/// below 0.2 (clear straights); >=0.4 classes a stable 55% of lap time as
/// cornering across stints.
pub const CORNER_SLIP_THRESHOLD: f32 = 0.4;
/// Sub-threshold gaps up to this many bins (x10 m) inside a corner run are
/// bridged — mid-corner slip dips must not split one corner into two.
const GAP_BINS: usize = 2;

#[derive(Debug, Clone, Copy)]
pub struct Attribution {
    /// Time delta summed over cornering bins (chassis-dominated road).
    /// Always equals entry_delta_s + exit_delta_s.
    pub corner_delta_s: f32,
    /// Time delta summed over straight bins (gearing-dominated road).
    pub straight_delta_s: f32,
    /// Share of the baseline's lap time classed as cornering.
    pub corner_share: f32,
    /// Corner delta before each corner's apex (turn-in to minimum speed).
    pub entry_delta_s: f32,
    /// Corner delta from each apex onward.
    pub exit_delta_s: f32,
    /// Corner runs found on the baseline.
    pub corners: usize,
}

/// Split a comparison's per-bin time delta by baseline road class and corner
/// phase.
pub fn split_delta(baseline: &StintProfile, bin_delta_s: &[f32]) -> Attribution {
    let bins = &baseline.composite.bins;
    let n = bin_delta_s.len().min(bins.len());

    // Corner bins by slip, then bridge short sub-threshold gaps BETWEEN
    // corner bins so one corner stays one run.
    let mut corner: Vec<bool> = bins[..n]
        .iter()
        .map(|b| (b.slip_front + b.slip_rear) / 2.0 >= CORNER_SLIP_THRESHOLD)
        .collect();
    let mut i = 0;
    while i < n {
        if corner[i] {
            i += 1;
            continue;
        }
        let start = i;
        while i < n && !corner[i] {
            i += 1;
        }
        if start > 0 && i < n && i - start <= GAP_BINS {
            corner[start..i].iter_mut().for_each(|c| *c = true);
        }
    }

    let mut a = Attribution {
        corner_delta_s: 0.0,
        straight_delta_s: 0.0,
        corner_share: 0.0,
        entry_delta_s: 0.0,
        exit_delta_s: 0.0,
        corners: 0,
    };
    let (mut t_corner, mut t_total) = (0.0f32, 0.0f32);
    let mut i = 0;
    while i < n {
        t_total += bins[i].time_s;
        if !corner[i] {
            a.straight_delta_s += bin_delta_s[i];
            i += 1;
            continue;
        }
        let start = i;
        while i < n && corner[i] {
            t_total += if i > start { bins[i].time_s } else { 0.0 };
            t_corner += bins[i].time_s;
            i += 1;
        }
        let run = start..i;
        let apex = run
            .clone()
            .min_by(|x, y| bins[*x].speed_avg.total_cmp(&bins[*y].speed_avg))
            .unwrap_or(start);
        a.entry_delta_s += bin_delta_s[start..apex].iter().sum::<f32>();
        a.exit_delta_s += bin_delta_s[apex..run.end].iter().sum::<f32>();
        a.corners += 1;
    }
    a.corner_delta_s = a.entry_delta_s + a.exit_delta_s;
    a.corner_share = if t_total > 0.0 { t_corner / t_total } else { 0.0 };
    a
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::profile::{build_composite, BinStats, LapProfile};

    fn profile_with(bins_spec: &[(f32, f32)]) -> StintProfile {
        let bins: Vec<BinStats> = bins_spec
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
        let baseline = profile_with(&road);

        let mut delta = vec![0.03f32; 10]; // corners: lost time
        delta.extend(vec![-0.01f32; 10]); // straights: gained time
        let a = split_delta(&baseline, &delta);
        assert!((a.corner_delta_s - 0.3).abs() < 1e-4, "{}", a.corner_delta_s);
        assert!((a.straight_delta_s + 0.1).abs() < 1e-4, "{}", a.straight_delta_s);
        // Corners are slow bins: they dominate lap time here.
        assert!(a.corner_share > 0.6, "{}", a.corner_share);
        assert_eq!(a.corners, 1);
    }

    /// A corner with a speed trough: pre-apex delta is entry, apex-onward is
    /// exit, and the two always sum to the corner delta.
    #[test]
    fn corner_delta_splits_at_the_apex() {
        // straight, then a corner decelerating 40->20 then back out to 40.
        let mut road = vec![(60.0, 0.1); 5];
        for v in [40.0, 30.0, 20.0, 30.0, 40.0] {
            road.push((v, 0.8));
        }
        road.extend(vec![(60.0, 0.1); 5]);
        let baseline = profile_with(&road);

        let mut delta = vec![0.0f32; 15];
        delta[5] = 0.10; // entry bins
        delta[6] = 0.10;
        delta[8] = 0.02; // exit bins
        delta[9] = 0.02;
        let a = split_delta(&baseline, &delta);
        assert!((a.entry_delta_s - 0.20).abs() < 1e-4, "{}", a.entry_delta_s);
        assert!((a.exit_delta_s - 0.04).abs() < 1e-4, "{}", a.exit_delta_s);
        assert!((a.corner_delta_s - 0.24).abs() < 1e-4);
        assert_eq!(a.corners, 1);
    }

    /// A short sub-threshold dip mid-corner must not split the corner (the
    /// apex would land wrong); a long straight between corners must.
    #[test]
    fn gap_bridging_keeps_one_corner_one_run() {
        let mut road = vec![(60.0, 0.1); 3];
        road.extend(vec![(30.0, 0.8); 3]);
        road.push((25.0, 0.2)); // 1-bin dip at the slowest point
        road.extend(vec![(30.0, 0.8); 3]);
        road.extend(vec![(60.0, 0.1); 3]);
        let baseline = profile_with(&road);
        let delta = vec![0.01f32; 13];
        let a = split_delta(&baseline, &delta);
        assert_eq!(a.corners, 1, "dip bridged into one corner");
        // The dip bin is the apex (slowest): 3 entry bins, apex + 3 exit bins.
        assert!((a.entry_delta_s - 0.03).abs() < 1e-4, "{}", a.entry_delta_s);
        assert!((a.exit_delta_s - 0.04).abs() < 1e-4, "{}", a.exit_delta_s);

        let mut road = vec![(30.0, 0.8); 3];
        road.extend(vec![(60.0, 0.1); 6]);
        road.extend(vec![(30.0, 0.8); 3]);
        let a = split_delta(&profile_with(&road), &vec![0.01f32; 12]);
        assert_eq!(a.corners, 2, "real straight keeps corners separate");
    }
}
