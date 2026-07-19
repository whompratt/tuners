//! Distance-binned lap profiles and the spliced "ideal lap" composite.
//!
//! Laps are resampled onto 10 m distance bins so the same piece of road lines up
//! across laps and sessions. The ideal lap is composited by splicing laps together
//! ONLY at points where their speeds match: between two equal-speed crossovers a
//! lap's span is taken whole, judged on total elapsed time. This keeps a mistake's
//! fast corner entry chained to its slow exit — a missed braking point is faster
//! into the corner and can never be combined with a clean lap's exit, which would
//! fabricate a physically impossible lap (docs/plans/003-comparison.md).

use super::{driving_segments, split_laps, LapSlice, TimedFrame};
use std::collections::BTreeSet;

pub const BIN_METERS: f32 = 10.0;
/// Laps may only be spliced at bins where their speeds match within this tolerance.
pub const SPLICE_SPEED_TOLERANCE_MPS: f32 = 2.5;
/// A span is only taken from another lap when it beats the base by this margin —
/// keeps the composite anchored on the real best lap instead of chasing noise.
pub const SPLICE_MIN_GAIN_S: f32 = 0.03;
/// A lap must start within this many seconds of its beginning to be profiled.
const LAP_START_TOLERANCE_S: f32 = 1.0;

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
        1 + self
            .source
            .windows(2)
            .filter(|w| w[0] != w[1])
            .count()
    }

    pub fn source_laps(&self) -> BTreeSet<usize> {
        self.source.iter().copied().collect()
    }
}

/// Build the composite over the first `shared` bins. Starts from the fastest lap;
/// each other lap may replace spans between equal-speed crossovers, judged on the
/// span's total time.
pub fn build_composite(laps: &[LapProfile], shared: usize) -> Composite {
    let base = laps
        .iter()
        .enumerate()
        .min_by(|a, b| a.1.time_s.total_cmp(&b.1.time_s))
        .map(|(i, _)| i)
        .unwrap_or(0);
    let mut bins: Vec<BinStats> = laps[base].bins[..shared].to_vec();
    let mut source = vec![base; shared];

    for (li, lap) in laps.iter().enumerate() {
        if li == base {
            continue;
        }
        // Splice boundaries: bins where the candidate's speed matches the composite's.
        let mut bounds = vec![0usize];
        bounds.extend((0..shared).filter(|b| {
            (bins[*b].speed_avg - lap.bins[*b].speed_avg).abs() <= SPLICE_SPEED_TOLERANCE_MPS
        }));
        bounds.push(shared);
        bounds.dedup();

        for w in bounds.windows(2) {
            let (s, e) = (w[0], w[1]);
            let current: f32 = bins[s..e].iter().map(|b| b.time_s).sum();
            let candidate: f32 = lap.bins[s..e].iter().map(|b| b.time_s).sum();
            if candidate < current - SPLICE_MIN_GAIN_S {
                bins[s..e].copy_from_slice(&lap.bins[s..e]);
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
pub struct SessionProfile {
    pub laps: Vec<LapProfile>,
    /// Bins shared by every profiled lap (lap lengths differ by a few meters).
    pub shared_bins: usize,
    pub composite: Composite,
    pub best_lap_time_s: f32,
    pub standing_start_only: bool,
    /// Car identity of the profiled laps, so compare can flag car mismatches.
    pub car_ordinal: i32,
}

pub fn session_profile(frames: &[TimedFrame]) -> Result<SessionProfile, String> {
    // Profiles are built from the kept timeline (rewinds erased, retries spliced
    // in), so a rewound lap arrives here as one continuous, physically consistent
    // lap — the game restored exact state at the splice point.
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
    if !standing_start_only {
        laps.retain(|l| !l.standing_start);
    }

    let shared_bins = laps.iter().map(|l| l.bins.len()).min().unwrap();
    let composite = build_composite(&laps, shared_bins);
    let best_lap_time_s = laps.iter().map(|l| l.time_s).fold(f32::INFINITY, f32::min);

    Ok(SessionProfile {
        shared_bins,
        composite,
        best_lap_time_s,
        standing_start_only,
        car_ordinal,
        laps,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::TelemetryFrame;

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
        assert!(overshoot.time_s > clean.time_s, "overshoot is slower overall");

        let laps = vec![clean.clone(), overshoot];
        let c = build_composite(&laps, 50);
        assert!(
            c.source.iter().all(|s| *s == 0),
            "no span of the overshoot lap is net-faster, so none may be taken: {:?}",
            c.source
        );
        assert!((c.time_s - clean.time_s).abs() < 1e-4, "ideal == the clean lap");
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
    /// must not leave empty bins — they'd chart as phantom 0-speed dips.
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

        let p = session_profile(&frames).unwrap();
        let numbers: Vec<u16> = p.laps.iter().map(|l| l.lap_number).collect();
        assert_eq!(numbers, vec![1, 2], "out lap and partial tail excluded");
        assert!(!p.standing_start_only);
        assert!(p.composite.time_s <= p.best_lap_time_s + 1e-3);
    }
}
