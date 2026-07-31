//! Front grip-curve scan (plan 015 phase 1): fit slip-angle vs lateral-G
//! curves per CAR (pooled across its stints: the curve is a car property and
//! per-stint fits proved unstable), locate each axle's grip plateau, then
//! measure each stint's occupancy: PUSH (front saturated, rear with spare:
//! the understeer signal) vs SLIDE (both saturated: drifts/oversteer). TSV
//! per stint; --curve dumps each car's fitted bins.
//!
//! Accepts .ftel recordings and .tar.zst sender bundles.
//!
//!   cargo run --release --example grip_scan -- sessions/*.ftel library/*/*.tar.zst

use std::collections::BTreeMap;
use std::path::Path;
use tuners::analysis::{self, TimedFrame, metrics};
use tuners::telemetry::{packet, stint::StintReader};

/// Cornering gate, matching metrics::CORNERING_LAT_ACCEL (m/s^2).
const CORNERING_LAT_ACCEL: f32 = 4.0;
/// Slip-angle bin width for the curve fit (game-normalized units, ~1.0 =
/// nominal limit).
const BIN_W: f32 = 0.05;
/// Curve range cap: slip angles beyond this are lumped into the last bin
/// (deep slides carry no curve information).
const ALPHA_MAX: f32 = 2.0;
/// A bin participates in peak-finding only with at least this many samples.
const BIN_MIN: usize = 200;
/// Minimum pooled cornering samples for a fit worth reporting.
const FIT_MIN: usize = 5000;
/// Saturation onset: first supported bin reaching this share of peak mean G
/// (the plateau start; the argmax alone overshoots into the plateau tail).
const PLATEAU_FRAC: f32 = 0.97;

struct Curve {
    bins: Vec<(usize, f32)>,
    /// Plateau onset: slip angle where mean G first reaches PLATEAU_FRAC of
    /// the peak. Time beyond this adds slip without adding grip.
    alpha_sat: f32,
    g_peak: f32,
}

fn fit(samples: impl Iterator<Item = (f32, f32)>) -> Option<Curve> {
    let n_bins = (ALPHA_MAX / BIN_W) as usize;
    let mut sums = vec![(0usize, 0.0f64); n_bins];
    let mut n_total = 0usize;
    for (alpha, g) in samples {
        let i = ((alpha / BIN_W) as usize).min(n_bins - 1);
        sums[i].0 += 1;
        sums[i].1 += g as f64;
        n_total += 1;
    }
    if n_total < FIT_MIN {
        return None;
    }
    let bins: Vec<(usize, f32)> = sums
        .iter()
        .map(|&(n, s)| (n, if n > 0 { (s / n as f64) as f32 } else { 0.0 }))
        .collect();
    // Sample-weighted 3-bin smoothing before plateau detection: a single
    // noisy bin must not set the peak.
    let smooth: Vec<(usize, f32)> = (0..bins.len())
        .map(|i| {
            let lo = i.saturating_sub(1);
            let hi = (i + 1).min(bins.len() - 1);
            let (mut n, mut s) = (0usize, 0.0f64);
            for &(bn, bg) in &bins[lo..=hi] {
                n += bn;
                s += bn as f64 * bg as f64;
            }
            (n, if n > 0 { (s / n as f64) as f32 } else { 0.0 })
        })
        .collect();
    let g_peak = smooth
        .iter()
        .filter(|(n, _)| *n >= BIN_MIN)
        .map(|(_, g)| *g)
        .max_by(f32::total_cmp)?;
    let onset_i = smooth
        .iter()
        .position(|(n, g)| *n >= BIN_MIN && *g >= PLATEAU_FRAC * g_peak)?;
    Some(Curve {
        bins,
        alpha_sat: (onset_i as f32 + 0.5) * BIN_W,
        g_peak,
    })
}

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

struct StintRow {
    name: String,
    car: i32,
    loose: bool,
    idx: Option<f32>,
    margin: Option<f32>,
    /// (front axle |alpha|, rear axle |alpha|, |lat G|) per cornering frame.
    samples: Vec<(f32, f32, f32)>,
}

fn main() {
    let mut dump_curve = false;
    let mut paths = Vec::new();
    for a in std::env::args().skip(1) {
        if a == "--curve" {
            dump_curve = true;
        } else {
            paths.push(a);
        }
    }

    // Pass 1: load every stint's cornering samples.
    let mut rows: Vec<StintRow> = Vec::new();
    for p in &paths {
        let path = Path::new(p);
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
        let mut samples = Vec::new();
        for tf in seg.iter() {
            let f = &tf.frame;
            let lat = f.acceleration[0];
            if lat.abs() <= CORNERING_LAT_ACCEL {
                continue;
            }
            let fa = (f.tire_slip_angle.fl.abs() + f.tire_slip_angle.fr.abs()) / 2.0;
            let ra = (f.tire_slip_angle.rl.abs() + f.tire_slip_angle.rr.abs()) / 2.0;
            samples.push((fa, ra, lat.abs()));
        }
        let name = path.file_stem().unwrap().to_string_lossy();
        let name = name.strip_suffix(".tar").unwrap_or(&name).to_string();
        rows.push(StintRow {
            name,
            car: seg.first().map(|f| f.frame.car_ordinal).unwrap_or(0),
            loose: m.surface_loose,
            idx: m.understeer_index,
            margin: m.margin_ratio(),
            samples,
        });
    }

    // Pass 2: pooled per-car curves. Surface splits the pool: the curve
    // differs on dirt.
    let mut car_curves: BTreeMap<(i32, bool), (Option<Curve>, Option<Curve>)> = BTreeMap::new();
    for ((car, loose), group) in &rows.iter().fold(
        BTreeMap::<(i32, bool), Vec<&StintRow>>::new(),
        |mut acc, r| {
            acc.entry((r.car, r.loose)).or_default().push(r);
            acc
        },
    ) {
        let front = fit(group
            .iter()
            .flat_map(|r| r.samples.iter().map(|s| (s.0, s.2))));
        let rear = fit(group
            .iter()
            .flat_map(|r| r.samples.iter().map(|s| (s.1, s.2))));
        if dump_curve && let Some(c) = &front {
            eprintln!(
                "car {car} ({}) front: onset {:.2}, peak G {:.1}",
                if *loose { "dirt" } else { "tarmac" },
                c.alpha_sat,
                c.g_peak
            );
            for (i, (n, g)) in c.bins.iter().enumerate() {
                if *n > 0 {
                    eprintln!("  {:.2}\t{n}\t{g:.2}", (i as f32 + 0.5) * BIN_W);
                }
            }
        }
        car_curves.insert((*car, *loose), (front, rear));
    }

    // Pass 3: per-stint occupancy against the car curve.
    println!(
        "file\tcar\tsurface\tncorner\tfront_sat\trear_sat\tpush%\tslide%\t\
         at_limit%\trear_use@push\tidx\tmargin"
    );
    for r in &rows {
        let Some((Some(front), Some(rear))) = car_curves.get(&(r.car, r.loose)) else {
            eprintln!("{}: no stable pooled curve for car {}", r.name, r.car);
            continue;
        };
        let mut push = 0usize;
        let mut slide = 0usize;
        let mut rear_use_sum = 0.0f32;
        for &(fa, ra, _) in &r.samples {
            if fa < front.alpha_sat {
                continue;
            }
            if ra < rear.alpha_sat {
                push += 1;
                rear_use_sum += ra / rear.alpha_sat;
            } else {
                slide += 1;
            }
        }
        let n = r.samples.len().max(1);
        println!(
            "{}\t{}\t{}\t{}\t{:.2}\t{:.2}\t{:.1}\t{:.1}\t{:.1}\t{:.2}\t{}\t{}",
            r.name,
            r.car,
            if r.loose { "dirt" } else { "tarmac" },
            r.samples.len(),
            front.alpha_sat,
            rear.alpha_sat,
            push as f32 / n as f32 * 100.0,
            slide as f32 / n as f32 * 100.0,
            (push + slide) as f32 / n as f32 * 100.0,
            rear_use_sum / push.max(1) as f32,
            r.idx.map(|i| format!("{i:+.3}")).unwrap_or_default(),
            r.margin.map(|m| format!("{m:.2}")).unwrap_or_default(),
        );
    }
}
