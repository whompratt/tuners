//! Behaviour-axis landscape calibration: pool the effect map's
//! normalized pace gradients per axis and print the fitted landscapes
//! per context group. The read-side companion of `tuners map` for
//! choosing gates (min pairs, min r) and the advice-worthy axis basis.
//!
//! Usage: landscape_scan [effect-map.tsv]

use tuners::advice::effectmap;
use tuners::analysis::effects;

fn print_group(map: &effectmap::EffectMap, name: &str, loose: bool, dt: Option<i32>) {
    let mut ls = effectmap::axis_landscapes(map, loose, dt);
    if ls.is_empty() {
        return;
    }
    ls.sort_by_key(|l| std::cmp::Reverse(l.n));
    println!("== {name} ==");
    println!(
        "{:<28} {:>3} {:>12} {:>6} {:>18} {:>10}",
        "axis", "n", "grad %lap/u", "r", "midpoint range", "optimum"
    );
    for l in ls {
        println!(
            "{:<28} {:>3} {:>12.3} {:>6.2} {:>8.3}..{:<8.3} {:>10}",
            effects::label(l.key),
            l.n,
            l.mean_gradient * 100.0,
            l.r,
            l.lo,
            l.hi,
            l.optimum
                .map(|o| format!("{o:.3}"))
                .unwrap_or_else(|| "-".into()),
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
    let eligible = map
        .samples
        .iter()
        .filter(|s| !s.weak && !s.attributed && s.lap_s.is_some())
        .count();
    println!(
        "{}: {} samples ({} landscape-eligible), {} campaigns\n",
        path,
        map.samples.len(),
        eligible,
        map.floors.len()
    );
    print_group(&map, "tarmac (all drivetrains)", false, None);
    for (dt, name) in [(0, "tarmac FWD"), (1, "tarmac RWD"), (2, "tarmac AWD")] {
        print_group(&map, name, false, Some(dt));
    }
    print_group(&map, "dirt (all drivetrains)", true, None);
}
