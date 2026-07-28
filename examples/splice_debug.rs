//! Debug dump of the spliced ideal for one stint: per-bin composite source,
//! corroboration, and each lap's speed, to verify the outlier-exit splice.

use std::path::Path;
use tuners::analysis::{self, profile};

fn main() {
    let path = std::env::args().nth(1).expect("usage: splice_debug <ftel>");
    let stint = analysis::Stint::load(Path::new(&path)).expect("load");
    let p = profile::stint_profile(&stint.frames).expect("profile");
    let c = p.corroboration();

    eprintln!(
        "laps: {:?}",
        p.laps
            .iter()
            .map(|l| (l.lap_number, l.time_s))
            .collect::<Vec<_>>()
    );
    eprintln!(
        "composite {:.3}s vs best {:.3}s, spans {}, sources {:?}, score {:.3}",
        p.composite.time_s,
        p.best_lap_time_s,
        p.composite.span_count(),
        p.composite.source_laps(),
        c.score
    );

    // Per-lap bin counts and time accounting: authoritative lap time vs bin sums.
    for l in &p.laps {
        let full: f32 = l.bins.iter().map(|b| b.time_s).sum();
        let shared: f32 = l.bins[..p.shared_bins].iter().map(|b| b.time_s).sum();
        eprintln!(
            "lap{}: {} bins, time_s {:.3}, bin sum {:.3}, shared-bin sum {:.3}",
            l.lap_number,
            l.bins.len(),
            l.time_s,
            full,
            shared
        );
    }

    // Real per-span times: composite vs every lap over each same-source span.
    let mut s = 0;
    while s < p.shared_bins {
        let mut e = s;
        while e < p.shared_bins && p.composite.source[e] == p.composite.source[s] {
            e += 1;
        }
        let comp: f32 = p.composite.bins[s..e].iter().map(|b| b.time_s).sum();
        let per: Vec<String> = p
            .laps
            .iter()
            .map(|l| format!("{:.2}", l.bins[s..e].iter().map(|b| b.time_s).sum::<f32>()))
            .collect();
        eprintln!(
            "span {:>3}-{:<3} src lap{} comp {:.2} | {}",
            s,
            e,
            p.laps[p.composite.source[s]].lap_number,
            comp,
            per.join(" ")
        );
        s = e;
    }

    // Header: bin, km, source lap, corroborated, composite speed, then each lap's speed.
    let lap_hdr: Vec<String> = p
        .laps
        .iter()
        .map(|l| format!("lap{}", l.lap_number))
        .collect();
    println!("bin\tkm\tsrc\tok\tcomp\t{}", lap_hdr.join("\t"));
    for b in 0..p.shared_bins {
        let speeds: Vec<String> = p
            .laps
            .iter()
            .map(|l| format!("{:.1}", l.bins[b].speed_avg))
            .collect();
        println!(
            "{}\t{:.2}\t{}\t{}\t{:.1}\t{}",
            b,
            b as f32 * profile::BIN_METERS / 1000.0,
            p.laps[p.composite.source[b]].lap_number,
            c.corroborated[b] as u8,
            p.composite.bins[b].speed_avg,
            speeds.join("\t")
        );
    }
}
