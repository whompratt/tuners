//! Understeer correction-event scan: per-stint rates of the driver fighting
//! a front that won't respond, clustered by corner across laps (scattered
//! one-offs are driving noise).
//!
//! - WIND-ON, FLAT YAW: steering deeper into the corner without yaw response.
//! - BRAKE REAPPLY: brake released mid-corner, then applied again.
//! - EXIT DOWNSHIFT: a downshift after the apex with the brake off.
//!
//! NEGATIVE RESULT (2026-07-31, two detector iterations): no channel
//! separates pushers from healthy cars, not even through the tester's
//! Mustang — the corpus's strongest push (14.5-30.5% front saturation)
//! driven by someone actively fighting it read wind-on 5.7-6.8% against
//! the same driver's healthy cars at 4.7-7.6%, zero clustered corners.
//! Correction events measure driver/track style, not car state; front
//! saturation remains the detection channel. Kept as the retest harness
//! should richer channels or corner-matched cross-lap deltas appear.
//!
//!   cargo run --release --example correction_scan -- sessions/*.ftel

use std::path::Path;
use tuners::analysis::{self, TimedFrame, corners, metrics};
use tuners::telemetry::{packet, stint::StintReader};

/// Lookback window for the wind-on test (s).
const WINDOW_S: f32 = 0.25;
/// |steer| gain over the window that counts as winding on (i8 units).
const WINDON_STEER: f32 = 15.0;
/// |yaw rate| gain below this over the same window = no response (rad/s).
const WINDON_YAW_GAIN: f32 = -0.01;
/// Brake must stay released this long before a reapplication counts (s).
const REAPPLY_GAP_S: f32 = 0.4;
const PEDAL_ON: u8 = 128;
/// A corner ordinal counts as CLUSTERED when affected in at least this share
/// of laps (and at least 2 laps).
const CLUSTER_FRAC: f32 = 0.5;

fn load_frames(path: &Path) -> Result<Vec<TimedFrame>, String> {
    let name = path.to_string_lossy();
    if name.ends_with(".tar.zst") {
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        let bundle = tuners::sharing::bundle::open(&bytes)?;
        let mut reader = StintReader::open_bytes(&bundle.stint).map_err(|e| e.to_string())?;
        let mut frames = Vec::new();
        while let Some((recv_us, payload)) = reader.next_packet().map_err(|e| e.to_string())? {
            if let Ok(frame) = packet::decode(&payload) {
                frames.push(TimedFrame { recv_us, frame });
            }
        }
        Ok(frames)
    } else {
        Ok(analysis::Stint::load(path)
            .map_err(|e| e.to_string())?
            .frames)
    }
}

/// Per-corner correction flags: (wind-on share of corner time, brake
/// reapplied, exit downshift).
fn corner_flags(slice: &[TimedFrame]) -> (f32, bool, bool) {
    // Wind-on with flat yaw, against a WINDOW_S lookback.
    let mut windon = 0usize;
    let mut j = 0usize;
    for (i, tf) in slice.iter().enumerate() {
        let t = tf.frame.current_race_time;
        while slice[j].frame.current_race_time < t - WINDOW_S {
            j += 1;
        }
        if j >= i {
            continue;
        }
        let (now, then) = (&tf.frame, &slice[j].frame);
        let into_corner = (now.steer > 0) == (now.acceleration[0] > 0.0);
        let steer_gain = now.steer.unsigned_abs() as f32 - then.steer.unsigned_abs() as f32;
        let yaw_gain = now.angular_velocity[1].abs() - then.angular_velocity[1].abs();
        if into_corner && steer_gain >= WINDON_STEER && yaw_gain <= WINDON_YAW_GAIN {
            windon += 1;
        }
    }
    // Brake reapplication after a sustained release.
    let mut reapply = false;
    let mut released_at: Option<f32> = None;
    let mut seen_brake = false;
    for tf in slice {
        let on = tf.frame.brake >= PEDAL_ON;
        let t = tf.frame.current_race_time;
        if on {
            if seen_brake && released_at.is_some_and(|r| t - r >= REAPPLY_GAP_S) {
                reapply = true;
            }
            seen_brake = true;
            released_at = None;
        } else if seen_brake && released_at.is_none() {
            released_at = Some(t);
        }
    }
    // Exit downshift: gear drops after the apex with the brake off.
    let apex = slice
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| a.frame.speed.total_cmp(&b.frame.speed))
        .map(|(i, _)| i)
        .unwrap_or(0);
    let mut downshift = false;
    let mut prev_gear: Option<u8> = None;
    for tf in &slice[apex..] {
        let g = tf.frame.gear;
        if (1..=10).contains(&g) {
            if let Some(p) = prev_gear
                && g < p
                && tf.frame.brake < PEDAL_ON
            {
                downshift = true;
            }
            prev_gear = Some(g);
        }
    }
    (
        windon as f32 / slice.len().max(1) as f32,
        reapply,
        downshift,
    )
}

fn main() {
    println!(
        "file\tcar\tsurface\tlaps\tcorners\twindon%\twindon_cl\treapply_cl\tdownshift_cl\tpush_ctx"
    );
    for p in std::env::args().skip(1) {
        let path = Path::new(&p);
        let frames = match load_frames(path) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("{p}: {e}");
                continue;
            }
        };
        let segments = analysis::driving_segments(&frames, 5.0);
        let Some(seg) = segments.iter().max_by_key(|s| s.len()) else {
            continue;
        };
        let m = metrics::stint_metrics(seg);
        let laps = analysis::split_laps(seg);
        let flying: Vec<_> = laps
            .iter()
            .filter(|l| l.time_s.is_some() && !l.standing_start)
            .collect();

        // Per-lap corner flags, keyed by ordinal; only laps with the modal
        // corner count align.
        let per_lap: Vec<Vec<(f32, bool, bool)>> = flying
            .iter()
            .map(|l| {
                corners::corner_events(l.frames)
                    .iter()
                    .map(|e| {
                        let s: Vec<TimedFrame> = l
                            .frames
                            .iter()
                            .filter(|tf| {
                                (e.start_s..=e.end_s).contains(&tf.frame.current_race_time)
                            })
                            .copied()
                            .collect();
                        corner_flags(&s)
                    })
                    .collect()
            })
            .collect();
        let mut counts = std::collections::BTreeMap::<usize, usize>::new();
        for l in &per_lap {
            *counts.entry(l.len()).or_default() += 1;
        }
        let modal = counts
            .iter()
            .max_by_key(|(len, n)| (**n, **len))
            .map(|(len, _)| *len)
            .unwrap_or(0);
        let aligned: Vec<&Vec<(f32, bool, bool)>> =
            per_lap.iter().filter(|l| l.len() == modal).collect();

        let mut windon_cl = 0usize;
        let mut reapply_cl = 0usize;
        let mut downshift_cl = 0usize;
        if aligned.len() >= 2 && modal > 0 {
            for c in 0..modal {
                let hits = |f: &dyn Fn(&(f32, bool, bool)) -> bool| {
                    aligned.iter().filter(|l| f(&l[c])).count() as f32 / aligned.len() as f32
                };
                windon_cl += (hits(&|x| x.0 >= 0.15) >= CLUSTER_FRAC) as usize;
                reapply_cl += (hits(&|x| x.1) >= CLUSTER_FRAC) as usize;
                downshift_cl += (hits(&|x| x.2) >= CLUSTER_FRAC) as usize;
            }
        }
        // Overall wind-on share of cornering time (all corners, all laps).
        let (mut wsum, mut wn) = (0.0f32, 0usize);
        for l in &per_lap {
            for &(w, _, _) in l {
                wsum += w;
                wn += 1;
            }
        }
        let name = path.file_stem().unwrap().to_string_lossy();
        let name = name.strip_suffix(".tar").unwrap_or(&name).to_string();
        println!(
            "{}\t{}\t{}\t{}\t{}\t{:.1}\t{}\t{}\t{}\t{}",
            name,
            m.car_ordinal,
            if m.surface_loose { "dirt" } else { "tarmac" },
            aligned.len(),
            modal,
            wsum / wn.max(1) as f32 * 100.0,
            windon_cl,
            reapply_cl,
            downshift_cl,
            m.grip_saturation
                .map(|g| format!("{:.1}", g.push_frac * 100.0))
                .unwrap_or_default(),
        );
    }
}
