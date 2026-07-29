//! Dump per-stint damping-channel summaries as TSV (rumble, speed, travel,
//! reversals, topped/bottomed shares), for roughness-normalization analysis.

use std::path::Path;
use tuners::analysis::{self, metrics};

fn main() {
    println!(
        "file\tcar\tsurface\trumble\tavg_mps\trev_f\trev_r\ttop_f%\ttop_r%\t\
         bot_f%\tbot_r%\ttravel_f\ttravel_r\tkerb_ev\tkerb%\tkerb_raw"
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
        let s = &m.suspension;
        let avg2 = |a: f32, b: f32| (a + b) / 2.0;
        let speed = seg.iter().map(|f| f.frame.speed).sum::<f32>() / seg.len().max(1) as f32;
        // Raw flag count over the WHOLE recording (not just the profiled
        // segment): a probe for whether FH6 populates the channel at all.
        let raw_strip_frames = stint
            .frames
            .iter()
            .filter(|f| f.frame.wheel_on_rumble_strip.to_array().iter().any(|s| *s))
            .count();
        println!(
            "{name}\t{car}\t{}\t{:.3}\t{:.1}\t{:.1}\t{:.1}\t{:.1}\t{:.1}\t{:.1}\t{:.1}\t{:.2}\t{:.2}\t{}\t{:.1}\t{}",
            if m.surface_loose { "dirt" } else { "tarmac" },
            m.surface_rumble_avg,
            speed,
            avg2(s.fl.reversals_per_sec, s.fr.reversals_per_sec),
            avg2(s.rl.reversals_per_sec, s.rr.reversals_per_sec),
            avg2(s.fl.topped_frac, s.fr.topped_frac) * 100.0,
            avg2(s.rl.topped_frac, s.rr.topped_frac) * 100.0,
            avg2(s.fl.bottomed_frac, s.fr.bottomed_frac) * 100.0,
            avg2(s.rl.bottomed_frac, s.rr.bottomed_frac) * 100.0,
            avg2(s.fl.avg, s.fr.avg),
            avg2(s.rl.avg, s.rr.avg),
            m.kerbs.events,
            m.kerbs.time_frac * 100.0,
            raw_strip_frames,
        );
    }
}
