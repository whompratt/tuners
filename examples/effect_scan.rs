//! Dump every recording's effect vector (overall + per
//! flying lap) as TSV, for offline noise-floor and responsiveness analysis.

use std::path::Path;
use tuners::analysis::{self, effects, metrics};

fn print_row(name: &str, car: i32, lap: &str, fx: &effects::Effects) {
    let cols: Vec<String> = effects::FIELDS
        .iter()
        .map(|(k, ..)| {
            fx.iter()
                .find(|(fk, _)| fk == k)
                .map(|(_, v)| format!("{v:.4}"))
                .unwrap_or_default()
        })
        .collect();
    println!("{name}\t{car}\t{lap}\t{}", cols.join("\t"));
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let header: Vec<&str> = effects::FIELDS.iter().map(|(k, ..)| *k).collect();
    println!("file\tcar\tlap\t{}", header.join("\t"));
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
        print_row(
            &name,
            car,
            "ALL",
            &effects::vector(&metrics::stint_metrics(seg)),
        );
        for lap in analysis::split_laps(seg) {
            if lap.standing_start || lap.time_s.is_none() {
                continue;
            }
            let fx = effects::vector(&metrics::stint_metrics(lap.frames));
            print_row(&name, car, &lap.number.to_string(), &fx);
        }
    }
}
