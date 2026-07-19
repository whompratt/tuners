//! Text rendering of stint metrics. Observations only — no advice.

use super::metrics::{stint_metrics, StintMetrics};
use super::LapSlice;
use crate::packet::{class_name, drivetrain_name};
use crate::util::{format_lap_time, MPS_TO_MPH};
use std::fmt::Write;

/// Lap table for a stint. Standing-start laps (rivals out laps) are labelled and
/// excluded from the best-lap comparison — they are not comparable to flying laps.
pub fn render_laps(laps: &[LapSlice]) -> String {
    let mut out = String::new();
    let best_flying = laps
        .iter()
        .filter(|l| !l.standing_start)
        .filter_map(|l| l.time_s)
        .fold(f32::INFINITY, f32::min);

    writeln!(out, "  laps").unwrap();
    for lap in laps {
        let m = stint_metrics(lap.frames);
        let label = format!("lap {}", lap.number + 1);
        let time = match lap.time_s {
            Some(t) => format!("{:>9}", format_lap_time(t)),
            None => format!("({:.1}s, incomplete)", m.duration_s),
        };
        let compare = match (lap.time_s, lap.standing_start) {
            (_, true) => " | standing start".to_string(),
            (Some(t), false) if t <= best_flying => " | best".to_string(),
            (Some(t), false) => format!(" | +{:.2}s vs best", t - best_flying),
            (None, false) => String::new(),
        };
        let extras = [
            m.wheelspin_frac.map(|w| format!("spin {:.1}%", w * 100.0)),
            m.lockup_frac.map(|l| format!("brake-slip {:.1}%", l * 100.0)),
            m.understeer_index.map(|i| format!("balance {i:+.2}")),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" | ");
        writeln!(
            out,
            "    {label}: {time}{compare} | max {:.0} mph | {extras}",
            m.max_speed * MPS_TO_MPH,
        )
        .unwrap();
    }
    out
}

pub fn render_stint(index: usize, m: &StintMetrics) -> String {
    let mut out = String::new();
    let pct = |f: f32| format!("{:.1}%", f * 100.0);

    writeln!(
        out,
        "stint {index}: {:.1}s | {:.2} mi | avg {:.0} mph | max {:.0} mph",
        m.duration_s,
        m.distance_m / 1609.34,
        m.avg_speed * MPS_TO_MPH,
        m.max_speed * MPS_TO_MPH,
    )
    .unwrap();
    writeln!(
        out,
        "car ordinal {}: class {} | PI {} | {} | {} cyl | redline {:.0}",
        m.car_ordinal,
        class_name(m.car_class),
        m.car_performance_index,
        drivetrain_name(m.drivetrain_type),
        m.num_cylinders,
        m.redline,
    )
    .unwrap();

    let t = &m.tire_temp;
    writeln!(out, "\n  tires (\u{b0}F avg/max)").unwrap();
    writeln!(out, "    FL {:>5.0}/{:<5.0}  FR {:>5.0}/{:<5.0}", t.fl.avg, t.fl.max, t.fr.avg, t.fr.max).unwrap();
    writeln!(out, "    RL {:>5.0}/{:<5.0}  RR {:>5.0}/{:<5.0}", t.rl.avg, t.rl.max, t.rr.avg, t.rr.max).unwrap();
    let front = (t.fl.avg + t.fr.avg) / 2.0;
    let rear = (t.rl.avg + t.rr.avg) / 2.0;
    let left = (t.fl.avg + t.rl.avg) / 2.0;
    let right = (t.fr.avg + t.rr.avg) / 2.0;
    writeln!(
        out,
        "    front {} {:.0}\u{b0}F vs rear | {} side {:.0}\u{b0}F hotter",
        if front >= rear { "hotter by" } else { "cooler by" },
        (front - rear).abs(),
        if left >= right { "left" } else { "right" },
        (left - right).abs(),
    )
    .unwrap();

    writeln!(out, "\n  grip").unwrap();
    let s = &m.slip_frac;
    writeln!(
        out,
        "    time over 100% slip: FL {} FR {} RL {} RR {}",
        pct(s.fl), pct(s.fr), pct(s.rl), pct(s.rr),
    )
    .unwrap();
    if let Some(w) = m.wheelspin_frac {
        writeln!(out, "    wheelspin on throttle: {}", pct(w)).unwrap();
    }
    if let Some(l) = m.lockup_frac {
        // With ABS on, sustained slip at the limit is normal threshold braking.
        writeln!(out, "    braking at/over slip limit: {}", pct(l)).unwrap();
    }
    match m.understeer_index {
        Some(idx) => writeln!(
            out,
            "    balance while cornering ({} of stint): {} (front-rear slip angle delta {:+.2})",
            pct(m.cornering_frac),
            if idx > 0.05 { "understeer" } else if idx < -0.05 { "oversteer" } else { "neutral" },
            idx,
        )
        .unwrap(),
        None => writeln!(out, "    no significant cornering in this stint").unwrap(),
    }

    writeln!(out, "\n  suspension (normalized travel avg | bottomed | topped)").unwrap();
    for (label, s) in [
        ("FL", m.suspension.fl),
        ("FR", m.suspension.fr),
        ("RL", m.suspension.rl),
        ("RR", m.suspension.rr),
    ] {
        writeln!(
            out,
            "    {label} {:.2} | {} | {}",
            s.avg,
            pct(s.bottomed_frac),
            pct(s.topped_frac),
        )
        .unwrap();
    }

    writeln!(out, "\n  gearing").unwrap();
    let per_gear: Vec<String> = m
        .gears
        .time_frac
        .iter()
        .map(|(g, f)| format!("{g}:{}", pct(*f)))
        .collect();
    writeln!(out, "    time per gear: {}", per_gear.join("  ")).unwrap();
    match m.gears.avg_upshift_rpm {
        Some(rpm) => writeln!(
            out,
            "    {} upshifts, avg at {:.0} rpm ({:.0}% of redline)",
            m.gears.upshifts,
            rpm,
            100.0 * rpm / m.redline.max(1.0),
        )
        .unwrap(),
        None => writeln!(out, "    no upshifts").unwrap(),
    }
    writeln!(
        out,
        "    top gear used: {} | time on limiter: {}",
        m.gears.top_gear,
        pct(m.gears.limiter_frac),
    )
    .unwrap();

    out
}
