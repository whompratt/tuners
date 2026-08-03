//! Race-vs-lap clock survey: the route-kind detector's calibration harness.
//!
//! VERDICT (2026-08-03, library-wide): both clocks tick together from the
//! countdown's GO. On circuits the lap clock RESETS to ~0 at the start-line
//! crossing, so the race clock leads it by the rollout time for the rest of
//! lap 0 (measured 1.86-5.74s over 66 race starts). On point-to-point routes
//! it never resets: the clocks stay locked (|offset| < 0.01s, three drivers).
//! The end-of-lap-0 offset separates the kinds with total margin; production
//! uses 1.0s (`analysis::POINT_TO_POINT_CLOCK_OFFSET_S`). Rewinds only ever
//! inflate the offset, so a rewound point-to-point run can read as a circuit
//! out lap but never the reverse. Details in telemetry.md.
//!
//!   cargo run --release --example clock_scan -- <recordings...>

use std::path::Path;
use tuners::analysis::{self, split_laps};

fn main() {
    println!(
        "file\tseg\tlap\tdur_s\tfirst_race\tfirst_lap\tend_race\tend_lap\tend_off\tlap_ticked"
    );
    for path in std::env::args().skip(1) {
        let stint = match analysis::Stint::load(Path::new(&path)) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{path}: {e}");
                continue;
            }
        };
        let name = Path::new(&path)
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let segments = analysis::driving_segments(&stint.frames, 5.0);
        for (si, seg) in segments.iter().enumerate() {
            for lap in split_laps(seg) {
                // Only race starts are informative: lap 0 from its beginning.
                if lap.number != 0 {
                    continue;
                }
                let f0 = lap.frames.first().unwrap().frame;
                let fn_ = lap.frames.last().unwrap().frame;
                if f0.current_race_time > 5.0 {
                    continue; // mid-race capture, not a race start
                }
                let dur = fn_.current_race_time - f0.current_race_time;
                let end_off = fn_.current_race_time - fn_.current_lap;
                println!(
                    "{name}\t{si}\t{}\t{dur:.1}\t{:.2}\t{:.2}\t{:.2}\t{:.2}\t{end_off:.2}\t{}",
                    lap.number,
                    f0.current_race_time,
                    f0.current_lap,
                    fn_.current_race_time,
                    fn_.current_lap,
                    fn_.current_lap > 0.5,
                );
            }
        }
    }
}
