//! Session analysis: recorded frames → tuning-relevant observations.
//! Metrics are measured facts only; prescriptive advice lives elsewhere.

pub mod compare;
pub mod metrics;
pub mod profile;
pub mod recommend;
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
    split_stint_ranges(frames, min_seconds)
        .into_iter()
        .map(|r| &frames[r])
        .collect()
}

/// Index ranges of stints, for callers that need positions in the full session
/// (e.g. to match stint starts against rewind-resume points).
pub fn split_stint_ranges(frames: &[TimedFrame], min_seconds: f32) -> Vec<std::ops::Range<usize>> {
    let mut stints = Vec::new();
    let mut start = None;
    for (i, tf) in frames.iter().enumerate() {
        match (tf.frame.is_race_on, start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                stints.push(s..i);
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        stints.push(s..frames.len());
    }
    stints.retain(|r| stint_seconds(&frames[r.clone()]) >= min_seconds);
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

/// What a race-off gap in the stream turned out to be, judged by the race clock
/// on resume: unchanged = pause, moved backwards = rewind, near zero = restart.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GapKind {
    Pause,
    Rewind { race_t_before: f32, race_t_after: f32 },
    Restart,
}

#[derive(Debug, Clone, Copy)]
pub struct SessionGap {
    /// Index of the first race-on frame after the gap.
    pub resume_frame: usize,
    /// Lap number driving resumed on.
    pub resume_lap: u16,
    pub kind: GapKind,
}

/// Race clock below this on resume = the session was restarted, not rewound.
const RESTART_RACE_T_S: f32 = 5.0;
/// Race clock must step back at least this far to call a gap a rewind.
const REWIND_MIN_STEP_S: f32 = 0.25;

/// Classify every race-off gap in a session. Rewound laps are already excluded
/// from profiling structurally (the gap splits the stint and the resumed slice
/// starts mid-lap); this exists so reports can say so honestly.
pub fn classify_gaps(frames: &[TimedFrame]) -> Vec<SessionGap> {
    let mut gaps = Vec::new();
    let mut last_on: Option<f32> = None;
    let mut in_gap = false;
    for (i, tf) in frames.iter().enumerate() {
        let f = &tf.frame;
        if !f.is_race_on {
            in_gap = last_on.is_some();
            continue;
        }
        if in_gap {
            let race_t_before = last_on.unwrap();
            let kind = if f.current_race_time < RESTART_RACE_T_S {
                GapKind::Restart
            } else if f.current_race_time < race_t_before - REWIND_MIN_STEP_S {
                GapKind::Rewind { race_t_before, race_t_after: f.current_race_time }
            } else {
                GapKind::Pause
            };
            gaps.push(SessionGap { resume_frame: i, resume_lap: f.lap_number, kind });
            in_gap = false;
        }
        last_on = Some(f.current_race_time);
    }
    gaps
}

/// In-game duration of a frame slice, measured on the race clock (which runs in
/// free roam too, pauses during pauses, and is monotonic in a kept timeline).
pub fn stint_seconds(frames: &[TimedFrame]) -> f32 {
    match (frames.first(), frames.last()) {
        (Some(a), Some(b)) => b.frame.current_race_time - a.frame.current_race_time,
        _ => 0.0,
    }
}

/// The session as the game kept it: driving segments with rewinds *erased*.
///
/// A rewind restores exact car state (clock, position, speed) and lets the player
/// re-drive — the game is doing equal-state splicing. For tune evaluation the kept
/// retry is the data that matters (leaderboard validity is irrelevant), so frames
/// superseded by a rewind (race clock >= the resume point) are dropped and the
/// retry is spliced on. Pauses are stitched over; restarts start a new segment.
/// Segments shorter than `min_seconds` on the race clock are dropped.
pub fn driving_segments(frames: &[TimedFrame], min_seconds: f32) -> Vec<Vec<TimedFrame>> {
    let mut segments: Vec<Vec<TimedFrame>> = Vec::new();
    let mut current: Vec<TimedFrame> = Vec::new();
    let mut in_gap = false;
    for tf in frames {
        if !tf.frame.is_race_on {
            in_gap = !current.is_empty();
            continue;
        }
        if in_gap {
            in_gap = false;
            let race_t = tf.frame.current_race_time;
            let last_t = current.last().map(|l| l.frame.current_race_time);
            if race_t < RESTART_RACE_T_S {
                segments.push(std::mem::take(&mut current));
            } else if last_t.is_some_and(|t| t >= race_t) {
                // Rewind: erase what the retry supersedes.
                while current
                    .last()
                    .is_some_and(|l| l.frame.current_race_time >= race_t)
                {
                    current.pop();
                }
            }
            // Pause: the race clock didn't move; just keep appending.
        }
        current.push(*tf);
    }
    if !current.is_empty() {
        segments.push(current);
    }
    segments.retain(|s| stint_seconds(s) >= min_seconds);
    segments
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tf(is_race_on: bool, timestamp_ms: u32) -> TimedFrame {
        TimedFrame {
            recv_us: timestamp_ms as u64 * 1000,
            frame: TelemetryFrame {
                is_race_on,
                timestamp_ms,
                current_race_time: timestamp_ms as f32 / 1000.0,
                ..Default::default()
            },
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

    fn racing(race_t: f32, lap: u16) -> TimedFrame {
        TimedFrame {
            recv_us: 0,
            frame: TelemetryFrame {
                is_race_on: true,
                current_race_time: race_t,
                lap_number: lap,
                ..Default::default()
            },
        }
    }

    fn gap_frame() -> TimedFrame {
        TimedFrame { recv_us: 0, frame: TelemetryFrame::default() }
    }

    /// The three race-off gap kinds, using the signature observed in the real
    /// rewind capture: pause resumes with the clock unchanged, rewind with it
    /// stepped back, restart with it near zero.
    #[test]
    fn gaps_classified_by_race_clock() {
        let mut frames = Vec::new();
        frames.extend((0..10).map(|i| racing(100.0 + i as f32 * 0.1, 3)));
        frames.push(gap_frame()); // pause
        frames.extend((0..10).map(|i| racing(100.9 + i as f32 * 0.1, 3)));
        frames.push(gap_frame()); // rewind: clock steps back 377.5 -> 376.5 style
        frames.extend((0..10).map(|i| racing(95.0 + i as f32 * 0.1, 3)));
        frames.push(gap_frame()); // restart
        frames.extend((0..10).map(|i| racing(0.5 + i as f32 * 0.1, 0)));

        let gaps = classify_gaps(&frames);
        assert_eq!(gaps.len(), 3);
        assert_eq!(gaps[0].kind, GapKind::Pause);
        assert!(matches!(gaps[1].kind, GapKind::Rewind { .. }));
        assert_eq!(gaps[1].resume_lap, 3);
        assert_eq!(gaps[2].kind, GapKind::Restart);
    }

    /// A mid-lap rewind produces a KEPT lap: superseded frames are erased and the
    /// retry splices on seamlessly (the game restored exact state), so the lap is
    /// profiled as one continuous, physically consistent lap.
    #[test]
    fn rewound_lap_keeps_the_retried_attempt() {
        // Completed lap 1 (with boundary), then lap 2 begins, rewind mid-lap,
        // lap 2 resumes earlier in the lap and completes; lap 3 provides the
        // boundary time.
        let mut frames = Vec::new();
        for i in 0..100 {
            frames.push(TimedFrame {
                recv_us: 0,
                frame: TelemetryFrame {
                    is_race_on: true,
                    timestamp_ms: i * 100,
                    lap_number: 1,
                    current_lap: i as f32 * 0.1,
                    current_race_time: 100.0 + i as f32 * 0.1,
                    distance_traveled: 1000.0 + i as f32 * 5.0,
                    speed: 50.0,
                    ..Default::default()
                },
            });
        }
        // lap 2 first attempt: 4s in, then rewind
        for i in 0..40 {
            frames.push(TimedFrame {
                recv_us: 0,
                frame: TelemetryFrame {
                    is_race_on: true,
                    timestamp_ms: 10_000 + i * 100,
                    lap_number: 2,
                    current_lap: i as f32 * 0.1,
                    current_race_time: 110.0 + i as f32 * 0.1,
                    last_lap: 10.0,
                    distance_traveled: 1500.0 + i as f32 * 5.0,
                    speed: 50.0,
                    ..Default::default()
                },
            });
        }
        frames.push(gap_frame());
        // Rewind lands 0.8s into lap 2; the game restored exact state, so the
        // retry continues from the matching clock and distance (frames with
        // race_t >= 110.8 above are superseded and get erased).
        for i in 0..92 {
            frames.push(TimedFrame {
                recv_us: 0,
                frame: TelemetryFrame {
                    is_race_on: true,
                    timestamp_ms: 20_000 + i * 100,
                    lap_number: 2,
                    current_lap: 0.8 + i as f32 * 0.1,
                    current_race_time: 110.8 + i as f32 * 0.1,
                    last_lap: 10.0,
                    distance_traveled: 1540.0 + i as f32 * 5.0,
                    speed: 50.0,
                    ..Default::default()
                },
            });
        }
        for i in 0..20 {
            frames.push(TimedFrame {
                recv_us: 0,
                frame: TelemetryFrame {
                    is_race_on: true,
                    timestamp_ms: 30_000 + i * 100,
                    lap_number: 3,
                    current_lap: i as f32 * 0.1,
                    current_race_time: 120.0 + i as f32 * 0.1,
                    last_lap: 10.0, // lap 2's finished time (real physics, rewind included)
                    distance_traveled: 2000.0 + i as f32 * 5.0,
                    speed: 50.0,
                    ..Default::default()
                },
            });
        }

        let gaps = classify_gaps(&frames);
        assert_eq!(gaps.len(), 1);
        assert!(matches!(gaps[0].kind, GapKind::Rewind { .. }));

        let profile = crate::analysis::profile::session_profile(&frames).unwrap();
        let numbers: Vec<u16> = profile.laps.iter().map(|l| l.lap_number).collect();
        assert_eq!(numbers, vec![1, 2], "the kept lap 2 must be profiled: {numbers:?}");

        // The spliced lap must carry no phantom time: bin times sum to ~the lap time.
        let lap2 = profile.laps.iter().find(|l| l.lap_number == 2).unwrap();
        let binned: f32 = lap2.bins.iter().map(|b| b.time_s).sum();
        assert!((binned - 10.0).abs() < 0.5, "binned lap time {binned}");
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
    fn duration_reads_the_race_clock() {
        let frames = vec![tf(true, 1000), tf(true, 3500)];
        assert!((stint_seconds(&frames) - 2.5).abs() < 0.01);
    }

    #[test]
    fn rewind_erases_superseded_frames() {
        let mut frames: Vec<TimedFrame> = (0..10).map(|i| racing(100.0 + i as f32 * 0.1, 3)).collect();
        frames.push(gap_frame());
        // rewind lands at 100.4s: the four frames at 100.4..100.9 are superseded
        frames.extend((0..6).map(|i| racing(100.4 + i as f32 * 0.1, 3)));

        let segments = driving_segments(&frames, 0.0);
        assert_eq!(segments.len(), 1);
        let times: Vec<f32> = segments[0].iter().map(|t| t.frame.current_race_time).collect();
        assert_eq!(times.len(), 10, "4 pre-rewind frames erased, 6 retry frames kept");
        assert!(
            times.windows(2).all(|w| w[1] > w[0]),
            "kept timeline must be monotonic on the race clock: {times:?}"
        );
    }

    #[test]
    fn pause_stitches_and_restart_splits() {
        let mut frames: Vec<TimedFrame> = (0..5).map(|i| racing(100.0 + i as f32 * 0.1, 2)).collect();
        frames.push(gap_frame()); // pause: clock resumes where it left off
        frames.extend((0..5).map(|i| racing(100.5 + i as f32 * 0.1, 2)));
        frames.push(gap_frame()); // restart: clock near zero
        frames.extend((0..5).map(|i| racing(0.5 + i as f32 * 0.1, 0)));

        let segments = driving_segments(&frames, 0.0);
        assert_eq!(segments.len(), 2, "pause stitched, restart split");
        assert_eq!(segments[0].len(), 10);
        assert_eq!(segments[1].len(), 5);
    }
}
