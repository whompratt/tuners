//! A/B session comparison over distance-binned profiles: which tune is faster,
//! where on the road, and what changed in behaviour (clean bins only).

use super::profile::{BIN_METERS, StintProfile};
use crate::util::format_lap_time;
use std::fmt::Write;

/// Bins per reported segment (250 m at 10 m bins).
const SEGMENT_BINS: usize = 25;
/// Sessions must be the same route: shared bin counts within this fraction.
const ROUTE_LENGTH_TOLERANCE: f32 = 0.02;
/// Route lengths can coincide across different tracks (DistanceTraveled is route
/// units, and two routes measured ~5.95km while one drove in 50s and the other in
/// 92s), so lap times are the tiebreaker. No tune change moves lap time this much.
const LAP_TIME_RATIO_LIMIT: f32 = 1.30;
/// Segment deltas smaller than this aren't worth listing (seconds).
const SEGMENT_NOISE_S: f32 = 0.05;

#[derive(Debug)]
pub struct Comparison {
    /// Per-bin: best clean B time minus best clean A time (negative = B faster).
    pub bin_delta_s: Vec<f32>,
    pub ideal_delta_s: f32,
    pub best_lap_delta_s: f32,
    /// Median flying-lap delta: the typical lap, robust to one ruined lap.
    pub median_lap_delta_s: f32,
    /// THE verdict currency: median of (ideal, best, median-lap) deltas, a
    /// 2-of-3 vote. The spliced ideal rewards spatial lap scatter it can
    /// harvest (a driving-style term, measured up to ~0.3s), so it no longer
    /// decides alone; it stays the tie-breaker when best and median-lap
    /// disagree. The per-bin spatial decomposition remains composite-based,
    /// so phase splits explain the ideal component, not the whole vote.
    pub verdict_delta_s: f32,
    /// Sessions were driven in different cars: a car comparison, not a tune A/B.
    pub car_mismatch: bool,
}

pub fn compare(a: &StintProfile, b: &StintProfile) -> Result<Comparison, String> {
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
            "one session has flying laps and the other only standing starts; \
             lap times are not comparable"
                .into(),
        );
    }
    let (fast, slow) = (
        a.best_lap_time_s.min(b.best_lap_time_s),
        a.best_lap_time_s.max(b.best_lap_time_s),
    );
    if fast > 0.0 && slow / fast > LAP_TIME_RATIO_LIMIT {
        return Err(format!(
            "sessions look like different routes despite similar route length: best \
             laps {} vs {} are too far apart for a tune change",
            format_lap_time(a.best_lap_time_s),
            format_lap_time(b.best_lap_time_s),
        ));
    }

    let bin_delta_s: Vec<f32> = (0..short)
        .map(|bin| b.composite.bins[bin].time_s - a.composite.bins[bin].time_s)
        .collect();

    let ideal_delta_s: f32 = bin_delta_s.iter().sum();
    let best_lap_delta_s = b.best_lap_time_s - a.best_lap_time_s;
    let median_lap_delta_s = b.median_lap_time_s() - a.median_lap_time_s();
    Ok(Comparison {
        ideal_delta_s,
        best_lap_delta_s,
        median_lap_delta_s,
        verdict_delta_s: median3(ideal_delta_s, best_lap_delta_s, median_lap_delta_s),
        car_mismatch: a.car_ordinal != b.car_ordinal,
        bin_delta_s,
    })
}

/// Middle of three values: the 2-of-3 vote.
fn median3(a: f32, b: f32, c: f32) -> f32 {
    a.max(b).min(a.max(c)).min(b.max(c))
}

pub fn render(a: &StintProfile, b: &StintProfile, cmp: &Comparison) -> String {
    let mut out = String::new();

    for (name, p) in [("A", a), ("B", b)] {
        // Point-to-point runs are all the game's lap 0: name them by run index
        // so the source list doesn't read "1,1,1,1".
        let lap_names: Vec<String> = p
            .composite
            .source_laps()
            .into_iter()
            .map(|i| {
                if p.point_to_point {
                    (i + 1).to_string()
                } else {
                    (p.laps[i].lap_number + 1).to_string()
                }
            })
            .collect();
        writeln!(
            out,
            "{name}: {} lap(s){} | best {} | ideal {} ({} span(s) from lap(s) {})",
            p.laps.len(),
            if p.point_to_point {
                " (point-to-point runs)"
            } else if p.standing_start_only {
                " (standing starts)"
            } else {
                ""
            },
            format_lap_time(p.best_lap_time_s),
            format_lap_time(p.composite.time_s),
            p.composite.span_count(),
            lap_names.join(","),
        )
        .unwrap();
    }

    if a.laps.len() != b.laps.len() {
        writeln!(
            out,
            "note: unequal lap counts. The session with more laps gives its ideal \
             more material, biasing the ideal comparison in its favor",
        )
        .unwrap();
    }
    if cmp.car_mismatch {
        writeln!(
            out,
            "note: different cars (ordinal {} vs {}): this compares cars, not tunes",
            a.car_ordinal, b.car_ordinal,
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
    writeln!(out, "{}", verdict(cmp.median_lap_delta_s, "median lap")).unwrap();
    writeln!(out, "{}", verdict(cmp.ideal_delta_s, "ideal lap (spliced)")).unwrap();
    writeln!(
        out,
        "{}",
        verdict(cmp.verdict_delta_s, "verdict (2-of-3 vote)")
    )
    .unwrap();
    if cmp.verdict_delta_s.signum() != cmp.ideal_delta_s.signum()
        && cmp.verdict_delta_s.abs() >= 0.01
    {
        writeln!(
            out,
            "note: best and median lap agree against the spliced ideal; the ideal \
             rewards laps that are fast in different places, so the vote overrules it",
        )
        .unwrap();
    }

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
    use crate::analysis::profile::{BinStats, LapProfile, build_composite};

    fn uniform_profile(n_bins: usize, bin_time: f32) -> StintProfile {
        let lap = LapProfile {
            lap_number: 1,
            time_s: bin_time * n_bins as f32,
            standing_start: false,
            point_to_point: false,
            bins: vec![
                BinStats {
                    time_s: bin_time,
                    speed_avg: 50.0,
                    samples: 10,
                    ..Default::default()
                };
                n_bins
            ],
        };
        let laps = vec![lap];
        StintProfile {
            composite: build_composite(&laps, n_bins),
            shared_bins: n_bins,
            best_lap_time_s: laps[0].time_s,
            standing_start_only: false,
            point_to_point: false,
            car_ordinal: 42,
            laps,
        }
    }

    /// Two routes can measure the same length in route units; wildly different
    /// lap times expose them (found comparing a 92s tarmac lap to a 50s dirt lap
    /// that both measured ~5.95km).
    #[test]
    fn same_length_different_route_rejected_by_lap_time() {
        let a = uniform_profile(100, 0.2); // 20s laps
        let b = uniform_profile(100, 0.11); // 11s laps, same bin count
        let err = compare(&a, &b).unwrap_err();
        assert!(err.contains("different routes"), "{err}");
    }

    #[test]
    fn different_cars_flagged_not_rejected() {
        let a = uniform_profile(100, 0.2);
        let mut b = uniform_profile(100, 0.2);
        b.car_ordinal = 7;
        let cmp = compare(&a, &b).unwrap();
        assert!(cmp.car_mismatch);
        assert!(render(&a, &b, &cmp).contains("compares cars, not tunes"));
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
        b.laps[0].time_s -= 0.25;
        b.best_lap_time_s -= 0.25;
        b.composite = build_composite(&b.laps, 100);
        let cmp = compare(&a, &b).unwrap();
        assert!((cmp.ideal_delta_s + 0.25).abs() < 1e-4);
        let rendered = render(&a, &b, &cmp);
        assert!(rendered.contains("B faster by 0.25s"), "{rendered}");
        assert!(rendered.contains("0.50-0.75 km"), "{rendered}");
    }

    fn profile_from_laps(bin_times: &[Vec<f32>]) -> StintProfile {
        let laps: Vec<LapProfile> = bin_times
            .iter()
            .enumerate()
            .map(|(i, times)| LapProfile {
                lap_number: i as u16 + 1,
                time_s: times.iter().sum(),
                standing_start: false,
                point_to_point: false,
                bins: times
                    .iter()
                    .map(|t| BinStats {
                        time_s: *t,
                        speed_avg: 50.0,
                        samples: 10,
                        ..Default::default()
                    })
                    .collect(),
            })
            .collect();
        let n = laps[0].bins.len();
        StintProfile {
            composite: build_composite(&laps, n),
            shared_bins: n,
            best_lap_time_s: laps.iter().map(|l| l.time_s).fold(f32::INFINITY, f32::min),
            standing_start_only: false,
            point_to_point: false,
            car_ordinal: 42,
            laps,
        }
    }

    /// The verdict is a 2-of-3 vote: a stint whose laps are fast in DIFFERENT
    /// places builds a low composite the driver never demonstrated as one
    /// lap; when best and median lap both disagree with the ideal, the vote
    /// sides with them and the ideal is overruled (the measured Porsche
    /// front-arb case).
    #[test]
    fn verdict_vote_overrules_scatter_harvesting_ideal() {
        // A: two 19.5s laps, each fast on a different stretch -> composite
        // harvests both (19.0s). B: two identical 19.4s laps, nothing to
        // harvest.
        let mut lap1 = vec![0.2f32; 100];
        let mut lap2 = vec![0.2f32; 100];
        lap1[10..20].fill(0.15);
        lap2[60..70].fill(0.15);
        let a = profile_from_laps(&[lap1, lap2]);
        let b = profile_from_laps(&[vec![0.194f32; 100], vec![0.194f32; 100]]);
        assert!((a.composite.time_s - 19.0).abs() < 0.01, "harvested both");

        let cmp = compare(&a, &b).unwrap();
        assert!(cmp.ideal_delta_s > 0.3, "ideal says B worse");
        assert!(cmp.best_lap_delta_s < 0.0, "best lap says B faster");
        assert!(cmp.median_lap_delta_s < 0.0, "median lap says B faster");
        assert!(
            (cmp.verdict_delta_s - cmp.best_lap_delta_s).abs() < 1e-4,
            "vote sides with the majority"
        );
        let rendered = render(&a, &b, &cmp);
        assert!(rendered.contains("the vote overrules it"), "{rendered}");
    }

    #[test]
    fn median_lap_time_averages_even_counts() {
        let p = profile_from_laps(&[
            vec![0.2f32; 100], // 20.0
            vec![0.21f32; 100],
            vec![0.25f32; 100], // ruined lap
        ]);
        assert!((p.median_lap_time_s() - 21.0).abs() < 0.01);
        let p2 = profile_from_laps(&[vec![0.2f32; 100], vec![0.21f32; 100]]);
        assert!((p2.median_lap_time_s() - 20.5).abs() < 0.01);
    }

    #[test]
    fn different_routes_rejected() {
        let a = uniform_profile(100, 0.2);
        let b = uniform_profile(200, 0.2);
        assert!(compare(&a, &b).is_err());
    }
}
