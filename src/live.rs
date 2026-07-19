//! Live view: tail the newest session file as `tuners capture` appends to it and
//! keep a shared snapshot (latest frame + data-quality summary) for the dashboard
//! to relay over SSE (docs/plans/006-dashboard.md phase 4).
//!
//! The serve process never binds the UDP port — capture owns it. SessionWriter
//! writes each record with a single unbuffered `write_all`, so tailing the file
//! sees new packets within one poll interval.

use crate::analysis::TimedFrame;
use crate::packet;
use crate::session::MAGIC;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How often the tail thread polls the sessions directory / file for new data.
pub const POLL_INTERVAL: Duration = Duration::from_millis(200);
/// Recompute the (whole-session) quality summary at most this often.
const QUALITY_INTERVAL: Duration = Duration::from_secs(2);

/// Incremental reader over a growing session file. Unlike SessionReader it
/// tolerates a partially-written record at end-of-file: bytes are buffered until
/// a complete record is available, and the next poll picks up where it left off.
pub struct SessionTail {
    file: File,
    buf: Vec<u8>,
    magic_ok: bool,
}

impl SessionTail {
    pub fn open(path: &Path) -> std::io::Result<Self> {
        Ok(Self { file: File::open(path)?, buf: Vec::new(), magic_ok: false })
    }

    /// Read whatever the file has appended since the last poll and return the
    /// complete records in it. An incomplete trailing record stays buffered.
    pub fn poll(&mut self) -> std::io::Result<Vec<(u64, Vec<u8>)>> {
        let mut chunk = [0u8; 64 * 1024];
        loop {
            let n = self.file.read(&mut chunk)?;
            if n == 0 {
                break;
            }
            self.buf.extend_from_slice(&chunk[..n]);
        }

        if !self.magic_ok {
            if self.buf.len() < MAGIC.len() {
                return Ok(Vec::new());
            }
            if &self.buf[..MAGIC.len()] != MAGIC {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "not a tuners session file (bad magic)",
                ));
            }
            self.buf.drain(..MAGIC.len());
            self.magic_ok = true;
        }

        let mut records = Vec::new();
        let mut pos = 0;
        while self.buf.len() - pos >= 12 {
            let recv_us = u64::from_le_bytes(self.buf[pos..pos + 8].try_into().unwrap());
            let len = u32::from_le_bytes(self.buf[pos + 8..pos + 12].try_into().unwrap()) as usize;
            if self.buf.len() - pos - 12 < len {
                break; // record still being written
            }
            records.push((recv_us, self.buf[pos + 12..pos + 12 + len].to_vec()));
            pos += 12 + len;
        }
        self.buf.drain(..pos);
        Ok(records)
    }
}

/// Confidence band for the dashboard gauge, derived from the corroboration
/// score. Cutoffs calibrated against the real session library (see plan 006):
/// sessions the user drew tuning conclusions from should land in Good.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Band {
    Low,
    Ok,
    Good,
}

/// Corroboration score at or above this reads green.
pub const GOOD_MIN_SCORE: f32 = 0.70;
/// At or above this reads orange; below is red.
pub const OK_MIN_SCORE: f32 = 0.40;

impl Band {
    pub fn from_score(score: f32) -> Band {
        if score >= GOOD_MIN_SCORE {
            Band::Good
        } else if score >= OK_MIN_SCORE {
            Band::Ok
        } else {
            Band::Low
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Band::Low => "low",
            Band::Ok => "ok",
            Band::Good => "good",
        }
    }
}

/// Data-quality summary of everything captured so far in the live session.
#[derive(Debug, Clone, Copy)]
pub struct Quality {
    /// Profiled comparable laps (flying laps, or standing runs point-to-point).
    pub laps: usize,
    pub standing_only: bool,
    pub best_lap_s: f32,
    /// (worst − best) / best over profiled laps — driving consistency.
    pub spread_frac: f32,
    /// Distance every profiled lap covers (the comparable route length).
    pub shared_km: f32,
    /// Time-weighted share of the ideal lap reproduced by a second lap
    /// (profile::Corroboration) — the headline confidence value.
    pub confidence: f32,
    pub band: Band,
}

/// Quality over the frames captured so far; None until a comparable lap exists.
pub fn compute_quality(frames: &[TimedFrame]) -> Option<Quality> {
    let profile = crate::analysis::profile::session_profile(frames).ok()?;
    let laps = profile.laps.len();
    let worst = profile.laps.iter().map(|l| l.time_s).fold(0.0f32, f32::max);
    let spread_frac = (worst - profile.best_lap_time_s).max(0.0) / profile.best_lap_time_s;
    let confidence = profile.corroboration().score;
    Some(Quality {
        laps,
        standing_only: profile.standing_start_only,
        best_lap_s: profile.best_lap_time_s,
        spread_frac,
        shared_km: profile.shared_bins as f32 * crate::analysis::profile::BIN_METERS / 1000.0,
        confidence,
        band: Band::from_score(confidence),
    })
}

/// Snapshot shared between the tail thread and SSE handlers. The frame timeline
/// itself stays owned by the tail thread — only the distilled state is shared.
#[derive(Default)]
pub struct LiveState {
    /// Session file currently being tailed (None until one exists).
    pub file: Option<PathBuf>,
    pub latest: Option<TimedFrame>,
    /// When the last packet was read from the file.
    pub last_data: Option<Instant>,
    pub quality: Option<Quality>,
    /// Bumped whenever quality is recomputed, so SSE resends only on change.
    pub quality_seq: u64,
}

pub type SharedLive = Arc<Mutex<LiveState>>;

/// Newest session file in the directory. Capture names files with a UTC stamp,
/// so the lexicographically greatest name is the most recent.
fn newest_session(dir: &str) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "ftel"))
        .max()
}

/// Tail the newest session file forever, keeping `state` current. Spawned as a
/// daemon thread by `serve::run`; exits only with the process.
pub fn run_tailer(dir: String, state: SharedLive) {
    let mut tail: Option<(PathBuf, SessionTail)> = None;
    let mut frames: Vec<TimedFrame> = Vec::new();
    let mut frames_at_last_quality = 0usize;
    let mut last_quality_at = Instant::now() - QUALITY_INTERVAL;

    loop {
        let newest = newest_session(&dir);
        if newest.as_deref() != tail.as_ref().map(|(p, _)| p.as_path()) {
            tail = None;
            frames.clear();
            frames_at_last_quality = 0;
            if let Some(path) = &newest
                && let Ok(t) = SessionTail::open(path)
            {
                tail = Some((path.clone(), t));
            }
            let mut s = state.lock().unwrap();
            *s = LiveState { file: newest, ..LiveState::default() };
        }

        if let Some((_, t)) = &mut tail {
            match t.poll() {
                Ok(records) if !records.is_empty() => {
                    for (recv_us, payload) in &records {
                        if let Ok(frame) = packet::decode(payload) {
                            frames.push(TimedFrame { recv_us: *recv_us, frame });
                        }
                    }
                    let mut s = state.lock().unwrap();
                    s.latest = frames.last().copied();
                    s.last_data = Some(Instant::now());
                }
                Ok(_) => {}
                Err(_) => {
                    // Unreadable (bad magic / IO error): stop tailing this file.
                    tail = None;
                }
            }
        }

        if frames.len() > frames_at_last_quality && last_quality_at.elapsed() >= QUALITY_INTERVAL {
            frames_at_last_quality = frames.len();
            last_quality_at = Instant::now();
            let quality = compute_quality(&frames);
            let mut s = state.lock().unwrap();
            s.quality = quality;
            s.quality_seq += 1;
        }

        std::thread::sleep(POLL_INTERVAL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::TelemetryFrame;
    use std::io::Write;

    fn temp_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("tuners-live-{tag}-{}", std::process::id()))
    }

    fn record(recv_us: u64, payload: &[u8]) -> Vec<u8> {
        let mut r = Vec::new();
        r.extend_from_slice(&recv_us.to_le_bytes());
        r.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        r.extend_from_slice(payload);
        r
    }

    #[test]
    fn tail_handles_partial_records_across_polls() {
        let path = temp_path("partial");
        let mut f = File::create(&path).unwrap();
        f.write_all(MAGIC).unwrap();
        f.write_all(&record(1, &[10, 11])).unwrap();
        f.write_all(&record(2, &[20; 324])).unwrap();

        let mut tail = SessionTail::open(&path).unwrap();
        let got = tail.poll().unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0], (1, vec![10, 11]));

        // Append only part of the next record: nothing is yielded...
        let rec = record(3, &[30; 100]);
        f.write_all(&rec[..50]).unwrap();
        f.flush().unwrap();
        assert!(tail.poll().unwrap().is_empty());

        // ...until the rest arrives, then exactly that record comes out whole.
        f.write_all(&rec[50..]).unwrap();
        f.flush().unwrap();
        let got = tail.poll().unwrap();
        assert_eq!(got, vec![(3, vec![30; 100])]);
        assert!(tail.poll().unwrap().is_empty());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn tail_waits_for_magic_then_rejects_bad_magic() {
        let path = temp_path("magic");
        let mut f = File::create(&path).unwrap();
        f.write_all(&MAGIC[..4]).unwrap(); // header still being written
        let mut tail = SessionTail::open(&path).unwrap();
        assert!(tail.poll().unwrap().is_empty());

        f.write_all(b"XXXX").unwrap(); // completes to a non-magic header
        assert!(tail.poll().is_err());
        std::fs::remove_file(&path).ok();
    }

    /// Three identical flying laps (plus boundary): quality is Good with zero
    /// spread and full coverage.
    fn synth_session(n_flying: usize) -> Vec<TimedFrame> {
        let mut frames = Vec::new();
        let lap_s = 10.0f32;
        let speed = 50.0f32;
        for lap in 0..(n_flying + 2) {
            let race_t0 = lap as f32 * lap_s;
            let mut t = 0.0f32;
            while t < lap_s {
                frames.push(TimedFrame {
                    recv_us: ((race_t0 + t) * 1e6) as u64,
                    frame: TelemetryFrame {
                        is_race_on: true,
                        lap_number: lap as u16,
                        current_lap: t,
                        current_race_time: race_t0 + t,
                        last_lap: if lap == 0 { 0.0 } else { lap_s },
                        distance_traveled: 1000.0 + (race_t0 + t) * speed,
                        speed,
                        ..Default::default()
                    },
                });
                t += 0.1;
            }
        }
        frames
    }

    #[test]
    fn quality_none_without_laps_then_confident_with_agreeing_laps() {
        assert!(compute_quality(&[]).is_none());

        let q = compute_quality(&synth_session(3)).unwrap();
        assert_eq!(q.laps, 3, "flying laps only (out lap and partial tail dropped)");
        assert!(!q.standing_only);
        assert!(q.confidence > 0.99, "identical laps corroborate fully: {}", q.confidence);
        assert_eq!(q.band, Band::Good);
        assert!(q.spread_frac < 1e-3);
        assert!((q.best_lap_s - 10.0).abs() < 1e-3);
    }

    #[test]
    fn quality_single_lap_has_zero_confidence() {
        let q = compute_quality(&synth_session(1)).unwrap();
        assert_eq!(q.laps, 1);
        assert_eq!(q.confidence, 0.0, "nothing corroborates a lone lap");
        assert_eq!(q.band, Band::Low);

        let q = compute_quality(&synth_session(2)).unwrap();
        assert_eq!(q.laps, 2);
        assert_eq!(q.band, Band::Good, "two agreeing laps corroborate each other");
    }
}
