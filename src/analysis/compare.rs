//! A/B session comparison over distance-binned profiles: which tune is faster,
//! where on the road, and what changed in behaviour — on clean bins only.

use super::profile::{best_clean_time, SessionProfile, BIN_METERS};
use std::fmt::Write;

/// Bins per reported segment (250 m at 10 m bins).
const SEGMENT_BINS: usize = 25;
/// Sessions must be the same route: shared bin counts within this fraction.
const ROUTE_LENGTH_TOLERANCE: f32 = 0.02;
/// Segment deltas smaller than this aren't worth listing (seconds).
const SEGMENT_NOISE_S: f32 = 0.05;

pub struct Comparison {
    /// Per-bin: best clean B time minus best clean A time (negative = B faster).
    pub bin_delta_s: Vec<f32>,
    pub ideal_delta_s: f32,
    pub best_lap_delta_s: f32,
}

pub fn compare(a: &SessionProfile, b: &SessionProfile) -> Result<Comparison, String> {
    let (short, long) = (
        a.shared_bins.min(b.shared_bins),
        a.shared_bins.max(b.shared_bins),
    );
    if (long - short) as f32 > long as f32 * ROUTE_LENGTH_TOLERANCE {
        return Err(format!(
            "sessions are different routes: {:.1} km vs {:.1} km driven per lap",
            a.shared_bins as f32 * BIN_METERS / 1000.0,
            b.shared_bins as f32 * BIN_METERS / 1000.0,
        ));
    }
    if a.standing_start_only != b.standing_start_only {
        return Err(
            "one session has flying laps and the other only standing starts — \
             lap times are not comparable"
                .into(),
        );
    }

    let bin_delta_s: Vec<f32> = (0..short)
        .map(|bin| {
            best_clean_time(&b.laps, &b.clean, bin) - best_clean_time(&a.laps, &a.clean, bin)
        })
        .collect();

    Ok(Comparison {
        ideal_delta_s: bin_delta_s.iter().sum(),
        best_lap_delta_s: b.best_lap_time_s - a.best_lap_time_s,
        bin_delta_s,
    })
}

pub fn render(a: &SessionProfile, b: &SessionProfile, cmp: &Comparison) -> String {
    let mut out = String::new();

    for (name, p) in [("A", a), ("B", b)] {
        writeln!(
            out,
            "{name}: {} lap(s){} | best {:.2}s | ideal {:.2}s | {} mistake bin(s) excluded",
            p.laps.len(),
            if p.standing_start_only { " (standing starts)" } else { "" },
            p.best_lap_time_s,
            p.ideal_time_s,
            p.dirty_bin_count(),
        )
        .unwrap();
    }

    let verdict = |d: f32, label: &str| {
        if d.abs() < 0.01 {
            format!("{label}: even")
        } else if d < 0.0 {
            format!("{label}: B faster by {:.2}s", -d)
        } else {
            format!("{label}: A faster by {:.2}s", d)
        }
    };
    writeln!(out, "\n{}", verdict(cmp.best_lap_delta_s, "best lap")).unwrap();
    writeln!(out, "{}", verdict(cmp.ideal_delta_s, "ideal lap (clean bins)")).unwrap();

    // Segment breakdown: where the time comes from.
    let mut segments: Vec<(usize, f32)> = cmp
        .bin_delta_s
        .chunks(SEGMENT_BINS)
        .enumerate()
        .map(|(i, chunk)| (i, chunk.iter().sum::<f32>()))
        .filter(|(_, d)| d.abs() >= SEGMENT_NOISE_S)
        .collect();
    segments.sort_by(|x, y| y.1.abs().total_cmp(&x.1.abs()));

    if !segments.is_empty() {
        writeln!(out, "\nbiggest segment differences:").unwrap();
        for (i, delta) in segments.iter().take(6) {
            let from_km = *i as f32 * SEGMENT_BINS as f32 * BIN_METERS / 1000.0;
            let to_km = from_km + SEGMENT_BINS as f32 * BIN_METERS / 1000.0;
            writeln!(
                out,
                "  {:.2}-{:.2} km: {} by {:.2}s",
                from_km,
                to_km,
                if *delta < 0.0 { "B faster" } else { "A faster" },
                delta.abs(),
            )
            .unwrap();
        }
    } else {
        writeln!(out, "\nno segment differs by more than {SEGMENT_NOISE_S}s").unwrap();
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::profile::{BinStats, LapProfile};

    fn uniform_profile(n_bins: usize, bin_time: f32) -> SessionProfile {
        let lap = LapProfile {
            lap_number: 1,
            time_s: bin_time * n_bins as f32,
            standing_start: false,
            bins: vec![
                BinStats { time_s: bin_time, speed_avg: 50.0, samples: 10, ..Default::default() };
                n_bins
            ],
        };
        SessionProfile {
            clean: vec![vec![true; n_bins]],
            shared_bins: n_bins,
            ideal_time_s: lap.time_s,
            best_lap_time_s: lap.time_s,
            standing_start_only: false,
            laps: vec![lap],
        }
    }

    #[test]
    fn self_comparison_is_even() {
        let a = uniform_profile(100, 0.2);
        let cmp = compare(&a, &a).unwrap();
        assert!(cmp.ideal_delta_s.abs() < 1e-4);
        assert!(cmp.best_lap_delta_s.abs() < 1e-4);
    }

    #[test]
    fn faster_segment_is_attributed() {
        let a = uniform_profile(100, 0.2);
        let mut b = uniform_profile(100, 0.2);
        // B gains 0.05s in each of bins 50..55 (the 0.50-0.55km stretch)
        for bin in 50..55 {
            b.laps[0].bins[bin].time_s = 0.15;
        }
        b.ideal_time_s -= 0.25;
        b.best_lap_time_s -= 0.25;
        let cmp = compare(&a, &b).unwrap();
        assert!((cmp.ideal_delta_s + 0.25).abs() < 1e-4);
        let rendered = render(&a, &b, &cmp);
        assert!(rendered.contains("B faster by 0.25s"), "{rendered}");
        assert!(rendered.contains("0.50-0.75 km"), "{rendered}");
    }

    #[test]
    fn different_routes_rejected() {
        let a = uniform_profile(100, 0.2);
        let b = uniform_profile(200, 0.2);
        assert!(compare(&a, &b).is_err());
    }
}
