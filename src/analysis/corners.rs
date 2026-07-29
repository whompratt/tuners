//! Corner-event segmentation: contiguous cornering episodes split
//! into entry / exit phases at the minimum-speed apex. This is the vocabulary
//! scenario-specific advice (diff decel, brake balance, rebound) is written
//! in; rules on top of these metrics wait for deliberate calibration A/Bs,
//! like damping did.

use super::TimedFrame;
use super::metrics::BandBalance;

/// |lateral accel| (m/s²) that opens a corner event. Matches the cornering
/// threshold used by the whole-stint balance metrics.
const ENTER_LAT: f32 = 4.0;
/// Hysteresis: an open corner only closes once |lat| stays below this...
const EXIT_LAT: f32 = 3.0;
/// ...for at least this long. Bridges bumpy apexes and quick transitions
/// without merging genuinely separate corners.
const MAX_GAP_S: f32 = 0.3;
/// Events shorter than this are kinks, not corners.
const MIN_CORNER_S: f32 = 0.8;
/// Brake input treated as pedal-on (same convention as the stint metrics).
const PEDAL_ON: u8 = 128;

#[derive(Debug, Clone, Copy)]
pub struct CornerEvent {
    /// Race-time bounds of the event (kept-timeline clock).
    pub start_s: f32,
    pub end_s: f32,
    /// Minimum speed in the event (m/s), i.e. the apex.
    pub apex_speed: f32,
    /// Balance over samples before / after the apex.
    pub entry: BandBalance,
    pub exit: BandBalance,
    /// Entry conditioned on the brake pedal: trail-braking samples vs
    /// coasting/turn-in samples. Positional entry mixes the two; the split
    /// separates a brake-bias push from a roll-stiffness push.
    pub entry_braking: BandBalance,
    pub entry_coasting: BandBalance,
}

/// Per-stint aggregate over all detected corners, sample-weighted.
#[derive(Debug, Clone, Copy, Default)]
pub struct CornerSummary {
    pub corners: usize,
    pub entry: BandBalance,
    pub exit: BandBalance,
    /// Entry split by brake pedal (see CornerEvent).
    pub entry_braking: BandBalance,
    pub entry_coasting: BandBalance,
    pub avg_apex_speed: f32,
}

/// Detect corner events in a continuous driving segment. Frames must be a
/// kept timeline (race clock monotonic), as produced by driving_segments.
pub fn corner_events(frames: &[TimedFrame]) -> Vec<CornerEvent> {
    let mut events = Vec::new();
    let mut open: Option<usize> = None; // start index of the open corner
    let mut below_since: Option<f32> = None; // race_t when |lat| dropped low
    let mut last_hot = 0usize; // last index with |lat| >= EXIT_LAT

    for (i, tf) in frames.iter().enumerate() {
        let lat = tf.frame.acceleration[0].abs();
        let t = tf.frame.current_race_time;
        match open {
            None => {
                if lat > ENTER_LAT {
                    open = Some(i);
                    below_since = None;
                    last_hot = i;
                }
            }
            Some(start) => {
                if lat >= EXIT_LAT {
                    below_since = None;
                    last_hot = i;
                } else {
                    let since = *below_since.get_or_insert(t);
                    if t - since >= MAX_GAP_S {
                        push_event(frames, start, last_hot, &mut events);
                        open = None;
                    }
                }
            }
        }
    }
    if let Some(start) = open {
        push_event(frames, start, last_hot, &mut events);
    }
    events
}

/// Close an event spanning `start..=end` frame indices, splitting phases at
/// the minimum-speed sample.
fn push_event(frames: &[TimedFrame], start: usize, end: usize, events: &mut Vec<CornerEvent>) {
    let slice = &frames[start..=end];
    let start_s = slice.first().unwrap().frame.current_race_time;
    let end_s = slice.last().unwrap().frame.current_race_time;
    if end_s - start_s < MIN_CORNER_S {
        return;
    }
    let apex = slice
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| a.frame.speed.total_cmp(&b.frame.speed))
        .map(|(i, _)| i)
        .unwrap_or(0);
    let entry_slice = &slice[..apex.max(1)];
    events.push(CornerEvent {
        start_s,
        end_s,
        apex_speed: slice[apex].frame.speed,
        entry: phase_balance(entry_slice, |_| true),
        exit: phase_balance(&slice[apex..], |_| true),
        entry_braking: phase_balance(entry_slice, |f| f.brake >= PEDAL_ON),
        entry_coasting: phase_balance(entry_slice, |f| f.brake < PEDAL_ON),
    });
}

fn phase_balance(
    frames: &[TimedFrame],
    keep: impl Fn(&crate::telemetry::packet::TelemetryFrame) -> bool,
) -> BandBalance {
    let mut front = 0.0f32;
    let mut rear = 0.0f32;
    let mut n = 0usize;
    for tf in frames {
        if !keep(&tf.frame) {
            continue;
        }
        let s = &tf.frame.tire_slip_angle;
        front += (s.fl.abs() + s.fr.abs()) / 2.0;
        rear += (s.rl.abs() + s.rr.abs()) / 2.0;
        n += 1;
    }
    BandBalance {
        samples: n,
        index: (n > 0).then(|| (front - rear) / n as f32),
        rear_slip: (n > 0).then(|| rear / n as f32),
    }
}

/// Sample-weighted aggregate of the per-corner phases.
pub fn summarize(frames: &[TimedFrame]) -> Option<CornerSummary> {
    let events = corner_events(frames);
    if events.is_empty() {
        return None;
    }
    let fold = |get: fn(&CornerEvent) -> BandBalance| {
        let mut samples = 0usize;
        let mut index = 0.0f32;
        let mut rear = 0.0f32;
        for e in &events {
            let b = get(e);
            if let (Some(i), Some(r)) = (b.index, b.rear_slip) {
                samples += b.samples;
                index += i * b.samples as f32;
                rear += r * b.samples as f32;
            }
        }
        BandBalance {
            samples,
            index: (samples > 0).then(|| index / samples as f32),
            rear_slip: (samples > 0).then(|| rear / samples as f32),
        }
    };
    Some(CornerSummary {
        corners: events.len(),
        entry: fold(|e| e.entry),
        exit: fold(|e| e.exit),
        entry_braking: fold(|e| e.entry_braking),
        entry_coasting: fold(|e| e.entry_coasting),
        avg_apex_speed: events.iter().map(|e| e.apex_speed).sum::<f32>() / events.len() as f32,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::packet::{Corners, TelemetryFrame};

    /// 10 samples/s timeline with the given (lat, speed, front slip, rear slip)
    /// per frame.
    fn timeline(samples: &[(f32, f32, f32, f32)]) -> Vec<TimedFrame> {
        samples
            .iter()
            .enumerate()
            .map(|(i, &(lat, speed, front, rear))| TimedFrame {
                recv_us: i as u64 * 100_000,
                frame: TelemetryFrame {
                    is_race_on: true,
                    current_race_time: i as f32 * 0.1,
                    acceleration: [lat, 0.0, 0.0],
                    speed,
                    tire_slip_angle: Corners {
                        fl: front,
                        fr: front,
                        rl: rear,
                        rr: rear,
                    },
                    ..Default::default()
                },
            })
            .collect()
    }

    fn straight(n: usize) -> Vec<(f32, f32, f32, f32)> {
        vec![(0.0, 50.0, 0.0, 0.0); n]
    }

    /// Speed dips into the corner and recovers; front slips more on entry,
    /// rear more on exit.
    fn corner(n: usize) -> Vec<(f32, f32, f32, f32)> {
        (0..n)
            .map(|i| {
                let half = n as f32 / 2.0;
                let progress = (i as f32 - half).abs() / half; // 1 at edges, 0 mid
                let speed = 30.0 + 20.0 * progress;
                if (i as f32) < half {
                    (6.0, speed, 0.8, 0.4) // entry: front works harder
                } else {
                    (6.0, speed, 0.4, 0.9) // exit: rear works harder
                }
            })
            .collect()
    }

    #[test]
    fn detects_one_corner_with_entry_exit_split() {
        let mut s = straight(20);
        s.extend(corner(30)); // 3s corner
        s.extend(straight(20));
        let events = corner_events(&timeline(&s));
        assert_eq!(events.len(), 1, "{events:?}");
        let e = events[0];
        assert!((e.apex_speed - 30.0).abs() < 2.0, "apex {}", e.apex_speed);
        assert!(e.entry.index.unwrap() > 0.2, "entry pushes: {:?}", e.entry);
        assert!(e.exit.index.unwrap() < -0.2, "exit rotates: {:?}", e.exit);

        let sum = summarize(&timeline(&s)).unwrap();
        assert_eq!(sum.corners, 1);
        assert!(sum.entry.index.unwrap() > 0.2);
        assert!(sum.exit.index.unwrap() < -0.2);
    }

    #[test]
    fn short_gap_bridges_one_corner_long_gap_splits_two() {
        // 0.2s dip mid-corner: still one event.
        let mut s = straight(10);
        s.extend(corner(10));
        s.extend(straight(2));
        s.extend(corner(10));
        s.extend(straight(10));
        assert_eq!(corner_events(&timeline(&s)).len(), 1);

        // 1s of straight between: two corners.
        let mut s = straight(10);
        s.extend(corner(10));
        s.extend(straight(10));
        s.extend(corner(10));
        s.extend(straight(10));
        assert_eq!(corner_events(&timeline(&s)).len(), 2);
    }

    #[test]
    fn kinks_and_straights_produce_no_events() {
        // 0.3s of lateral load is a kink, not a corner.
        let mut s = straight(10);
        s.extend(corner(3));
        s.extend(straight(10));
        assert!(corner_events(&timeline(&s)).is_empty());
        assert!(summarize(&timeline(&straight(30))).is_none());
    }

    #[test]
    fn corner_still_open_at_segment_end_is_kept() {
        let mut s = straight(10);
        s.extend(corner(20));
        let events = corner_events(&timeline(&s));
        assert_eq!(events.len(), 1);
    }
}
