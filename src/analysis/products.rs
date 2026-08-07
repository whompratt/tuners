//! Compact per-stint analysis products, and a process-wide cache of them.
//!
//! A raw recording is tens of MB of frames, but everything the campaign
//! analysis consumes pair-to-pair is KB-scale: the lap profile, the
//! longest-segment metrics, the effect vector, and the cornering grip
//! samples. `StintData` is that distillation; frames are decoded, reduced,
//! and dropped. The cache keys on canonical path + (size, mtime), so the
//! still-recording newest stint re-derives whenever it grows while every
//! closed stint is computed exactly once per process.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

use super::{
    MIN_DRIVING_SPEED_MPS, RESTART_RACE_T_S, Stint, TimedFrame, driving_segments, effects,
    finish_certificate, grip, metrics, profile, split_laps,
};
use crate::telemetry::packet::{self, TelemetryFrame};
use crate::telemetry::stint::StintReader;

/// Everything downstream analysis needs from one recording, without frames.
pub struct StintData {
    /// First driving frame's car ordinal.
    pub car: Option<i32>,
    /// Lap profile; Err when the recording has no complete laps (yet).
    pub profile: Result<profile::StintProfile, String>,
    /// Overall metrics of the longest driving segment (None when too short).
    pub met: Option<metrics::StintMetrics>,
    /// Per-flying-lap metrics of the longest segment.
    pub per_lap: Vec<metrics::StintMetrics>,
    /// Effect vector from `met` (empty when metrics are absent).
    pub fx: effects::Effects,
    /// Cornering grip samples of the longest segment.
    pub samples: Vec<grip::GripSample>,
}

pub fn compute(stint: &Stint) -> StintData {
    let car = stint
        .frames
        .iter()
        .find(|t| t.frame.car_ordinal != 0)
        .map(|t| t.frame.car_ordinal);
    let profile = profile::stint_profile(&stint.frames);
    let segments = driving_segments(&stint.frames, 5.0);
    let longest = segments.iter().max_by_key(|s| s.len());
    let met = longest.map(|l| metrics::stint_metrics(l));
    let per_lap = longest
        .map(|l| {
            split_laps(l)
                .iter()
                .filter(|lap| lap.time_s.is_some() && !lap.standing_start)
                .map(|lap| metrics::stint_metrics(lap.frames))
                .collect()
        })
        .unwrap_or_default();
    let fx = met.as_ref().map(effects::vector).unwrap_or_default();
    let samples = longest
        .map(|l| grip::cornering_samples(l))
        .unwrap_or_default();
    StintData {
        car,
        profile,
        met,
        per_lap,
        fx,
        samples,
    }
}

/// Sibling dependency of a cached entry: the NEXT recording's head can
/// complete this recording's final run (a finish certificate stranded across
/// the recorder's idle cut; see `adopt_stranded_finish`). `settled` means the
/// sibling's head can no longer change the outcome (certificate found, a
/// different car, or real driving began before one), so a still-growing
/// sibling stops invalidating this entry.
struct Dep {
    path: PathBuf,
    len: u64,
    mtime: Option<SystemTime>,
    settled: bool,
}

struct Entry {
    len: u64,
    mtime: Option<SystemTime>,
    /// The recording ends with a time-less run, so a future sibling
    /// recording could still complete it.
    tail_open: bool,
    dep: Option<Dep>,
    tick: u64,
    data: Arc<StintData>,
}

/// Records scanned at most from a sibling's head before giving up (about
/// half an hour of frames; a certificate arrives within minutes of the cut).
const HEAD_SCAN_CAP: usize = 120_000;

/// Trailing "YYYYMMDD-HHMMSS" stamp of a recording filename.
fn trailing_stamp(name: &str) -> Option<&str> {
    let stem = name.strip_suffix(".ftel")?;
    let s = stem.get(stem.len().checked_sub(15)?..)?;
    (s.as_bytes()[8] == b'-'
        && s.bytes()
            .enumerate()
            .all(|(i, b)| i == 8 || b.is_ascii_digit()))
    .then_some(s)
}

/// The recording that follows `path` in its directory, by stamp order.
fn next_recording(path: &Path) -> Option<PathBuf> {
    let dir = path.parent()?;
    let my = trailing_stamp(path.file_name()?.to_str()?)?.to_string();
    let mut best: Option<(String, PathBuf)> = None;
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let name = e.file_name();
        let Some(stamp) = name.to_str().and_then(trailing_stamp) else {
            continue;
        };
        if stamp > my.as_str() && best.as_ref().is_none_or(|(b, _)| stamp < b.as_str()) {
            best = Some((stamp.to_string(), e.path()));
        }
    }
    best.map(|(_, p)| p)
}

/// Scan the head of a sibling recording for a finish certificate completing
/// `last` (the previous recording's final driving frame). Returns the
/// certificate frame if found, and whether the head is SETTLED: a found
/// certificate, a different car, or real driving starting all mean later
/// growth cannot change the answer; a clean EOF first means keep watching.
fn head_certificate(
    path: &Path,
    car: Option<i32>,
    last: &TelemetryFrame,
) -> (Option<TimedFrame>, bool) {
    let Ok(mut reader) = StintReader::open(path) else {
        return (None, false);
    };
    let mut n = 0usize;
    while let Ok(Some((recv_us, payload))) = reader.next_packet() {
        n += 1;
        if n > HEAD_SCAN_CAP {
            return (None, true);
        }
        let Ok(f) = packet::decode(&payload) else {
            continue;
        };
        if !f.is_race_on {
            continue;
        }
        if car.is_some() && f.car_ordinal != 0 && Some(f.car_ordinal) != car {
            return (None, true);
        }
        if finish_certificate(last, &f) {
            return (Some(TimedFrame { recv_us, frame: f }), true);
        }
        if f.current_race_time < RESTART_RACE_T_S && f.speed > MIN_DRIVING_SPEED_MPS {
            return (None, true);
        }
    }
    (None, false)
}

/// If this recording ends with a run that never got its finish (the game
/// cuts to race-off AT the line and the certificate frame arrives from the
/// results screen later; when the recorder's idle cut lands in between, the
/// certificate opens the NEXT recording instead), pull the certificate from
/// the sibling's head and append it (behind a synthetic race-off frame) so
/// `driving_segments` adopts the time exactly as it does in-file. Returns
/// (tail_open, sibling dependency) for cache invalidation.
fn adopt_stranded_finish(stint: &mut Stint, path: &Path) -> (bool, Option<Dep>) {
    let Some(idx) = stint
        .frames
        .iter()
        .rposition(|t| t.frame.is_race_on && t.frame.speed > MIN_DRIVING_SPEED_MPS)
    else {
        return (false, None);
    };
    let last = stint.frames[idx].frame;
    let closed = stint.frames[idx + 1..]
        .iter()
        .any(|t| t.frame.is_race_on && t.frame.lap_number == last.lap_number.wrapping_add(1));
    if closed {
        return (false, None);
    }
    let Some(sibling) = next_recording(path) else {
        return (true, None);
    };
    let car = stint
        .frames
        .iter()
        .find(|t| t.frame.car_ordinal != 0)
        .map(|t| t.frame.car_ordinal);
    let meta = std::fs::metadata(&sibling).ok();
    let (cert, settled) = head_certificate(&sibling, car, &last);
    if let Some(cert) = cert {
        stint.frames.push(TimedFrame {
            recv_us: cert.recv_us.saturating_sub(1),
            frame: TelemetryFrame::default(),
        });
        stint.frames.push(cert);
    }
    let dep = meta.map(|m| Dep {
        path: sibling,
        len: m.len(),
        mtime: m.modified().ok(),
        settled,
    });
    (true, dep)
}

/// Is a cached entry's sibling dependency still current? Settled deps never
/// invalidate; an unsettled one re-checks the sibling's identity, and a
/// tail-open entry with no sibling re-checks that none has appeared.
fn dep_current(e: &Entry, path: &Path) -> bool {
    if !e.tail_open {
        return true;
    }
    match &e.dep {
        None => next_recording(path).is_none(),
        Some(d) if d.settled => true,
        Some(d) => match std::fs::metadata(&d.path) {
            Ok(m) => m.len() == d.len && m.modified().ok() == d.mtime,
            Err(_) => false,
        },
    }
}

#[derive(Default)]
struct Cache {
    map: HashMap<PathBuf, Entry>,
    tick: u64,
    hits: u64,
    misses: u64,
}

static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();

/// Entry cap: compact products run ~0.5-2 MB per stint, so the cache stays
/// in the low hundreds of MB even against a monster library.
const CAP: usize = 256;

fn cache() -> &'static Mutex<Cache> {
    CACHE.get_or_init(Default::default)
}

/// Load a recording's products through the cache. Recomputes when the file's
/// size or mtime changed (the recorder appends to the newest stint); errors
/// carry no path prefix, callers add their own.
pub fn cached(path: &Path) -> Result<Arc<StintData>, String> {
    // Journal references are root-relative; resolve them against the data
    // root so callers are CWD-independent.
    let path = &crate::util::resolve_data(path);
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    let key = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let (len, mtime) = (meta.len(), meta.modified().ok());
    {
        let mut c = cache().lock().unwrap();
        c.tick += 1;
        let tick = c.tick;
        let hit = c.map.get_mut(&key).and_then(|e| {
            (e.len == len && e.mtime == mtime && dep_current(e, path)).then(|| {
                e.tick = tick;
                e.data.clone()
            })
        });
        if let Some(data) = hit {
            c.hits += 1;
            return Ok(data);
        }
        c.misses += 1;
    }
    // Compute outside the lock: concurrent misses on different stints (the
    // background map refresher vs an advise call) must not serialize.
    let mut stint = Stint::load(path).map_err(|e| e.to_string())?;
    let (tail_open, dep) = adopt_stranded_finish(&mut stint, path);
    let data = Arc::new(compute(&stint));
    let mut c = cache().lock().unwrap();
    let tick = c.tick;
    if c.map.len() >= CAP
        && !c.map.contains_key(&key)
        && let Some(oldest) = c
            .map
            .iter()
            .min_by_key(|(_, e)| e.tick)
            .map(|(k, _)| k.clone())
    {
        c.map.remove(&oldest);
    }
    c.map.insert(
        key,
        Entry {
            len,
            mtime,
            tail_open,
            dep,
            tick,
            data: data.clone(),
        },
    );
    Ok(data)
}

/// (hits, misses) since process start, for the advise trace.
pub fn cache_counters() -> (u64, u64) {
    let c = cache().lock().unwrap();
    (c.hits, c.misses)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A grown file (the still-recording stint) must re-derive; an untouched
    /// one must come back as the same Arc.
    #[test]
    fn cache_tracks_file_identity() {
        let dir = std::env::temp_dir().join(format!("tuners-products-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("stint-20260101-000000.ftel");
        let mut bytes = crate::telemetry::stint::MAGIC.to_vec();
        std::fs::write(&path, &bytes).unwrap();
        let a = cached(&path).unwrap();
        let b = cached(&path).unwrap();
        assert!(Arc::ptr_eq(&a, &b), "unchanged file must hit the cache");
        // Append a record (12-byte header, zero-length payload; decode
        // failures only count, they don't error): the size change must
        // invalidate.
        bytes.extend_from_slice(&[0u8; 12]);
        std::fs::write(&path, &bytes).unwrap();
        let c = cached(&path).unwrap();
        assert!(!Arc::ptr_eq(&a, &c), "grown file must recompute");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn p2p_frame(t: f32) -> TelemetryFrame {
        TelemetryFrame {
            is_race_on: true,
            current_race_time: t,
            current_lap: t,
            distance_traveled: t * 44.0 + 1.0,
            speed: 44.0,
            car_ordinal: 42,
            ..Default::default()
        }
    }

    fn write_frames(path: &Path, frames: &[TelemetryFrame]) {
        let mut w = crate::telemetry::stint::StintWriter::create(path).unwrap();
        for (i, f) in frames.iter().enumerate() {
            w.write_packet(1_000_000 + i as u64 * 20_000, &packet::encode(f))
                .unwrap();
        }
    }

    /// A run whose finish certificate landed in the NEXT recording (the
    /// recorder's idle cut fell between the line and the results-screen
    /// flicker) gets its time from the sibling's head; the certificate
    /// arriving LATER (the live case: the sibling is still recording when
    /// the first advise runs) invalidates the cached entry and completes
    /// the run on recompute.
    #[test]
    fn stranded_certificate_completes_across_recordings() {
        let dir = std::env::temp_dir().join(format!("tuners-stranded-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("stint-20260101-000000.ftel");
        let frames: Vec<TelemetryFrame> = (0..7000).map(|i| p2p_frame(i as f32 * 0.02)).collect();
        write_frames(&a, &frames);
        // Sibling exists but holds only menu frames so far.
        let b = dir.join("stint-20260101-001000.ftel");
        write_frames(&b, &[TelemetryFrame::default(); 10]);

        let d = cached(&a).unwrap();
        assert!(d.profile.is_err(), "no certificate yet: run is time-less");

        // The certificate frame arrives in the sibling (raw record append:
        // recv_us u64 LE + payload len u32 LE + payload).
        let cert = TelemetryFrame {
            is_race_on: true,
            current_race_time: 151.0,
            current_lap: 0.0,
            last_lap: 140.02,
            lap_number: 1,
            distance_traveled: 196.0,
            speed: 5.2,
            car_ordinal: 42,
            ..Default::default()
        };
        let payload = packet::encode(&cert);
        let mut rec = 9_000_000u64.to_le_bytes().to_vec();
        rec.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        rec.extend_from_slice(&payload);
        use std::io::Write;
        std::fs::OpenOptions::new()
            .append(true)
            .open(&b)
            .unwrap()
            .write_all(&rec)
            .unwrap();

        let d = cached(&a).unwrap();
        let p = d.profile.as_ref().expect("adopted run must profile");
        assert_eq!(p.laps.len(), 1);
        assert!((p.best_lap_time_s - 140.02).abs() < 0.01);
        assert!(p.point_to_point);

        // Settled now: the sibling growing further must not invalidate.
        let again = cached(&a).unwrap();
        assert!(Arc::ptr_eq(&d, &again));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
