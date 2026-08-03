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

use super::{Stint, driving_segments, effects, grip, metrics, profile, split_laps};

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

struct Entry {
    len: u64,
    mtime: Option<SystemTime>,
    tick: u64,
    data: Arc<StintData>,
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
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    let key = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let (len, mtime) = (meta.len(), meta.modified().ok());
    {
        let mut c = cache().lock().unwrap();
        c.tick += 1;
        let tick = c.tick;
        let hit = c.map.get_mut(&key).and_then(|e| {
            (e.len == len && e.mtime == mtime).then(|| {
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
    let stint = Stint::load(path).map_err(|e| e.to_string())?;
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
}
