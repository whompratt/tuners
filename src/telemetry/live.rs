//! Live view: tail the newest session file as `tuners capture` appends to it and
//! keep a shared snapshot (latest frame + data-quality summary) for the dashboard
//! to relay over SSE.
//!
//! The tailer never binds the UDP port; the recorder (or capture) owns it. StintWriter
//! writes each record with a single unbuffered `write_all`, so tailing the file
//! sees new packets within one poll interval.

use crate::analysis::TimedFrame;
use crate::telemetry::packet;
use crate::telemetry::stint::MAGIC;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How often the tail thread polls the sessions directory / file for new data.
pub const POLL_INTERVAL: Duration = Duration::from_millis(200);
/// Recompute the (whole-session) quality summary at most this often.
const QUALITY_INTERVAL: Duration = Duration::from_secs(2);

/// Incremental reader over a growing session file. Unlike StintReader it
/// tolerates a partially-written record at end-of-file: bytes are buffered until
/// a complete record is available, and the next poll picks up where it left off.
pub struct StintTail {
    file: File,
    buf: Vec<u8>,
    magic_ok: bool,
}

impl StintTail {
    pub fn open(path: &Path) -> std::io::Result<Self> {
        Ok(Self {
            file: File::open(path)?,
            buf: Vec::new(),
            magic_ok: false,
        })
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
/// score. Cutoffs calibrated against the real session library:
/// sessions the user drew tuning conclusions from should land in Good.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Band {
    Low,
    Ok,
    Good,
}

/// Corroboration score at or above this reads green. Calibrated against the
/// GRADED score's library distribution (2026-08-01): per lap count the
/// medians read 0.62 (2 laps) / 0.80 (3) / 0.89 (4) / 0.95+ (6+), so green
/// needs either 4+ agreeing laps or an unusually tight 3-lap stint —
/// deliberately cautious, the gauge asks for more laps rather than
/// flattering thin data.
pub const GOOD_MIN_SCORE: f32 = 0.85;
/// At or above this reads orange; below is red. Two-lap stints land orange
/// (median 0.62) unless the two laps genuinely disagree.
pub const OK_MIN_SCORE: f32 = 0.60;

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
    /// Standing runs with the lap clock locked to the race clock: a
    /// point-to-point route, not a circuit's out laps.
    pub point_to_point: bool,
    /// Recordings contributing laps: 1 = the live one alone, more when
    /// same-setup predecessors (crash- or idle-cut) pool into the score.
    pub recordings: u32,
    pub best_lap_s: f32,
    /// (worst − best) / best over profiled laps: driving consistency.
    pub spread_frac: f32,
    /// Distance every profiled lap covers (the comparable route length).
    pub shared_km: f32,
    /// Time-weighted graded support of the ideal lap by the other laps
    /// (profile::Corroboration), the headline confidence value.
    pub confidence: f32,
    pub band: Band,
    /// Why confidence is not green, when it isn't: the one thing the driver
    /// can do about it. None when the band is Good.
    pub note: Option<&'static str>,
}

/// Quality over the frames captured so far; None until a comparable lap
/// exists somewhere. `priors` are same-setup predecessor recordings (the
/// journal proves no tune change since — see `pool_predecessors`): their laps
/// pool with the live recording's so a crash- or idle-cut run does not reset
/// confidence to zero. Only priors on the same route (shared length within
/// 2%) and of the same standing-start character as the anchor pool; the
/// anchor is the live recording once it has a lap, else the newest prior.
pub fn compute_quality(
    frames: &[TimedFrame],
    priors: &[std::sync::Arc<crate::analysis::products::StintData>],
) -> Option<Quality> {
    use crate::analysis::profile::StintProfile;
    let current = crate::analysis::profile::stint_profile(frames).ok();
    let prior_profiles: Vec<&StintProfile> = priors
        .iter()
        .filter_map(|d| d.profile.as_ref().ok())
        .collect();
    let (anchor_bins, anchor_standing, anchor_car) = {
        let a = current
            .as_ref()
            .or_else(|| prior_profiles.first().copied())?;
        (a.shared_bins, a.standing_start_only, a.car_ordinal)
    };
    let compatible = |p: &StintProfile| {
        let (short, long) = (
            p.shared_bins.min(anchor_bins),
            p.shared_bins.max(anchor_bins),
        );
        p.standing_start_only == anchor_standing && (long - short) as f32 <= long as f32 * 0.02
    };
    let mut pooled_laps = Vec::new();
    let mut recordings = 0u32;
    for p in prior_profiles.iter().filter(|p| compatible(p)) {
        pooled_laps.extend(p.laps.iter().cloned());
        recordings += 1;
    }
    let profile = match current {
        Some(cur) if recordings > 0 => {
            pooled_laps.extend(cur.laps.iter().cloned());
            recordings += 1;
            StintProfile::from_laps(pooled_laps, cur.car_ordinal)?
        }
        Some(cur) => {
            recordings = 1;
            cur
        }
        None => StintProfile::from_laps(pooled_laps, anchor_car)?,
    };
    let laps = profile.laps.len();
    let worst = profile.laps.iter().map(|l| l.time_s).fold(0.0f32, f32::max);
    let spread_frac = (worst - profile.best_lap_time_s).max(0.0) / profile.best_lap_time_s;
    let corr = profile.corroboration();
    let confidence = corr.score;
    let band = Band::from_score(confidence);
    // Point-to-point sessions (and restart-per-run circuit driving) have no
    // flying laps: every kept lap is a standing run, so the nudges say "run".
    let standing = profile.standing_start_only;
    let note = match band {
        Band::Good => None,
        _ if laps < 2 => Some(if standing {
            "one run: nothing can confirm it yet — more runs build confidence"
        } else {
            "one comparable lap: nothing can confirm it yet — more laps build confidence"
        }),
        _ if laps == 2 => Some(if standing {
            "two runs: every stretch rests on a single confirmation — more runs build confidence"
        } else {
            "two laps: every stretch rests on a single confirmation — more laps build confidence"
        }),
        _ if corr.harvest_support.is_some_and(|s| s < 0.5) => Some(if standing {
            "the optimal run uses stretches no other run reproduced — more consistent runs build confidence"
        } else {
            "the optimal lap uses stretches no other lap reproduced — more consistent laps build confidence"
        }),
        _ => Some(if standing {
            "runs vary run-to-run — more consistent runs build confidence"
        } else {
            "laps vary lap-to-lap — more consistent laps build confidence"
        }),
    };
    Some(Quality {
        laps,
        standing_only: profile.standing_start_only,
        point_to_point: profile.point_to_point,
        recordings,
        best_lap_s: profile.best_lap_time_s,
        spread_frac,
        shared_km: profile.shared_bins as f32 * crate::analysis::profile::BIN_METERS / 1000.0,
        confidence,
        band,
        note,
    })
}

/// Snapshot shared between the tail thread and SSE handlers. The frame timeline
/// itself stays owned by the tail thread; only the distilled state is shared.
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
    /// A start-line lap-clock reset was seen in the CURRENT race start: the
    /// run is a circuit lap, not a point-to-point run. Cleared on restart.
    /// Lets the gauge assume point-to-point and update at the line crossing.
    pub circuit_seen: bool,
}

pub type SharedLive = Arc<Mutex<LiveState>>;

/// Same-setup predecessors of `current`, newest first: the recordings whose
/// laps may pool into the live confidence score. Journal notes mark setup
/// boundaries (a note on a stint = the setup changed AT that stint), so the
/// walk goes backwards from the current recording and stops after including
/// the first noted stint — that stint drove the current setup first. A note
/// on the CURRENT stint means a fresh setup: nothing pools. Unjournaled
/// stints between lines are no-change cuts (crash, idle, car re-entry) and
/// pool freely. Without a journal (blind mode) nothing is provably
/// same-setup, so nothing pools.
fn same_setup_predecessors(
    noted: &std::collections::HashSet<String>,
    since: Option<&str>,
    stints_newest_first: &[String],
    current_name: &str,
) -> Vec<String> {
    if noted.contains(current_name) {
        return Vec::new();
    }
    let name_of = |p: &str| {
        Path::new(p)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let mut out = Vec::new();
    let mut seen_current = false;
    for p in stints_newest_first {
        let name = name_of(p);
        if !seen_current {
            seen_current = name == current_name;
            continue;
        }
        // Resumed campaigns own only stints at or after the resume stamp.
        if let (Some(floor), Some(stamp)) = (since, crate::advice::advise::stint_stamp(&name))
            && stamp < floor
        {
            break;
        }
        out.push(p.clone());
        if noted.contains(&name) || out.len() >= MAX_POOLED_PREDECESSORS {
            break;
        }
    }
    out
}

/// Cap on pooled predecessor recordings: enough to bridge a string of
/// crash/idle cuts without profiling half the library at file switch.
const MAX_POOLED_PREDECESSORS: usize = 6;

/// Load the poolable predecessors of `current` (see
/// `same_setup_predecessors`) through the process-wide product cache.
fn pool_predecessors(
    dir: &str,
    session_file: &Path,
    journal_base: &str,
    current: &Path,
) -> Vec<Arc<crate::analysis::products::StintData>> {
    use crate::advice::advise::campaign_bound;
    let session = crate::advice::tuning::TuningSession::load(session_file);
    let Some(car) = session.car else {
        return Vec::new();
    };
    let journal = crate::advice::tuning::journal_path_for(Some(car), journal_base);
    let Ok(text) = std::fs::read_to_string(&journal) else {
        return Vec::new();
    };
    let since = match campaign_bound(&text) {
        crate::advice::advise::CampaignBound::Open => None,
        crate::advice::advise::CampaignBound::Closed => return Vec::new(),
        crate::advice::advise::CampaignBound::Since(s) => Some(s),
    };
    let noted: std::collections::HashSet<String> = crate::advice::journal::parse_journal(&text)
        .into_iter()
        .filter(|e| e.note.is_some())
        .filter_map(|e| {
            Path::new(&e.path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
        })
        .collect();
    let stints: Vec<String> =
        crate::advice::advise::stints_for_car_newest_first(dir, Some(car)).collect();
    let current_name = current
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    same_setup_predecessors(&noted, since.as_deref(), &stints, &current_name)
        .iter()
        .filter_map(|p| crate::analysis::products::cached(Path::new(p)).ok())
        .collect()
}

/// Newest session file in the directory. Capture names files with a UTC stamp,
/// so the lexicographically greatest name is the most recent.
fn newest_stint(dir: &str) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "ftel"))
        .max()
}

/// Tail the newest session file forever, keeping `state` current. Spawned as a
/// daemon thread by the app shell; exits only with the process. The session
/// file and journal base identify same-setup predecessor recordings whose
/// laps pool into the live confidence score.
pub fn run_tailer(dir: String, session_file: String, journal_base: String, state: SharedLive) {
    let mut tail: Option<(PathBuf, StintTail)> = None;
    let mut frames: Vec<TimedFrame> = Vec::new();
    let mut frames_at_last_quality = 0usize;
    let mut last_quality_at = Instant::now() - QUALITY_INTERVAL;
    // Route-kind tracking for the current race start (see LiveState).
    let mut prev_race: Option<crate::telemetry::packet::TelemetryFrame> = None;
    let mut circuit_seen = false;
    // Same-setup predecessors of the tailed file (refreshed at file switch).
    let mut priors: Vec<Arc<crate::analysis::products::StintData>> = Vec::new();

    loop {
        let newest = newest_stint(&dir);
        if newest.as_deref() != tail.as_ref().map(|(p, _)| p.as_path()) {
            tail = None;
            frames.clear();
            frames_at_last_quality = 0;
            prev_race = None;
            circuit_seen = false;
            if let Some(path) = &newest
                && let Ok(t) = StintTail::open(path)
            {
                tail = Some((path.clone(), t));
            }
            priors = newest
                .as_deref()
                .map(|p| pool_predecessors(&dir, session_file.as_ref(), &journal_base, p))
                .unwrap_or_default();
            let quality = (!priors.is_empty())
                .then(|| compute_quality(&[], &priors))
                .flatten();
            let mut s = state.lock().unwrap();
            *s = LiveState {
                file: newest,
                quality,
                quality_seq: s.quality_seq + 1,
                ..LiveState::default()
            };
        }

        if let Some((_, t)) = &mut tail {
            match t.poll() {
                Ok(records) if !records.is_empty() => {
                    for (recv_us, payload) in &records {
                        if let Ok(frame) = packet::decode(payload) {
                            if frame.is_race_on {
                                if let Some(p) = &prev_race {
                                    // Restart: the race clock stepped back to ~0.
                                    if frame.current_race_time < p.current_race_time - 0.25
                                        && frame.current_race_time < 5.0
                                    {
                                        circuit_seen = false;
                                    } else if crate::analysis::line_reset_step(p, &frame) {
                                        circuit_seen = true;
                                    }
                                }
                                prev_race = Some(frame);
                            }
                            frames.push(TimedFrame {
                                recv_us: *recv_us,
                                frame,
                            });
                        }
                    }
                    let mut s = state.lock().unwrap();
                    s.latest = frames.last().copied();
                    s.last_data = Some(Instant::now());
                    s.circuit_seen = circuit_seen;
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
            let quality = compute_quality(&frames, &priors);
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
    use crate::telemetry::packet::TelemetryFrame;
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

        let mut tail = StintTail::open(&path).unwrap();
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
        let mut tail = StintTail::open(&path).unwrap();
        assert!(tail.poll().unwrap().is_empty());

        f.write_all(b"XXXX").unwrap(); // completes to a non-magic header
        assert!(tail.poll().is_err());
        std::fs::remove_file(&path).ok();
    }

    /// Three identical flying laps (plus boundary): quality is Good with zero
    /// spread and full coverage.
    fn synth_session(n_flying: usize) -> Vec<TimedFrame> {
        synth_session_laps(n_flying, 10.0)
    }

    fn synth_session_laps(n_flying: usize, lap_s: f32) -> Vec<TimedFrame> {
        let mut frames = Vec::new();
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
        assert!(compute_quality(&[], &[]).is_none());

        let q = compute_quality(&synth_session(3), &[]).unwrap();
        assert_eq!(
            q.laps, 3,
            "flying laps only (out lap and partial tail dropped)"
        );
        assert!(!q.standing_only);
        assert!(
            q.confidence > 0.99,
            "identical laps corroborate fully: {}",
            q.confidence
        );
        assert_eq!(q.band, Band::Good);
        assert!(q.spread_frac < 1e-3);
        assert!((q.best_lap_s - 10.0).abs() < 1e-3);
    }

    #[test]
    fn quality_single_lap_has_zero_confidence() {
        let q = compute_quality(&synth_session(1), &[]).unwrap();
        assert_eq!(q.laps, 1);
        assert_eq!(q.confidence, 0.0, "nothing corroborates a lone lap");
        assert_eq!(q.band, Band::Low);

        let q = compute_quality(&synth_session(2), &[]).unwrap();
        assert_eq!(q.laps, 2);
        assert_eq!(
            q.band,
            Band::Good,
            "two agreeing laps corroborate each other"
        );
    }

    /// A crash-cut recording pools with its same-setup predecessors: one live
    /// lap plus a two-lap prior recording scores as three laps across two
    /// recordings, so the gauge survives the cut. A prior from a different
    /// route (shared length off by more than 2%) must not pool.
    #[test]
    fn quality_pools_same_setup_predecessor_recordings() {
        use crate::analysis::{Stint, products};
        let prior = Arc::new(products::compute(&Stint {
            frames: synth_session(2),
            decode_errors: 0,
        }));
        let q = compute_quality(&synth_session(1), std::slice::from_ref(&prior)).unwrap();
        assert_eq!(q.laps, 3);
        assert_eq!(q.recordings, 2);
        assert_eq!(q.band, Band::Good, "prior laps corroborate the live one");

        // Before the live recording completes a lap, priors alone carry it.
        let q = compute_quality(&[], std::slice::from_ref(&prior)).unwrap();
        assert_eq!(q.laps, 2);
        assert_eq!(q.recordings, 1);

        let other_route = Arc::new(products::compute(&Stint {
            frames: synth_session_laps(2, 15.0),
            decode_errors: 0,
        }));
        let q = compute_quality(&synth_session(1), std::slice::from_ref(&other_route)).unwrap();
        assert_eq!(q.recordings, 1, "different route must not pool");
        assert_eq!(q.laps, 1);
    }

    /// Journal notes bound the same-setup walk: pooling includes unjournaled
    /// cuts and the first noted stint (which drove this setup first), stops
    /// there, drops everything when the CURRENT stint carries a note (fresh
    /// setup), and respects a campaign resume floor.
    #[test]
    fn predecessor_selection_stops_at_setup_changes() {
        let stints: Vec<String> = [
            "sessions/stint-20260803-000500.ftel",
            "sessions/stint-20260803-000400.ftel",
            "sessions/stint-20260803-000300.ftel",
            "sessions/stint-20260803-000200.ftel",
            "sessions/stint-20260803-000100.ftel",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let noted: std::collections::HashSet<String> =
            ["stint-20260803-000300.ftel".to_string()].into();

        let picked = same_setup_predecessors(&noted, None, &stints, "stint-20260803-000500.ftel");
        assert_eq!(
            picked,
            [
                "sessions/stint-20260803-000400.ftel",
                "sessions/stint-20260803-000300.ftel"
            ],
            "pool the unjournaled cut and the noted setup-first stint, then stop"
        );

        let current_noted: std::collections::HashSet<String> =
            ["stint-20260803-000500.ftel".to_string()].into();
        assert!(
            same_setup_predecessors(&current_noted, None, &stints, "stint-20260803-000500.ftel")
                .is_empty(),
            "a note on the current stint means a fresh setup"
        );

        let picked = same_setup_predecessors(
            &std::collections::HashSet::new(),
            Some("20260803-000300"),
            &stints,
            "stint-20260803-000500.ftel",
        );
        assert_eq!(
            picked,
            [
                "sessions/stint-20260803-000400.ftel",
                "sessions/stint-20260803-000300.ftel"
            ],
            "resume floor keeps stints at or after the stamp"
        );
    }
}
