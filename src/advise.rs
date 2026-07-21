//! The advise engine, shared by the CLI (`tuners advise`) and the dashboard
//! (`/api/advise`): journal trajectory with measured step outcomes, blind
//! recommendations reconciled with the last step, and current-tune enrichment.
//! With no journal yet (a session's first stint), falls back to blind
//! recommendations on the latest stint of the session car — the journal
//! starts with the first tune change.

use crate::analysis::{self, journal};
use crate::tuning::TuningSession;
use std::path::Path;

pub struct StepView {
    pub path: String,
    pub laps: usize,
    pub best_s: f32,
    pub ideal_s: f32,
    /// (understeer index, front slip frac, rear slip frac).
    pub balance: Option<(f32, f32, f32)>,
    pub note: Option<String>,
    /// Slider positions relative to baseline, when the note trail supports them.
    pub pos: Option<(f32, f32)>,
    /// Measured outcome vs the previous step: Ok((word, ideal delta, unequal
    /// laps)) or Err(reason) when not comparable. None for the first step.
    pub outcome: Option<Result<(&'static str, f32, bool), String>>,
    /// Where the time moved vs the previous step: (corner entry, corner exit,
    /// straights). Corner total = entry + exit.
    pub split: Option<(f32, f32, f32)>,
}

/// Drift-corrected reading of a trailing excursion-and-revert pair: the two
/// deltas around a net-zero setup change decompose into the excursion's true
/// cost and the driver/track drift both stints share.
pub struct AbaView {
    /// Areas the excursion touched ("differential", "balance+gearing", ...).
    pub families: String,
    /// Ideal-lap cost of the excursion with drift cancelled (positive = the
    /// excursion was slower).
    pub effect_s: f32,
    /// Per-stint drift over the pair — the noise floor for outcome margins.
    pub drift_s: f32,
}

/// The honest comparison for the last stint: the prior stint whose SETUP
/// STATE differs least, which for chained compound steps ("revert X; try Y")
/// is usually the shared baseline, not the chronological neighbor. Steps
/// record deltas; comparisons should be between states.
pub struct AnchorView {
    /// 1-based trajectory index of the anchor stint.
    pub vs_step: usize,
    /// Experiment areas the setups differ in ("damping"); empty = same setup
    /// (the delta is pure driver/track drift).
    pub areas: String,
    /// Human description of the setup difference ("front rebound +12.2; ...").
    pub changes: String,
    /// Ideal-lap delta anchor -> last (positive = last is slower).
    pub delta_s: f32,
    pub word: &'static str,
    /// Single-flying-lap comparison on either side.
    pub weak: bool,
    /// Whether this comparison drove reconciliation (single-area anchors do;
    /// multi-area anchors are informational).
    pub reconciled: bool,
    /// Where the time moved vs the anchor: (entry, exit, straights).
    pub split: (f32, f32, f32),
}

pub struct AdviseView {
    /// Journal file the trajectory came from; None = blind fallback (no journal).
    pub journal: Option<String>,
    pub steps: Vec<StepView>,
    /// Setup-state comparison for the last stint (see AnchorView).
    pub anchor: Option<AnchorView>,
    /// Present when the last two steps form an A-B-A (see AbaView).
    pub aba: Option<AbaView>,
    /// Journaled stint with no completed laps yet (still recording): excluded
    /// from the trajectory, advice targets the previous stint meanwhile.
    pub in_progress: Option<String>,
    /// Stint the recommendations are for.
    pub advice_for: String,
    pub recommendations: Vec<analysis::recommend::Recommendation>,
    /// Latest tune revision as (phrase, value, canonical unit), for display.
    pub current_tune: Vec<(String, String, Option<&'static str>)>,
}

fn stint_balance(stint: &analysis::Stint) -> Option<(f32, f32, f32)> {
    let segments = analysis::driving_segments(&stint.frames, 5.0);
    let longest = segments.iter().max_by_key(|s| s.len())?;
    let m = analysis::metrics::stint_metrics(longest);
    Some((m.understeer_index?, m.cornering_front_slip?, m.cornering_rear_slip?))
}

fn blind_recommendations(
    stint: &analysis::Stint,
    path: &str,
) -> Result<Vec<analysis::recommend::Recommendation>, String> {
    let segments = analysis::driving_segments(&stint.frames, 5.0);
    let longest = segments
        .iter()
        .max_by_key(|s| s.len())
        .ok_or_else(|| format!("{path}: no driving stints of 5s or longer"))?;
    let overall = analysis::metrics::stint_metrics(longest);
    let per_lap: Vec<_> = analysis::split_laps(longest)
        .iter()
        .filter(|l| l.time_s.is_some() && !l.standing_start)
        .map(|l| analysis::metrics::stint_metrics(l.frames))
        .collect();
    Ok(analysis::recommend::recommend(&overall, &per_lap))
}

fn family_keys(family: journal::Family) -> &'static [&'static str] {
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
    }
}

/// When a family's advised direction is exhausted (all sliders pinned at the
/// advised bound), the other end of the car often offers the same balance
/// change from the opposite side. Returns (partner family, partner direction,
/// replacement advice).
fn exhausted_flip(
    family: journal::Family,
    softer: bool,
) -> Option<(journal::Family, bool, &'static str)> {
    use journal::Family as F;
    let (partner, text): (F, &str) = match (family, softer) {
        (F::FrontRoll, true) => (F::RearRoll, "front roll sliders are at minimum — stiffen the rear instead (rear anti-roll bar first)"),
        (F::FrontRoll, false) => (F::RearRoll, "front roll sliders are at maximum — soften the rear instead"),
        (F::RearRoll, true) => (F::FrontRoll, "rear roll sliders are at minimum — stiffen the front instead (front anti-roll bar first)"),
        (F::RearRoll, false) => (F::FrontRoll, "rear roll sliders are at maximum — soften the front instead"),
        (F::FrontAero, false) => (F::RearAero, "front aero is at maximum — reduce rear aero instead"),
        (F::FrontAero, true) => (F::RearAero, "front aero is at minimum — add rear aero instead"),
        (F::RearAero, false) => (F::FrontAero, "rear aero is at maximum — reduce front aero instead"),
        (F::RearAero, true) => (F::FrontAero, "rear aero is at minimum — add front aero instead"),
        _ => return None,
    };
    Some((partner, !softer, text))
}

/// Attach current-tune absolutes (with slider headroom when limits are on
/// file) to family-matched recommendations and build the display list of the
/// latest revision. Advice whose direction is exhausted flips to the partner
/// end of the car, or is downgraded when no partner exists.
fn enrich_with_tune(
    recs: &mut [analysis::recommend::Recommendation],
    session: &TuningSession,
) -> Vec<(String, String, Option<&'static str>)> {
    let Some(rev) = session.latest() else { return Vec::new() };
    for r in recs.iter_mut() {
        let Some(implied) = r.implied else { continue };
        let keys = family_keys(implied.family);
        let mut known = Vec::new();
        let mut with_limit = 0usize;
        let mut pinned = 0usize;
        let mut primary_pinned = false;
        for (idx, k) in keys.iter().enumerate() {
            let Some(v) = rev.values.get(*k) else { continue };
            let mut line = format!(
                "{} = {}",
                crate::tuning::field_phrase(k),
                crate::tuning::display_value(k, v, &session.facts),
            );
            if let (Ok(val), Some(lim)) =
                (v.parse::<f32>(), crate::tuning::limit_of(&session.facts, k))
            {
                with_limit += 1;
                line.push_str(&format!(
                    " (range {}..{})",
                    crate::tuning::display_value(k, &lim.0.to_string(), &session.facts),
                    crate::tuning::display_value(k, &lim.1.to_string(), &session.facts),
                ));
                if crate::tuning::pinned(val, lim, implied.softer, k) {
                    pinned += 1;
                    primary_pinned |= idx == 0;
                    line.push_str(if implied.softer { " AT MINIMUM" } else { " AT MAXIMUM" });
                }
            }
            known.push(line);
        }
        if !known.is_empty() {
            r.evidence.push(format!("current setting: {}", known.join(", ")));
        }
        // Exhausted = every slider of the family has a known limit and sits
        // at the advised bound. Unknown limits never claim exhaustion.
        if with_limit > 0 && with_limit == known.len() && pinned == with_limit {
            if let Some((pf, ps, text)) = exhausted_flip(implied.family, implied.softer) {
                r.evidence.push(format!("advised direction exhausted (was: {})", r.advice));
                r.advice = text.to_string();
                r.implied =
                    Some(journal::Change { family: pf, softer: ps, magnitude: None });
            } else {
                r.evidence.push(
                    "every slider on this channel is already at the advised bound — \
                     direction exhausted"
                        .into(),
                );
                r.confidence = analysis::recommend::Confidence::Low;
            }
        } else if primary_pinned && keys.len() > 1 {
            r.evidence.push(format!(
                "{} is at its bound — work with {}",
                crate::tuning::field_phrase(keys[0]),
                keys[1..]
                    .iter()
                    .map(|k| crate::tuning::field_phrase(k))
                    .collect::<Vec<_>>()
                    .join(" / "),
            ));
        }
    }
    rev.values
        .iter()
        .map(|(k, v)| {
            (
                crate::tuning::field_phrase(k).to_string(),
                crate::tuning::display_value(k, v, &session.facts),
                None,
            )
        })
        .collect()
}

/// Trailing "YYYYMMDD-HHMMSS" stamp of a stint filename, comparable with
/// tune revision stamps (same fixed format, so string order = time order).
fn stint_stamp(path: &str) -> Option<&str> {
    let name = Path::new(path).file_stem()?.to_str()?;
    let stamp = name.get(name.len().checked_sub(15)?..)?;
    (stamp.as_bytes()[8] == b'-'
        && stamp.bytes().enumerate().all(|(i, b)| i == 8 || b.is_ascii_digit()))
    .then_some(stamp)
}

/// Newest stint recording in `dir` whose first driving frame matches `car`
/// (any car when None).
pub fn latest_stint_for_car(dir: &str, car: Option<i32>) -> Option<String> {
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "ftel"))
        .collect();
    paths.sort();
    paths.iter().rev().find_map(|p| {
        let matches = match car {
            None => true,
            Some(car) => crate::serve::stint_car(p) == Some(car),
        };
        matches.then(|| p.to_string_lossy().into_owned())
    })
}

/// Full advise: journal trajectory when one exists, blind fallback otherwise.
pub fn advise(
    journal_path: &str,
    session_path: &Path,
    stints_dir: &str,
) -> Result<AdviseView, String> {
    let session = TuningSession::load(session_path);
    let text = std::fs::read_to_string(journal_path).unwrap_or_default();
    let entries = journal::parse_journal(&text);

    if entries.is_empty() {
        // No journal yet: blind advice on the session car's latest stint.
        let path = latest_stint_for_car(stints_dir, session.car)
            .ok_or("no stints recorded yet — drive first")?;
        let stint =
            analysis::Stint::load(path.as_ref()).map_err(|e| format!("{path}: {e}"))?;
        let mut recs = blind_recommendations(&stint, &path)?;
        let current_tune = enrich_with_tune(&mut recs, &session);
        return Ok(AdviseView {
            journal: None,
            steps: Vec::new(),
            anchor: None,
            aba: None,
            in_progress: None,
            advice_for: path,
            recommendations: recs,
            current_tune,
        });
    }

    // Load and profile every stint, in journal (chronological) order. The
    // LAST entry may still be recording (journaled at the tune save, no
    // completed laps yet): drop it gracefully and advise on the prefix. A
    // middle entry failing is real data trouble and stays a hard error.
    let mut loaded = Vec::new();
    let mut in_progress = None;
    for (i, entry) in entries.iter().enumerate() {
        let stint = analysis::Stint::load(entry.path.as_ref())
            .map_err(|e| format!("{}: {e}", entry.path))?;
        match analysis::profile::stint_profile(&stint.frames) {
            Ok(profile) => loaded.push((entry, stint, profile)),
            Err(_) if i == entries.len() - 1 => in_progress = Some(entry.path.clone()),
            Err(e) => return Err(format!("{}: {e}", entry.path)),
        }
    }
    if loaded.is_empty() {
        return Err(format!(
            "{}: no stints with completed laps in the journal yet — drive a lap first",
            journal_path
        ));
    }

    let changes: Vec<_> = loaded
        .iter()
        .map(|(e, _, _)| e.note.as_deref().map(journal::parse_clauses).unwrap_or_default())
        .collect();
    let positions = journal::track_positions(&changes);

    let mut steps = Vec::new();
    let mut last_step: Option<(journal::Change, journal::Outcome, &str)> = None;
    // Compound final step: (attribution, total ideal delta, note) for
    // per-clause channel reconciliation below.
    let mut last_compound: Option<(analysis::attribution::Attribution, f32, &str)> = None;
    // Either side of the last comparison ran a single flying lap: the ideal
    // has no corroboration, so outcome-driven advice must not act on it.
    let mut last_weak = false;
    // Per-step ideal deltas, for the A-B-A decomposition below.
    let mut deltas: Vec<Option<f32>> = Vec::new();
    for i in 0..loaded.len() {
        let (entry, stint, profile) = &loaded[i];
        let mut split = None;
        deltas.push(None);
        let outcome = if i == 0 {
            None
        } else {
            let prev = &loaded[i - 1].2;
            match analysis::compare::compare(prev, profile) {
                Ok(cmp) => {
                    let attr = analysis::attribution::split_delta(prev, &cmp.bin_delta_s);
                    split = Some((attr.entry_delta_s, attr.exit_delta_s, attr.straight_delta_s));
                    *deltas.last_mut().unwrap() = Some(cmp.ideal_delta_s);
                    let outcome = journal::judge(cmp.ideal_delta_s);
                    let word = match outcome {
                        journal::Outcome::Improved(_) => "improved",
                        journal::Outcome::Worsened(_) => "WORSE",
                        _ => "inconclusive",
                    };
                    // Reconciliation uses THE last step only. Single-family
                    // steps reconcile on the measured outcome; compound steps
                    // reconcile per clause on channel-attributed deltas below.
                    if i == loaded.len() - 1
                        && let Some(note) = &entry.note
                    {
                        last_weak = prev.laps.len().min(profile.laps.len()) < 2;
                        if let Some(change) = journal::parse_change(note) {
                            last_step = Some((change, outcome, note));
                        } else if !journal::parse_clauses(note).is_empty() {
                            last_compound = Some((attr, cmp.ideal_delta_s, note));
                        }
                    }
                    Some(Ok((word, cmp.ideal_delta_s, prev.laps.len() != profile.laps.len())))
                }
                Err(e) => Some(Err(e)),
            }
        };
        steps.push(StepView {
            path: entry.path.clone(),
            laps: profile.laps.len(),
            best_s: profile.best_lap_time_s,
            ideal_s: profile.composite.time_s,
            balance: stint_balance(stint),
            note: entry.note.clone(),
            pos: match positions[i] {
                (Some(f), Some(r)) if f != 0.0 || r != 0.0 => Some((f, r)),
                _ => None,
            },
            outcome,
            split,
        });
    }

    // Setup state per step: the latest tune revision saved before the stint
    // began. Only bound when the stint really is the session car's — an
    // explicitly passed foreign journal must not inherit this car's tunes.
    let setups: Vec<Option<&crate::tuning::Revision>> = loaded
        .iter()
        .map(|(entry, stint, _)| {
            let car = stint
                .frames
                .iter()
                .find(|t| t.frame.car_ordinal != 0)
                .map(|t| t.frame.car_ordinal);
            if car.is_none() || car != session.car {
                return None;
            }
            let stamp = stint_stamp(&entry.path)?;
            session.revisions.iter().rev().find(|r| r.stamp.as_str() < stamp)
        })
        .collect();

    // The honest comparison for the last stint is the prior stint whose SETUP
    // differs least (ties -> most recent). Chained experiments ("revert X;
    // try Y") make the chronological neighbor a compound comparison while the
    // shared baseline is a clean single-area A/B.
    let mut anchor = None;
    let mut anchor_change: Option<(journal::Change, journal::Outcome, String, bool)> = None;
    let n = loaded.len();
    if let Some(Some(last_setup)) = setups.last()
        && n >= 2
    {
        let mut best: Option<(usize, Vec<String>)> = None;
        for (i, setup) in setups[..n - 1].iter().enumerate() {
            let Some(setup) = setup else { continue };
            let keys = crate::tuning::diff_keys(setup, last_setup);
            if best.as_ref().is_none_or(|(_, bk)| keys.len() <= bk.len()) {
                best = Some((i, keys));
            }
        }
        if let Some((i, keys)) = best
            && let Ok(cmp) = analysis::compare::compare(&loaded[i].2, &loaded[n - 1].2)
        {
            let attr = analysis::attribution::split_delta(&loaded[i].2, &cmp.bin_delta_s);
            let mut areas: Vec<&str> =
                keys.iter().map(|k| crate::tuning::field_area(k)).collect();
            areas.sort();
            areas.dedup();
            let changes = crate::tuning::diff_note(setups[i].unwrap(), last_setup);
            let weak =
                loaded[i].2.laps.len().min(loaded[n - 1].2.laps.len()) < 2;
            let outcome = journal::judge(cmp.ideal_delta_s);
            let single_family =
                (areas.len() == 1).then(|| journal::family_for_area(areas[0])).flatten();
            if let Some(family) = single_family {
                let deltas: Vec<f32> = keys
                    .iter()
                    .filter_map(|k| {
                        let old = setups[i].unwrap().values.get(k)?.parse::<f32>().ok()?;
                        let new = last_setup.values.get(k)?.parse::<f32>().ok()?;
                        Some(new - old)
                    })
                    .collect();
                let change = journal::Change {
                    family,
                    softer: deltas.iter().sum::<f32>() < 0.0,
                    magnitude: (deltas.len() == 1).then(|| deltas[0]),
                };
                anchor_change = Some((change, outcome, changes.clone(), weak));
            }
            anchor = Some(AnchorView {
                vs_step: i + 1,
                areas: areas.join(", "),
                changes,
                delta_s: cmp.ideal_delta_s,
                word: match outcome {
                    journal::Outcome::Improved(_) => "improved",
                    journal::Outcome::Worsened(_) => "WORSE",
                    _ => "inconclusive",
                },
                weak,
                reconciled: anchor_change.is_some(),
                split: (attr.entry_delta_s, attr.exit_delta_s, attr.straight_delta_s),
            });
        }
    }

    // Trailing excursion-and-revert (A-B-A): the pair's deltas cancel drift.
    // effect = (d_exc − d_rev)/2, drift = (d_exc + d_rev)/2. Requires 2+
    // flying laps on all three stints involved — single-lap ideals are the
    // same trap this decomposition exists to avoid.
    let n = loaded.len();
    let aba = (n >= 3)
        .then(|| {
            let (exc, rev) = (&changes[n - 2], &changes[n - 1]);
            let laps_ok = loaded[n - 3..].iter().all(|(_, _, p)| p.laps.len() >= 2);
            if !laps_ok || !journal::is_reverse(exc, rev) {
                return None;
            }
            let (d_exc, d_rev) = (deltas[n - 2]?, deltas[n - 1]?);
            let mut areas: Vec<&str> =
                exc.iter().map(|c| journal::family_area(c.family)).collect();
            areas.dedup();
            Some(AbaView {
                families: areas.join("+"),
                effect_s: (d_exc - d_rev) / 2.0,
                drift_s: (d_exc + d_rev) / 2.0,
            })
        })
        .flatten();

    let (last_entry, last_stint, _) = loaded.last().unwrap();
    let mut recs = blind_recommendations(last_stint, &last_entry.path)?;
    let anchor_drift = anchor.as_ref().is_some_and(|a| a.areas.is_empty());
    if let Some((change, outcome, note, weak)) = &anchor_change {
        // Setup-state comparison: a direct single-area A/B against the anchor
        // stint — supersedes note-based reconciliation entirely.
        if !journal::reconcile(&mut recs, *change, *outcome, note, None, *weak)
            && let Some(rec) = journal::history_revert(*change, *outcome, note, None, *weak)
        {
            recs.push(rec);
        }
        if let Some(a) = &anchor {
            let (e, x, st) = a.split;
            for r in recs.iter_mut().filter(|r| {
                r.implied.is_some_and(|i| i.family == change.family)
            }) {
                r.evidence.push(format!(
                    "where the time moved vs step {}: corner entry {e:+.2}s / \
                     exit {x:+.2}s / straights {st:+.2}s",
                    a.vs_step,
                ));
            }
        }
    } else if anchor_drift {
        // Same setup as the anchor: the measured delta is pure drift; there is
        // no change to charge it to.
    } else if let Some((change, outcome, note)) = last_step {
        if !journal::reconcile(&mut recs, change, outcome, note, None, last_weak)
            && let Some(rec) = journal::history_revert(change, outcome, note, None, last_weak)
        {
            recs.push(rec);
        }
    } else if let Some((attr, total, note)) = last_compound {
        // Channel attribution: chassis clauses are judged on the cornering
        // share of the delta, gearing clauses on the straight share.
        let evidence = format!(
            "outcome attributed from a compound step (\"{note}\"): corner entry \
             {:+.2}s / exit {:+.2}s / straights {:+.2}s of {total:+.2}s total \
             ({:.0}% of lap time is cornering) — inferred from where the time \
             moved, not measured in isolation",
            attr.entry_delta_s,
            attr.exit_delta_s,
            attr.straight_delta_s,
            attr.corner_share * 100.0,
        );
        let mut seen = Vec::new();
        for clause in journal::parse_clauses(note) {
            if seen.contains(&clause.family) {
                continue;
            }
            seen.push(clause.family);
            // Each family is judged on the road its fingerprint lives on.
            // Calibrated 2026-07-21: brake bias showed cleanly on entry even
            // inside a compound step; diff lock (accel AND decel) measured
            // SPREAD across phases, so both judge on the corner total.
            let channel_delta = match clause.family {
                journal::Family::Gearing => attr.straight_delta_s,
                journal::Family::Brakes => attr.entry_delta_s,
                _ => attr.corner_delta_s,
            };
            let outcome = journal::judge(channel_delta);
            if !journal::reconcile(&mut recs, clause, outcome, note, Some(&evidence), last_weak)
                && let Some(rec) =
                    journal::history_revert(clause, outcome, note, Some(&evidence), last_weak)
            {
                recs.push(rec);
            }
        }
    }
    // History-only recs arrive unsorted; keep most-confident-first for display.
    recs.sort_by_key(|r| std::cmp::Reverse(r.confidence));
    // Cite tune absolutes only when the journal's stints are the session
    // car's — an explicitly passed foreign journal must not quote this car's
    // sliders as if they were its own.
    let last_car = last_stint
        .frames
        .iter()
        .find(|t| t.frame.car_ordinal != 0)
        .map(|t| t.frame.car_ordinal);
    let current_tune = if last_car == session.car {
        enrich_with_tune(&mut recs, &session)
    } else {
        Vec::new()
    };

    Ok(AdviseView {
        journal: Some(journal_path.to_string()),
        steps,
        anchor,
        aba,
        in_progress,
        advice_for: last_entry.path.clone(),
        recommendations: recs,
        current_tune,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::recommend::{Confidence, Recommendation};
    use crate::tuning::Revision;

    fn balance_rec() -> Recommendation {
        Recommendation {
            area: "balance",
            advice: "reduce front roll stiffness".into(),
            evidence: vec![],
            confidence: Confidence::High,
            implied: Some(journal::Change {
                family: journal::Family::FrontRoll,
                softer: true,
                magnitude: None,
            }),
        }
    }

    fn session_with(values: &[(&str, &str)], facts: &[(&str, &str)]) -> TuningSession {
        let mut s = TuningSession::default();
        let mut rev = Revision { stamp: "20260721-000000".into(), ..Default::default() };
        for (k, v) in values {
            rev.values.insert(k.to_string(), v.to_string());
        }
        s.revisions.push(rev);
        for (k, v) in facts {
            s.facts.insert(k.to_string(), v.to_string());
        }
        s
    }

    /// Front roll advised softer with both front sliders at minimum: the
    /// advice flips to stiffening the rear, implied direction included.
    #[test]
    fn exhausted_front_softening_flips_to_rear() {
        let session = session_with(
            &[("arb_f", "1"), ("springs_f", "100")],
            &[("limit_arb_f", "1..65"), ("limit_springs_f", "100..800")],
        );
        let mut recs = vec![balance_rec()];
        enrich_with_tune(&mut recs, &session);
        assert!(recs[0].advice.contains("stiffen the rear instead"), "{}", recs[0].advice);
        let implied = recs[0].implied.unwrap();
        assert_eq!(implied.family, journal::Family::RearRoll);
        assert!(!implied.softer);
        assert!(recs[0].evidence.iter().any(|e| e.contains("AT MINIMUM")), "{:?}", recs[0].evidence);
        assert!(recs[0].evidence.iter().any(|e| e.contains("advised direction exhausted")));
    }

    /// Only the primary slider pinned: advice stands, evidence points at the
    /// secondary slider instead of flipping ends.
    #[test]
    fn primary_pinned_points_at_secondary() {
        let session = session_with(
            &[("arb_f", "1"), ("springs_f", "300")],
            &[("limit_arb_f", "1..65"), ("limit_springs_f", "100..800")],
        );
        let mut recs = vec![balance_rec()];
        enrich_with_tune(&mut recs, &session);
        assert!(recs[0].advice.contains("reduce front roll stiffness"), "{}", recs[0].advice);
        assert_eq!(recs[0].implied.unwrap().family, journal::Family::FrontRoll);
        assert!(
            recs[0].evidence.iter().any(|e| e.contains("work with front springs")),
            "{:?}",
            recs[0].evidence
        );
    }

    /// No limits on file: values cited, no exhaustion claims possible.
    #[test]
    fn unknown_limits_never_claim_exhaustion() {
        let session = session_with(&[("arb_f", "1"), ("springs_f", "100")], &[]);
        let mut recs = vec![balance_rec()];
        enrich_with_tune(&mut recs, &session);
        assert!(recs[0].advice.contains("reduce front roll stiffness"));
        assert!(recs[0].evidence.iter().all(|e| !e.contains("MINIMUM")));
    }

    #[test]
    fn stint_stamps_parse_from_both_naming_schemes() {
        assert_eq!(stint_stamp("sessions/stint-20260720-233644.ftel"), Some("20260720-233644"));
        assert_eq!(stint_stamp("sessions/session-20260719-115355.ftel"), Some("20260719-115355"));
        assert_eq!(stint_stamp("sessions/other.ftel"), None);
    }
}
