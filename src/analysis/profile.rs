//! Distance-binned lap profiles and the spliced "ideal lap" composite.
//!
//! Laps are resampled onto 10 m distance bins so the same piece of road lines up
//! across laps and sessions. The ideal lap is composited by splicing laps together
//! ONLY at points where their speeds match AND neither lap is cornering: between
//! two such crossovers a lap's span is taken whole, judged on total elapsed time.
//! This keeps a corner's entry chained to its exit. Speed match alone is not
//! enough inside a corner: a lap driven slow-in/fast-out passes through the
//! pack's speed near the apex, but its early-power exit is only reachable from
//! its own entry line — splicing the pack's fast entry onto that exit fabricates
//! a physically impossible corner. Corners (slip-classified, braking zones
//! included) are therefore atomic: a differently-driven corner is adopted whole
//! together with the exit advantage it carries, or not at all.

use super::attribution::corner_mask;
use super::{LapSlice, TimedFrame, driving_segments, split_laps};
use std::collections::BTreeSet;

pub const BIN_METERS: f32 = 10.0;
/// Laps may only be spliced at bins where their speeds match within this tolerance.
pub const SPLICE_SPEED_TOLERANCE_MPS: f32 = 2.5;
/// A span is only taken from another lap when it beats the base by this margin.
/// Keeps the composite anchored on the real best lap instead of chasing noise.
pub const SPLICE_MIN_GAIN_S: f32 = 0.03;
/// A lap must start within this many seconds of its beginning to be profiled.
const LAP_START_TOLERANCE_S: f32 = 1.0;
/// A lap's bin is a data hole when its time is below this share of the
/// cross-lap median time for the same bin. A route-spline teleport spreads one
/// frame's ~16 ms across every bin it crossed, leaving bins that claim 10 m of
/// road in near-zero time (2.5 s vanished from one real lap this way). The test
/// is relative to the other laps because DistanceTraveled is spline progress,
/// not meters — its scale versus true speed varies by track — so no absolute
/// time/speed/distance check holds. Hole bins never contribute fabricated pace:
/// the splicer charges them the median time, and they cannot corroborate.
const HOLE_TIME_SHARE: f32 = 0.5;

/// Per-bin cross-lap median time: the consensus cost of each bin of road.
fn median_bin_times(laps: &[LapProfile], shared: usize) -> Vec<f32> {
    let mut buf: Vec<f32> = Vec::with_capacity(laps.len());
    (0..shared)
        .map(|b| {
            buf.clear();
            buf.extend(laps.iter().map(|l| l.bins[b].time_s));
            buf.sort_by(f32::total_cmp);
            buf[buf.len() / 2]
        })
        .collect()
}

#[derive(Debug, Clone, Copy, Default)]
pub struct BinStats {
    pub time_s: f32,
    pub speed_avg: f32,
    /// Mean |combined slip| front / rear.
    pub slip_front: f32,
    pub slip_rear: f32,
    /// Fraction of bin samples with brake pressed.
    pub brake_frac: f32,
    pub samples: u32,
}

#[derive(Debug, Clone)]
pub struct LapProfile {
    pub lap_number: u16,
    pub time_s: f32,
    pub standing_start: bool,
    /// Lap clock stayed locked to the race clock: a point-to-point run
    /// (circuit lap 0 resets the lap clock at the start line). See `LapSlice`.
    pub point_to_point: bool,
    pub bins: Vec<BinStats>,
}

/// Profile one lap. None if the lap can't be profiled: no authoritative time,
/// distance not live (free roam), or the slice doesn't cover the lap from its start.
pub fn lap_profile(lap: &LapSlice) -> Option<LapProfile> {
    let time_s = lap.time_s?;
    let first = lap.frames.first()?.frame;
    let last = lap.frames.last()?.frame;
    if last.distance_traveled - first.distance_traveled < BIN_METERS {
        return None; // DistanceTraveled dead (free roam) or degenerate slice
    }
    if first.current_lap > LAP_START_TOLERANCE_S {
        return None; // slice starts mid-lap (e.g. capture began during a lap)
    }

    let n_bins = ((last.distance_traveled - first.distance_traveled) / BIN_METERS) as usize + 1;
    let mut time = vec![0.0f32; n_bins];
    let mut speed = vec![0.0f32; n_bins];
    let mut slip_f = vec![0.0f32; n_bins];
    let mut slip_r = vec![0.0f32; n_bins];
    let mut brake = vec![0u32; n_bins];
    let mut samples = vec![0u32; n_bins];

    let mut prev_race_t = first.current_race_time;
    let mut prev_bin: Option<usize> = None;
    for tf in lap.frames {
        let f = &tf.frame;
        let d = f.distance_traveled - first.distance_traveled;
        let bin = ((d / BIN_METERS) as usize).min(n_bins - 1);
        let dt_s = (f.current_race_time - prev_race_t).clamp(0.0, 1.0);
        prev_race_t = f.current_race_time;

        // DistanceTraveled is route-spline progress and can snap forward 10-20m in
        // one frame (see telemetry.md); spread such a hop across the bins it
        // crossed so none are left empty (they'd read as phantom 0-speed bins).
        let start = match prev_bin {
            Some(pb) if bin > pb + 1 => pb + 1,
            _ => bin,
        };
        prev_bin = Some(bin);
        let share = dt_s / (bin - start + 1) as f32;
        for b in start..=bin {
            time[b] += share;
            speed[b] += f.speed;
            slip_f[b] += (f.tire_combined_slip.fl.abs() + f.tire_combined_slip.fr.abs()) / 2.0;
            slip_r[b] += (f.tire_combined_slip.rl.abs() + f.tire_combined_slip.rr.abs()) / 2.0;
            brake[b] += (f.brake >= 128) as u32;
            samples[b] += 1;
        }
    }

    let bins = (0..n_bins)
        .map(|i| {
            let n = samples[i].max(1) as f32;
            BinStats {
                time_s: time[i],
                speed_avg: speed[i] / n,
                slip_front: slip_f[i] / n,
                slip_rear: slip_r[i] / n,
                brake_frac: brake[i] as f32 / n,
                samples: samples[i],
            }
        })
        .collect();

    Some(LapProfile {
        lap_number: first.lap_number,
        time_s,
        standing_start: lap.standing_start,
        point_to_point: lap.point_to_point,
        bins,
    })
}

/// The spliced ideal lap: per-bin stats and which lap each bin came from.
#[derive(Debug)]
pub struct Composite {
    pub bins: Vec<BinStats>,
    /// Index into the session's laps, per bin.
    pub source: Vec<usize>,
    pub time_s: f32,
}

impl Composite {
    /// Number of contiguous same-source spans the composite is stitched from.
    pub fn span_count(&self) -> usize {
        1 + self.source.windows(2).filter(|w| w[0] != w[1]).count()
    }

    pub fn source_laps(&self) -> BTreeSet<usize> {
        self.source.iter().copied().collect()
    }
}

/// Build the composite over the first `shared` bins. Starts from the fastest lap;
/// each other lap may replace spans between equal-speed crossovers, judged on the
/// span's total time.
pub fn build_composite(laps: &[LapProfile], shared: usize) -> Composite {
    build_composite_with_tolerance(laps, shared, SPLICE_SPEED_TOLERANCE_MPS)
}

/// `build_composite` with the seam speed tolerance as a parameter, for
/// sensitivity probes of the splice-bonus mechanism. Production always uses
/// `SPLICE_SPEED_TOLERANCE_MPS`.
pub fn build_composite_with_tolerance(
    laps: &[LapProfile],
    shared: usize,
    tolerance_mps: f32,
) -> Composite {
    let base = laps
        .iter()
        .enumerate()
        .min_by(|a, b| a.1.time_s.total_cmp(&b.1.time_s))
        .map(|(i, _)| i)
        .unwrap_or(0);
    let med = median_bin_times(laps, shared);
    let mut bins: Vec<BinStats> = laps[base].bins[..shared].to_vec();
    // Repair the base's own holes up front so the composite's time is honest.
    for (bin, m) in bins.iter_mut().zip(&med) {
        if bin.time_s < HOLE_TIME_SHARE * m {
            bin.time_s = *m;
        }
    }
    let mut source = vec![base; shared];

    for (li, lap) in laps.iter().enumerate() {
        if li == base {
            continue;
        }
        // Splice boundaries: bins where the candidate's speed matches the
        // composite's, on road neither lap is cornering (corners are atomic).
        let comp_corner = corner_mask(&bins);
        let cand_corner = corner_mask(&lap.bins[..shared]);
        let mut bounds = vec![0usize];
        bounds.extend((0..shared).filter(|b| {
            !comp_corner[*b]
                && !cand_corner[*b]
                && (bins[*b].speed_avg - lap.bins[*b].speed_avg).abs() <= tolerance_mps
        }));
        bounds.push(shared);
        bounds.dedup();

        for w in bounds.windows(2) {
            let (s, e) = (w[0], w[1]);
            let current: f32 = bins[s..e].iter().map(|b| b.time_s).sum();
            // Hole bins are charged the median time, so a teleport can never
            // read as pace; only genuinely measured speed wins a span.
            let candidate: f32 = (s..e)
                .map(|b| {
                    let t = lap.bins[b].time_s;
                    if t < HOLE_TIME_SHARE * med[b] {
                        med[b]
                    } else {
                        t
                    }
                })
                .sum();
            if candidate < current - SPLICE_MIN_GAIN_S {
                bins[s..e].copy_from_slice(&lap.bins[s..e]);
                for b in s..e {
                    if bins[b].time_s < HOLE_TIME_SHARE * med[b] {
                        bins[b].time_s = med[b];
                    }
                }
                source[s..e].fill(li);
            }
        }
    }

    Composite {
        time_s: bins.iter().map(|b| b.time_s).sum(),
        bins,
        source,
    }
}

#[derive(Debug)]
pub struct StintProfile {
    pub laps: Vec<LapProfile>,
    /// Bins shared by every profiled lap (lap lengths differ by a few meters).
    pub shared_bins: usize,
    pub composite: Composite,
    pub best_lap_time_s: f32,
    pub standing_start_only: bool,
    /// Standing-start-only AND every profiled run kept its lap clock locked to
    /// the race clock: a point-to-point route (vs restart-per-lap circuit
    /// driving, whose lap 0 resets the lap clock at the start line).
    pub point_to_point: bool,
    /// Car identity of the profiled laps, so compare can flag car mismatches.
    pub car_ordinal: i32,
}

/// How much of the composite ideal has been independently reproduced.
///
/// GRADED: each lap other than the one the composite took a bin from
/// contributes agreement weight falling linearly from 1 (identical speed) to
/// 0 at the splice tolerance, and laps combine with diminishing returns
/// (1 − Π(1 − w)) — a third similar lap raises support, a near-identical lap
/// counts for more than a barely-in-tolerance one. The score is the
/// time-weighted mean support (a confirmed hairpin counts for more than a
/// confirmed straight of equal length). Reproducibility, not optimality: a
/// corner driven consistently wrong still corroborates. Monotone in laps
/// driven: a mistake lap fails to support but never lowers the score.
/// (The binary within-tolerance score saturated at 0.94-0.98 on healthy
/// stints and carried no signal; graded spreads 0.62/0.80/0.89 at 2/3/4
/// laps — confidence now rises with laps driven, by construction.)
#[derive(Debug)]
pub struct Corroboration {
    /// Per shared bin: reproduced by a second lap (binary, for the strip).
    pub corroborated: Vec<bool>,
    /// Time-weighted graded support, 0..1.
    pub score: f32,
    /// Graded support over HARVESTED bins only (composite bins not sourced
    /// from the base lap); None when the composite is entirely the base lap.
    /// Low values mean the optimal lap leans on road other laps didn't
    /// reproduce.
    pub harvest_support: Option<f32>,
}

impl StintProfile {
    /// Median flying-lap time: the stint's typical lap, robust to a single
    /// ruined lap (traffic, mistake). Even lap counts average the middle two.
    pub fn median_lap_time_s(&self) -> f32 {
        let mut ts: Vec<f32> = self.laps.iter().map(|l| l.time_s).collect();
        ts.sort_by(f32::total_cmp);
        let n = ts.len();
        if n % 2 == 1 {
            ts[n / 2]
        } else {
            (ts[n / 2 - 1] + ts[n / 2]) / 2.0
        }
    }

    pub fn corroboration(&self) -> Corroboration {
        let med = median_bin_times(&self.laps, self.shared_bins);
        let base = self
            .laps
            .iter()
            .enumerate()
            .min_by(|a, b| a.1.time_s.total_cmp(&b.1.time_s))
            .map(|(i, _)| i)
            .unwrap_or(0);
        let mut corroborated = vec![false; self.shared_bins];
        let (mut sup, mut total) = (0.0f32, 0.0f32);
        let (mut h_sup, mut h_total) = (0.0f32, 0.0f32);
        for (b, ok) in corroborated.iter_mut().enumerate() {
            let cb = &self.composite.bins[b];
            let src = self.composite.source[b];
            let mut miss = 1.0f32;
            for (li, lap) in self.laps.iter().enumerate() {
                if li == src || lap.bins[b].time_s < HOLE_TIME_SHARE * med[b] {
                    continue;
                }
                let dv = (lap.bins[b].speed_avg - cb.speed_avg).abs();
                *ok |= dv <= SPLICE_SPEED_TOLERANCE_MPS;
                miss *= 1.0 - (1.0 - dv / SPLICE_SPEED_TOLERANCE_MPS).max(0.0);
            }
            let support = 1.0 - miss;
            sup += support * cb.time_s;
            total += cb.time_s;
            if src != base {
                h_sup += support * cb.time_s;
                h_total += cb.time_s;
            }
        }
        Corroboration {
            corroborated,
            score: if total > 0.0 { sup / total } else { 0.0 },
            harvest_support: (h_total > 0.0).then(|| h_sup / h_total),
        }
    }
}

pub fn stint_profile(frames: &[TimedFrame]) -> Result<StintProfile, String> {
    // Profiles are built from the kept timeline (rewinds erased, retries spliced
    // in), so a rewound lap arrives here as one continuous, physically consistent
    // lap; the game restored exact state at the splice point.
    let segments = driving_segments(frames, 5.0);
    let mut laps: Vec<LapProfile> = Vec::new();
    let mut car_ordinal = 0;
    for segment in &segments {
        for lap in split_laps(segment) {
            if let Some(p) = lap_profile(&lap) {
                if laps.is_empty() {
                    car_ordinal = lap.frames[0].frame.car_ordinal;
                }
                laps.push(p);
            }
        }
    }
    if laps.is_empty() {
        return Err(
            "no profileable laps (needs a race-mode session: live DistanceTraveled, \
             completed laps captured from their start)"
                .into(),
        );
    }

    // Out laps aren't comparable to flying laps; drop them when flying laps exist.
    // On point-to-point routes every run is a standing start, so keep them all.
    let standing_start_only = laps.iter().all(|l| l.standing_start);
    let point_to_point = standing_start_only && laps.iter().all(|l| l.point_to_point);
    if !standing_start_only {
        laps.retain(|l| !l.standing_start);
    }

    let shared_bins = laps.iter().map(|l| l.bins.len()).min().unwrap();
    let composite = build_composite(&laps, shared_bins);
    let best_lap_time_s = laps.iter().map(|l| l.time_s).fold(f32::INFINITY, f32::min);

    Ok(StintProfile {
        shared_bins,
        composite,
        best_lap_time_s,
        standing_start_only,
        point_to_point,
        car_ordinal,
        laps,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::packet::TelemetryFrame;

    /// Hand-built lap: one bin per entry of `speeds` (10 m at v m/s -> 10/v seconds).
    fn lap_from_speeds(lap_number: u16, speeds: &[f32]) -> LapProfile {
        let bins: Vec<BinStats> = speeds
            .iter()
            .map(|v| BinStats {
                time_s: BIN_METERS / v,
                speed_avg: *v,
                samples: 5,
                ..Default::default()
            })
            .collect();
        LapProfile {
            lap_number,
            time_s: bins.iter().map(|b| b.time_s).sum(),
            standing_start: false,
            point_to_point: false,
            bins,
        }
    }

    /// The bug this design exists to prevent: an overshoot is FASTER into the corner
    /// and slower out. Its fast-entry bins must never be spliced onto a clean exit.
    #[test]
    fn overshoot_entry_stays_chained_to_its_slow_exit() {
        let clean = lap_from_speeds(1, &[50.0; 50]);
        // Overshoot: carries 60 m/s through bins 10-12, crawls at 30 m/s bins 13-18.
        let mut speeds = [50.0f32; 50];
        speeds[10..13].fill(60.0);
        speeds[13..19].fill(30.0);
        let overshoot = lap_from_speeds(2, &speeds);
        assert!(
            overshoot.time_s > clean.time_s,
            "overshoot is slower overall"
        );

        let laps = vec![clean.clone(), overshoot];
        let c = build_composite(&laps, 50);
        assert!(
            c.source.iter().all(|s| *s == 0),
            "no span of the overshoot lap is net-faster, so none may be taken: {:?}",
            c.source
        );
        assert!(
            (c.time_s - clean.time_s).abs() < 1e-4,
            "ideal == the clean lap"
        );
    }

    /// Marks a bin range as cornering road (slip above the classifier threshold).
    fn mark_corner(lap: &mut LapProfile, range: std::ops::Range<usize>) {
        for b in &mut lap.bins[range] {
            b.slip_front = 0.6;
            b.slip_rear = 0.6;
        }
    }

    /// The real-data failure the synthetic overshoot test missed: a lap driven
    /// slow-in/fast-out passes through the pack's speed near the apex, so a
    /// matching bin EXISTS mid-corner. Its fast exit must not be spliced onto
    /// the pack's fast entry — the corner is atomic.
    #[test]
    fn slow_entry_fast_exit_is_not_cherry_picked_at_the_apex() {
        // Pack lap: 50 flat, corner bins 10..26 sweeping 44..30..44.
        let mut pack = [50.0f32; 50];
        pack[10..26].copy_from_slice(&[
            44.0, 42.0, 40.0, 38.0, 36.0, 34.0, 32.0, 30.0, 32.0, 34.0, 36.0, 38.0, 40.0, 42.0,
            44.0, 47.0,
        ]);
        // Outlier: brakes early (slower bins 6..17), same 30 m/s apex at bin 17
        // (the crossover the old splicer exploited), then powers out earlier and
        // carries the advantage onto the straight. Net slower over the lap.
        let mut outlier = pack;
        outlier[6..17].copy_from_slice(&[
            40.0, 36.0, 33.0, 31.0, 30.0, 29.5, 29.0, 29.0, 29.0, 29.5, 30.0,
        ]);
        outlier[17..30].copy_from_slice(&[
            30.0, 34.0, 38.0, 42.0, 46.0, 50.0, 53.0, 55.0, 55.0, 55.0, 54.0, 53.0, 52.0,
        ]);

        let mut a = lap_from_speeds(1, &pack);
        let mut b = lap_from_speeds(2, &outlier);
        assert!(b.time_s > a.time_s, "outlier is net slower");
        mark_corner(&mut a, 10..26);
        mark_corner(&mut b, 6..22);

        let c = build_composite(&[a.clone(), b], 50);
        assert!(
            c.source.iter().all(|s| *s == 0),
            "no exit-only span may be taken from the outlier: {:?}",
            c.source
        );
        assert!((c.time_s - a.time_s).abs() < 1e-4, "ideal == the pack lap");
    }

    /// The honest counterpart: when the slow-in/fast-out corner is net FASTER,
    /// it is adopted WHOLE — slow entry included — never entry-from-one-lap,
    /// exit-from-another.
    #[test]
    fn net_faster_corner_is_adopted_whole() {
        let mut pack = [50.0f32; 50];
        pack[10..26].copy_from_slice(&[
            44.0, 42.0, 40.0, 38.0, 36.0, 34.0, 32.0, 30.0, 32.0, 34.0, 36.0, 38.0, 40.0, 42.0,
            44.0, 47.0,
        ]);
        // Mildly slower entry, much stronger exit: net faster through the corner.
        let mut outlier = pack;
        outlier[8..17].copy_from_slice(&[42.0, 39.0, 37.0, 35.0, 33.0, 31.0, 30.0, 30.0, 30.0]);
        outlier[17..30].copy_from_slice(&[
            32.0, 37.0, 42.0, 47.0, 51.0, 54.0, 56.0, 57.0, 57.0, 56.0, 55.0, 54.0, 53.0,
        ]);

        let mut a = lap_from_speeds(1, &pack);
        let mut b = lap_from_speeds(2, &outlier);
        assert!(b.time_s < a.time_s, "outlier is net faster");
        mark_corner(&mut a, 10..26);
        mark_corner(&mut b, 8..22);

        // Outlier is the base (fastest); the pack lap must not overwrite its
        // entry with a fast-entry span that dead-ends at the apex.
        let c = build_composite(&[a, b.clone()], 50);
        assert!(
            (8..30).all(|i| c.source[i] == 1),
            "corner stays whole from the outlier: {:?}",
            &c.source[6..32]
        );
    }

    /// A route-spline teleport leaves bins claiming 10 m in one frame's time.
    /// Such a span reads impossibly fast and must never be adopted, and the
    /// hole's bins must not corroborate the composite either.
    #[test]
    fn teleport_hole_is_neither_adopted_nor_corroborating() {
        // Base is authoritatively fastest; the holed lap's REAL lap time is
        // honest (holes hide time in bins, not in the game's lap timing).
        let base = lap_from_speeds(1, &[50.5; 50]);
        let mut holed = lap_from_speeds(2, &[50.0; 50]);
        for b in &mut holed.bins[20..30] {
            b.time_s = 0.016; // one 60 Hz frame spread across the crossed bins
        }
        let hole_span: f32 = holed.bins[20..30].iter().map(|b| b.time_s).sum();
        assert!(hole_span < 0.2, "the hole reads impossibly fast");

        let laps = vec![base, holed];
        let c = build_composite(&laps, 50);
        assert!(
            c.source.iter().all(|s| *s == 0),
            "hole span must not be spliced in: {:?}",
            &c.source[18..32]
        );

        let p = profile_of(laps);
        let cor = p.corroboration();
        // Composite bins 20..30 match the holed lap on speed, but the hole is
        // not evidence; outside the hole the two laps corroborate normally.
        assert!(
            (20..30).all(|b| !cor.corroborated[b]),
            "teleport bins must not corroborate"
        );
        assert!((0..20).chain(30..50).all(|b| cor.corroborated[b]));
    }

    /// A genuine sustained gain (higher speed, no compensating loss) IS adopted,
    /// even from a lap that is slower overall elsewhere.
    #[test]
    fn genuine_gain_is_spliced_in() {
        let base = lap_from_speeds(1, &[50.0; 50]);
        let mut speeds = [50.0f32; 50];
        speeds[20..30].fill(60.0); // genuinely quicker mid-section
        speeds[40..48].fill(40.0); // but loses more time later -> slower overall
        let mixed = lap_from_speeds(2, &speeds);
        assert!(mixed.time_s > base.time_s);

        let laps = vec![base.clone(), mixed];
        let c = build_composite(&laps, 50);
        assert!(
            (20..30).all(|b| c.source[b] == 1),
            "fast mid-section must come from lap 2: {:?}",
            &c.source[18..32]
        );
        assert!(
            (40..48).all(|b| c.source[b] == 0),
            "slow section must stay with the base lap"
        );
        assert!(c.time_s < base.time_s - 0.2);
        assert_eq!(c.source_laps().len(), 2);
    }

    /// Ideal can never beat the best lap through noise alone: with near-identical
    /// laps the margin keeps the composite anchored on the base.
    #[test]
    fn near_identical_laps_do_not_fabricate_gains() {
        let a = lap_from_speeds(1, &[50.0; 50]);
        let b = lap_from_speeds(2, &[50.05; 50]); // trivially faster everywhere
        let best = b.time_s.min(a.time_s);
        let c = build_composite(&[a, b], 50);
        assert!((c.time_s - best).abs() < 1e-3, "no noise-spliced ideal");
    }

    /// A route-spline snap (DistanceTraveled leaping several bins in one frame)
    /// must not leave empty bins; they'd chart as phantom 0-speed dips.
    #[test]
    fn distance_snap_leaves_no_empty_bins() {
        let mut frames = synth_lap(0, 500.0, 50.0, 100.0);
        // Inject a 30m forward snap partway through the lap.
        let snap_at = frames.len() / 2;
        for tf in &mut frames[snap_at..] {
            tf.frame.distance_traveled += 30.0;
        }
        let laps = split_laps(&frames);
        let mut lap = laps.into_iter().next().unwrap();
        lap.time_s = Some(10.0);
        let p = lap_profile(&lap).unwrap();
        assert!(
            p.bins.iter().all(|b| b.samples > 0 && b.speed_avg > 0.0),
            "no empty bins after a snap"
        );
        let total: f32 = p.bins.iter().map(|b| b.time_s).sum();
        assert!((total - 10.0).abs() < 0.3, "time conserved: {total}");
    }

    fn profile_of(laps: Vec<LapProfile>) -> StintProfile {
        let shared = laps.iter().map(|l| l.bins.len()).min().unwrap();
        let composite = build_composite(&laps, shared);
        let best = laps.iter().map(|l| l.time_s).fold(f32::INFINITY, f32::min);
        StintProfile {
            shared_bins: shared,
            composite,
            best_lap_time_s: best,
            standing_start_only: false,
            point_to_point: false,
            car_ordinal: 1,
            laps,
        }
    }

    #[test]
    fn corroboration_zero_with_one_lap_full_with_two_agreeing() {
        let p = profile_of(vec![lap_from_speeds(1, &[50.0; 50])]);
        let c = p.corroboration();
        assert_eq!(c.score, 0.0);
        assert!(c.corroborated.iter().all(|b| !b));

        // GRADED: a lap 0.5 m/s off supports at 1 - 0.5/2.5 = 0.8, not 1.0
        // (near-identical counts for more than barely-in-tolerance).
        let p = profile_of(vec![
            lap_from_speeds(1, &[50.0; 50]),
            lap_from_speeds(2, &[50.5; 50]), // within splice tolerance everywhere
        ]);
        let c = p.corroboration();
        assert!((c.score - 0.8).abs() < 0.01, "score {}", c.score);
        assert!(c.corroborated.iter().all(|b| *b), "binary strip still full");

        // An identical second lap is full support; a slower third lap can
        // only raise it (diminishing returns, never a penalty).
        let p = profile_of(vec![
            lap_from_speeds(1, &[50.0; 50]),
            lap_from_speeds(2, &[50.0; 50]),
            lap_from_speeds(3, &[49.0; 50]),
        ]);
        let c = p.corroboration();
        assert!(c.score > 0.999, "identical lap = full support: {}", c.score);
    }

    /// The user-struggle case: one segment driven three wildly different ways
    /// stays uncorroborated and holds the score down by its time share.
    #[test]
    fn struggle_segment_stays_uncorroborated() {
        let mut a = [50.0f32; 50];
        let mut b = [50.0f32; 50];
        let base = [50.0f32; 50]; // fastest overall -> composite base
        a[20..30].fill(40.0);
        b[20..30].fill(30.0);
        let p = profile_of(vec![
            lap_from_speeds(1, &base),
            lap_from_speeds(2, &a),
            lap_from_speeds(3, &b),
        ]);
        let c = p.corroboration();
        assert!(
            (20..30).all(|i| !c.corroborated[i]),
            "struggle bins unconfirmed"
        );
        assert!((0..20).chain(30..50).all(|i| c.corroborated[i]));
        // 10 of 50 equal-time bins unconfirmed -> score ~0.8
        assert!((c.score - 0.8).abs() < 0.02, "score {}", c.score);
    }

    /// Monotonicity: a mistake lap fails to corroborate but never lowers the
    /// score two clean laps established.
    #[test]
    fn mistake_lap_does_not_lower_score() {
        let clean = vec![
            lap_from_speeds(1, &[50.0; 50]),
            lap_from_speeds(2, &[50.2; 50]),
        ];
        let before = profile_of(clean.clone()).corroboration().score;

        let mut wild = [50.0f32; 50];
        wild[10..25].fill(20.0); // spin: way off through a long stretch
        let mut with_mistake = clean;
        with_mistake.push(lap_from_speeds(3, &wild));
        let after = profile_of(with_mistake).corroboration().score;
        assert!(after >= before - 1e-6, "before {before} after {after}");
    }

    // --- frame-level tests ---

    fn synth_lap(lap_number: u16, length_m: f32, speed: f32, race_t0: f32) -> Vec<TimedFrame> {
        let mut frames = Vec::new();
        let mut d = 0.0f32;
        let mut t = 0.0f32;
        while d < length_m {
            frames.push(TimedFrame {
                recv_us: (t * 1e6) as u64,
                frame: TelemetryFrame {
                    is_race_on: true,
                    timestamp_ms: ((race_t0 + t) * 1000.0) as u32,
                    lap_number,
                    current_lap: t,
                    current_race_time: race_t0 + t,
                    distance_traveled: 1000.0 + race_t0 * speed + d,
                    speed,
                    ..Default::default()
                },
            });
            d += speed * 0.1;
            t += 0.1;
        }
        frames
    }

    #[test]
    fn bins_capture_time_and_speed() {
        let frames = synth_lap(0, 500.0, 50.0, 100.0);
        let laps = split_laps(&frames);
        let mut lap = laps.into_iter().next().unwrap();
        lap.time_s = Some(10.0); // synthetic: 500m at 50 m/s
        let p = lap_profile(&lap).unwrap();
        assert!((49..=51).contains(&p.bins.len()), "bins {}", p.bins.len());
        let mid = &p.bins[20];
        assert!((mid.time_s - 0.2).abs() < 0.05, "bin time {}", mid.time_s);
        assert!((mid.speed_avg - 50.0).abs() < 0.1);
    }

    #[test]
    fn session_profile_drops_out_lap_and_partial_lap() {
        let mut frames = Vec::new();
        frames.extend(synth_lap(0, 500.0, 50.0, 0.0)); // standing start (race_t 0)
        frames.extend(synth_lap(1, 500.0, 50.0, 10.0));
        frames.extend(synth_lap(2, 500.0, 50.0, 20.0));
        let mut tail = synth_lap(3, 15.0, 50.0, 30.0); // partial, provides boundaries
        for tf in &mut tail {
            tf.frame.last_lap = 10.0;
        }
        for tf in frames.iter_mut().filter(|tf| tf.frame.lap_number >= 1) {
            tf.frame.last_lap = 10.0;
        }
        frames.extend(tail);

        let p = stint_profile(&frames).unwrap();
        let numbers: Vec<u16> = p.laps.iter().map(|l| l.lap_number).collect();
        assert_eq!(numbers, vec![1, 2], "out lap and partial tail excluded");
        assert!(!p.standing_start_only);
        assert!(p.composite.time_s <= p.best_lap_time_s + 1e-3);
    }
}
