//! Session analysis: recorded frames → tuning-relevant observations.
//! Metrics are measured facts only; prescriptive advice lives elsewhere.

pub mod metrics;
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
    fn survives_timestamp_overflow() {
        let frames = vec![tf(true, u32::MAX - 500), tf(true, 500)];
        assert!((stint_seconds(&frames) - 1.001).abs() < 0.01);
    }
}
