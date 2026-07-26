//! Scratch scan: per-stint and per-lap understeer index across recordings,
//! to measure the index's real range and lap-to-lap noise floor.

use std::path::Path;
use tuners::analysis::{self, metrics};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    println!("file\tcar\tlap\ttime_s\tbalance");
    for path in &args {
        let stint = match analysis::Stint::load(Path::new(path)) {
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
        let name = Path::new(path).file_stem().unwrap().to_string_lossy();
        let overall = metrics::stint_metrics(seg);
        if let Some(b) = overall.understeer_index {
            println!("{name}\t{car}\tALL\t{:.1}\t{b:+.3}", overall.duration_s);
        }
        for lap in analysis::split_laps(seg) {
            if lap.standing_start {
                continue;
            }
            let (Some(t), m) = (lap.time_s, metrics::stint_metrics(lap.frames)) else {
                continue;
            };
            if let Some(b) = m.understeer_index {
                println!("{name}\t{car}\t{}\t{t:.3}\t{b:+.3}", lap.number);
            }
        }
    }
}
