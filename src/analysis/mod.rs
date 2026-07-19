//! Session analysis: recorded frames → tuning-relevant observations.
//! Metrics are measured facts only; prescriptive advice lives elsewhere.

pub mod compare;
pub mod metrics;
pub mod profile;
pub mod report;

use crate::packet::{self, TelemetryFrame};
use crate::session::SessionReader;
use std::io;
use std::path::Path;

#[derive(Debug, Clone, Copy)]
pub struct TimedFrame {
    pub recv_us: u64,
    pub frame: TelemetryFrame,
}

pub struct Session {
    pub frames: Vec<TimedFrame>,
    pub decode_errors: u64,
}

impl Session {
    pub fn load(path: &Path) -> io::Result<Self> {
        let mut reader = SessionReader::open(path)?;
        let mut frames = Vec::new();
        let mut decode_errors = 0u64;
        while let Some((recv_us, payload)) = reader.next_packet()? {
            match packet::decode(&payload) {
                Ok(frame) => frames.push(TimedFrame { recv_us, frame }),
                Err(_) => decode_errors += 1,
            }
        }
        Ok(Self { frames, decode_errors })
    }
}

/// Contiguous `IsRaceOn` runs at least `min_seconds` long (by in-game time).
/// Menu/pause frames are zeroed by the game, so they carry no information.
pub fn split_stints(frames: &[TimedFrame], min_seconds: f32) -> Vec<&[TimedFrame]> {
    let mut stints = Vec::new();
    let mut start = None;
    for (i, tf) in frames.iter().enumerate() {
        match (tf.frame.is_race_on, start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                stints.push(&frames[s..i]);
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        stints.push(&frames[s..]);
    }
    stints.retain(|s| stint_seconds(s) >= min_seconds);
    stints
}

/// One lap within a stint. Lap numbers are the game's 0-based `LapNumber`.
pub struct LapSlice<'a> {
    pub number: u16,
    pub frames: &'a [TimedFrame],
    /// Authoritative lap time from the next lap's `LastLap` field; None for the
    /// final (usually incomplete) lap of a stint.
    pub time_s: Option<f32>,
    /// True when this is the race's first lap — a standing start (rivals out lap).
    /// Its time is not comparable to flying laps. Detected by the race clock and
    /// lap clock having started together, which survives capture starting late.
    pub standing_start: bool,
}

/// Max seconds the race clock may lead the lap clock on a standing start
/// (covers the pre-launch countdown offset, ~2s observed).
const STANDING_START_CLOCK_OFFSET_S: f32 = 5.0;

/// Split a stint into laps on `LapNumber` transitions. A stint with no transitions
/// (free roam, where LapNumber stays 0) yields a single slice — callers should only
/// treat the result as laps when there is more than one.
pub fn split_laps(stint: &[TimedFrame]) -> Vec<LapSlice<'_>> {
    let mut bounds = vec![0];
    for i in 1..stint.len() {
        if stint[i].frame.lap_number != stint[i - 1].frame.lap_number {
            bounds.push(i);
        }
    }
    bounds.push(stint.len());

    (0..bounds.len() - 1)
        .map(|k| {
            let frames = &stint[bounds[k]..bounds[k + 1]];
            // LastLap in the first frames after the boundary is the finished lap's time.
            let time_s = stint
                .get(bounds[k + 1])
                .map(|next| next.frame.last_lap)
                .filter(|t| *t > 0.0);
            let first = frames[0].frame;
            LapSlice {
                number: first.lap_number,
                frames,
                time_s,
                standing_start: first.current_race_time - first.current_lap
                    < STANDING_START_CLOCK_OFFSET_S,
            }
        })
        .collect()
}

/// In-game duration of a frame slice, tolerant of TimestampMS overflow.
pub fn stint_seconds(frames: &[TimedFrame]) -> f32 {
    match (frames.first(), frames.last()) {
        (Some(a), Some(b)) => {
            b.frame.timestamp_ms.wrapping_sub(a.frame.timestamp_ms) as f32 / 1000.0
        }
        _ => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tf(is_race_on: bool, timestamp_ms: u32) -> TimedFrame {
        TimedFrame {
            recv_us: timestamp_ms as u64 * 1000,
            frame: TelemetryFrame { is_race_on, timestamp_ms, ..Default::default() },
        }
    }

    #[test]
    fn splits_on_race_off_and_drops_short_stints() {
        let mut frames = Vec::new();
        // 10s driving, 5s menu, 1s driving (too short), 5s menu, 8s driving
        for t in 0..100 {
            frames.push(tf(true, t * 100));
        }
        for t in 100..150 {
            frames.push(tf(false, t * 100));
        }
        for t in 150..160 {
            frames.push(tf(true, t * 100));
        }
        for t in 160..210 {
            frames.push(tf(false, t * 100));
        }
        for t in 210..290 {
            frames.push(tf(true, t * 100));
        }
        let stints = split_stints(&frames, 5.0);
        assert_eq!(stints.len(), 2);
        assert_eq!(stints[0].len(), 100);
        assert_eq!(stints[1].len(), 80);
    }

    #[test]
    fn splits_laps_with_times_and_standing_start() {
        // Lap 0 from a standing start (race clock at 0), lap 1 flying, lap 2 partial.
        let mut frames = Vec::new();
        for (lap, race_t0, n) in [(0u16, 0.0f32, 50), (1, 60.0, 50), (2, 120.0, 10)] {
            for i in 0..n {
                frames.push(TimedFrame {
                    recv_us: 0,
                    frame: TelemetryFrame {
                        is_race_on: true,
                        lap_number: lap,
                        current_race_time: race_t0 + i as f32 * 0.1,
                        // LastLap holds the previous lap's time
                        last_lap: match lap {
                            0 => 0.0,
                            1 => 60.0,
                            _ => 59.0,
                        },
                        ..Default::default()
                    },
                });
            }
        }
        let laps = split_laps(&frames);
        assert_eq!(laps.len(), 3);
        assert!(laps[0].standing_start);
        assert_eq!(laps[0].time_s, Some(60.0));
        assert!(!laps[1].standing_start);
        assert_eq!(laps[1].time_s, Some(59.0));
        assert_eq!(laps[2].time_s, None, "final partial lap has no finished time");
    }

    /// Regression: capture starting shortly after launch (race clock already at
    /// 0.58s) must still identify the out lap as a standing start.
    #[test]
    fn late_capture_start_still_flags_out_lap() {
        let mut frames = Vec::new();
        for (lap, cur0, race0, n) in [(0u16, 0.58f32, 0.58f32, 20), (1, 0.0, 95.0, 20)] {
            for i in 0..n {
                frames.push(TimedFrame {
                    recv_us: 0,
                    frame: TelemetryFrame {
                        is_race_on: true,
                        lap_number: lap,
                        current_lap: cur0 + i as f32 * 0.1,
                        current_race_time: race0 + i as f32 * 0.1,
                        last_lap: if lap == 1 { 94.0 } else { 0.0 },
                        ..Default::default()
                    },
                });
            }
        }
        let laps = split_laps(&frames);
        assert!(laps[0].standing_start, "late-started capture of the out lap");
        assert!(!laps[1].standing_start);
    }

    #[test]
    fn free_roam_is_a_single_slice() {
        let frames: Vec<TimedFrame> = (0..10)
            .map(|i| TimedFrame {
                recv_us: 0,
                frame: TelemetryFrame {
                    is_race_on: true,
                    current_race_time: 100.0 + i as f32,
                    ..Default::default()
                },
            })
            .collect();
        assert_eq!(split_laps(&frames).len(), 1);
    }

    #[test]
    fn survives_timestamp_overflow() {
        let frames = vec![tf(true, u32::MAX - 500), tf(true, 500)];
        assert!((stint_seconds(&frames) - 1.001).abs() < 0.01);
    }
}
