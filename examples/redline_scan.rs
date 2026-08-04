//! Redline forensics: per-gear rpm ceilings, the effective-redline detector's
//! view, and the torque-cut signature (full-throttle frames with torque <= 0
//! at high rpm = the limiter cutting), per recording.
//!
//! Usage:
//!   cargo run --release --example redline_scan -- <path.ftel>   # detail
//!   cargo run --release --example redline_scan -- --sweep <dir> # one line each

use tuners::analysis::{self, Stint, TimedFrame};

const AIRBORNE_TRAVEL: f32 = 0.06;

struct Scan {
    ordinal: i32,
    reported: f32,
    frames: usize,
    gear_counts: [usize; 12],
    gear_max: [f32; 12],
    ceiling: f32,
    gears_at_ceiling: usize,
    old_adopts: bool,
    /// Rpm at each cut ONSET: full-throttle, moving, no gear change within
    /// ±10 frames, torque crosses from >0 to <=0. The limiter's fingerprint.
    onset_rpms: Vec<f32>,
    /// Total no-shift full-throttle torque<=0 frames (dwell mass in the cut).
    cut_frames: usize,
}

fn scan(frames: &[TimedFrame]) -> Scan {
    let first = &frames[0].frame;
    let mut gear_counts = [0usize; 12];
    let mut gear_max = [0.0f32; 12];
    let mut hist = std::collections::BTreeMap::<i32, u32>::new();
    let mut onset_rpms: Vec<f32> = Vec::new();
    let mut cut_frames = 0usize;
    let n = frames.len();
    let shift_near = |i: usize| {
        let lo = i.saturating_sub(10);
        let hi = (i + 10).min(n - 1);
        let g = frames[i].frame.gear;
        (lo..=hi).any(|j| frames[j].frame.gear != g)
    };
    for i in 0..n {
        let f = &frames[i].frame;
        if !f.is_race_on || !(1..=10).contains(&f.gear) {
            continue;
        }
        let travel = f.norm_suspension_travel.to_array();
        if travel.iter().all(|v| *v <= AIRBORNE_TRAVEL) {
            continue;
        }
        gear_counts[f.gear as usize] += 1;
        gear_max[f.gear as usize] = gear_max[f.gear as usize].max(f.current_engine_rpm);
        *hist
            .entry((f.current_engine_rpm / 25.0) as i32)
            .or_default() += 1;
        // Cut evidence: full throttle, moving (not a launch cap), torque
        // collapsed, away from any gear change (shift dips straddle one).
        if f.accel >= 250 && f.speed > 15.0 && f.torque <= 0.0 && i > 0 && !shift_near(i) {
            cut_frames += 1;
            if frames[i - 1].frame.torque > 0.0 {
                onset_rpms.push(
                    f.current_engine_rpm
                        .max(frames[i - 1].frame.current_engine_rpm),
                );
            }
        }
    }
    let ceiling = hist
        .iter()
        .rev()
        .find(|(_, count)| **count >= 15)
        .map(|(bucket, _)| (*bucket as f32 + 1.0) * 25.0)
        .unwrap_or(0.0);
    let gears_at_ceiling = (1..=10usize)
        .filter(|g| {
            gear_counts[*g] >= 100
                && gear_max[*g] >= 0.995 * ceiling
                && gear_max[*g] <= 1.01 * ceiling
        })
        .count();
    let old_adopts =
        ceiling > 0.0 && gears_at_ceiling >= 3 && ceiling < 0.97 * first.engine_max_rpm;
    onset_rpms.sort_by(f32::total_cmp);
    Scan {
        ordinal: first.car_ordinal,
        reported: first.engine_max_rpm,
        frames: frames.len(),
        gear_counts,
        gear_max,
        ceiling,
        gears_at_ceiling,
        old_adopts,
        onset_rpms,
        cut_frames,
    }
}

fn pct(v: &[f32], p: f32) -> f32 {
    v[((v.len() - 1) as f32 * p) as usize]
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--sweep") {
        let dir = args.get(1).expect("usage: redline_scan --sweep <dir>");
        let mut paths: Vec<_> = std::fs::read_dir(dir)
            .expect("read_dir")
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "ftel"))
            .collect();
        paths.sort();
        for path in paths {
            let Ok(stint) = Stint::load(&path) else {
                continue;
            };
            for frames in analysis::split_stints(&stint.frames, 5.0) {
                let s = scan(frames);
                if s.frames < 3000 {
                    continue;
                }
                println!(
                    "{:<40} car {:<5} rep {:>5.0} | ceil {:>5.0} ({:>5.1}%) g@ {} old {} | cutfr {:>4} onsets {:>3}{}",
                    path.file_name().unwrap().to_string_lossy(),
                    s.ordinal,
                    s.reported,
                    s.ceiling,
                    100.0 * s.ceiling / s.reported,
                    s.gears_at_ceiling,
                    if s.old_adopts { "Y" } else { "n" },
                    s.cut_frames,
                    s.onset_rpms.len(),
                    if s.onset_rpms.is_empty() {
                        String::new()
                    } else {
                        format!(
                            " p10/p50/p90 {:.0}/{:.0}/{:.0} ({:.1}% rep)",
                            pct(&s.onset_rpms, 0.1),
                            pct(&s.onset_rpms, 0.5),
                            pct(&s.onset_rpms, 0.9),
                            100.0 * pct(&s.onset_rpms, 0.5) / s.reported
                        )
                    }
                );
            }
        }
        return;
    }

    let path = args.first().expect("usage: redline_scan <path.ftel>");
    let stint = Stint::load(path.as_ref()).expect("load");
    for (si, frames) in analysis::split_stints(&stint.frames, 5.0)
        .iter()
        .enumerate()
    {
        let s = scan(frames);
        println!(
            "segment {si}: car {} reported redline {:.0}, {} frames",
            s.ordinal, s.reported, s.frames
        );
        for g in 1..=10usize {
            if s.gear_counts[g] > 0 {
                println!(
                    "  gear {g}: {} frames, max rpm {:.0} ({:.2}% of reported)",
                    s.gear_counts[g],
                    s.gear_max[g],
                    100.0 * s.gear_max[g] / s.reported
                );
            }
        }
        println!(
            "  sustained ceiling: {:.0} ({:.2}% of reported) | gears at ceiling: {} | old detector adopts: {}",
            s.ceiling,
            100.0 * s.ceiling / s.reported,
            s.gears_at_ceiling,
            s.old_adopts
        );
        println!(
            "  no-shift full-throttle torque<=0 frames: {} | cut onsets: {}",
            s.cut_frames,
            s.onset_rpms.len()
        );
        if !s.onset_rpms.is_empty() {
            println!(
                "  onset rpm p10/p50/p90/max: {:.0} / {:.0} / {:.0} / {:.0}",
                pct(&s.onset_rpms, 0.1),
                pct(&s.onset_rpms, 0.5),
                pct(&s.onset_rpms, 0.9),
                s.onset_rpms[s.onset_rpms.len() - 1]
            );
        }
    }
}
