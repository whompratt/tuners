//! Front wheelspin symmetry scan: during on-throttle cornering, how much
//! does the inside front spin vs the outside front? The front twin of
//! diff_scan (same gates and thresholds, so numbers are comparable to the
//! rear calibration). Drivetrain column included: only FWD rows are
//! meaningful for front diff accel direction — AWD fronts are
//! center-diff-confounded, RWD fronts free-roll. TSV per recording.

use std::path::Path;
use tuners::analysis;

const CORNERING_LAT_ACCEL: f32 = 4.0;
const PEDAL_ON: u8 = 128;
const SPIN: f32 = 1.0;
const GRIPPING: f32 = 0.7;

fn main() {
    println!("file\tcar\tdt\tframes\tin_slip\tout_slip\tasym\tinside%\tboth%\toutside%");
    for path in std::env::args().skip(1) {
        let stint = match analysis::Stint::load(Path::new(&path)) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{path}: {e}");
                continue;
            }
        };
        let segments = analysis::driving_segments(&stint.frames, 5.0);
        let Some(seg) = segments.iter().max_by_key(|s| s.len()) else {
            continue;
        };
        let car = seg.first().map(|f| f.frame.car_ordinal).unwrap_or(0);
        let dt = seg.first().map(|f| f.frame.drivetrain_type).unwrap_or(-1);
        let name = Path::new(&path).file_stem().unwrap().to_string_lossy();

        let (mut n, mut in_sum, mut out_sum) = (0u32, 0f64, 0f64);
        let (mut inside_only, mut both, mut outside_only) = (0u32, 0u32, 0u32);
        for f in seg {
            let f = &f.frame;
            if f.accel < PEDAL_ON || f.acceleration[0].abs() <= CORNERING_LAT_ACCEL {
                continue;
            }
            // The loaded (more compressed) front is the outside wheel.
            let t = f.norm_suspension_travel;
            let (inside, outside) = if t.fl < t.fr {
                (f.tire_slip_ratio.fl.abs(), f.tire_slip_ratio.fr.abs())
            } else {
                (f.tire_slip_ratio.fr.abs(), f.tire_slip_ratio.fl.abs())
            };
            n += 1;
            in_sum += inside as f64;
            out_sum += outside as f64;
            match (inside > SPIN, outside > SPIN) {
                (true, true) => both += 1,
                (true, false) if outside < GRIPPING => inside_only += 1,
                (false, true) if inside < GRIPPING => outside_only += 1,
                _ => {}
            }
        }
        if n == 0 {
            continue;
        }
        let pct = |c: u32| 100.0 * c as f64 / n as f64;
        println!(
            "{name}\t{car}\t{}\t{n}\t{:.3}\t{:.3}\t{:+.3}\t{:.1}\t{:.1}\t{:.1}",
            tuners::telemetry::packet::drivetrain_name(dt),
            in_sum / n as f64,
            out_sum / n as f64,
            (in_sum - out_sum) / n as f64,
            pct(inside_only),
            pct(both),
            pct(outside_only),
        );
    }
}
