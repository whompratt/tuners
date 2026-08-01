//! Per-recording splice-bonus survey: composite ideal vs best/mean lap,
//! lap-time spread, and seam discontinuities (donor switches and the speed
//! mismatch at each). TSV to stdout; one row per recording with >= 2 flying
//! laps. Investigates whether the composite's advantage over the best lap
//! scales with lap variance (a bias that would punish consistent stints).

use std::path::Path;
use tuners::analysis::{self, profile};

fn main() {
    println!(
        "file\tcar\tn_laps\tbest_s\tmean_s\tsd_s\tbin_sd_s\tideal_s\tbonus_s\tspans\tdonors\tseams\tsum_abs_dv\tmax_abs_dv\tcorr\tgraded\tgraded_adopted\tadopted_bins\tbonus_t15\tbonus_t10\tbonus_t05"
    );
    for path in std::env::args().skip(1) {
        let stint = match analysis::Stint::load(Path::new(&path)) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{path}: {e}");
                continue;
            }
        };
        let p = match profile::stint_profile(&stint.frames) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("{path}: {e}");
                continue;
            }
        };
        if p.standing_start_only || p.laps.len() < 2 {
            continue;
        }

        let n = p.laps.len() as f32;
        let mean = p.laps.iter().map(|l| l.time_s).sum::<f32>() / n;
        let sd = (p
            .laps
            .iter()
            .map(|l| (l.time_s - mean).powi(2))
            .sum::<f32>()
            / (n - 1.0))
            .sqrt();

        // Spatial lap variance: laps can be fast in DIFFERENT places while
        // having near-identical totals (time-sd blind). Summed per-bin
        // cross-lap sd of bin time = the dispersion the splicer can harvest.
        let bin_sd: f32 = (0..p.shared_bins)
            .map(|b| {
                let m = p.laps.iter().map(|l| l.bins[b].time_s).sum::<f32>() / n;
                (p.laps
                    .iter()
                    .map(|l| (l.bins[b].time_s - m).powi(2))
                    .sum::<f32>()
                    / (n - 1.0))
                    .sqrt()
            })
            .sum();

        // Seam discontinuities: where the composite switches donor, the
        // outgoing lap's own speed at the switch bin vs the incoming lap's.
        let mut seams = 0usize;
        let mut sum_dv = 0.0f32;
        let mut max_dv = 0.0f32;
        for b in 1..p.shared_bins {
            let (prev, cur) = (p.composite.source[b - 1], p.composite.source[b]);
            if prev == cur {
                continue;
            }
            seams += 1;
            let dv = (p.laps[prev].bins[b].speed_avg - p.laps[cur].bins[b].speed_avg).abs();
            sum_dv += dv;
            max_dv = max_dv.max(dv);
        }

        // GRADED corroboration prototype: instead of the binary
        // within-2.5-m/s flag, each non-source lap contributes agreement
        // weight 1 - |dv|/2.5 (linear falloff, 0 beyond tolerance), and
        // multiple laps combine with diminishing returns
        // (1 - prod(1 - w)); hole bins contribute nothing, as in
        // production corroboration. Reported for all bins and for the
        // HARVESTED bins only (source != base lap), where the binary
        // score saturates.
        let med: Vec<f32> = (0..p.shared_bins)
            .map(|b| {
                let mut ts: Vec<f32> = p.laps.iter().map(|l| l.bins[b].time_s).collect();
                ts.sort_by(f32::total_cmp);
                ts[ts.len() / 2]
            })
            .collect();
        let base = p
            .laps
            .iter()
            .enumerate()
            .min_by(|a, b| a.1.time_s.total_cmp(&b.1.time_s))
            .map(|(i, _)| i)
            .unwrap_or(0);
        let (mut g_num, mut g_den) = (0.0f32, 0.0f32);
        let (mut a_num, mut a_den) = (0.0f32, 0.0f32);
        let mut adopted_bins = 0usize;
        for (b, med_b) in med.iter().enumerate() {
            let cb = &p.composite.bins[b];
            let src = p.composite.source[b];
            let mut miss = 1.0f32;
            for (li, lap) in p.laps.iter().enumerate() {
                if li == src || lap.bins[b].time_s < 0.5 * med_b {
                    continue;
                }
                let w = (1.0 - (lap.bins[b].speed_avg - cb.speed_avg).abs() / 2.5).max(0.0);
                miss *= 1.0 - w;
            }
            let support = 1.0 - miss;
            g_num += support * cb.time_s;
            g_den += cb.time_s;
            if src != base {
                adopted_bins += 1;
                a_num += support * cb.time_s;
                a_den += cb.time_s;
            }
        }
        let graded = if g_den > 0.0 { g_num / g_den } else { 0.0 };
        let graded_adopted = if a_den > 0.0 { a_num / a_den } else { 1.0 };

        // Tolerance sensitivity: the composite rebuilt with tighter seam
        // speed tolerances (production is 2.5 m/s).
        let tol_bonus: Vec<f32> = [1.5f32, 1.0, 0.5]
            .iter()
            .map(|t| {
                p.best_lap_time_s
                    - profile::build_composite_with_tolerance(&p.laps, p.shared_bins, *t).time_s
            })
            .collect();

        println!(
            "{}\t{}\t{}\t{:.3}\t{:.3}\t{:.3}\t{:.3}\t{:.3}\t{:.3}\t{}\t{}\t{}\t{:.2}\t{:.2}\t{:.3}\t{:.3}\t{:.3}\t{}\t{:.3}\t{:.3}\t{:.3}",
            Path::new(&path).file_stem().unwrap().to_string_lossy(),
            p.car_ordinal,
            p.laps.len(),
            p.best_lap_time_s,
            mean,
            sd,
            bin_sd,
            p.composite.time_s,
            p.best_lap_time_s - p.composite.time_s,
            p.composite.span_count(),
            p.composite.source_laps().len(),
            seams,
            sum_dv,
            max_dv,
            p.corroboration().score,
            graded,
            graded_adopted,
            adopted_bins,
            tol_bonus[0],
            tol_bonus[1],
            tol_bonus[2],
        );
    }
}
