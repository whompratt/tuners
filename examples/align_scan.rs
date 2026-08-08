//! Vector-alignment lever-choice validation: per context group, locate
//! the most recent pair's baseline position on the pooled landscapes,
//! derive the needed displacement, and rank the map's cells by
//! alignment × selectivity. The read-side sanity harness for the
//! displacement gates and the clean-vs-blunt lever separation.
//!
//! Usage: align_scan [effect-map.tsv]

use tuners::advice::effectmap::{self, MapContext};
use tuners::analysis::effects;

fn latest_position(map: &effectmap::EffectMap, loose: bool, dt: i32) -> Option<&effectmap::Sample> {
    map.samples
        .iter()
        .filter(|s| {
            !s.weak
                && !s.attributed
                && s.surface_loose == loose
                && s.drivetrain == dt
                && !s.position.is_empty()
        })
        .max_by(|a, b| a.to.cmp(&b.to))
}

fn print_group(map: &effectmap::EffectMap, cells: &[effectmap::Cell], loose: bool, dt: i32) {
    let Some(sample) = latest_position(map, loose, dt) else {
        return;
    };
    let surface = if loose { "dirt" } else { "tarmac" };
    let landscapes = effectmap::axis_landscapes(map, loose, Some(dt));
    let disp = effectmap::needed_displacement(&landscapes, &sample.position);
    println!(
        "== {surface} drivetrain {dt} — position from {}:{} pair {}→{} ==",
        sample.driver, sample.campaign, sample.from, sample.to
    );
    if disp.is_empty() {
        println!("  no displacement (landscape silent) -> univariate fallback\n");
        return;
    }
    for d in &disp {
        println!(
            "  want {:<28} {:+.1} floor units (at {:.3}{}, weight {:.2}, n {})",
            effects::label(d.key),
            d.move_units,
            d.at,
            d.optimum
                .map(|o| format!(", optimum {o:.3}"))
                .unwrap_or_default(),
            d.weight,
            d.n,
        );
    }
    let ctx = MapContext {
        drivetrain: dt,
        surface_loose: loose,
        aero: sample.aero,
    };
    let ranked = effectmap::align(cells, &disp, &ctx);
    if ranked.is_empty() {
        println!("  -> no aligned lever\n");
        return;
    }
    for c in ranked.iter().take(6) {
        let cell = c.cell;
        println!(
            "  {:<34} score {:>5.2} selectivity {:.2}  n={} ({} direct, {} yours) time {:+.2}s",
            effectmap::direction_phrase(&cell.family, cell.softer),
            c.score,
            c.selectivity,
            cell.n,
            cell.direct_n,
            cell.own_n,
            cell.delta_mean,
        );
    }
    println!();
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "effect-map.tsv".into());
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        eprintln!("{path}: {e}");
        std::process::exit(1);
    });
    let map = effectmap::parse(&text).unwrap_or_else(|e| {
        eprintln!("{path}: {e}");
        std::process::exit(1);
    });
    let cells = effectmap::aggregate(&map);
    for loose in [false, true] {
        for dt in [0, 1, 2] {
            print_group(&map, &cells, loose, dt);
        }
    }
}
