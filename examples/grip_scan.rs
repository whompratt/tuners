//! Grip-curve calibration scan: runs the production analysis::grip fit
//! (pooled per car+surface, speed-banded with agreement-collapse) over a
//! set of recordings and prints per-car curve summaries plus per-stint
//! PUSH/SLIDE occupancy. The curve summary lines (pooled + per-band onsets)
//! are the calibration data for the band-agreement tolerance; the TSV is
//! the threshold-calibration corpus.
//!
//! Accepts .ftel recordings and .tar.zst sender bundles.
//!
//!   cargo run --release --example grip_scan -- sessions/*.ftel library/*/*.tar.zst
//!   --curve additionally dumps each car's raw per-bin means.

use std::collections::BTreeMap;
use std::path::Path;
use tuners::analysis::{self, TimedFrame, grip, metrics};
use tuners::telemetry::{packet, stint::StintReader};

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
    samples: Vec<grip::GripSample>,
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
        let name = path.file_stem().unwrap().to_string_lossy();
        let name = name.strip_suffix(".tar").unwrap_or(&name).to_string();
        rows.push(StintRow {
            name,
            car: seg.first().map(|f| f.frame.car_ordinal).unwrap_or(0),
            loose: m.surface_loose,
            idx: m.understeer_index,
            margin: m.margin_ratio(),
            samples: grip::cornering_samples(seg),
        });
    }

    // Pass 2: pooled per-car curves via the production banded fit. Surface
    // splits the pool: the curve differs on dirt.
    let mut car_curves: BTreeMap<(i32, bool), grip::CarCurves> = BTreeMap::new();
    for ((car, loose), group) in &rows.iter().fold(
        BTreeMap::<(i32, bool), Vec<&StintRow>>::new(),
        |mut acc, r| {
            acc.entry((r.car, r.loose)).or_default().push(r);
            acc
        },
    ) {
        let pooled: Vec<grip::GripSample> = group
            .iter()
            .flat_map(|r| r.samples.iter().copied())
            .collect();
        let Some(curves) = grip::fit_curves(&pooled) else {
            eprintln!(
                "car {car} ({}): no stable pooled curve ({} samples)",
                if *loose { "dirt" } else { "tarmac" },
                pooled.len()
            );
            continue;
        };
        let pair = |p: &grip::AxlePair| {
            format!(
                "F {:.2} R {:.2} pkG {:.1}",
                p.front.onset, p.rear.onset, p.front.peak_g
            )
        };
        let band = |p: &Option<grip::AxlePair>| p.as_ref().map(&pair).unwrap_or_else(|| "-".into());
        let ratio = match (&curves.low, &curves.high) {
            (Some(l), Some(h)) => format!("{:.2}", h.front.peak_g / l.front.peak_g),
            _ => "-".into(),
        };
        eprintln!(
            "car {car} ({}): pooled {} | low {} | high {} | pk-ratio {ratio} | banded {}",
            if *loose { "dirt" } else { "tarmac" },
            pair(&curves.pooled),
            band(&curves.low),
            band(&curves.high),
            curves.banded,
        );
        if dump_curve {
            let bins = grip::bin_means(pooled.iter().map(|s| (s.front, s.lat_g)));
            for (i, (n, g)) in bins.iter().enumerate() {
                if *n > 0 {
                    eprintln!("  {:.2}\t{n}\t{g:.2}", grip::bin_alpha(i));
                }
            }
        }
        car_curves.insert((*car, *loose), curves);
    }

    // Pass 3: per-stint occupancy against the car curve.
    println!(
        "file\tcar\tsurface\tncorner\tpush%\tslide%\tcover%\trear_use@push\tbanded\tidx\tmargin"
    );
    for r in &rows {
        let Some(curves) = car_curves.get(&(r.car, r.loose)) else {
            continue;
        };
        let Some(occ) = grip::occupancy(&r.samples, curves, grip::CurveSource::CarPool) else {
            eprintln!(
                "{}: occupancy withheld ({} cornering samples)",
                r.name,
                r.samples.len()
            );
            continue;
        };
        println!(
            "{}\t{}\t{}\t{}\t{:.1}\t{:.1}\t{:.0}\t{}\t{}\t{}\t{}",
            r.name,
            r.car,
            if r.loose { "dirt" } else { "tarmac" },
            r.samples.len(),
            occ.push_frac * 100.0,
            occ.slide_frac * 100.0,
            occ.coverage * 100.0,
            occ.rear_use_at_push
                .map(|u| format!("{u:.2}"))
                .unwrap_or_default(),
            occ.banded,
            r.idx.map(|i| format!("{i:+.3}")).unwrap_or_default(),
            r.margin.map(|m| format!("{m:.2}")).unwrap_or_default(),
        );
    }
}
