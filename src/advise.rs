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
    /// The outcome split into (cornering, straight) road — where the time moved.
    pub split: Option<(f32, f32)>,
}

pub struct AdviseView {
    /// Journal file the trajectory came from; None = blind fallback (no journal).
    pub journal: Option<String>,
    pub steps: Vec<StepView>,
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

/// Attach current-tune absolutes to family-matched recommendations and build
/// the display list of the latest revision.
fn enrich_with_tune(
    recs: &mut [analysis::recommend::Recommendation],
    session: &TuningSession,
) -> Vec<(String, String, Option<&'static str>)> {
    let Some(rev) = session.latest() else { return Vec::new() };
    for r in recs.iter_mut() {
        let Some(implied) = r.implied else { continue };
        let keys: &[&str] = match implied.family {
            journal::Family::FrontRoll => &["arb_f", "springs_f"],
            journal::Family::RearRoll => &["arb_r", "springs_r"],
            journal::Family::Gearing => &["final_drive"],
        };
        let known: Vec<String> = keys
            .iter()
            .filter_map(|k| {
                rev.values.get(*k).map(|v| {
                    format!(
                        "{} = {}",
                        crate::tuning::field_phrase(k),
                        crate::tuning::display_value(k, v, &session.facts),
                    )
                })
            })
            .collect();
        if !known.is_empty() {
            r.evidence.push(format!("current setting: {}", known.join(", ")));
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
            advice_for: path,
            recommendations: recs,
            current_tune,
        });
    }

    // Load and profile every stint, in journal (chronological) order.
    let mut loaded = Vec::new();
    for entry in &entries {
        let stint = analysis::Stint::load(entry.path.as_ref())
            .map_err(|e| format!("{}: {e}", entry.path))?;
        let profile = analysis::profile::stint_profile(&stint.frames)
            .map_err(|e| format!("{}: {e}", entry.path))?;
        loaded.push((entry, stint, profile));
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
    for i in 0..loaded.len() {
        let (entry, stint, profile) = &loaded[i];
        let mut split = None;
        let outcome = if i == 0 {
            None
        } else {
            let prev = &loaded[i - 1].2;
            match analysis::compare::compare(prev, profile) {
                Ok(cmp) => {
                    let attr = analysis::attribution::split_delta(prev, &cmp.bin_delta_s);
                    split = Some((attr.corner_delta_s, attr.straight_delta_s));
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

    let (last_entry, last_stint, _) = loaded.last().unwrap();
    let mut recs = blind_recommendations(last_stint, &last_entry.path)?;
    if let Some((change, outcome, note)) = last_step {
        journal::reconcile(&mut recs, change, outcome, note, None);
    } else if let Some((attr, total, note)) = last_compound {
        // Channel attribution: chassis clauses are judged on the cornering
        // share of the delta, gearing clauses on the straight share.
        let evidence = format!(
            "outcome attributed from a compound step (\"{note}\"): corners \
             {:+.2}s / straights {:+.2}s of {total:+.2}s total ({:.0}% of lap \
             time is cornering) — inferred from where the time moved, not \
             measured in isolation",
            attr.corner_delta_s,
            attr.straight_delta_s,
            attr.corner_share * 100.0,
        );
        let mut seen = Vec::new();
        for clause in journal::parse_clauses(note) {
            if seen.contains(&clause.family) {
                continue;
            }
            seen.push(clause.family);
            let channel_delta = match clause.family {
                journal::Family::Gearing => attr.straight_delta_s,
                _ => attr.corner_delta_s,
            };
            journal::reconcile(
                &mut recs,
                clause,
                journal::judge(channel_delta),
                note,
                Some(&evidence),
            );
        }
    }
    let current_tune = enrich_with_tune(&mut recs, &session);

    Ok(AdviseView {
        journal: Some(journal_path.to_string()),
        steps,
        advice_for: last_entry.path.clone(),
        recommendations: recs,
        current_tune,
    })
}
