//! Distance-binned lap profiles: every lap resampled onto a common distance axis so
//! the same piece of road can be compared across laps and sessions, with driving
//! mistakes detected as per-bin outliers. See docs/plans/003-comparison.md.

use super::{split_laps, split_stints, LapSlice, TimedFrame};

pub const BIN_METERS: f32 = 10.0;
/// A lap's bin is "dirty" (mistake: overshoot, offroad) when its average speed falls
/// this fraction below the best across the session's laps at that bin.
pub const OUTLIER_SPEED_FRAC: f32 = 0.10;
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

    let mut prev_ts = first.timestamp_ms;
    for tf in lap.frames {
        let f = &tf.frame;
        let d = f.distance_traveled - first.distance_traveled;
        let bin = ((d / BIN_METERS) as usize).min(n_bins - 1);
        let dt_ms = f.timestamp_ms.wrapping_sub(prev_ts).min(1000);
        prev_ts = f.timestamp_ms;

        time[bin] += dt_ms as f32 / 1000.0;
        speed[bin] += f.speed;
        slip_f[bin] += (f.tire_combined_slip.fl.abs() + f.tire_combined_slip.fr.abs()) / 2.0;
        slip_r[bin] += (f.tire_combined_slip.rl.abs() + f.tire_combined_slip.rr.abs()) / 2.0;
        brake[bin] += (f.brake >= 128) as u32;
        samples[bin] += 1;
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

#[derive(Debug)]
pub struct SessionProfile {
    pub laps: Vec<LapProfile>,
    /// clean[lap][bin]: bin not flagged as a driving mistake.
    pub clean: Vec<Vec<bool>>,
    /// Bins shared by every profiled lap (lap lengths differ by a few meters).
    pub shared_bins: usize,
    /// Sum of per-bin best clean times: the composited "ideal lap".
    pub ideal_time_s: f32,
    pub best_lap_time_s: f32,
    pub standing_start_only: bool,
}

pub fn session_profile(frames: &[TimedFrame]) -> Result<SessionProfile, String> {
    let mut laps: Vec<LapProfile> = Vec::new();
    for stint in split_stints(frames, 5.0) {
        for lap in split_laps(stint) {
            if let Some(p) = lap_profile(&lap) {
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
    let clean: Vec<Vec<bool>> = laps
        .iter()
        .map(|lap| {
            (0..shared_bins)
                .map(|b| {
                    let best = laps
                        .iter()
                        .map(|l| l.bins[b].speed_avg)
                        .fold(0.0f32, f32::max);
                    lap.bins[b].speed_avg >= best * (1.0 - OUTLIER_SPEED_FRAC)
                })
                .collect()
        })
        .collect();

    let ideal_time_s = (0..shared_bins)
        .map(|b| best_clean_time(&laps, &clean, b))
        .sum();
    let best_lap_time_s = laps.iter().map(|l| l.time_s).fold(f32::INFINITY, f32::min);

    Ok(SessionProfile {
        clean,
        shared_bins,
        ideal_time_s,
        best_lap_time_s,
        standing_start_only,
        laps,
    })
}

/// Best time for a bin among laps that drove it cleanly (all laps if none did).
pub fn best_clean_time(laps: &[LapProfile], clean: &[Vec<bool>], bin: usize) -> f32 {
    let clean_best = laps
        .iter()
        .zip(clean)
        .filter(|(_, c)| c[bin])
        .map(|(l, _)| l.bins[bin].time_s)
        .fold(f32::INFINITY, f32::min);
    if clean_best.is_finite() {
        clean_best
    } else {
        laps.iter()
            .map(|l| l.bins[bin].time_s)
            .fold(f32::INFINITY, f32::min)
    }
}

impl SessionProfile {
    pub fn dirty_bin_count(&self) -> usize {
        self.clean
            .iter()
            .map(|lap| lap.iter().filter(|c| !**c).count())
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::TelemetryFrame;

    /// A lap at constant `speed` m/s except bins [slow_from, slow_to) at half speed.
    /// 10 samples per second; frames carry distance, lap number, and race time.
    fn synth_lap(
        lap_number: u16,
        length_m: f32,
        speed: f32,
        slow: Option<(f32, f32)>,
        race_t0: f32,
    ) -> Vec<TimedFrame> {
        let mut frames = Vec::new();
        let mut d = 0.0f32;
        let mut t = 0.0f32;
        while d < length_m {
            let v = match slow {
                Some((from, to)) if d >= from && d < to => speed / 2.0,
                _ => speed,
            };
            frames.push(TimedFrame {
                recv_us: (t * 1e6) as u64,
                frame: TelemetryFrame {
                    is_race_on: true,
                    timestamp_ms: ((race_t0 + t) * 1000.0) as u32,
                    lap_number,
                    current_lap: t,
                    current_race_time: race_t0 + t,
                    distance_traveled: 1000.0 + race_t0 * speed + d, // monotonic across laps
                    speed: v,
                    ..Default::default()
                },
            });
            d += v * 0.1;
            t += 0.1;
        }
        frames
    }

    #[test]
    fn bins_capture_time_and_speed() {
        let frames = synth_lap(0, 500.0, 50.0, None, 100.0);
        let laps = split_laps(&frames);
        let mut lap = laps.into_iter().next().unwrap();
        lap.time_s = Some(10.0); // synthetic: 500m at 50 m/s
        let p = lap_profile(&lap).unwrap();
        assert!((49..=51).contains(&p.bins.len()), "bins {}", p.bins.len());
        // interior bins: 10m at 50 m/s = 0.2s
        let mid = &p.bins[20];
        assert!((mid.time_s - 0.2).abs() < 0.05, "bin time {}", mid.time_s);
        assert!((mid.speed_avg - 50.0).abs() < 0.1);
    }

    #[test]
    fn mistake_bins_flagged_dirty_and_ideal_uses_clean_laps() {
        // Three laps; lap B botches 100-150m (half speed). Laps must be joined into
        // one frame series with contiguous lap numbers for split_laps.
        let mut frames = Vec::new();
        frames.extend(synth_lap(0, 500.0, 50.0, None, 0.0));
        frames.extend(synth_lap(1, 500.0, 50.0, Some((100.0, 150.0)), 10.0));
        frames.extend(synth_lap(2, 500.0, 50.0, None, 21.0));
        // trailing frame so lap 2 gets a LastLap-style boundary
        let mut end = synth_lap(3, 15.0, 50.0, None, 31.0);
        for tf in &mut end {
            tf.frame.last_lap = 10.0;
        }
        // give laps 0/1 boundary times via the next lap's frames
        for tf in frames.iter_mut().filter(|tf| tf.frame.lap_number == 1) {
            tf.frame.last_lap = 10.0;
        }
        for tf in frames.iter_mut().filter(|tf| tf.frame.lap_number == 2) {
            tf.frame.last_lap = 11.0;
        }
        frames.extend(end);

        let profile = session_profile(&frames).unwrap();
        // Standing-start lap 0 is dropped (flying laps exist); the trailing partial
        // lap has no boundary after it, so no authoritative time -> not profiled.
        let numbers: Vec<u16> = profile.laps.iter().map(|l| l.lap_number).collect();
        assert_eq!(numbers, vec![1, 2]);

        let slow_lap = profile.laps.iter().position(|l| l.lap_number == 1).unwrap();
        let dirty_bins: Vec<usize> = (0..profile.shared_bins)
            .filter(|b| !profile.clean[slow_lap][*b])
            .collect();
        assert!(!dirty_bins.is_empty(), "botched segment must flag dirty bins");
        assert!(
            dirty_bins.iter().all(|b| (9..=16).contains(b)),
            "dirty bins {dirty_bins:?} should sit in the 100-150m range"
        );
        // ideal must beat the botched lap's time
        assert!(profile.ideal_time_s < 11.0);
    }
}
