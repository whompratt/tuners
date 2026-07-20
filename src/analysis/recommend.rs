//! Recommendation rules: observations -> directional tune advice with evidence.
//! Blind mode: tune values and slider limits are unknown, so advice is a direction
//! phrased to survive unknown limits. See docs/plans/004-recommendations.md.

use super::journal::{Change, Family};
use super::metrics::StintMetrics;

/// Balance index magnitudes: mild tendency vs clear problem.
const BALANCE_MILD: f32 = 0.05;
const BALANCE_CLEAR: f32 = 0.10;
/// Working tire temperature band (°F); outside it pressures likely need adjusting.
const TEMP_BAND_LOW_F: f32 = 160.0;
const TEMP_BAND_HIGH_F: f32 = 210.0;
/// °F beyond the band edge before the pressure rule speaks with medium confidence.
const TEMP_CLEAR_MARGIN_F: f32 = 20.0;
/// Wheelspin as a fraction of on-throttle time.
const WHEELSPIN_MED: f32 = 0.08;
const WHEELSPIN_HIGH: f32 = 0.15;
/// Time on the rev limiter worth reacting to.
const LIMITER_FRAC: f32 = 0.02;
/// Minimum cornering samples in a conditioned band (speed / throttle) before
/// band rules speak — ~8s of cornering at 60 Hz.
const BAND_MIN_SAMPLES: usize = 500;
/// High-speed-only imbalance (aero signature): the high band must read at
/// least this while the low band stays under BALANCE_MILD. Calibrated on the
/// real library: converged tarmac tunes show band gaps ~0.05 with BOTH bands
/// imbalanced; only a genuinely speed-dependent car separates like this.
const AERO_HIGH_INDEX: f32 = 0.10;
const AERO_BAND_GAP: f32 = 0.10;
/// On−off throttle index shift marking a power-on balance change. Healthy
/// tarmac reads -0.02..-0.09 on the real library.
const POWER_SHIFT: f32 = 0.10;
/// The on-throttle index itself must clear this (in the shift direction): the
/// power end genuinely works harder, not merely less pushy than entry.
const POWER_INDEX: f32 = 0.08;
/// Loose surfaces: throttle rotation is driving technique (dirt shifts read
/// -0.13..-0.62 on well-tuned rally cars with on-throttle index ~0), so the
/// on-throttle index gate is stricter before the diff rule speaks.
const POWER_INDEX_LOOSE: f32 = 0.12;
/// Top gear never reaching this fraction of redline = final drive too long.
const UNUSED_REV_FRAC: f32 = 0.90;
/// Suspension travel fractions.
const BOTTOMING_FRAC: f32 = 0.03;
const TOPPING_FRAC: f32 = 0.10;
/// Damping calibration from deliberate min/max A/Bs (see plan 008). Baselines
/// are SURFACE-driven: healthy reads ~5.5-6 reversals/s on tarmac but 12-16 on
/// dirt, so thresholds are per-surface.
const UNDERDAMPED_REV_TARMAC: f32 = 7.0;
const UNDERDAMPED_TOPPED_TARMAC: f32 = 0.05;
/// Loose-surface underdamping calibrated from a dirt min-damping capture:
/// healthy axle averages read 11.8-16.2 rev/s, min damping 17.6-19.5 — tight
/// separation (one car), hence Medium confidence when it fires.
const UNDERDAMPED_REV_LOOSE: f32 = 17.0;
const UNDERDAMPED_TOPPED_LOOSE: f32 = 0.15;
/// Overdamping, strong form (any surface): the wheel lives at full extension
/// instead of tracking the surface. Dirt max-damping read 66-84%; healthy dirt
/// 3-26%, healthy tarmac 1-7%.
const OVERDAMPED_TOPPED_FRAC: f32 = 0.40;
/// Overdamping, mild tarmac form: suspension barely articulates.
const OVERDAMPED_REV_TARMAC: f32 = 3.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Confidence {
    Low,
    Medium,
    High,
}

impl Confidence {
    pub fn label(self) -> &'static str {
        match self {
            Confidence::High => "high",
            Confidence::Medium => "medium",
            Confidence::Low => "low",
        }
    }
}

#[derive(Debug)]
pub struct Recommendation {
    pub area: &'static str,
    pub advice: String,
    pub evidence: Vec<String>,
    pub confidence: Confidence,
    /// The journal-comparable direction this advice implies, when it maps to a
    /// parameter family the journal tracks. Lets history reconcile with advice.
    pub implied: Option<Change>,
}

/// `overall` covers the whole stint; `per_lap` holds metrics for each completed
/// flying lap (used to judge consistency). Sorted most-confident first.
pub fn recommend(overall: &StintMetrics, per_lap: &[StintMetrics]) -> Vec<Recommendation> {
    let mut recs = Vec::new();

    let balance_sign = balance_rule(overall, per_lap, &mut recs);
    aero_rule(overall, &mut recs);
    power_balance_rule(overall, &mut recs);
    tire_pressure_rule(overall, balance_sign, &mut recs);
    traction_rule(overall, &mut recs);
    gearing_rule(overall, &mut recs);
    suspension_rule(overall, &mut recs);
    damping_rule(overall, &mut recs);

    recs.sort_by_key(|r| std::cmp::Reverse(r.confidence));
    recs
}

/// Returns the balance direction sign (+1 understeer, -1 oversteer) when the rule
/// fired, so the tire rule can attribute axle heat to scrub.
fn balance_rule(
    overall: &StintMetrics,
    per_lap: &[StintMetrics],
    recs: &mut Vec<Recommendation>,
) -> Option<f32> {
    let idx = overall.understeer_index?;
    if idx.abs() < BALANCE_MILD {
        return None;
    }
    let lap_indices: Vec<f32> = per_lap.iter().filter_map(|m| m.understeer_index).collect();
    let consistent = lap_indices.len() >= 2
        && lap_indices
            .iter()
            .all(|i| i.signum() == idx.signum() && i.abs() >= BALANCE_MILD);

    let confidence = match (idx.abs() >= BALANCE_CLEAR, consistent) {
        (true, true) => Confidence::High,
        (true, false) => Confidence::Medium,
        (false, _) => Confidence::Low,
    };
    let understeer = idx > 0.0;
    let advice = if understeer {
        "reduce front roll stiffness: soften the front anti-roll bar first (springs \
         second); if the front is already at minimum, stiffen the rear instead. More \
         front aero also helps where the route allows."
    } else {
        "reduce rear roll stiffness: soften the rear anti-roll bar first (springs \
         second); if the rear is already at minimum, stiffen the front instead. \
         Less rear-biased aero can also help."
    };

    let mut evidence = vec![format!(
        "{} tendency: front−rear slip angle delta {idx:+.2} while cornering",
        if understeer { "understeer" } else { "oversteer" },
    )];
    if !lap_indices.is_empty() {
        let laps_fmt: Vec<String> = lap_indices.iter().map(|i| format!("{i:+.2}")).collect();
        evidence.push(format!(
            "{} across {} flying lap(s): {}",
            if consistent { "consistent" } else { "not consistent" },
            lap_indices.len(),
            laps_fmt.join(", "),
        ));
    }
    let (front_t, rear_t) = axle_temps(overall);
    if (front_t - rear_t).abs() >= 5.0 {
        evidence.push(format!(
            "{} tires run {:.0}°F hotter",
            if front_t > rear_t { "front" } else { "rear" },
            (front_t - rear_t).abs(),
        ));
    }
    let front_slip = (overall.slip_frac.fl + overall.slip_frac.fr) / 2.0;
    let rear_slip = (overall.slip_frac.rl + overall.slip_frac.rr) / 2.0;
    evidence.push(format!(
        "time over slip limit: front {:.0}% vs rear {:.0}%",
        front_slip * 100.0,
        rear_slip * 100.0,
    ));
    if let (Some(lo), Some(hi)) = supported_pair(&overall.balance_low_speed, &overall.balance_high_speed) {
        evidence.push(if (lo - hi) * idx.signum() >= AERO_BAND_GAP / 2.0 {
            format!(
                "concentrated at low speed ({lo:+.2} below 85 mph vs {hi:+.2} above) — \
                 mechanical grip, so bars/springs over aero"
            )
        } else {
            format!("by speed: {lo:+.2} below 85 mph, {hi:+.2} above")
        });
    }
    if let Some(c) = &overall.corners
        && let (Some(entry), Some(exit)) = supported_pair(&c.entry, &c.exit)
    {
        evidence.push(format!(
            "by corner phase: {entry:+.2} into corners, {exit:+.2} out \
             ({} corners)",
            c.corners,
        ));
    }

    recs.push(Recommendation {
        area: "balance",
        advice: advice.into(),
        evidence,
        confidence,
        implied: Some(Change {
            family: if understeer { Family::FrontRoll } else { Family::RearRoll },
            softer: true,
            magnitude: None,
        }),
    });
    Some(idx.signum())
}

/// Both bands' indices, when each has enough cornering samples to trust.
fn supported_pair(
    a: &crate::analysis::metrics::BandBalance,
    b: &crate::analysis::metrics::BandBalance,
) -> (Option<f32>, Option<f32>) {
    let idx = |band: &crate::analysis::metrics::BandBalance| {
        (band.samples >= BAND_MIN_SAMPLES).then_some(band.index).flatten()
    };
    (idx(a), idx(b))
}

/// Imbalance that lives ONLY in the high-speed band is an aero problem, not a
/// bars problem — mechanical imbalance shows at every speed (and on the real
/// library, mechanical understeer reads STRONGER at low speed).
fn aero_rule(overall: &StintMetrics, recs: &mut Vec<Recommendation>) {
    let (Some(lo), Some(hi)) = supported_pair(&overall.balance_low_speed, &overall.balance_high_speed)
    else {
        return;
    };
    if hi.abs() < AERO_HIGH_INDEX || lo.abs() >= BALANCE_MILD || (hi - lo).abs() < AERO_BAND_GAP {
        return;
    }
    let understeer = hi > 0.0;
    let advice = if understeer {
        "add front aero (or reduce rear aero): the car only pushes at high speed, \
         where downforce balance outweighs the bars"
    } else {
        "add rear aero: the car is only loose at high speed, where downforce \
         balance outweighs the bars"
    };
    recs.push(Recommendation {
        area: "aero",
        advice: advice.into(),
        evidence: vec![format!(
            "{} at speed only: index {hi:+.2} above 85 mph vs {lo:+.2} below \
             (neutral) — a bars change would upset the low-speed balance that is \
             currently fine",
            if understeer { "understeer" } else { "oversteer" },
        )],
        confidence: if overall.surface_loose { Confidence::Low } else { Confidence::Medium },
        implied: Some(Change {
            family: if understeer { Family::FrontAero } else { Family::RearAero },
            softer: false,
            magnitude: None,
        }),
    });
}

/// Balance that degrades when the throttle is applied points at the driveline,
/// not the bars: acceleration diff lock (and center torque split on AWD).
fn power_balance_rule(overall: &StintMetrics, recs: &mut Vec<Recommendation>) {
    let (Some(on), Some(off)) = supported_pair(&overall.balance_on_throttle, &overall.balance_off_throttle)
    else {
        return;
    };
    let shift = on - off;
    let index_gate = if overall.surface_loose { POWER_INDEX_LOOSE } else { POWER_INDEX };
    let rear_drive = overall.drivetrain_type != 0; // RWD or AWD
    let front_drive = overall.drivetrain_type != 1; // FWD or AWD
    let awd = overall.drivetrain_type == 2;

    let (advice, index_evt) = if shift <= -POWER_SHIFT && on <= -index_gate && rear_drive {
        (
            format!(
                "reduce rear differential acceleration lock: the rear breaks away \
                 specifically under power{}",
                if awd { " (AWD: shifting center torque forward also helps)" } else { "" },
            ),
            "oversteer",
        )
    } else if shift >= POWER_SHIFT && on >= index_gate && front_drive {
        (
            format!(
                "reduce front differential acceleration lock: the front washes out \
                 specifically under power{}",
                if awd { " (AWD: shifting center torque rearward also helps)" } else { "" },
            ),
            "understeer",
        )
    } else {
        return;
    };
    let mut evidence = vec![format!(
        "{index_evt} appears with throttle: index {on:+.2} on power vs {off:+.2} \
         off power while cornering"
    )];
    if let Some(rear) = overall.balance_on_throttle.rear_slip {
        evidence.push(format!(
            "rear tires at {:.0}% of grip limit while cornering on power",
            rear * 100.0
        ));
    }
    recs.push(Recommendation {
        area: "differential",
        advice,
        evidence,
        confidence: Confidence::Medium,
        implied: Some(Change { family: Family::DiffAccel, softer: true, magnitude: None }),
    });
}

fn tire_pressure_rule(
    overall: &StintMetrics,
    balance_sign: Option<f32>,
    recs: &mut Vec<Recommendation>,
) {
    let (front, rear) = axle_temps(overall);
    for (axle, avg, scrub_heated) in [
        ("front", front, balance_sign == Some(1.0)),
        ("rear", rear, balance_sign == Some(-1.0)),
    ] {
        let (advice, margin) = if avg > TEMP_BAND_HIGH_F {
            (
                format!("raise {axle} tire pressures a step — {axle} tires run hot"),
                avg - TEMP_BAND_HIGH_F,
            )
        } else if avg < TEMP_BAND_LOW_F {
            (
                format!("lower {axle} tire pressures a step to build temperature"),
                TEMP_BAND_LOW_F - avg,
            )
        } else {
            continue;
        };
        let mut evidence = vec![format!(
            "{axle} avg temp {avg:.0}°F vs {TEMP_BAND_LOW_F:.0}-{TEMP_BAND_HIGH_F:.0}°F working band"
        )];
        let mut confidence = if margin >= TEMP_CLEAR_MARGIN_F {
            Confidence::Medium
        } else {
            Confidence::Low
        };
        if scrub_heated && avg > TEMP_BAND_HIGH_F {
            confidence = Confidence::Low;
            evidence.push(
                "heat is partly scrub from the balance issue above — fix balance first".into(),
            );
        }
        recs.push(Recommendation { area: "tires", advice, evidence, confidence, implied: None });
    }
}

fn traction_rule(overall: &StintMetrics, recs: &mut Vec<Recommendation>) {
    let Some(spin) = overall.wheelspin_frac else { return };
    if spin < WHEELSPIN_MED {
        return;
    }
    let drive_axle = match overall.drivetrain_type {
        0 => "front",
        1 => "rear",
        _ => "drive",
    };
    recs.push(Recommendation {
        area: "traction",
        advice: format!(
            "improve {drive_axle}-axle traction: reduce differential acceleration lock \
             first; softer {drive_axle} springs/dampers also help put power down"
        ),
        evidence: vec![format!(
            "wheelspin during {:.0}% of on-throttle time ({} drivetrain)",
            spin * 100.0,
            crate::packet::drivetrain_name(overall.drivetrain_type),
        )],
        confidence: if spin >= WHEELSPIN_HIGH { Confidence::High } else { Confidence::Medium },
        implied: Some(Change { family: Family::DiffAccel, softer: true, magnitude: None }),
    });
}

fn gearing_rule(overall: &StintMetrics, recs: &mut Vec<Recommendation>) {
    let g = &overall.gears;
    if g.limiter_frac >= LIMITER_FRAC {
        let mut evidence = vec![format!(
            "on the rev limiter {:.1}% of the stint (redline {:.0} rpm)",
            g.limiter_frac * 100.0,
            overall.redline,
        )];
        if let Some(rpm) = g.avg_upshift_rpm {
            evidence.push(format!("average upshift at {rpm:.0} rpm"));
        }
        evidence.push(format!("top gear used: {}", g.top_gear));
        recs.push(Recommendation {
            area: "gearing",
            advice: "lengthen the final drive (or the gears that hit the limiter) so the \
                     engine stays below redline at the route's top speeds"
                .into(),
            evidence,
            confidence: Confidence::Medium,
            implied: Some(Change { family: Family::Gearing, softer: true, magnitude: None }),
        });
        return;
    }

    // Telemetry-only signal in the other direction: the top of the rev range goes
    // unused, so the whole gear stack is longer than the route needs.
    if g.top_gear == 0 || overall.redline <= 0.0 {
        return;
    }
    if g.top_gear_max_rpm < UNUSED_REV_FRAC * overall.redline {
        let top_time = g
            .time_frac
            .last()
            .map(|(_, f)| *f)
            .unwrap_or(0.0);
        recs.push(Recommendation {
            area: "gearing",
            advice: "shorten the final drive: the top of the rev range goes unused, so \
                     every gear is longer than the route needs — shorter gearing gives \
                     more acceleration at no top-speed cost. (Ignore if this route just \
                     lacks a flat-out section.)"
                .into(),
            evidence: vec![format!(
                "highest rpm in top gear ({}) was {:.0} — {:.0}% of the {:.0} redline, \
                 with {:.1}% of the stint spent there and no limiter time",
                g.top_gear,
                g.top_gear_max_rpm,
                100.0 * g.top_gear_max_rpm / overall.redline,
                overall.redline,
                top_time * 100.0,
            )],
            confidence: Confidence::Medium,
            implied: Some(Change { family: Family::Gearing, softer: false, magnitude: None }),
        });
    }
}

fn suspension_rule(overall: &StintMetrics, recs: &mut Vec<Recommendation>) {
    let s = &overall.suspension;
    for (axle, bottomed, topped) in [
        ("front", s.fl.bottomed_frac.max(s.fr.bottomed_frac), s.fl.topped_frac.max(s.fr.topped_frac)),
        ("rear", s.rl.bottomed_frac.max(s.rr.bottomed_frac), s.rl.topped_frac.max(s.rr.topped_frac)),
    ] {
        if bottomed >= BOTTOMING_FRAC {
            recs.push(Recommendation {
                area: "suspension",
                advice: format!(
                    "{axle} suspension bottoms out: stiffen {axle} springs or raise \
                     {axle} ride height"
                ),
                evidence: vec![format!(
                    "{axle} at full compression {:.1}% of the stint",
                    bottomed * 100.0
                )],
                confidence: Confidence::Medium,
                implied: None,
            });
        }
        if topped >= TOPPING_FRAC {
            recs.push(Recommendation {
                area: "suspension",
                advice: format!(
                    "{axle} suspension spends long at full extension — if the route is \
                     smooth this can mean over-stiff {axle} springs or too much rebound; \
                     over crests and jumps it is normal"
                ),
                evidence: vec![format!(
                    "{axle} at full extension {:.1}% of the stint",
                    topped * 100.0
                )],
                confidence: Confidence::Low,
                implied: None,
            });
        }
    }
}

fn damping_rule(overall: &StintMetrics, recs: &mut Vec<Recommendation>) {
    let s = &overall.suspension;
    let loose = overall.surface_loose;
    let baseline = if loose { "~12-16 on loose surfaces" } else { "~5.5-6 on tarmac" };
    let (under_rev, under_topped) = if loose {
        (UNDERDAMPED_REV_LOOSE, UNDERDAMPED_TOPPED_LOOSE)
    } else {
        (UNDERDAMPED_REV_TARMAC, UNDERDAMPED_TOPPED_TARMAC)
    };

    for (axle, a, b) in [("front", s.fl, s.fr), ("rear", s.rl, s.rr)] {
        let rev = (a.reversals_per_sec + b.reversals_per_sec) / 2.0;
        let topped = a.topped_frac.max(b.topped_frac);

        if topped >= OVERDAMPED_TOPPED_FRAC {
            // Strong overdamping: the wheel lives at full extension instead of
            // tracking the surface.
            let mut evidence = vec![format!(
                "{axle} wheel at full extension {:.0}% of the stint (healthy reads \
                 under ~25% even on rough dirt)",
                topped * 100.0,
            )];
            if loose && let Some(flutter) = overall.rpm_flutter {
                evidence.push(format!(
                    "rpm flutter {flutter:.0} rpm/s on throttle — skipping drive \
                     wheels (roughly doubles vs healthy damping on the same surface)"
                ));
            }
            recs.push(Recommendation {
                area: "damping",
                advice: format!(
                    "reduce {axle} damping (rebound first): the {axle} wheels are \
                     held off the surface instead of following it"
                ),
                evidence,
                confidence: Confidence::High,
                implied: None,
            });
        } else if rev >= under_rev && topped >= under_topped {
            recs.push(Recommendation {
                area: "damping",
                advice: format!(
                    "increase {axle} damping (rebound especially): the {axle} wheels \
                     oscillate and spend long stretches at full extension — bouncing \
                     off the surface costs grip everywhere"
                ),
                evidence: vec![
                    format!(
                        "{axle} suspension reverses direction {rev:.1}x/s (healthy \
                         baseline {baseline})"
                    ),
                    format!("{axle} at full extension {:.1}% of the stint", topped * 100.0),
                ],
                confidence: if loose { Confidence::Medium } else { Confidence::High },
                implied: None,
            });
        } else if !loose && rev > 0.0 && rev <= OVERDAMPED_REV_TARMAC {
            recs.push(Recommendation {
                area: "damping",
                advice: format!(
                    "consider softer {axle} damping: the {axle} suspension barely \
                     articulates. On smooth tarmac this costs little; on bumpy or \
                     off-road surfaces it will cost real grip"
                ),
                evidence: vec![format!(
                    "{axle} suspension reverses direction only {rev:.1}x/s (healthy \
                     baseline {baseline})"
                )],
                confidence: Confidence::Low,
                implied: None,
            });
        }
    }
}

fn axle_temps(m: &StintMetrics) -> (f32, f32) {
    (
        (m.tire_temp.fl.avg + m.tire_temp.fr.avg) / 2.0,
        (m.tire_temp.rl.avg + m.tire_temp.rr.avg) / 2.0,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::metrics::{StintMetrics, TempStats};
    use crate::packet::Corners;

    fn base_metrics() -> StintMetrics {
        StintMetrics {
            samples: 1000,
            duration_s: 90.0,
            distance_m: 5000.0,
            avg_speed: 45.0,
            max_speed: 70.0,
            car_ordinal: 1,
            car_class: 4,
            car_performance_index: 800,
            drivetrain_type: 1,
            num_cylinders: 6,
            redline: 8000.0,
            tire_temp: Corners {
                fl: TempStats { avg: 185.0, max: 200.0 },
                fr: TempStats { avg: 185.0, max: 200.0 },
                rl: TempStats { avg: 185.0, max: 200.0 },
                rr: TempStats { avg: 185.0, max: 200.0 },
            },
            slip_frac: Corners::default(),
            understeer_index: Some(0.0),
            cornering_front_slip: Some(0.5),
            cornering_rear_slip: Some(0.5),
            cornering_frac: 0.3,
            balance_low_speed: Default::default(),
            balance_high_speed: Default::default(),
            balance_on_throttle: Default::default(),
            balance_off_throttle: Default::default(),
            corners: None,
            wheelspin_frac: Some(0.02),
            lockup_frac: Some(0.5),
            suspension: Corners::default(),
            gears: Default::default(),
            surface_rumble_avg: 0.0,
            surface_loose: false,
            jumps: 0,
            landing_bottomed_excluded: 0,
            rpm_flutter: None,
            wheelspeed_flutter: None,
        }
    }

    fn lap_with_index(idx: f32) -> StintMetrics {
        StintMetrics { understeer_index: Some(idx), ..base_metrics() }
    }

    #[test]
    fn consistent_understeer_is_high_confidence_front_advice() {
        let mut overall = base_metrics();
        overall.understeer_index = Some(0.24);
        let laps = [lap_with_index(0.23), lap_with_index(0.26), lap_with_index(0.24)];
        let recs = recommend(&overall, &laps);
        let balance = recs.iter().find(|r| r.area == "balance").unwrap();
        assert_eq!(balance.confidence, Confidence::High);
        assert!(balance.advice.contains("front anti-roll bar"), "{}", balance.advice);
        assert!(balance.evidence.iter().any(|e| e.contains("consistent across 3")));
    }

    #[test]
    fn oversteer_points_at_the_rear_and_inconsistency_lowers_confidence() {
        let mut overall = base_metrics();
        overall.understeer_index = Some(-0.15);
        let laps = [lap_with_index(-0.20), lap_with_index(0.02)];
        let recs = recommend(&overall, &laps);
        let balance = recs.iter().find(|r| r.area == "balance").unwrap();
        assert_eq!(balance.confidence, Confidence::Medium);
        assert!(balance.advice.contains("rear anti-roll bar"), "{}", balance.advice);
    }

    #[test]
    fn neutral_car_gets_no_balance_advice() {
        let recs = recommend(&base_metrics(), &[]);
        assert!(recs.iter().all(|r| r.area != "balance"));
    }

    #[test]
    fn hot_fronts_defer_to_understeer() {
        let mut overall = base_metrics();
        overall.understeer_index = Some(0.24);
        overall.tire_temp.fl.avg = 245.0;
        overall.tire_temp.fr.avg = 245.0;
        let recs = recommend(&overall, &[]);
        let tires = recs.iter().find(|r| r.area == "tires").unwrap();
        assert_eq!(tires.confidence, Confidence::Low, "scrub heat defers to balance");
        assert!(tires.evidence.iter().any(|e| e.contains("fix balance first")));
    }

    #[test]
    fn cold_tires_suggest_lower_pressure() {
        let mut overall = base_metrics();
        for t in [
            &mut overall.tire_temp.fl,
            &mut overall.tire_temp.fr,
            &mut overall.tire_temp.rl,
            &mut overall.tire_temp.rr,
        ] {
            t.avg = 120.0;
        }
        let recs = recommend(&overall, &[]);
        let tire_recs: Vec<_> = recs.iter().filter(|r| r.area == "tires").collect();
        assert_eq!(tire_recs.len(), 2, "both axles cold");
        assert!(tire_recs.iter().all(|r| r.advice.contains("lower")));
        assert!(tire_recs.iter().all(|r| r.confidence == Confidence::Medium));
    }

    fn band(samples: usize, index: f32) -> crate::analysis::metrics::BandBalance {
        crate::analysis::metrics::BandBalance {
            samples,
            index: Some(index),
            rear_slip: Some(0.6),
        }
    }

    /// Understeer only above 85 mph with neutral low-speed balance = aero, and
    /// the balance rule must stay quiet (overall index diluted below MILD is
    /// not required — here overall is neutral so only aero speaks).
    #[test]
    fn high_speed_only_understeer_fires_aero_not_bars() {
        let mut overall = base_metrics();
        overall.understeer_index = Some(0.04);
        overall.balance_low_speed = band(2000, 0.02);
        overall.balance_high_speed = band(2000, 0.14);
        let recs = recommend(&overall, &[]);
        let aero = recs.iter().find(|r| r.area == "aero").unwrap();
        assert!(aero.advice.contains("add front aero"), "{}", aero.advice);
        assert_eq!(aero.confidence, Confidence::Medium);
        assert_eq!(aero.implied.unwrap().family, Family::FrontAero);
        assert!(recs.iter().all(|r| r.area != "balance"));
    }

    #[test]
    fn high_speed_oversteer_asks_for_rear_wing() {
        let mut overall = base_metrics();
        overall.balance_low_speed = band(2000, -0.01);
        overall.balance_high_speed = band(2000, -0.13);
        let recs = recommend(&overall, &[]);
        let aero = recs.iter().find(|r| r.area == "aero").unwrap();
        assert!(aero.advice.contains("add rear aero"), "{}", aero.advice);
        assert_eq!(aero.implied.unwrap().family, Family::RearAero);
    }

    /// The real tarmac signature (Ford GT / McLaren): understeer at EVERY speed,
    /// stronger below 85 mph. Mechanical — aero must stay quiet and the balance
    /// rec must say the imbalance is concentrated at low speed.
    #[test]
    fn uniform_understeer_is_mechanical_not_aero() {
        let mut overall = base_metrics();
        overall.understeer_index = Some(0.24);
        overall.balance_low_speed = band(9000, 0.37);
        overall.balance_high_speed = band(30000, 0.20);
        let recs = recommend(&overall, &[]);
        assert!(recs.iter().all(|r| r.area != "aero"));
        let balance = recs.iter().find(|r| r.area == "balance").unwrap();
        assert!(
            balance.evidence.iter().any(|e| e.contains("concentrated at low speed")),
            "{:?}",
            balance.evidence
        );
    }

    #[test]
    fn starved_band_stays_quiet() {
        let mut overall = base_metrics();
        overall.balance_low_speed = band(2000, 0.0);
        overall.balance_high_speed = band(100, 0.30); // too few samples to trust
        let recs = recommend(&overall, &[]);
        assert!(recs.iter().all(|r| r.area != "aero"));
    }

    /// Rear breaks away specifically under power (RWD): diff accel advice.
    #[test]
    fn power_oversteer_fires_diff_accel() {
        let mut overall = base_metrics();
        overall.balance_on_throttle = band(2000, -0.12);
        overall.balance_off_throttle = band(2000, 0.02);
        let recs = recommend(&overall, &[]);
        let diff = recs.iter().find(|r| r.area == "differential").unwrap();
        assert!(diff.advice.contains("rear differential acceleration lock"), "{}", diff.advice);
        assert_eq!(diff.implied.unwrap().family, Family::DiffAccel);
        assert!(diff.implied.unwrap().softer);
    }

    /// AWD power understeer: front diff / center torque advice.
    #[test]
    fn power_understeer_on_awd_names_the_front_diff() {
        let mut overall = base_metrics();
        overall.drivetrain_type = 2;
        overall.balance_on_throttle = band(2000, 0.15);
        overall.balance_off_throttle = band(2000, 0.02);
        let recs = recommend(&overall, &[]);
        let diff = recs.iter().find(|r| r.area == "differential").unwrap();
        assert!(diff.advice.contains("front differential"), "{}", diff.advice);
        assert!(diff.advice.contains("center torque rearward"), "{}", diff.advice);
    }

    /// The real dirt pattern (Fiesta/Audi): big on-off shift but on-throttle
    /// index near zero — throttle rotation as technique, not a diff problem.
    #[test]
    fn dirt_throttle_rotation_stays_quiet() {
        let mut overall = base_metrics();
        overall.surface_loose = true;
        overall.balance_on_throttle = band(9000, -0.06);
        overall.balance_off_throttle = band(5000, 0.57);
        let recs = recommend(&overall, &[]);
        assert!(recs.iter().all(|r| r.area != "differential"), "{recs:?}");
    }

    /// Understeer everywhere with a small on-off shift (the healthy tarmac
    /// signature) must NOT read as power understeer — the shift gate protects it.
    #[test]
    fn uniform_understeer_is_not_power_understeer() {
        let mut overall = base_metrics();
        overall.drivetrain_type = 2;
        overall.understeer_index = Some(0.27);
        overall.balance_on_throttle = band(27000, 0.26);
        overall.balance_off_throttle = band(16000, 0.30);
        let recs = recommend(&overall, &[]);
        assert!(recs.iter().all(|r| r.area != "differential"));
    }

    #[test]
    fn rwd_wheelspin_names_the_rear() {
        let mut overall = base_metrics();
        overall.wheelspin_frac = Some(0.2);
        let recs = recommend(&overall, &[]);
        let traction = recs.iter().find(|r| r.area == "traction").unwrap();
        assert_eq!(traction.confidence, Confidence::High);
        assert!(traction.advice.contains("rear-axle"), "{}", traction.advice);
    }

    #[test]
    fn limiter_time_triggers_final_drive_advice() {
        let mut overall = base_metrics();
        overall.gears.limiter_frac = 0.05;
        overall.gears.top_gear = 6;
        overall.gears.top_gear_max_rpm = 7900.0;
        let recs = recommend(&overall, &[]);
        let gearing = recs.iter().find(|r| r.area == "gearing").unwrap();
        assert!(gearing.advice.contains("lengthen the final drive"), "{}", gearing.advice);
    }

    /// The inverse gap caught on real data: no limiter time and the top gear never
    /// gets near redline -> the whole stack is too long, shorten the final drive.
    #[test]
    fn unused_rev_range_suggests_shorter_final_drive() {
        let mut overall = base_metrics();
        overall.gears.limiter_frac = 0.0;
        overall.gears.top_gear = 6;
        overall.gears.top_gear_max_rpm = 6400.0; // 80% of the 8000 redline
        overall.gears.time_frac = vec![(4, 0.5), (5, 0.4), (6, 0.03)];
        let recs = recommend(&overall, &[]);
        let gearing = recs.iter().find(|r| r.area == "gearing").unwrap();
        assert!(gearing.advice.contains("shorten the final drive"), "{}", gearing.advice);
        assert!(gearing.evidence[0].contains("80%"), "{}", gearing.evidence[0]);
    }

    #[test]
    fn healthy_rev_usage_stays_quiet() {
        let mut overall = base_metrics();
        overall.gears.limiter_frac = 0.0;
        overall.gears.top_gear = 6;
        overall.gears.top_gear_max_rpm = 7600.0; // 95% of redline, no clipping
        let recs = recommend(&overall, &[]);
        assert!(recs.iter().all(|r| r.area != "gearing"));
    }

    #[test]
    fn bottoming_names_the_axle() {
        let mut overall = base_metrics();
        overall.suspension.rl.bottomed_frac = 0.06;
        let recs = recommend(&overall, &[]);
        let susp = recs.iter().find(|r| r.area == "suspension").unwrap();
        assert!(susp.advice.contains("rear springs"), "{}", susp.advice);
    }

    fn susp(reversals: f32, topped: f32) -> crate::analysis::metrics::SuspensionStats {
        crate::analysis::metrics::SuspensionStats {
            avg: 0.4,
            bottomed_frac: 0.0,
            topped_frac: topped,
            reversals_per_sec: reversals,
        }
    }

    #[test]
    fn underdamped_axle_gets_more_damping_advice() {
        let mut overall = base_metrics();
        overall.suspension = Corners { fl: susp(7.5, 0.18), fr: susp(7.4, 0.19), rl: susp(5.6, 0.03), rr: susp(5.5, 0.03) };
        let recs = recommend(&overall, &[]);
        let damping: Vec<_> = recs.iter().filter(|r| r.area == "damping").collect();
        assert_eq!(damping.len(), 1, "only the front fires");
        assert!(damping[0].advice.contains("increase front damping"), "{}", damping[0].advice);
        assert_eq!(damping[0].confidence, Confidence::High);
    }

    #[test]
    fn overdamped_axle_gets_low_confidence_softening_advice() {
        let mut overall = base_metrics();
        overall.suspension = Corners { fl: susp(2.1, 0.05), fr: susp(2.3, 0.06), rl: susp(2.0, 0.02), rr: susp(2.2, 0.02) };
        let recs = recommend(&overall, &[]);
        let damping: Vec<_> = recs.iter().filter(|r| r.area == "damping").collect();
        assert_eq!(damping.len(), 2);
        assert!(damping.iter().all(|r| r.confidence == Confidence::Low));
        assert!(damping.iter().all(|r| r.advice.contains("softer")));
    }

    /// Real min-damping-on-dirt numbers must fire underdamped advice (and the
    /// front's 37% topped stays below the 40% strong-overdamped line).
    #[test]
    fn underdamped_dirt_fires_increase_damping() {
        let mut overall = base_metrics();
        overall.surface_loose = true;
        overall.suspension = Corners {
            fl: susp(17.6, 0.37),
            fr: susp(18.7, 0.32),
            rl: susp(19.2, 0.21),
            rr: susp(19.5, 0.20),
        };
        let recs = recommend(&overall, &[]);
        let damping: Vec<_> = recs.iter().filter(|r| r.area == "damping").collect();
        assert_eq!(damping.len(), 2, "{damping:?}");
        assert!(damping.iter().all(|r| r.advice.contains("increase")));
        assert!(damping.iter().all(|r| r.confidence == Confidence::Medium));
    }

    /// Regression from the real dirt captures: healthy dirt reads 12-16 rev/s and
    /// up to ~26% topped — tarmac thresholds must not fire on it.
    #[test]
    fn healthy_dirt_damping_stays_quiet() {
        let mut overall = base_metrics();
        overall.surface_loose = true;
        overall.surface_rumble_avg = 0.13;
        overall.suspension = Corners {
            fl: susp(15.6, 0.26),
            fr: susp(16.2, 0.21),
            rl: susp(11.8, 0.10),
            rr: susp(12.0, 0.09),
        };
        let recs = recommend(&overall, &[]);
        assert!(recs.iter().all(|r| r.area != "damping"), "{recs:?}");
    }

    /// The real overdamped-dirt signature: wheels at full extension most of the
    /// stint fires the strong rule regardless of surface baseline.
    #[test]
    fn extreme_topped_fires_reduce_damping() {
        let mut overall = base_metrics();
        overall.surface_loose = true;
        overall.rpm_flutter = Some(12000.0);
        overall.suspension = Corners {
            fl: susp(2.6, 0.84),
            fr: susp(3.6, 0.74),
            rl: susp(2.9, 0.77),
            rr: susp(3.1, 0.66),
        };
        let recs = recommend(&overall, &[]);
        let damping: Vec<_> = recs.iter().filter(|r| r.area == "damping").collect();
        assert_eq!(damping.len(), 2);
        assert!(damping.iter().all(|r| r.confidence == Confidence::High));
        assert!(damping.iter().all(|r| r.advice.contains("reduce")));
        assert!(damping[0].evidence.iter().any(|e| e.contains("rpm flutter")));
    }

    #[test]
    fn healthy_damping_stays_quiet() {
        let mut overall = base_metrics();
        overall.suspension = Corners { fl: susp(5.8, 0.03), fr: susp(5.6, 0.03), rl: susp(5.9, 0.01), rr: susp(5.5, 0.01) };
        let recs = recommend(&overall, &[]);
        assert!(recs.iter().all(|r| r.area != "damping"));
    }

    #[test]
    fn quiet_session_produces_no_advice() {
        let recs = recommend(&base_metrics(), &[]);
        assert!(recs.is_empty(), "nothing wrong -> nothing to say: {recs:?}");
    }
}
