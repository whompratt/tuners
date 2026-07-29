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
        journal::Family::DiffAccel => &["diff_accel_f", "diff_accel_r", "diff_center"],
        journal::Family::DiffDecel => &["diff_decel_f", "diff_decel_r"],
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
            "front roll sliders are at minimum; stiffen the rear instead (rear anti-roll bar first)",
        ),
        (F::FrontRoll, false) => (
            F::RearRoll,
            "front roll sliders are at maximum; soften the rear instead",
        ),
        (F::RearRoll, true) => (
            F::FrontRoll,
            "rear roll sliders are at minimum; stiffen the front instead (front anti-roll bar first)",
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
            "front aero is at minimum; add rear aero instead",
        ),
        (F::RearAero, false) => (
            F::FrontAero,
            "rear aero is at maximum; reduce front aero instead",
        ),
        (F::RearAero, true) => (
            F::FrontAero,
            "rear aero is at minimum; add front aero instead",
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
        (F::FrontRoll, true) => "soften the front (anti-roll bar or springs) instead",
        (F::FrontRoll, false) => "stiffen the front (anti-roll bar or springs) instead",
        (F::RearRoll, true) => "soften the rear (anti-roll bar or springs) instead",
        (F::RearRoll, false) => "stiffen the rear (anti-roll bar or springs) instead",
        (F::FrontAero, true) => "reduce front aero instead",
        (F::FrontAero, false) => "add front aero instead",
        (F::RearAero, true) => "reduce rear aero instead",
        (F::RearAero, false) => "add rear aero instead",
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
    let dir = crate::advice::effectmap::direction_word(&cell.family, cell.softer);
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
                if t.history {
                    " across your other cars"
                } else {
                    ""
                },
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
                m.desc,
                m.outcome.word(),
            )
        });
    Some(recommend::Recommendation {
        apply: Vec::new(),
        area: journal::family_area(family),
        suggestion: None,
        advice: format!(
            "untried this campaign: on similar builds, {} {} moved the \
             behaviours your pace has tracked; worth one probing step \
             (map prior, not a measurement)",
            cell.family, dir,
        ),
        evidence: {
            let mut ev = vec![
                format!("pace trend: {}", trend_desc.join("; ")),
                format!(
                    "effect map ({} {}{}): {} {} over n={} ({} direct{}) read {}; \
                     measured {:+.2}s ±{:.2} there",
                    if cell.surface_loose { "dirt" } else { "tarmac" },
                    crate::telemetry::packet::drivetrain_name(cell.drivetrain),
                    match cell.aero {
                        Some(true) => " aero",
                        Some(false) => " no-aero",
                        None => "",
                    },
                    cell.family,
                    dir,
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
        implied: Some(journal::Change {
            family,
            softer: cell.softer,
            magnitude: None,
        }),
    })
}
