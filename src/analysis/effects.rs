//! Effect vectors: per-stint setup-sensitive behavioural
//! scalars, and pairwise deltas attached to campaign measurements.
//!
//! Purely empirical: fields are recorded, never interpreted here. The 2026-07
//! evidence: sustained averaged metrics move above noise but are
//! drift-contaminated (the Ford GT ARB series' balance index tracked the
//! session clock, not the slider), so consumers must read deltas against the
//! per-field noise floors, and A-B-A pairs get the drift-corrected form.
//! Which fields respond to which setup family is phase B's question; this
//! module just makes every measurement carry the whole movement.

use super::metrics::StintMetrics;

/// One stint's effect scalars, as (field key, value) in FIELDS order. Absent
/// fields (no cornering, no braking, no drag fit...) are simply omitted.
pub type Effects = Vec<(&'static str, f32)>;

/// Field registry: key, human label, unit hint for display ("" = plain
/// number; fractions are 0..1 and display as %). Keys are stable API for
/// JSON consumers and the phase-B validation table.
pub const FIELDS: &[(&str, &str, &str)] = &[
    ("balance", "balance index", ""),
    ("balance_low", "balance below 85 mph", ""),
    ("balance_high", "balance above 85 mph", ""),
    ("balance_on", "balance on throttle", ""),
    ("balance_off", "balance off throttle", ""),
    ("balance_brake", "balance while braking", ""),
    ("entry_balance", "corner-entry balance", ""),
    ("exit_balance", "corner-exit balance", ""),
    ("apex_speed", "matched-corner apex speed", "m/s"),
    ("front_slip", "front share of limit", "frac"),
    ("rear_slip", "rear share of limit", "frac"),
    ("countersteer", "counter-steer share", "frac"),
    ("os_flash", "oversteer-flash share", "frac"),
    ("os_on_power", "on-power oversteer share", "frac"),
    ("wheelspin", "wheelspin share", "frac"),
    ("temp_front", "front tire temp", "F"),
    ("temp_rear", "rear tire temp", "F"),
    ("top_gear_time", "top-gear time share", "frac"),
    ("high_rev", "top-gear high-rev share", "frac"),
    ("limiter", "limiter share", "frac"),
    ("reversals_front", "front damper reversals", "/s"),
    ("reversals_rear", "rear damper reversals", "/s"),
    ("bottomed", "bottoming share", "frac"),
    ("topped", "topped-out share", "frac"),
    ("brake_dive", "brake dive", ""),
    ("drag_beta", "drag per mass", "1e-3"),
];

/// Per-field library noise floor: largest same-setup stint-mean movement we
/// should shrug at. Calibrated 2026-07-26 on the real recording library
/// (examples/effect_scan.rs: same-setup cross-stint pairs plus lap-to-lap
/// spread). Campaign-measured floors, when a
/// campaign has same-setup pairs, take the max with these.
pub fn noise_floor(key: &str) -> f32 {
    match key {
        "balance" | "balance_high" => 0.03,
        "balance_low" => 0.07, // few samples per lap in the low band
        "balance_on" => 0.04,
        "balance_off" | "balance_brake" => 0.05,
        "entry_balance" | "exit_balance" => 0.05,
        // m/s, pair-level corner-matched. Same-setup pairs measured 0.33
        // (McLaren) / 0.38 (Ford GT) 2026-07-28; the stint-level average
        // this replaced needed 5.0 (corner-mix dominated).
        "apex_speed" => 0.5,
        "front_slip" | "rear_slip" => 0.04,
        "countersteer" => 0.010,
        "os_flash" => 0.015,
        "os_on_power" => 0.012,
        "wheelspin" => 0.02,
        "temp_front" => 4.0, // F
        "temp_rear" => 5.0,
        "top_gear_time" => 0.05, // 0.04 observed at same setup (route mix)
        "high_rev" => 0.05,
        "limiter" => 0.02,
        "reversals_front" | "reversals_rear" => 0.4,
        "bottomed" => 0.005,
        "topped" => 0.01,
        "brake_dive" => 0.02,
        "drag_beta" => 0.08, // displayed x1e3; canonical ~1e-4..1e-3 range
        _ => f32::MAX,
    }
}

/// Bands built on a handful of frames produce garbage indices; require ~2s
/// of 60 Hz samples before a band contributes a field.
const MIN_BAND_SAMPLES: usize = 120;

fn band(v: &super::metrics::BandBalance) -> Option<f32> {
    (v.samples >= MIN_BAND_SAMPLES).then_some(v.index).flatten()
}

/// The stint's effect vector. Every field is a plain measured scalar; None
/// components are omitted rather than zeroed so deltas never invent data.
pub fn vector(m: &StintMetrics) -> Effects {
    let mut out: Effects = Vec::new();
    let mut push = |k: &'static str, v: Option<f32>| {
        if let Some(v) = v
            && v.is_finite()
        {
            out.push((k, v));
        }
    };
    let os = &m.transient_oversteer;
    let cornering = m.understeer_index.is_some();
    push("balance", m.understeer_index);
    push("balance_low", band(&m.balance_low_speed));
    push("balance_high", band(&m.balance_high_speed));
    push("balance_on", band(&m.balance_on_throttle));
    push("balance_off", band(&m.balance_off_throttle));
    push("balance_brake", band(&m.balance_on_brake));
    // apex_speed is NOT a per-stint field: the stint-level average is
    // corner-mix dominated (floor 5 m/s, useless). It enters effect DELTAS
    // pair-level via attribution::apex_speed_delta (position-matched
    // corners), injected by the campaign loader.
    if let Some(c) = &m.corners {
        push("entry_balance", band(&c.entry));
        push("exit_balance", band(&c.exit));
    }
    push("front_slip", m.cornering_front_slip);
    push("rear_slip", m.cornering_rear_slip);
    // Transient shares are fractions OF cornering time: meaningless (always
    // zero) when the stint never cornered.
    push("countersteer", cornering.then_some(os.countersteer_frac));
    push("os_flash", cornering.then_some(os.clear_frac));
    push("os_on_power", cornering.then_some(os.on_power_frac));
    push("wheelspin", m.wheelspin_frac);
    let t = &m.tire_temp;
    push("temp_front", Some((t.fl.avg + t.fr.avg) / 2.0));
    push("temp_rear", Some((t.rl.avg + t.rr.avg) / 2.0));
    let g = &m.gears;
    push(
        "top_gear_time",
        g.time_frac
            .iter()
            .find(|(gear, _)| *gear == g.top_gear)
            .map(|(_, f)| *f),
    );
    push(
        "high_rev",
        (g.top_gear_max_rpm > 0.0).then_some(g.top_gear_high_rev_frac),
    );
    push("limiter", Some(g.limiter_frac));
    let s = &m.suspension;
    push(
        "reversals_front",
        Some((s.fl.reversals_per_sec + s.fr.reversals_per_sec) / 2.0),
    );
    push(
        "reversals_rear",
        Some((s.rl.reversals_per_sec + s.rr.reversals_per_sec) / 2.0),
    );
    let avg4 = |f: fn(&super::metrics::SuspensionStats) -> f32| {
        (f(&s.fl) + f(&s.fr) + f(&s.rl) + f(&s.rr)) / 4.0
    };
    push("bottomed", Some(avg4(|w| w.bottomed_frac)));
    push("topped", Some(avg4(|w| w.topped_frac)));
    push("brake_dive", m.brake_dive_front);
    // beta is ~1e-4..1e-3 in SI; scale to a display-friendly magnitude.
    push("drag_beta", m.driveline.as_ref().map(|d| d.beta * 1e3));
    out
}

/// The registry's static key for a serialized field name, for parsers
/// rebuilding `Effects` from text.
pub fn key_of(name: &str) -> Option<&'static str> {
    FIELDS.iter().find(|(k, ..)| *k == name).map(|(k, ..)| *k)
}

/// Human label for a field key ("balance" -> "balance index").
pub fn label(key: &str) -> &str {
    FIELDS
        .iter()
        .find(|(k, ..)| *k == key)
        .map_or(key, |(_, l, _)| *l)
}

fn get(v: &Effects, key: &str) -> Option<f32> {
    v.iter().find(|(k, _)| *k == key).map(|(_, x)| *x)
}

/// to − from, per field present on BOTH sides, in FIELDS order.
pub fn delta(from: &Effects, to: &Effects) -> Effects {
    to.iter()
        .filter_map(|(k, tv)| Some((*k, tv - get(from, k)?)))
        .collect()
}

/// Drift-corrected A-B-A effect: (excursion delta − revert delta) / 2 per
/// field, mirroring the lap-time decomposition (journal/advise).
pub fn aba(exc: &Effects, rev: &Effects) -> Effects {
    exc.iter()
        .filter_map(|(k, e)| Some((*k, (e - get(rev, k)?) / 2.0)))
        .collect()
}

/// Fold a same-setup pair's |delta| into a per-field campaign floor.
pub fn fold_floor(floor: &mut Effects, d: &Effects) {
    for (k, v) in d {
        match floor.iter_mut().find(|(fk, _)| fk == k) {
            Some((_, f)) => *f = f.max(v.abs()),
            None => floor.push((k, v.abs())),
        }
    }
}

/// Fields of a delta that moved at or above their floor (library default,
/// raised by the campaign's own same-setup floor when one exists), strongest
/// first by floor multiples. The display cut for CLI/dashboard lines;
/// consumers wanting the raw vector read `Effects` directly.
pub fn movers(d: &Effects, campaign_floor: Option<&Effects>) -> Effects {
    let floor_of = |k: &str| {
        let base = noise_floor(k);
        campaign_floor
            .and_then(|f| get(f, k))
            .map_or(base, |c| c.max(base))
    };
    let mut out: Vec<(&'static str, f32, f32)> = d
        .iter()
        .filter_map(|(k, v)| {
            let f = floor_of(k);
            (v.abs() >= f).then_some((*k, *v, v.abs() / f))
        })
        .collect();
    out.sort_by(|a, b| b.2.total_cmp(&a.2));
    out.into_iter().map(|(k, v, _)| (k, v)).collect()
}

/// "counter-steer share +2.1%, balance on throttle +0.07": human form of a
/// (sub)set of effect fields, in the given order.
pub fn describe(fields: &Effects) -> String {
    fields
        .iter()
        .map(|(k, v)| {
            let (label, unit) = FIELDS
                .iter()
                .find(|(fk, ..)| fk == k)
                .map_or((*k, ""), |(_, l, u)| (*l, *u));
            match unit {
                "frac" => format!("{label} {:+.1}%", v * 100.0),
                "" => format!("{label} {v:+.2}"),
                u => format!("{label} {v:+.1} {u}"),
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(pairs: &[(&'static str, f32)]) -> Effects {
        pairs.to_vec()
    }

    #[test]
    fn delta_matches_fields_and_skips_one_sided() {
        let a = v(&[("balance", 0.20), ("apex_speed", 30.0), ("wheelspin", 0.05)]);
        let b = v(&[
            ("balance", 0.25),
            ("apex_speed", 29.0),
            ("countersteer", 0.02),
        ]);
        let d = delta(&a, &b);
        assert!((get(&d, "balance").unwrap() - 0.05).abs() < 1e-6);
        assert_eq!(get(&d, "apex_speed"), Some(-1.0));
        // present on only one side -> omitted, not zeroed
        assert_eq!(get(&d, "wheelspin"), None);
        assert_eq!(get(&d, "countersteer"), None);
    }

    #[test]
    fn aba_halves_the_drift_cancelled_difference() {
        let exc = v(&[("balance", 0.08), ("countersteer", 0.02)]);
        let rev = v(&[("balance", 0.02), ("countersteer", -0.02)]);
        let e = aba(&exc, &rev);
        assert_eq!(get(&e, "balance"), Some(0.03));
        assert_eq!(get(&e, "countersteer"), Some(0.02));
    }

    #[test]
    fn movers_gates_on_floor_and_ranks_by_multiples() {
        // balance floor 0.04: 0.05 passes; countersteer floor 0.012: 0.036 = 3x
        let d = v(&[
            ("balance", 0.05),
            ("countersteer", 0.036),
            ("temp_front", 2.0),
        ]);
        let m = movers(&d, None);
        assert_eq!(m.first().map(|(k, _)| *k), Some("countersteer"));
        assert_eq!(m.len(), 2, "temp_front 2.0F is under its 6F floor: {m:?}");
    }

    #[test]
    fn campaign_floor_raises_but_never_lowers_the_gate() {
        let d = v(&[("balance", 0.05)]);
        // campaign saw a same-setup balance swing of 0.06 -> 0.05 is noise here
        let floor = v(&[("balance", 0.06)]);
        assert!(movers(&d, Some(&floor)).is_empty());
        // a campaign floor BELOW the library default must not admit noise
        let low = v(&[("balance", 0.01)]);
        let small = v(&[("balance", 0.02)]);
        assert!(movers(&small, Some(&low)).is_empty());
    }

    #[test]
    fn fold_floor_keeps_the_max_abs() {
        let mut floor = Effects::new();
        fold_floor(&mut floor, &v(&[("balance", -0.03)]));
        fold_floor(&mut floor, &v(&[("balance", 0.01), ("apex_speed", 0.4)]));
        assert_eq!(get(&floor, "balance"), Some(0.03));
        assert_eq!(get(&floor, "apex_speed"), Some(0.4));
    }

    #[test]
    fn describe_formats_by_unit() {
        let s = describe(&v(&[("countersteer", 0.021), ("balance", 0.05)]));
        assert_eq!(s, "counter-steer share +2.1%, balance index +0.05");
    }
}
