//! Session analysis: recorded frames → tuning-relevant observations.
//! Metrics are measured facts only; prescriptive advice lives elsewhere.

pub mod attribution;
pub mod compare;
pub mod corners;
pub mod driveline;
pub mod effects;
pub mod grip;
pub mod metrics;
pub mod products;
pub mod profile;
pub mod report;

use crate::telemetry::packet::{self, TelemetryFrame};
use crate::telemetry::stint::StintReader;
use std::io;
use std::path::Path;

#[derive(Debug, Clone, Copy)]
pub struct TimedFrame {
    pub recv_us: u64,
    pub frame: TelemetryFrame,
}

pub struct Stint {
    pub frames: Vec<TimedFrame>,
    pub decode_errors: u64,
}

impl Stint {
    pub fn load(path: &Path) -> io::Result<Self> {
        let mut reader = StintReader::open(path)?;
        let mut frames = Vec::new();
        let mut decode_errors = 0u64;
        while let Some((recv_us, payload)) = reader.next_packet()? {
            match packet::decode(&payload) {
                Ok(frame) => frames.push(TimedFrame { recv_us, frame }),
                Err(_) => decode_errors += 1,
            }
        }
        Ok(Self {
            frames,
            decode_errors,
        })
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
    /// True when this is the race's first lap: a standing start (rivals out lap).
    /// Its time is not comparable to flying laps. Detected by the race clock and
    /// lap clock having started together, which survives capture starting late.
    pub standing_start: bool,
    /// True when no start-line lap-clock reset was seen in the slice: a
    /// point-to-point run, the default assumption. On circuits the lap clock
    /// RESETS to ~0 at the start-line crossing while the race clock keeps
    /// advancing; on point-to-point routes it never resets (see telemetry.md).
    /// A rewind also steps the lap clock down, but steps the race clock (and
    /// distance) back with it, so requiring an advancing race clock and
    /// non-retreating distance excludes rewinds (even a rewind to the GO
    /// moment) and restart-menu flicker. Only meaningful on standing starts.
    pub point_to_point: bool,
}

/// Max seconds the race clock may lead the lap clock on a standing start
/// (covers the pre-launch countdown offset, ~2s observed).
const STANDING_START_CLOCK_OFFSET_S: f32 = 5.0;
/// A start-line reset must step the lap clock down at least this far...
const LAP_RESET_MIN_DROP_S: f32 = 0.25;
/// ...landing near zero (a rewind restores a mid-lap clock; the line reset
/// lands within one frame of 0, measured 0.01-0.02s at 60 Hz).
const LAP_RESET_LANDING_MAX_S: f32 = 0.5;

/// Did the lap clock reset to ~0 at a start-line crossing between these two
/// frames? The circuit signature: lap clock steps down to near zero while the
/// race clock advances and distance does not retreat. Rewind splices step the
/// race clock back; restart-menu flicker teleports distance backwards: neither
/// qualifies. Shared by lap classification and the live tailer (which flips
/// the gauge from point-to-point to out lap the moment the line is crossed).
pub fn line_reset_step(a: &TelemetryFrame, b: &TelemetryFrame) -> bool {
    b.current_lap < a.current_lap - LAP_RESET_MIN_DROP_S
        && b.current_lap < LAP_RESET_LANDING_MAX_S
        && b.current_race_time > a.current_race_time - 0.05
        && b.distance_traveled > a.distance_traveled - 5.0
}

/// Was a start-line reset seen anywhere inside this slice?
fn line_reset_seen(frames: &[TimedFrame]) -> bool {
    frames
        .windows(2)
        .any(|w| line_reset_step(&w[0].frame, &w[1].frame))
}

/// Max seconds the certificate's LastLap may exceed the run's last seen lap
/// clock: the game cuts to race-off AT the line, so the official time trails
/// the last driving frame by well under a second (measured +0.004s).
const FINISH_CERT_SLACK_S: f32 = 5.0;

/// Is `resume` a finish certificate for a run whose last driving frame was
/// `last`? At the finish line FH6 goes race-off BEFORE any frame carries the
/// run time; the results screen later flashes brief race-on frames with
/// LapNumber incremented and LastLap holding the official time, while the
/// race clock kept running and the position channels are garbage (distance
/// teleports; see telemetry.md). Matching LastLap against the run's own lap
/// clock makes false adoption implausible: a pre-race flash of a new event
/// carries lap 0 / LastLap 0, and a stale flash of another event cannot
/// match this run's clock. The race clock must have ADVANCED across the gap
/// (results screens keep it running, measured +2.97..+11.36s): a pause taken
/// right at a lap line also resumes on lap+1 with a matching LastLap, but a
/// real pause resumes within hundredths and its driving must stay stitched.
pub fn finish_certificate(last: &TelemetryFrame, resume: &TelemetryFrame) -> bool {
    resume.lap_number == last.lap_number.wrapping_add(1)
        && resume.last_lap > 0.0
        && resume.last_lap >= last.current_lap - 0.5
        && resume.last_lap <= last.current_lap + FINISH_CERT_SLACK_S
        && resume.current_race_time > last.current_race_time + 0.5
}

/// Split a stint into laps on `LapNumber` transitions. A stint with no transitions
/// (free roam, where LapNumber stays 0) yields a single slice; callers should only
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
            let standing_start =
                first.current_race_time - first.current_lap < STANDING_START_CLOCK_OFFSET_S;
            LapSlice {
                number: first.lap_number,
                frames,
                time_s,
                standing_start,
                point_to_point: standing_start && !line_reset_seen(frames),
            }
        })
        .collect()
}

/// What a race-off gap in the stream turned out to be, judged by the race clock
/// on resume: unchanged = pause, moved backwards = rewind, near zero = restart,
/// jumped forward = the game kept the clock running through the block (post-race
/// results screen), so the missing time is not driving.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GapKind {
    Pause,
    Rewind {
        race_t_before: f32,
        race_t_after: f32,
    },
    Restart,
    ClockRan {
        skipped_s: f32,
    },
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
pub(crate) const RESTART_RACE_T_S: f32 = 5.0;
/// Race clock must step back at least this far to call a gap a rewind.
const REWIND_MIN_STEP_S: f32 = 0.25;
/// Race clock advancing more than this across a race-off gap means the game
/// kept the clock running through the block (post-race results screen keeps
/// racing time; real pauses resume within hundredths): not a pause.
const PAUSE_MAX_CLOCK_ADVANCE_S: f32 = 5.0;

/// Classify every race-off gap in a session. Rewound laps are already excluded
/// from profiling structurally (the gap splits the stint and the resumed slice
/// starts mid-lap); this exists so reports can say so honestly.
///
/// Race-on runs where the car never moves are menu flicker (pre-race menus can
/// flash race-on frames carrying the previous event's clock): they neither end
/// nor anchor a gap, so the surrounding gap is judged driving-run to
/// driving-run and a flicker before the first real drive reports nothing.
pub fn classify_gaps(frames: &[TimedFrame]) -> Vec<SessionGap> {
    let mut runs: Vec<std::ops::Range<usize>> = Vec::new();
    let mut start = None;
    for (i, tf) in frames.iter().enumerate() {
        match (tf.frame.is_race_on, start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                runs.push(s..i);
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        runs.push(s..frames.len());
    }
    runs.retain(|r| {
        frames[r.clone()]
            .iter()
            .any(|f| f.frame.speed > MIN_DRIVING_SPEED_MPS)
    });

    runs.windows(2)
        .map(|pair| {
            let race_t_before = frames[pair[0].end - 1].frame.current_race_time;
            let resume_frame = pair[1].start;
            let f = &frames[resume_frame].frame;
            let kind = if f.current_race_time < RESTART_RACE_T_S {
                GapKind::Restart
            } else if f.current_race_time < race_t_before - REWIND_MIN_STEP_S {
                GapKind::Rewind {
                    race_t_before,
                    race_t_after: f.current_race_time,
                }
            } else if f.current_race_time > race_t_before + PAUSE_MAX_CLOCK_ADVANCE_S {
                GapKind::ClockRan {
                    skipped_s: f.current_race_time - race_t_before,
                }
            } else {
                GapKind::Pause
            };
            SessionGap {
                resume_frame,
                resume_lap: f.lap_number,
                kind,
            }
        })
        .collect()
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
/// re-drive: the game is doing equal-state splicing. For tune evaluation the kept
/// retry is the data that matters (leaderboard validity is irrelevant), so frames
/// superseded by a rewind (race clock >= the resume point) are dropped and the
/// retry is spliced on. Pauses are stitched over; restarts start a new segment,
/// as does a gap the race clock ran through (post-race results screen): the
/// frames on either side of it are not continuous driving.
/// Segments shorter than `min_seconds` on the race clock are dropped, as are
/// segments where the car never moves (sitting on track before ducking into the
/// setup menu produces a race-on stint of pure zeroes).
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
            if let Some(last) = current.last().copied()
                && finish_certificate(&last.frame, &tf.frame)
            {
                // The run finished at the race-off: adopt the certificate's
                // official time by synthesizing the lap boundary the game
                // never sent while driving. The flicker frames themselves
                // stay out (garbage position); leftovers form a sub-5s
                // segment the retain below drops.
                let mut boundary = last;
                boundary.recv_us += 20_000;
                boundary.frame.lap_number = last.frame.lap_number.wrapping_add(1);
                boundary.frame.last_lap = tf.frame.last_lap;
                boundary.frame.current_race_time += 0.02;
                boundary.frame.current_lap += 0.02;
                current.push(boundary);
                segments.push(std::mem::take(&mut current));
                continue;
            }
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
            } else if last_t.is_some_and(|t| race_t - t > PAUSE_MAX_CLOCK_ADVANCE_S) {
                // The clock ran through the race-off block: not a pause.
                segments.push(std::mem::take(&mut current));
            }
            // Pause: the race clock didn't move; just keep appending.
        }
        current.push(*tf);
    }
    if !current.is_empty() {
        segments.push(current);
    }
    segments.retain(|s| {
        stint_seconds(s) >= min_seconds && s.iter().any(|f| f.frame.speed > MIN_DRIVING_SPEED_MPS)
    });
    segments
}

/// A segment must exceed this speed at least once to count as driving (~11 mph).
pub(crate) const MIN_DRIVING_SPEED_MPS: f32 = 5.0;

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
        assert_eq!(
            laps[2].time_s, None,
            "final partial lap has no finished time"
        );
    }

    /// Route-kind detection: the ONLY circuit signature is the lap clock
    /// resetting to ~0 at the start-line crossing while the race clock
    /// advances and distance does not retreat. Rewinds (race clock steps back
    /// with the lap clock, even to the GO moment) and restart-menu flicker
    /// (distance teleports backwards) must not read as circuits.
    #[test]
    fn line_reset_detects_circuit_not_rewind_or_flicker() {
        let clocked = |race_t: f32, lap_t: f32, dist: f32| TimedFrame {
            recv_us: 0,
            frame: TelemetryFrame {
                is_race_on: true,
                lap_number: 0,
                current_race_time: race_t,
                current_lap: lap_t,
                distance_traveled: dist,
                ..Default::default()
            },
        };

        // Circuit: clocks locked from GO, lap clock resets at the line (~2s in).
        let circuit = vec![
            clocked(0.0, 0.0, -30.0),
            clocked(1.0, 1.0, -10.0),
            clocked(2.0, 2.0, -0.5),
            clocked(2.02, 0.01, 0.5),
            clocked(3.0, 0.99, 20.0),
        ];
        let laps = split_laps(&circuit);
        assert!(laps[0].standing_start && !laps[0].point_to_point);

        // Point-to-point: clocks stay locked, no reset.
        let p2p = vec![
            clocked(0.0, 0.0, -15.0),
            clocked(1.0, 1.0, -5.0),
            clocked(2.0, 2.0, 10.0),
            clocked(60.0, 60.0, 2500.0),
        ];
        let laps = split_laps(&p2p);
        assert!(laps[0].standing_start && laps[0].point_to_point);

        // Rewind to the GO moment: lap clock lands near 0 but the race clock
        // (and distance) step back with it — not a line crossing.
        let rewound = vec![
            clocked(0.0, 0.0, -15.0),
            clocked(8.0, 8.0, 300.0),
            clocked(0.1, 0.1, -14.0),
            clocked(5.0, 5.0, 150.0),
        ];
        let laps = split_laps(&rewound);
        assert!(
            laps[0].point_to_point,
            "rewind to GO is not a circuit reset"
        );

        // Restart-menu flicker: lap clock drops under a running race clock but
        // distance teleports backwards (measured pattern, telemetry.md).
        let flicker = vec![
            clocked(0.0, 0.0, -15.0),
            clocked(14.0, 14.0, 400.0),
            clocked(15.0, 0.45, 196.0),
        ];
        let laps = split_laps(&flicker);
        assert!(
            laps[0].point_to_point,
            "restart flicker is not a line reset"
        );
    }

    fn racing(race_t: f32, lap: u16) -> TimedFrame {
        TimedFrame {
            recv_us: 0,
            frame: TelemetryFrame {
                is_race_on: true,
                current_race_time: race_t,
                lap_number: lap,
                speed: 50.0,
                ..Default::default()
            },
        }
    }

    /// A race-on stint where the car never moves (waiting on track, then ducking
    /// into the setup menu, which restarts the race clock) is not driving and
    /// must not reach the user.
    #[test]
    fn stationary_stints_are_dropped() {
        let mut frames: Vec<TimedFrame> = (0..300)
            .map(|i| {
                let mut tf = racing(i as f32 * 0.1, 0);
                tf.frame.speed = 0.0;
                tf
            })
            .collect();
        frames.push(gap_frame());
        // setup menu resets the race clock -> restart -> separate segment
        frames.extend((0..300).map(|i| racing(0.5 + i as f32 * 0.1, 0)));

        let segments = driving_segments(&frames, 5.0);
        assert_eq!(segments.len(), 1, "only the moving stint survives");
        assert!(segments[0].iter().all(|f| f.frame.speed > 0.0));
    }

    fn gap_frame() -> TimedFrame {
        TimedFrame {
            recv_us: 0,
            frame: TelemetryFrame::default(),
        }
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

        let profile = crate::analysis::profile::stint_profile(&frames).unwrap();
        let numbers: Vec<u16> = profile.laps.iter().map(|l| l.lap_number).collect();
        assert_eq!(
            numbers,
            vec![1, 2],
            "the kept lap 2 must be profiled: {numbers:?}"
        );

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
        assert!(
            laps[0].standing_start,
            "late-started capture of the out lap"
        );
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
        let mut frames: Vec<TimedFrame> =
            (0..10).map(|i| racing(100.0 + i as f32 * 0.1, 3)).collect();
        frames.push(gap_frame());
        // rewind lands at 100.4s: the four frames at 100.4..100.9 are superseded
        frames.extend((0..6).map(|i| racing(100.4 + i as f32 * 0.1, 3)));

        let segments = driving_segments(&frames, 0.0);
        assert_eq!(segments.len(), 1);
        let times: Vec<f32> = segments[0]
            .iter()
            .map(|t| t.frame.current_race_time)
            .collect();
        assert_eq!(
            times.len(),
            10,
            "4 pre-rewind frames erased, 6 retry frames kept"
        );
        assert!(
            times.windows(2).all(|w| w[1] > w[0]),
            "kept timeline must be monotonic on the race clock: {times:?}"
        );
    }

    /// FH6 keeps the race clock running through the post-race results screen
    /// (a race-off block): the resume must split the stint, not stitch a
    /// ghost span onto it (real case: a 275s block read as a 278s "lap").
    #[test]
    fn results_screen_clock_run_splits_and_is_reported() {
        let mut frames: Vec<TimedFrame> = (0..100)
            .map(|i| racing(100.0 + i as f32 * 0.1, 3))
            .collect();
        frames.push(gap_frame());
        // clock ran 275s while race-off; driving resumes far ahead
        frames.extend((0..100).map(|i| racing(385.0 + i as f32 * 0.1, 3)));

        let segments = driving_segments(&frames, 0.0);
        assert_eq!(segments.len(), 2, "clock-run gap must split the stint");
        assert!((stint_seconds(&segments[0]) - 9.9).abs() < 0.01);
        assert!((stint_seconds(&segments[1]) - 9.9).abs() < 0.01);

        let gaps = classify_gaps(&frames);
        assert_eq!(gaps.len(), 1);
        assert!(
            matches!(gaps[0].kind, GapKind::ClockRan { skipped_s } if (skipped_s - 275.1).abs() < 0.2),
            "{:?}",
            gaps[0].kind
        );
    }

    /// At the finish line the game goes race-off BEFORE any frame carries
    /// the run time; the results screen later flashes a race-on frame with
    /// LapNumber+1 and LastLap = the official time while the race clock
    /// kept running (measured live: race-off at 135.16s, certificate 16.7s
    /// of wall time later at race_t 146.52 / last_lap 135.164). The
    /// certificate's time must be adopted so the run reads complete, and
    /// the flicker frame's garbage position must stay out of the segment.
    #[test]
    fn finish_certificate_completes_the_run() {
        let mut frames: Vec<TimedFrame> = (0..1350)
            .map(|i| {
                let t = i as f32 * 0.1;
                let mut tf = racing(t, 0);
                tf.frame.current_lap = t;
                tf.frame.distance_traveled = t * 44.0 + 1.0;
                tf
            })
            .collect();
        frames.push(gap_frame());
        let mut cert = racing(146.5, 1);
        cert.frame.current_lap = 0.0;
        cert.frame.last_lap = 135.02;
        cert.frame.distance_traveled = 196.0; // teleported garbage
        frames.push(cert);
        frames.push(gap_frame());

        let segments = driving_segments(&frames, 5.0);
        assert_eq!(segments.len(), 1);
        assert!(
            stint_seconds(&segments[0]) < 135.5,
            "no ghost results-screen time: {}",
            stint_seconds(&segments[0])
        );
        let laps = split_laps(&segments[0]);
        assert_eq!(laps.len(), 2, "run + one-frame boundary slice");
        assert_eq!(laps[0].time_s, Some(135.02));
        assert!(laps[0].standing_start && laps[0].point_to_point);
    }

    /// The same pattern on a circuit's FINAL lap (mid-race crossings emit
    /// the boundary while race-on; only the last one goes race-off at the
    /// line): the certificate's LastLap matches the lap clock, not the race
    /// clock, and the final lap gets its time.
    #[test]
    fn finish_certificate_completes_a_circuit_final_lap() {
        let mut frames: Vec<TimedFrame> = (0..450)
            .map(|i| {
                let t = i as f32 * 0.1;
                let mut tf = racing(230.55 + t, 3);
                tf.frame.current_lap = t;
                tf.frame.distance_traveled = 13000.0 + t * 44.0;
                tf
            })
            .collect();
        frames.push(gap_frame());
        let mut cert = racing(550.0, 4); // clock ran 275s through results
        cert.frame.current_lap = 0.0;
        cert.frame.last_lap = 45.0;
        cert.frame.distance_traveled = 196.0;
        frames.push(cert);

        let segments = driving_segments(&frames, 5.0);
        let laps = split_laps(&segments[0]);
        assert_eq!(laps[0].time_s, Some(45.0));
        assert!(!laps[0].standing_start);
    }

    /// A pause taken right at a lap line resumes on lap+1 with a matching
    /// LastLap, but the race clock resumes within hundredths: NOT a
    /// certificate, and the driving on both sides must stay one stitched
    /// segment (found on the real library: the certificate branch split
    /// mid-race pauses and shifted longest-segment metrics).
    #[test]
    fn pause_at_the_lap_line_is_not_a_certificate() {
        let mut frames: Vec<TimedFrame> = (0..600)
            .map(|i| {
                let t = i as f32 * 0.1;
                let mut tf = racing(t, 0);
                tf.frame.current_lap = t;
                tf.frame.distance_traveled = t * 44.0 + 1.0;
                tf
            })
            .collect();
        frames.push(gap_frame()); // pause exactly at the line
        frames.extend((0..600).map(|i| {
            let t = 60.0 + i as f32 * 0.1;
            let mut tf = racing(t + 0.02, 1);
            tf.frame.current_lap = i as f32 * 0.1;
            tf.frame.last_lap = 60.0;
            tf.frame.distance_traveled = t * 44.0 + 1.0;
            tf
        }));

        let segments = driving_segments(&frames, 5.0);
        assert_eq!(segments.len(), 1, "mid-race pause must stay stitched");
        let laps = split_laps(&segments[0]);
        assert_eq!(laps.len(), 2);
        assert_eq!(laps[0].time_s, Some(60.0));
    }

    /// A resume whose LastLap does not match the run's own lap clock is NOT
    /// a certificate: the ClockRan split stands and the run stays timeless.
    #[test]
    fn mismatched_last_lap_is_not_a_certificate() {
        let mut frames: Vec<TimedFrame> = (0..1350)
            .map(|i| {
                let t = i as f32 * 0.1;
                let mut tf = racing(t, 0);
                tf.frame.current_lap = t;
                tf.frame.distance_traveled = t * 44.0 + 1.0;
                tf
            })
            .collect();
        frames.push(gap_frame());
        let mut cert = racing(146.5, 1);
        cert.frame.current_lap = 0.0;
        cert.frame.last_lap = 60.0; // some other event's lap time
        frames.push(cert);

        let segments = driving_segments(&frames, 5.0);
        assert_eq!(segments.len(), 1, "flicker-only leftover segment dropped");
        let laps = split_laps(&segments[0]);
        assert!(laps[0].time_s.is_none());
    }

    /// Pre-race menus flicker race-on frames carrying the PREVIOUS event's
    /// clock (stationary, distance 0). The drop from that clock to the real
    /// race's zero must not read as a session restart.
    #[test]
    fn prerace_menu_flicker_reports_no_gap() {
        let mut frames: Vec<TimedFrame> = (0..20)
            .map(|i| {
                let mut tf = racing(395.0 + i as f32 * 0.1, 0);
                tf.frame.speed = 0.0;
                tf
            })
            .collect();
        frames.push(gap_frame());
        frames.extend((0..50).map(|i| racing(0.5 + i as f32 * 0.1, 0)));

        assert!(
            classify_gaps(&frames).is_empty(),
            "flicker before the first drive must report nothing"
        );
        assert_eq!(driving_segments(&frames, 0.0).len(), 1);
    }

    /// A flicker BETWEEN two drives merges into the gap: the gap is judged
    /// driving-run to driving-run, so a new event after a finished one still
    /// reads as a restart (and never as a flicker-anchored clock run).
    #[test]
    fn flicker_between_drives_merges_into_the_gap() {
        let mut frames: Vec<TimedFrame> =
            (0..50).map(|i| racing(240.0 + i as f32 * 0.1, 3)).collect();
        frames.push(gap_frame());
        frames.extend((0..10).map(|i| {
            let mut tf = racing(395.0 + i as f32 * 0.1, 0);
            tf.frame.speed = 0.0;
            tf
        }));
        frames.push(gap_frame());
        frames.extend((0..50).map(|i| racing(0.5 + i as f32 * 0.1, 0)));

        let gaps = classify_gaps(&frames);
        assert_eq!(gaps.len(), 1, "one gap, drive to drive");
        assert_eq!(gaps[0].kind, GapKind::Restart);
    }

    #[test]
    fn pause_stitches_and_restart_splits() {
        let mut frames: Vec<TimedFrame> =
            (0..5).map(|i| racing(100.0 + i as f32 * 0.1, 2)).collect();
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
