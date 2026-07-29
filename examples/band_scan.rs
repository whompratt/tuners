//! Dump every recording's banded balance summary as TSV (surface,
//! drivetrain, per-band indices, corner phases, transient shares), for
//! offline threshold calibration against the healthy library.

use std::path::Path;
use tuners::analysis::{self, metrics};

fn band(b: &metrics::BandBalance) -> String {
    match b.index {
        Some(i) => format!("{i:+.3}/{}", b.samples),
        None => format!("-/{}", b.samples),
    }
}

fn main() {
    println!(
        "file\tcar\tsurface\tdrive\tbal\tlow\thigh\ton\toff\tbrake\tentry\texit\t\
         flash%\tonpow%\trearfirst%\tcsteer%"
    );
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
        let m = metrics::stint_metrics(seg);
        let car = seg.first().map(|f| f.frame.car_ordinal).unwrap_or(0);
        let name = Path::new(&path).file_stem().unwrap().to_string_lossy();
        let (entry, exit) = m
            .corners
            .as_ref()
            .map(|c| (band(&c.entry), band(&c.exit)))
            .unwrap_or_else(|| ("-".into(), "-".into()));
        println!(
            "{name}\t{car}\t{}\t{:?}\t{}\t{}\t{}\t{}\t{}\t{}\t{entry}\t{exit}\t{:.1}\t{:.1}\t{:.1}\t{:.1}",
            if m.surface_loose { "dirt" } else { "tarmac" },
            m.drivetrain_type,
            m.understeer_index
                .map(|i| format!("{i:+.3}"))
                .unwrap_or_default(),
            band(&m.balance_low_speed),
            band(&m.balance_high_speed),
            band(&m.balance_on_throttle),
            band(&m.balance_off_throttle),
            band(&m.balance_on_brake),
            m.transient_oversteer.clear_frac * 100.0,
            m.transient_oversteer.on_power_frac * 100.0,
            m.transient_oversteer.rear_first_frac * 100.0,
            m.transient_oversteer.countersteer_frac * 100.0,
        );
    }
}
