//! Turning raw recommendations into setup-aware advice: current-tune
//! absolutes, exhausted-direction flips, and the cross-campaign map prior.

use super::*;

pub(super) fn family_keys(family: journal::Family) -> &'static [&'static str] {
    match family {
        journal::Family::FrontRoll => &["arb_f", "springs_f"],
        journal::Family::RearRoll => &["arb_r", "springs_r"],
        journal::Family::Gearing => &["final_drive"],
        journal::Family::FrontAero => &["aero_f"],
        journal::Family::RearAero => &["aero_r"],
        journal::Family::DiffAccel => &["diff_accel_f", "diff_accel_r"],
        journal::Family::DiffDecel => &["diff_decel_f", "diff_decel_r"],
        journal::Family::CenterDiff => &["diff_center"],
        journal::Family::Brakes => &["brake_balance", "brake_pressure"],
        journal::Family::Damping => &["rebound_f", "rebound_r", "bump_f", "bump_r"],
        journal::Family::TirePressure => &["tire_pressure_f", "tire_pressure_r"],
        journal::Family::Alignment => &["camber_f", "camber_r", "toe_f", "toe_r", "caster"],
        journal::Family::RideHeight => &["ride_height_f", "ride_height_r"],
    }
}

/// When a family's advised direction is exhausted (all sliders pinned at the
/// advised bound), the other end of the car often offers the same balance
/// change from the opposite side. Returns (partner family, partner direction,
/// replacement advice).
pub(super) fn exhausted_flip(
    family: journal::Family,
    softer: bool,
) -> Option<(journal::Family, bool, &'static str)> {
    use journal::Family as F;
    let (partner, text): (F, &str) = match (family, softer) {
        (F::FrontRoll, true) => (
            F::RearRoll,
            "front roll sliders are at minimum; stiffen the rear instead",
        ),
        (F::FrontRoll, false) => (
            F::RearRoll,
            "front roll sliders are at maximum; soften the rear instead",
        ),
        (F::RearRoll, true) => (
            F::FrontRoll,
            "rear roll sliders are at minimum; stiffen the front instead",
        ),
        (F::RearRoll, false) => (
            F::FrontRoll,
            "rear roll sliders are at maximum; soften the front instead",
        ),
        (F::FrontAero, false) => (
            F::RearAero,
            "front aero is at maximum; reduce rear aero instead",
        ),
        (F::FrontAero, true) => (
            F::RearAero,
            "front aero is at minimum; increase rear aero instead",
        ),
        (F::RearAero, false) => (
            F::FrontAero,
            "rear aero is at maximum; reduce front aero instead",
        ),
        (F::RearAero, true) => (
            F::FrontAero,
            "rear aero is at minimum; increase front aero instead",
        ),
        _ => return None,
    };
    Some((partner, !softer, text))
}

/// The flip target's own action phrase, for advice rewritten because the
/// original family is not adjustable on this build (vs exhausted_flip's
/// at-the-bound wording).
fn flip_action(partner: journal::Family, softer: bool) -> &'static str {
    use journal::Family as F;
    match (partner, softer) {
        (F::FrontRoll, true) => "soften front ARB or springs instead",
        (F::FrontRoll, false) => "stiffen front ARB or springs instead",
        (F::RearRoll, true) => "soften rear ARB or springs instead",
        (F::RearRoll, false) => "stiffen the rear ARB or springs instead",
        (F::FrontAero, true) => "reduce front aero instead",
        (F::FrontAero, false) => "increase front aero instead",
        (F::RearAero, true) => "reduce rear aero instead",
        (F::RearAero, false) => "increase rear aero instead",
        _ => "work the other end of the car instead",
    }
}

/// Attach current-tune absolutes (with slider headroom when limits are on
/// file) to family-matched recommendations and build the display list of the
/// latest revision. Advice whose direction is exhausted flips to the partner
/// end of the car, or is dropped when no tunable partner exists; advice for
/// a family the baseline tune omits entirely (the upgrade isn't fitted, so
/// the game has no such sliders) is redirected or dropped the same way —
/// never emitted as-is.
pub(super) fn enrich_with_tune(
    recs: &mut Vec<recommend::Recommendation>,
    session: &TuningSession,
) -> Vec<(String, String, Option<&'static str>)> {
    let Some(rev) = session.latest() else {
        return Vec::new();
    };
    let tunable = |f: journal::Family| family_keys(f).iter().any(|k| rev.values.contains_key(*k));
    // Non-tunable gate: tune entry is mandatory and the baseline grid is
    // complete, so a whole family group absent from the baseline means the
    // car cannot adjust it. The gate reads the implied family, or the rec's
    // area for advice without a direction claim (damping advice on a car
    // with no damper adjustment is equally impossible).
    recs.retain_mut(|r| {
        let Some(family) = r
            .implied
            .map(|i| i.family)
            .or_else(|| journal::family_for_area(r.area))
        else {
            return true;
        };
        if tunable(family) {
            return true;
        }
        let Some(implied) = r.implied else {
            return false;
        };
        let Some((pf, ps, _)) = exhausted_flip(family, implied.softer) else {
            return false;
        };
        if !tunable(pf) {
            return false;
        }
        r.evidence.push(format!(
            "the {} group is not in the baseline tune (not adjustable on this \
             build); advice redirected (was: {})",
            journal::family_key(family),
            r.advice,
        ));
        r.advice = format!(
            "no {} adjustment on this build; {}",
            journal::family_key(family),
            flip_action(pf, ps),
        );
        r.implied = Some(journal::Change {
            family: pf,
            softer: ps,
            magnitude: None,
        });
        r.suggestion = None;
        r.apply.clear();
        true
    });
    // Slider-limit gate: a family whose every present slider sits at the
    // advised bound cannot move that way. Flip to the partner end when one
    // exists, is tunable, and has headroom of its own; otherwise the rec is
    // dropped — "reduce X" with X at minimum must never be emitted.
    let all_pinned = |family: journal::Family, softer: bool| {
        let (mut present, mut with_limit, mut pinned_n) = (0usize, 0usize, 0usize);
        for k in family_keys(family) {
            let Some(v) = rev.values.get(*k) else {
                continue;
            };
            present += 1;
            if let (Ok(val), Some(lim)) = (
                v.parse::<f32>(),
                crate::advice::tuning::limit_of(&session.facts, k),
            ) {
                with_limit += 1;
                if crate::advice::tuning::pinned(val, lim, softer, k) {
                    pinned_n += 1;
                }
            }
        }
        with_limit > 0 && with_limit == present && pinned_n == with_limit
    };
    recs.retain_mut(|r| {
        let Some(implied) = r.implied else {
            return true;
        };
        let keys = family_keys(implied.family);
        let mut known = Vec::new();
        let mut with_limit = 0usize;
        let mut pinned = 0usize;
        let mut primary_pinned = false;
        for (idx, k) in keys.iter().enumerate() {
            let Some(v) = rev.values.get(*k) else {
                continue;
            };
            let mut line = format!(
                "{} = {}",
                crate::advice::tuning::field_phrase(k),
                crate::advice::tuning::display_value(k, v, &session.facts),
            );
            if let (Ok(val), Some(lim)) = (
                v.parse::<f32>(),
                crate::advice::tuning::limit_of(&session.facts, k),
            ) {
                with_limit += 1;
                line.push_str(&format!(
                    " (range {}..{})",
                    crate::advice::tuning::display_value(k, &lim.0.to_string(), &session.facts),
                    crate::advice::tuning::display_value(k, &lim.1.to_string(), &session.facts),
                ));
                if crate::advice::tuning::pinned(val, lim, implied.softer, k) {
                    pinned += 1;
                    primary_pinned |= idx == 0;
                    line.push_str(if implied.softer {
                        " AT MINIMUM"
                    } else {
                        " AT MAXIMUM"
                    });
                }
            }
            known.push(line);
        }
        if !known.is_empty() {
            r.evidence
                .push(format!("current setting: {}", known.join(", ")));
        }
        // Exhausted = every slider of the family has a known limit and sits
        // at the advised bound. Unknown limits never claim exhaustion.
        if with_limit > 0 && with_limit == known.len() && pinned == with_limit {
            let flip = exhausted_flip(implied.family, implied.softer)
                .filter(|(pf, ps, _)| tunable(*pf) && !all_pinned(*pf, *ps));
            let Some((pf, ps, text)) = flip else {
                // No partner (or the partner is itself untunable/pinned):
                // the advice is impossible to follow, so it is not advice.
                return false;
            };
            r.evidence
                .push(format!("advised direction exhausted (was: {})", r.advice));
            r.advice = text.to_string();
            r.implied = Some(journal::Change {
                family: pf,
                softer: ps,
                magnitude: None,
            });
            // Any concrete value suggested for the exhausted end no
            // longer applies to the rewritten advice.
            r.suggestion = None;
            r.apply.clear();
        } else if primary_pinned && keys.len() > 1 {
            r.evidence.push(format!(
                "{} is at its bound; work with {}",
                crate::advice::tuning::field_phrase(keys[0]),
                keys[1..]
                    .iter()
                    .map(|k| crate::advice::tuning::field_phrase(k))
                    .collect::<Vec<_>>()
                    .join(" / "),
            ));
        }
        true
    });
    rev.values
        .iter()
        .map(|(k, v)| {
            (
                crate::advice::tuning::field_phrase(k).to_string(),
                crate::advice::tuning::display_value(k, v, &session.facts),
                None,
            )
        })
        .collect()
}

/// Resolve the drag model's final-drive scale ("ideal ≈ current × N") into a
/// concrete caveated target on the gearing rec. Runs AFTER enrich_with_tune
/// so non-tunable/exhausted rewrites have already cleared what no longer
/// applies, and only decorates a rec that carries no suggestion of its own:
/// measured paths (vertex, probe, return-to-best) always outrank a model
/// estimate. The direction must agree (scale > 1 = shorten = higher number).
pub(super) fn apply_fd_scale(
    recs: &mut [recommend::Recommendation],
    session: &TuningSession,
    scale: Option<f32>,
) {
    let Some(scale) = scale else { return };
    let Some(cur) = session
        .latest()
        .and_then(|rev| rev.values.get("final_drive"))
        .and_then(|v| v.parse::<f32>().ok())
    else {
        return;
    };
    let mut target = cur * scale;
    if let Some((lo, hi)) = crate::advice::tuning::limit_of(&session.facts, "final_drive") {
        target = target.clamp(lo, hi);
    }
    let target = (target * 100.0).round() / 100.0;
    if (target - cur).abs() < 0.005 {
        return;
    }
    for r in recs {
        let Some(implied) = r.implied else { continue };
        if implied.family != journal::Family::Gearing
            || r.suggestion.is_some()
            || !r.apply.is_empty()
            || (scale > 1.0) == implied.softer
        {
            continue;
        }
        r.suggestion = Some(format!(
            "final drive {cur} → {target} (drag-model estimate is rough; a \
             driven step will refine it)"
        ));
        r.apply = vec![("final_drive".to_string(), target.to_string())];
        return;
    }
}

/// One Low-confidence suggestion from the cross-campaign effect map (built
/// by `tuners map`): the best grounded, context-matched cell whose pooled
/// behavioural movement aligns with the pace trends, for a family without
/// trustworthy local evidence. Graded gating: a family with any NON-WEAK
/// local measurement is owned by that evidence; weak-only local evidence
/// tempers the prior (quoted) instead of silencing it. A cell must also be
/// a distribution, not an anecdote: one attributed clause from one other
/// car (n=1, no direct A/B) never carries a suggestion. None = the map is
/// silent.
pub(super) fn map_prior(
    emap: &crate::advice::effectmap::EffectMap,
    trends: &[crate::advice::effectmap::PaceTrend],
    ctx: &crate::advice::effectmap::MapContext,
    measurements: &[Measurement],
    recs: &[recommend::Recommendation],
    baseline: Option<&crate::advice::tuning::Revision>,
    facts: &std::collections::BTreeMap<String, String>,
) -> Option<recommend::Recommendation> {
    let cells = crate::advice::effectmap::aggregate(emap);
    let ranked = crate::advice::effectmap::rank(&cells, trends, ctx);
    if std::env::var_os("TUNERS_MAP_TRACE").is_some() {
        for (score, cell) in &ranked {
            eprintln!(
                "  ranked: {} softer={} score={score:+.2} n={}",
                cell.family, cell.softer, cell.n
            );
        }
    }
    let (cell, family) = ranked.into_iter().find_map(|(score, cell)| {
        let family = journal::family_for_area(&cell.family)?;
        let grounded = cell.n >= 2 || cell.direct_n >= 1;
        let tried = measurements
            .iter()
            .any(|m| m.change.family == family && !m.weak);
        let advised = recs
            .iter()
            .any(|r| r.implied.is_some_and(|i| i.family == family));
        // A map cell for a family this build cannot adjust (whole group
        // absent from the baseline) is not a suggestible experiment here.
        let tunable = baseline.is_none_or(|rev| {
            family_keys(family)
                .iter()
                .any(|k| rev.values.contains_key(*k))
        });
        (grounded && tunable && !tried && !advised && score >= 1.0).then_some((cell, family))
    })?;
    let phrase = crate::advice::effectmap::direction_phrase(&cell.family, cell.softer);
    let movers: effects::Effects = cell
        .fields
        .iter()
        .filter(|(k, _, m, _)| m.abs() >= effects::noise_floor(k))
        .map(|(k, _, m, _)| (*k, *m))
        .collect();
    // Quote only the trends this cell's movement actually matches: the
    // intersection is the case for the suggestion.
    let trend_desc: Vec<String> = trends
        .iter()
        .filter(|t| movers.iter().any(|(k, _)| *k == t.key))
        .map(|t| {
            format!(
                "faster stints moved {} {} (r {:+.2}, {} pairs{})",
                effects::label(t.key),
                if t.r > 0.0 { "down" } else { "up" },
                t.r,
                t.n,
                if t.history { " across other cars" } else { "" },
            )
        })
        .collect();
    // Weak-only local evidence tempers rather than silences: say so.
    let weak_local = measurements
        .iter()
        .find(|m| m.change.family == family && m.weak)
        .map(|m| {
            format!(
                "local evidence exists but is weak (\"{}\", {}); \
                 the prior stands until a trustworthy measurement lands",
                crate::advice::tuning::display_note(&m.desc, facts),
                m.outcome.word(),
            )
        });
    Some(recommend::Recommendation {
        apply: Vec::new(),
        area: journal::family_area(family),
        suggestion: None,
        advice: format!(
            "untried so far. Similar builds found {phrase} moved pace in \
            your favour. Worth a probe to explore."
        ),
        evidence: {
            let mut ev = vec![
                format!("pace trend: {}", trend_desc.join("; ")),
                format!(
                    "effect map ({} {}{}): {phrase} over n={} ({} direct{}) read {}; \
                     measured {:+.2}s ±{:.2} there",
                    if cell.surface_loose { "dirt" } else { "tarmac" },
                    crate::telemetry::packet::drivetrain_name(cell.drivetrain),
                    match cell.aero {
                        Some(true) => " aero",
                        Some(false) => " no-aero",
                        None => "",
                    },
                    cell.n,
                    cell.direct_n,
                    if cell.own_n < cell.n {
                        format!(", {} yours", cell.own_n)
                    } else {
                        String::new()
                    },
                    if movers.is_empty() {
                        "no above-floor movement".to_string()
                    } else {
                        effects::describe(&movers)
                    },
                    cell.delta_mean,
                    cell.delta_sd,
                ),
            ];
            ev.extend(weak_local);
            ev
        },
        confidence: recommend::Confidence::Low,
        probe: false,
        implied: Some(journal::Change {
            family,
            softer: cell.softer,
            magnitude: None,
        }),
    })
}

/// Setup-lint tier (plan 016): conventions read from the TUNE STATE itself,
/// phrased as convention ("commonly run..."), never as measurement. Each lint
/// defers to measured evidence on its family — a non-weak campaign
/// measurement or an existing recommendation on the family silences it.
/// Emitted BEFORE enrich_with_tune so the non-tunable gate and current-value
/// enrichment apply to lints like any other advice.
pub(super) fn setup_lints(
    session: &TuningSession,
    measurements: &[Measurement],
    recs: &[recommend::Recommendation],
    met: Option<&crate::analysis::metrics::StintMetrics>,
) -> Vec<recommend::Recommendation> {
    // Community consensus (2026-08-01 damping research, plan 016): bump
    // commonly 40-70% of rebound per end, "about two-thirds".
    const BUMP_OF_REBOUND_LO: f32 = 0.40;
    const BUMP_OF_REBOUND_HI: f32 = 0.70;
    /// Front-share distance from 50/50 both splits must show before the
    /// mirror lint calls them contradictory (guards ~equal splits).
    const SPLIT_MARGIN: f32 = 0.02;
    /// Bottoming below this (per wheel, landing-excluded) counts as "no
    /// bottoming" for the ride-height lint.
    const BOTTOM_FREE_FRAC: f32 = 0.005;

    let Some(rev) = session.latest() else {
        return Vec::new();
    };
    let val = |k: &str| rev.values.get(k)?.parse::<f32>().ok();
    let covered = |fam: journal::Family| {
        measurements
            .iter()
            .any(|m| m.change.family == fam && !m.weak)
            || recs.iter().any(|r| {
                r.implied.is_some_and(|i| i.family == fam)
                    || journal::family_for_area(r.area) == Some(fam)
                    || r.area == journal::family_area(fam)
            })
    };
    let mut out = Vec::new();

    // 1. Bump/rebound ratio per end.
    if !covered(journal::Family::Damping) {
        let mut off = Vec::new();
        for (end, bump_k, reb_k) in [
            ("front", "bump_f", "rebound_f"),
            ("rear", "bump_r", "rebound_r"),
        ] {
            let (Some(b), Some(r)) = (val(bump_k), val(reb_k)) else {
                continue;
            };
            if r <= 0.0 {
                continue;
            }
            let ratio = b / r;
            if !(BUMP_OF_REBOUND_LO..=BUMP_OF_REBOUND_HI).contains(&ratio) {
                off.push(format!(
                    "{end} bump {b} is {:.0}% of {end} rebound {r}",
                    ratio * 100.0
                ));
            }
        }
        if !off.is_empty() {
            out.push(recommend::Recommendation {
                apply: Vec::new(),
                area: "damping",
                suggestion: None,
                advice: "bump/rebound split is outside typical band. Bump is commonly \
                         40-70% of rebound. Worth trying, unless this is deliberate."
                    .into(),
                evidence: off
                    .into_iter()
                    .map(|s| format!("{s} (convention, not a measurement)"))
                    .collect(),
                confidence: recommend::Confidence::Low,
                probe: false,
                implied: None,
            });
        }
    }

    // 2. Dampers mirror springs: fires on INTERNAL inconsistency only (the
    // stiffer-sprung end carrying the softer dampers), never on degree.
    if !covered(journal::Family::Damping)
        && let (Some(sf), Some(sr)) = (val("springs_f"), val("springs_r"))
        && let (Some(rf), Some(rr), Some(bf), Some(br)) = (
            val("rebound_f"),
            val("rebound_r"),
            val("bump_f"),
            val("bump_r"),
        )
        && sf + sr > 0.0
        && rf + rr + bf + br > 0.0
    {
        let s_share = sf / (sf + sr);
        let d_share = (rf + bf) / (rf + rr + bf + br);
        if (s_share - 0.5).abs() > SPLIT_MARGIN
            && (d_share - 0.5).abs() > SPLIT_MARGIN
            && (s_share - 0.5).signum() != (d_share - 0.5).signum()
        {
            let (stiff, soft) = if s_share > 0.5 {
                ("front", "rear")
            } else {
                ("rear", "front")
            };
            out.push(recommend::Recommendation {
                apply: Vec::new(),
                area: "damping",
                suggestion: None,
                advice: format!(
                    "the {stiff} end has the stiffer springs but the {soft} \
                     end has the stiffer dampers. The damper split commonly \
                     mirrors the spring split. Worth aligning, unless \
                     deliberate."
                ),
                evidence: vec![format!(
                    "front holds {:.0}% of spring rate but {:.0}% of damper \
                     stiffness (convention, not a measurement)",
                    s_share * 100.0,
                    d_share * 100.0,
                )],
                confidence: recommend::Confidence::Low,
                probe: false,
                implied: None,
            });
        }
    }

    // 3. Ride height above minimum with zero measured bottoming (tarmac):
    // the missing LOWER side of the bottoming rule. Deliberately lint, not a
    // blind rule — it needs the current values plus limit facts, and a
    // telemetry-only version would nag on nearly every stint forever. Only
    // fires with recorded limit facts (ride height has no universal range).
    if !covered(journal::Family::RideHeight)
        && let Some(m) = met
        && !m.surface_loose
        && [
            m.suspension.fl.bottomed_frac,
            m.suspension.fr.bottomed_frac,
            m.suspension.rl.bottomed_frac,
            m.suspension.rr.bottomed_frac,
        ]
        .iter()
        .all(|f| *f < BOTTOM_FREE_FRAC)
    {
        let mut headroom = Vec::new();
        for k in ["ride_height_f", "ride_height_r"] {
            let (Some(v), Some((min, _))) =
                (val(k), crate::advice::tuning::limit_of(&session.facts, k))
            else {
                continue;
            };
            if v - min >= 0.05 {
                headroom.push(format!(
                    "{} = {} with slider minimum {}",
                    crate::advice::tuning::field_phrase(k),
                    crate::advice::tuning::display_value(k, &v.to_string(), &session.facts),
                    crate::advice::tuning::display_value(k, &min.to_string(), &session.facts),
                ));
            }
        }
        if !headroom.is_empty() {
            let mut evidence = headroom;
            evidence.push(
                "no non-landing bottoming out measured this stint; \
                 convention, not a measurement"
                    .into(),
            );
            out.push(recommend::Recommendation {
                apply: Vec::new(),
                area: "ride height",
                suggestion: None,
                advice: "ride height is above minimum, but the car isn't bottoming \
                         out. Lowering centre of mass is typically free speed. Try \
                         lowering, unless deliberate."
                    .into(),
                evidence,
                confidence: recommend::Confidence::Low,
                probe: false,
                implied: Some(journal::Change {
                    family: journal::Family::RideHeight,
                    softer: true,
                    magnitude: None,
                }),
            });
        }
    }

    out
}
