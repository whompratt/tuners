//! Lever-signature scan (understeer-diagnosis phase 3): prints the
//! production per-stint lever channels — wheel-speed splits (diff lock),
//! damper phase occupancy (rebound), roll gradient + jounce (ARB/springs) —
//! for calibration sweeps over recordings and sender bundles.
//!
//!   cargo run --release --example lever_scan -- sessions/*.ftel library/*/*.tar.zst

use std::path::Path;
use tuners::analysis::{self, TimedFrame, metrics};
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

fn opt(v: Option<f32>, prec: usize) -> String {
    v.map(|v| format!("{v:+.prec$}")).unwrap_or_default()
}

fn main() {
    println!(
        "file\tcar\tdrv\tsurface\trear_off\tfront_off\tconv_off\trear_on\tfront_on\tconv_on\t\
         ext_share_f\text_share_r\tvratio_f\tvratio_r\trollg_f\trollg_r\tjounce_mm"
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
        let name = path.file_stem().unwrap().to_string_lossy();
        let name = name.strip_suffix(".tar").unwrap_or(&name).to_string();
        let (dd, dp, ru) = (&m.diff_drag, &m.damper_phase, &m.roll_use);
        println!(
            "{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{:.1}",
            name,
            m.car_ordinal,
            m.drivetrain_type,
            if m.surface_loose { "dirt" } else { "tarmac" },
            opt(dd.rear_off, 3),
            opt(dd.front_off, 3),
            opt(dd.conv_off(), 2),
            opt(dd.rear_on, 3),
            opt(dd.front_on, 3),
            opt(dd.conv_on(), 2),
            opt(dp.ext_share_front, 3),
            opt(dp.ext_share_rear, 3),
            opt(dp.vratio_front, 2),
            opt(dp.vratio_rear, 2),
            opt(ru.grad_front, 2),
            opt(ru.grad_rear, 2),
            ru.jounce_mm,
        );
    }
}
