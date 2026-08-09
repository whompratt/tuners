//! The advise engine, shared by the CLI (`tuners advise`) and the dashboard
//! (`/api/advise`): journal trajectory with measured step outcomes, blind
//! recommendations reconciled with the last step, and current-tune enrichment.
//! With no journal yet (a session's first stint), falls back to blind
//! recommendations on the latest stint of the session car; the journal
//! starts with the first tune change.

use crate::advice::{journal, recommend, tuning::TuningSession};
use crate::analysis::{self, effects};
use std::path::Path;

mod campaign;
mod compose;
mod enrich;
mod landscape;
#[cfg(test)]
mod tests;
mod view;

use campaign::pair_effects;
pub(crate) use campaign::{
    Campaign, CampaignBound, Measurement, attach_saturation, campaign_bound, campaign_closed,
    implicit_steps, load_campaign, stint_stamp,
};
pub use campaign::{latest_stint_for_car, stints_for_car_newest_first};
pub(crate) use compose::composition_proposal;
use enrich::{enrich_with_tune, map_prior, setup_lints};
use landscape::{key_from_phrase, probe_value, quad_fit};
pub use view::*;

/// A composite ideal dramatically faster than the stint's own best complete
/// lap is an UNCORROBORATED splice: rewinds, drafting in a race, or route
/// anomalies stitched segments that never co-occurred in one lap. Such a
/// stint's comparisons cannot be trusted. The anchor is the best kept lap of
/// either kind: on point-to-point routes (and restart-per-run circuit
/// driving) every kept lap is a standing run and the composite is stitched
/// from those same runs, so the run anchors it exactly as a flying lap
/// would (standing-only recordings measure 0.996-0.999 of best, bonuses
/// 0.16-0.47s on ~130s runs — the flying-lap range).
fn splice_trusted(p: &analysis::profile::StintProfile) -> bool {
    p.best_lap_time_s.is_finite() && p.composite.time_s >= 0.95 * p.best_lap_time_s
}

/// The prior stint whose SETUP differs least from `target` (ties -> most
/// recent): the honest comparison partner for a step. Searches the given
/// prefix of the per-step setups.
fn min_diff_ancestor(
    setups: &[Option<&crate::advice::tuning::Revision>],
    target: &crate::advice::tuning::Revision,
) -> Option<(usize, Vec<String>)> {
    let mut best: Option<(usize, Vec<String>)> = None;
    for (i, s) in setups.iter().enumerate() {
        let Some(s) = s else { continue };
        let keys = crate::advice::tuning::diff_keys(s, target);
        if best.as_ref().is_none_or(|(_, bk)| keys.len() <= bk.len()) {
            best = Some((i, keys));
        }
    }
    best
}

/// Distinct tuning areas the changed keys span, sorted for stable display.
fn area_list(keys: &[String]) -> Vec<&'static str> {
    let mut areas: Vec<&'static str> = keys
        .iter()
        .map(|k| crate::advice::tuning::field_area(k))
        .collect();
    areas.sort();
    areas.dedup();
    areas
}

/// "17 → -0.16s, 18 → -0.49s" listing of a landscape's tried values.
fn nodes_summary(nodes: &[(f32, f32, usize)]) -> String {
    nodes
        .iter()
        .map(|(v, cum, _)| format!("{v} → {cum:+.2}s"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Sample sd of a stint's flying-lap times; None under 3 laps (a 2-lap "sd"
/// is just half the gap between them, not a spread estimate).
fn lap_scatter(profile: &analysis::profile::StintProfile) -> Option<f32> {
    let n = profile.laps.len();
    if n < 3 {
        return None;
    }
    let mean = profile.laps.iter().map(|l| l.time_s).sum::<f32>() / n as f32;
    Some(
        (profile
            .laps
            .iter()
            .map(|l| (l.time_s - mean).powi(2))
            .sum::<f32>()
            / (n - 1) as f32)
            .sqrt(),
    )
}

/// Rule context from the session: tire compound fact + whether the build has
/// aero fitted (absent aero fields in the latest revision = the upgrade isn't
/// there; no revisions = unknown).
fn rule_context(session: &TuningSession) -> recommend::Context<'_> {
    recommend::Context {
        compound: session.facts.get("tire_compound").map(String::as_str),
        aero_tunable: session
            .latest()
            .map(|rev| rev.values.keys().any(|k| k.starts_with("aero_"))),
    }
}

/// Fit a grip curve from the car's recordings in `stints_dir` (the target's
/// own samples plus era-nearest siblings, up to CAR_POOL_SIBLINGS) and
/// classify `longest` against it. Labeled CarPool only when another
/// recording really joined the pool: a lone recording stays SelfFit, which
/// detection ignores (single-recording fits measured push 0.1-19.6% on
/// known-healthy stints, 2026-07-31 — display-only noise).
pub fn car_pool_saturation(
    target: &[analysis::grip::GripSample],
    stint_path: &str,
    stints_dir: &str,
    car: Option<i32>,
) -> Option<analysis::grip::GripSaturation> {
    use analysis::grip;
    // "YYYYMMDD-HHMMSS" stamp as a sortable number for era distance.
    fn stamp_num(path: &str) -> Option<u64> {
        stint_stamp(path)?
            .bytes()
            .filter(u8::is_ascii_digit)
            .try_fold(0u64, |n, b| {
                n.checked_mul(10)?.checked_add((b - b'0') as u64)
            })
    }
    let mut pooled = target.to_vec();
    let mut source = grip::CurveSource::SelfFit;
    // Nearest-in-time siblings first: grip curves move with setup changes
    // (aero, pressures, compound), so the pool must be era-local to the
    // target — a newest-first pool judged a pre-aero-cut baseline against
    // the cut-era curve and misread it as a pusher.
    let target_stamp = stamp_num(stint_path);
    let mut siblings: Vec<String> = stints_for_car_newest_first(stints_dir, car)
        .filter(|p| Path::new(p).file_name() != Path::new(stint_path).file_name())
        .collect();
    siblings.sort_by_key(|p| match (stamp_num(p), target_stamp) {
        (Some(s), Some(t)) => s.abs_diff(t),
        _ => u64::MAX,
    });
    // The pool must not be dominated by the target's own idiosyncrasy, so
    // the loop is driven by SIBLING sample count, not total pool size — and
    // it aims well above FIT_MIN because sub-20k pools misread (BIN_MIN is
    // absolute; see grip.rs).
    let mut sibling_samples = 0usize;
    let mut files = 0usize;
    for path in siblings {
        if sibling_samples >= grip::CAR_POOL_SIBLINGS || files >= 12 {
            break;
        }
        let Ok(sib) = analysis::products::cached(path.as_ref()) else {
            continue;
        };
        if sib.samples.is_empty() || sib.met.as_ref().is_none_or(|m| m.surface_loose) {
            continue;
        }
        sibling_samples += sib.samples.len();
        files += 1;
        pooled.extend(sib.samples.iter().copied());
        source = grip::CurveSource::CarPool;
    }
    let curves = grip::fit_curves(&pooled)?;
    grip::occupancy(target, &curves, source)
}

/// Returns the recommendations plus the drag model's final-drive scale (the
/// "ideal ≈ current × N" estimate), which enrichment resolves into a concrete
/// caveated target once the current tune is known.
fn blind_recommendations(
    data: &analysis::products::StintData,
    path: &str,
    ctx: &recommend::Context,
    sat: Option<analysis::grip::GripSaturation>,
    stints_dir: Option<&str>,
) -> Result<(Vec<recommend::Recommendation>, Option<f32>), String> {
    let mut overall = data
        .met
        .clone()
        .ok_or_else(|| format!("{path}: no driving stints of 5s or longer"))?;
    // The balance rule's understeer detection is saturation-led (plan 015)
    // and needs a POOLED curve: campaign-pooled when the caller has one,
    // else pooled across the car's recordings (blind mode; tarmac only).
    if !overall.surface_loose {
        overall.grip_saturation = sat.or_else(|| {
            stints_dir.and_then(|dir| car_pool_saturation(&data.samples, path, dir, data.car))
        });
    }
    let fd_scale = overall
        .driveline
        .as_ref()
        .and_then(|d| d.final_drive_scale(overall.gears.effective_redline));
    Ok((recommend::recommend(&overall, &data.per_lap, ctx), fd_scale))
}

/// Full advise: journal trajectory when one exists, blind fallback otherwise.
pub fn advise(
    journal_path: &str,
    session_path: &Path,
    stints_dir: &str,
) -> Result<AdviseView, String> {
    let mut session = TuningSession::load(session_path);
    let text = std::fs::read_to_string(journal_path).unwrap_or_default();
    let mut entries = journal::parse_journal(&text);

    implicit_steps(&text, &mut entries, session.car, stints_dir);

    if entries.is_empty() {
        // No journal yet: blind advice on the session car's newest stint
        // that contains real driving. The newest recording is surprisingly
        // often a menu artifact (event entered, quit from the pause menu
        // auto-cuts a tiny stint) — skip back to the last real drive
        // instead of erroring on it.
        let mut first_err: Option<String> = None;
        let mut picked = None;
        for path in stints_for_car_newest_first(stints_dir, session.car) {
            match analysis::products::cached(path.as_ref()) {
                Ok(data) => match blind_recommendations(
                    &data,
                    &path,
                    &rule_context(&session),
                    None,
                    Some(stints_dir),
                ) {
                    Ok((recs, fd_scale)) => {
                        picked = Some((path, data, recs, fd_scale));
                        break;
                    }
                    Err(e) => {
                        first_err.get_or_insert(e);
                    }
                },
                Err(e) => {
                    first_err.get_or_insert(format!("{path}: {e}"));
                }
            }
        }
        let Some((path, data, mut recs, fd_scale)) = picked else {
            return Err(first_err.unwrap_or_else(|| "no stints recorded yet; drive first".into()));
        };
        // Cold start: no journal means no local trends, but the driver's
        // pooled history from other cars — and the crowd artifact on a
        // fresh install with no map at all — can still rank one untried
        // lever: the first informed suggestion of a fresh campaign
        // (plan 007).
        if let Some(met) = data.met.as_ref() {
            let emap = std::fs::read_to_string(crate::util::data_path("effect-map.tsv"))
                .ok()
                .and_then(|t| crate::advice::effectmap::parse(&t).ok())
                .unwrap_or_default();
            let trends = crate::advice::effectmap::driver_trends(&emap, "local", data.car);
            if std::env::var_os("TUNERS_MAP_TRACE").is_some() {
                eprintln!("map-prior trace (blind): trends:");
                for t in &trends {
                    eprintln!("  {} r={:+.2} n={} (history)", t.key, t.r, t.n);
                }
            }
            let ctx = crate::advice::effectmap::MapContext {
                drivetrain: met.drivetrain_type,
                surface_loose: met.surface_loose,
                aero: rule_context(&session).aero_tunable,
            };
            let (cells, landscapes) =
                crate::advice::priors::merged_view(&emap, met.surface_loose, met.drivetrain_type);
            if let Some(rec) = map_prior(
                &cells,
                &landscapes,
                &trends,
                &ctx,
                &recs,
                &enrich::PriorInputs {
                    measurements: &[],
                    baseline: session.latest(),
                    facts: &session.facts,
                },
                Some(&data.fx),
            ) {
                recs.push(rec);
            }
        }
        let lints = setup_lints(&session, &[], &recs, data.met.as_ref());
        recs.extend(lints);
        let current_tune = enrich_with_tune(&mut recs, &session);
        enrich::apply_fd_scale(&mut recs, &session, fd_scale, None);
        recs.sort_by_key(|r| (r.kind.rank_group(), std::cmp::Reverse(r.confidence)));
        return Ok(AdviseView {
            journal: None,
            steps: Vec::new(),
            anchor: None,
            aba: None,
            in_progress: None,
            missing: Vec::new(),
            no_laps: Vec::new(),
            landscapes: Vec::new(),
            drift_floor: None,
            effect_floor: Vec::new(),
            advice_for: path,
            recommendations: recs,
            current_tune,
        });
    }

    // A journal for another car (explicitly passed while a different session
    // is active) resolves that car's ARCHIVED session file, so its setups,
    // facts, and landscapes work instead of degrading to blind mode. The
    // journal's own sibling (tune-journal-X.txt -> tune-session-X.txt, the
    // pair naming both the named-session archives and the legacy per-car
    // scheme use) wins over the legacy per-car derivation, which cannot see
    // stamped archives.
    if let Some(first) = entries.first()
        && let Ok(data) = analysis::products::cached(first.path.as_ref())
    {
        let journal_car = data.car;
        if journal_car.is_some() && journal_car != session.car {
            let sibling = journal_path.replace("tune-journal", "tune-session");
            let candidates = [
                sibling,
                crate::advice::tuning::journal_path_for(
                    journal_car,
                    &session_path.to_string_lossy(),
                ),
            ];
            if let Some(archived) = candidates
                .iter()
                .map(|p| TuningSession::load(p.as_ref()))
                .find(|s| s.car == journal_car)
            {
                session = archived;
            }
        }
    }

    let mut c = load_campaign(entries, &session, journal_path)?;
    let t_sat = std::time::Instant::now();
    attach_saturation(&mut c.stints, stints_dir);
    if std::env::var_os("TUNERS_ADVISE_TRACE").is_some() {
        eprintln!("advise-trace: attach_saturation in {:.2?}", t_sat.elapsed());
    }
    let n = c.stints.len();

    // One trajectory row per SETUP STATE: a consecutive same-setup group is
    // one experiment corroborated over several runs, and its honest outcome
    // is state-vs-state on the pooled profiles (the per-run neighbor verdict
    // on a 1-lap p2p run is a coin flip the measurements already refuse to
    // judge on). Per-run detail rides along in `runs`.
    let heads: Vec<usize> = (0..n).filter(|&k| c.groups[k] == k).collect();
    let mut steps = Vec::new();
    for (gi, &g) in heads.iter().enumerate() {
        let members: Vec<usize> = (g..n).take_while(|&m| c.groups[m] == g).collect();
        let e = *members.last().expect("group has its head");
        let runs: Vec<RunView> = members
            .iter()
            .map(|&m| {
                let cs = &c.stints[m];
                RunView {
                    n: m + 1,
                    path: cs.entry.path.clone(),
                    laps: cs.profile().laps.len(),
                    best_s: cs.profile().best_lap_time_s,
                    ideal_s: cs.profile().composite.time_s,
                    scatter_s: lap_scatter(cs.profile()),
                    balance: cs.met.as_ref().and_then(|m| {
                        Some((
                            m.understeer_index?,
                            m.cornering_front_slip?,
                            m.cornering_rear_slip?,
                        ))
                    }),
                    note: (m > g)
                        .then(|| {
                            cs.entry
                                .note
                                .as_deref()
                                .map(|n| crate::advice::tuning::display_note(n, &session.facts))
                        })
                        .flatten(),
                    drift_s: (m > g)
                        .then(|| {
                            cs.vs_prev
                                .as_ref()
                                .and_then(|r| r.as_ref().ok().map(|pv| pv.verdict_s))
                        })
                        .flatten(),
                }
            })
            .collect();
        let sp = c.state_profile(g);
        let mut split = None;
        let mut currencies = None;
        let outcome = (gi > 0).then(|| {
            let prev = g - 1; // last run of the previous state
            match analysis::compare::compare(c.state_profile(prev), sp) {
                Ok(cmp) => {
                    let attr =
                        analysis::attribution::split_delta(c.state_profile(prev), &cmp.bin_delta_s);
                    split = Some((attr.entry_delta_s, attr.exit_delta_s, attr.straight_delta_s));
                    currencies = Some((
                        cmp.ideal_delta_s,
                        cmp.best_lap_delta_s,
                        cmp.median_lap_delta_s,
                    ));
                    // Adjacent states with identical setups (a group break on
                    // standing-start character or an unpoolable repeat): the
                    // delta is still pure drift, not a change effect.
                    let word = match (c.setups[prev], c.setups[g]) {
                        (Some(a), Some(b)) if crate::advice::tuning::diff_keys(a, b).is_empty() => {
                            "drift"
                        }
                        _ => journal::judge(cmp.verdict_delta_s).word(),
                    };
                    let d = cmp.verdict_delta_s;
                    let (confidence, why) = if word == "drift" {
                        (None, None)
                    } else if c.thin(prev, g) {
                        (Some("low"), Some("single lap on a side; corroborate"))
                    } else if c.weak_pair(prev, g) {
                        (Some("low"), Some("suspect-tagged run in the comparison"))
                    } else if c.drift_floor.is_some_and(|(_, f)| d.abs() <= f) {
                        (Some("low"), Some("margin within the measured drift"))
                    } else if d.signum() != cmp.ideal_delta_s.signum()
                        && (d.abs() >= 0.05 || cmp.ideal_delta_s.abs() >= 0.05)
                    {
                        (
                            Some("medium"),
                            Some("optimal-lap read overruled by the vote"),
                        )
                    } else if c.drift_floor.is_none() {
                        (
                            Some("medium"),
                            Some("no same-setup drift floor measured yet"),
                        )
                    } else {
                        (Some("high"), None)
                    };
                    Ok(StepOutcome {
                        word,
                        delta_s: d,
                        confidence,
                        why,
                    })
                }
                Err(e) => Err(e),
            }
        });
        let head = &c.stints[g];
        steps.push(StepView {
            first: g + 1,
            last: e + 1,
            runs,
            laps: sp.laps.len(),
            best_s: sp.best_lap_time_s,
            ideal_s: sp.composite.time_s,
            scatter_s: lap_scatter(sp),
            currencies,
            balance: members.iter().rev().find_map(|&m| {
                let met = c.stints[m].met.as_ref()?;
                Some((
                    met.understeer_index?,
                    met.cornering_front_slip?,
                    met.cornering_rear_slip?,
                ))
            }),
            note: head
                .entry
                .note
                .as_deref()
                .map(|n| crate::advice::tuning::display_note(n, &session.facts)),
            pos: match c.positions[g] {
                (Some(f), Some(r)) if f != 0.0 || r != 0.0 => Some((f, r)),
                _ => None,
            },
            outcome,
            split,
            anchor: None,
            families: {
                let mut fams: Vec<journal::Family> =
                    head.changes.iter().map(|c| c.family).collect();
                fams.dedup();
                fams.into_iter()
                    .map(|f| StepFamily {
                        area: journal::family_area(f),
                        channel: match f {
                            journal::Family::Gearing => "straights",
                            journal::Family::Brakes => "entry",
                            _ => "corners",
                        },
                    })
                    .collect()
            },
        });
    }

    // Per-row honest verdicts: each state compared against its minimal-diff
    // ancestor, shown only when that ancestor is NOT the previous state (the
    // row's own outcome column already covers the neighbor) and not for the
    // last state (the prominent anchor line below covers it).
    for (gi, step) in steps
        .iter_mut()
        .enumerate()
        .take(heads.len().saturating_sub(1))
        .skip(1)
    {
        let g = heads[gi];
        let Some(sg) = c.setups[g] else { continue };
        let Some((i, keys)) = min_diff_ancestor(&c.setups[..g], sg) else {
            continue;
        };
        if c.groups[i] == heads[gi - 1] {
            continue; // the previous state: the row outcome covers it
        }
        let Ok(cmp) = analysis::compare::compare(c.state_profile(i), c.state_profile(g)) else {
            continue;
        };
        step.anchor = Some(RowAnchor {
            vs_step: i + 1,
            areas: area_list(&keys).join(", "),
            delta_s: cmp.verdict_delta_s,
            word: journal::judge(cmp.verdict_delta_s).word(),
            weak: c.thin(i, g),
        });
    }

    // The honest comparison for the last stint is the prior stint whose SETUP
    // differs least (ties -> most recent). Chained experiments ("revert X;
    // try Y") make the chronological neighbor a compound comparison while the
    // shared baseline is a clean single-area A/B. The last stint's own
    // consecutive same-setup group is excluded from the search (those are
    // corroboration runs pooled into ITS side, not comparison partners) and
    // both sides compare through their pooled state profiles.
    let mut anchor = None;
    let mut anchor_change: Option<(journal::Change, journal::Outcome, String, bool)> = None;
    let last_group_start = c.groups.last().copied().unwrap_or(0);
    if let Some(Some(last_setup)) = c.setups.last()
        && last_group_start >= 1
        && let Some((i, keys)) = min_diff_ancestor(&c.setups[..last_group_start], last_setup)
        && let Ok(cmp) = analysis::compare::compare(c.state_profile(i), c.state_profile(n - 1))
    {
        let attr = analysis::attribution::split_delta(c.state_profile(i), &cmp.bin_delta_s);
        let areas = area_list(&keys);
        let changes = crate::advice::tuning::diff_note(c.setups[i].unwrap(), last_setup);
        let weak = c.weak_pair(i, n - 1);
        let outcome = journal::judge(cmp.verdict_delta_s);
        let single_family = (areas.len() == 1)
            .then(|| journal::family_for_area(areas[0]))
            .flatten();
        if let Some(family) = single_family {
            let deltas: Vec<f32> = keys
                .iter()
                .filter_map(|k| {
                    let old = c.setups[i].unwrap().values.get(k)?.parse::<f32>().ok()?;
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
        let pools: Vec<String> = [c.pooled_runs(i), c.pooled_runs(n - 1)]
            .into_iter()
            .flatten()
            .collect();
        anchor = Some(AnchorView {
            vs_step: i + 1,
            areas: areas.join(", "),
            changes: crate::advice::tuning::display_note(&changes, &session.facts),
            delta_s: cmp.verdict_delta_s,
            currencies: (
                cmp.ideal_delta_s,
                cmp.best_lap_delta_s,
                cmp.median_lap_delta_s,
            ),
            word: outcome.word(),
            weak,
            reconciled: anchor_change.is_some(),
            split: (attr.entry_delta_s, attr.exit_delta_s, attr.straight_delta_s),
            pooled: (!pools.is_empty()).then(|| pools.join(" + ")),
            effects: pair_effects(&c.stints[i], &c.stints[n - 1]),
        });
    }

    // Trailing excursion-and-revert (A-B-A): the pair's deltas cancel drift.
    // effect = (d_exc − d_rev)/2, drift = (d_exc + d_rev)/2. Requires 2+
    // flying laps on all three stints involved; single-lap ideals are the
    // same trap this decomposition exists to avoid.
    let delta_of = |idx: usize| {
        c.stints[idx]
            .vs_prev
            .as_ref()
            .and_then(|r| r.as_ref().ok().map(|pv| pv.verdict_s))
    };
    let aba = (n >= 3)
        .then(|| {
            let (exc, rev) = (&c.stints[n - 2].changes, &c.stints[n - 1].changes);
            let laps_ok = c.stints[n - 3..]
                .iter()
                .all(|s| s.profile().laps.len() >= 2);
            if !laps_ok || !journal::is_reverse(exc, rev) {
                return None;
            }
            let (d_exc, d_rev) = (delta_of(n - 2)?, delta_of(n - 1)?);
            let mut areas: Vec<&str> = exc.iter().map(|c| journal::family_area(c.family)).collect();
            areas.dedup();
            Some(AbaView {
                families: areas.join("+"),
                effect_s: (d_exc - d_rev) / 2.0,
                drift_s: (d_exc + d_rev) / 2.0,
                effects: effects::aba(
                    &pair_effects(&c.stints[n - 3], &c.stints[n - 2]),
                    &pair_effects(&c.stints[n - 2], &c.stints[n - 1]),
                ),
            })
        })
        .flatten();

    let latest = c.latest();

    let last = c.stints.last().unwrap();
    let (mut recs, fd_scale) = blind_recommendations(
        &last.data,
        &last.entry.path,
        &rule_context(&session),
        last.met.as_ref().and_then(|m| m.grip_saturation),
        None,
    )?;
    let mut matched_families: Vec<journal::Family> = Vec::new();
    for m in &latest {
        if journal::reconcile(
            &mut recs,
            m.change,
            m.outcome,
            &crate::advice::tuning::display_note(&m.desc, &session.facts),
            m.attributed.as_deref(),
            m.weak,
        ) {
            matched_families.push(m.change.family);
        }
    }
    // History-only reverts stay scoped to the LAST stint's own deviation from
    // its anchor: a past experiment already reverted needs no advice. The
    // suggestion is the anchor's own values: reverting fully means returning
    // to a measured state, not arithmetic.
    if let Some((change, outcome, note, weak)) = &anchor_change
        && !matched_families.contains(&change.family)
        && let Some(mut rec) = journal::history_revert(
            *change,
            *outcome,
            &crate::advice::tuning::display_note(note, &session.facts),
            None,
            *weak,
        )
    {
        if let Some(a) = &anchor
            && let (Some(Some(anchor_setup)), Some(Some(last_setup))) =
                (c.setups.get(a.vs_step - 1), c.setups.last())
        {
            let restore: Vec<(String, String)> =
                crate::advice::tuning::diff_keys(anchor_setup, last_setup)
                    .into_iter()
                    .filter_map(|k| {
                        let v = anchor_setup.values.get(&k)?.clone();
                        Some((k, v))
                    })
                    .collect();
            if !restore.is_empty() {
                rec.suggestion = Some(
                    restore
                        .iter()
                        .map(|(k, v)| {
                            format!(
                                "{}: {}",
                                crate::advice::tuning::field_phrase(k),
                                crate::advice::tuning::display_value(k, v, &session.facts),
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(", "),
                );
                rec.apply = restore;
            }
        }
        recs.push(rec);
    }
    if let Some((change, ..)) = &anchor_change
        && let Some(a) = &anchor
    {
        let (e, x, st) = a.split;
        let moved = effects::movers(&a.effects, Some(&c.effect_floor));
        for r in recs
            .iter_mut()
            .filter(|r| r.implied.is_some_and(|i| i.family == change.family))
        {
            r.evidence.push(format!(
                "where the time moved vs step {}: corner entry {e:+.2}s / \
                 exit {x:+.2}s / straights {st:+.2}s",
                a.vs_step,
            ));
            if !moved.is_empty() {
                r.evidence.push(format!(
                    "behaviour that moved with it (above noise): {}",
                    effects::describe(&moved),
                ));
            }
        }
    }
    // ---- measured landscapes: the campaign's response per slider ----
    // Chained deltas from non-weak measurements build a cumulative curve over
    // a slider's tried values ("decaying improvement" reads as a curve shape,
    // not a single verdict). With 3+ points, meaningful spread, and an
    // interior minimum, the fit's vertex becomes the suggestion:
    // interpolation over the mapped landscape instead of last-step bisection.
    // Every family's landscape is kept on the view for the history panel.
    let mut curve_fams: Vec<journal::Family> = Vec::new();
    for m in &c.measurements {
        if !curve_fams.contains(&m.change.family) {
            curve_fams.push(m.change.family);
        }
    }
    let mut landscapes: Vec<LandscapeView> = Vec::new();
    for family in curve_fams {
        let fam_all: Vec<&Measurement> = c
            .measurements
            .iter()
            .filter(|m| m.change.family == family)
            .collect();
        let mviews: Vec<MeasurementView> = fam_all
            .iter()
            .map(|m| MeasurementView {
                from_step: m.i + 1,
                to_step: m.j + 1,
                desc: crate::advice::tuning::display_note(&m.desc, &session.facts),
                delta_s: m.outcome.delta_s().unwrap_or(0.0),
                split: m.split,
                weak: m.weak,
                direct: m.direct,
                effects: m.effects.clone(),
            })
            .collect();

        // Axis: the single slider all non-weak keyed measurements agree on.
        let mut axis: Vec<&str> = fam_all
            .iter()
            .filter(|m| !m.weak)
            .filter_map(|m| m.key.as_deref())
            .collect();
        axis.sort();
        axis.dedup();
        let key: Option<String> = match axis[..] {
            [k] => Some(k.to_string()),
            _ => None,
        };

        let mut nodes: Vec<(f32, f32, usize)> = Vec::new();
        let mut provisional: Vec<(f32, f32)> = Vec::new();
        if let Some(key) = key.as_deref() {
            let value_of = |idx: usize| -> Option<f32> {
                c.setups
                    .get(idx)?
                    .as_ref()?
                    .values
                    .get(key)?
                    .parse::<f32>()
                    .ok()
            };
            let mut edges: Vec<(usize, f32, f32, f32)> = fam_all
                .iter()
                .filter(|m| !m.weak && m.clean)
                .filter_map(|m| {
                    let d = m.outcome.delta_s()?;
                    Some((m.j, value_of(m.i)?, value_of(m.j)?, d))
                })
                .collect();
            edges.sort_by_key(|e| e.0);
            // Accumulate cumulative deltas per tried value, averaging repeats.
            if let Some(&(_, v0, ..)) = edges.first() {
                nodes.push((v0, 0.0, 1));
            }
            for (_, vf, vt, d) in edges {
                let Some(cum_f) = nodes.iter().find(|n| (n.0 - vf).abs() < 1e-3).map(|n| n.1)
                else {
                    continue;
                };
                let cum_t = cum_f + d;
                match nodes.iter_mut().find(|n| (n.0 - vt).abs() < 1e-3) {
                    Some(n) => {
                        n.1 = (n.1 * n.2 as f32 + cum_t) / (n.2 + 1) as f32;
                        n.2 += 1;
                    }
                    None => nodes.push((vt, cum_t, 1)),
                }
            }
            nodes.sort_by(|x, y| x.0.total_cmp(&y.0));
            // Tried values whose only measurements were too weak (single-lap
            // side) or channel-dirty to join the curve get a PROVISIONAL
            // point anchored on their clean from-node: a value the user
            // drove must never be invisible in the setup history. Excluded
            // from the fit and the vertex.
            for m in fam_all.iter().filter(|m| m.weak || !m.clean) {
                if m.key.as_deref() != Some(key) {
                    continue;
                }
                let Some(d) = m.outcome.delta_s() else {
                    continue;
                };
                let (Some(vf), Some(vt)) = (value_of(m.i), value_of(m.j)) else {
                    continue;
                };
                if nodes.iter().any(|n| (n.0 - vt).abs() < 1e-3)
                    || provisional.iter().any(|p| (p.0 - vt).abs() < 1e-3)
                {
                    continue;
                }
                let Some(cum_f) = nodes.iter().find(|n| (n.0 - vf).abs() < 1e-3).map(|n| n.1)
                else {
                    continue;
                };
                provisional.push((vt, cum_f + d));
            }
            provisional.sort_by(|x, y| x.0.total_cmp(&y.0));
        }

        let pts: Vec<(f32, f32)> = nodes.iter().map(|n| (n.0, n.1)).collect();
        let fit = quad_fit(&pts).map(|(a, b, c)| (a as f32, b as f32, c as f32));
        // Display-space copy for everything the user SEES (chart nodes,
        // fitted curve, "mapped so far" listings): unit-bearing sliders
        // (aero, springs, ride height, pressures) render in the session's
        // units while every decision below stays canonical.
        let (dk, ddp) = key
            .as_deref()
            .and_then(|k| crate::advice::tuning::display_spec(k, &session.facts))
            .map_or((1.0, 4u32), |(f, dp, _)| (f, dp as u32));
        let dround = |v: f32| {
            let p = 10f32.powi(ddp as i32);
            (v * dk * p).round() / p
        };
        let disp_nodes: Vec<(f32, f32, usize)> =
            nodes.iter().map(|n| (dround(n.0), n.1, n.2)).collect();
        let disp_provisional: Vec<(f32, f32)> =
            provisional.iter().map(|p| (dround(p.0), p.1)).collect();
        let disp_fit = if (dk - 1.0).abs() < 1e-9 {
            fit
        } else {
            let dpts: Vec<(f32, f32)> = disp_nodes.iter().map(|n| (n.0, n.1)).collect();
            quad_fit(&dpts).map(|(a, b, c)| (a as f32, b as f32, c as f32))
        };
        let (lo, hi) = nodes.iter().fold((f32::MAX, f32::MIN), |(lo, hi), n| {
            (lo.min(n.1), hi.max(n.1))
        });
        let mut vertex_out = None;
        if let (Some((a, b, _)), Some(key)) = (fit, key.as_deref())
            && a > 0.0
            && nodes.len() >= 3
            && hi - lo >= 0.10
        {
            let mut vertex = -b / (2.0 * a);
            let (vmin, vmax) = (nodes.first().unwrap().0, nodes.last().unwrap().0);
            if vertex >= vmin && vertex <= vmax {
                if let Some(lim) = crate::advice::tuning::limit_of(&session.facts, key) {
                    vertex = vertex.clamp(lim.0, lim.1);
                }
                let step = crate::advice::tuning::slider_step(key);
                // Reciprocal form keeps tenths exact in f32 (4.2, not 4.2000003).
                let vertex = (vertex * (1.0 / step)).round() / (1.0 / step);
                vertex_out = Some(vertex);
                let phrase = crate::advice::tuning::field_phrase(key);
                // Already there? Then the ask is NOTHING; repeats tighten
                // the estimate, but no change is being requested.
                let at_optimum = c
                    .setups
                    .last()
                    .copied()
                    .flatten()
                    .and_then(|b| b.values.get(key)?.parse::<f32>().ok())
                    .is_some_and(|cur| (cur - vertex).abs() < step * 0.5);
                let landscape = nodes_summary(&disp_nodes);
                let disp =
                    crate::advice::tuning::display_value(key, &vertex.to_string(), &session.facts);
                // A fitted optimum away from the current setting deserves a
                // recommendation even when no behavioural rule speaks for the
                // family (the pressure rule is blind on cars whose temps
                // never leave the band; the landscape is not).
                if !at_optimum
                    && !recs
                        .iter()
                        .any(|r| r.implied.is_some_and(|i| i.family == family))
                {
                    recs.push(recommend::Recommendation {
                        kind: recommend::Kind::Hone,
                        apply: Vec::new(),
                        area: journal::family_area(family),
                        suggestion: None,
                        advice: String::new(),
                        evidence: Vec::new(),
                        confidence: recommend::Confidence::Medium,
                        probe: false,
                        implied: Some(journal::Change {
                            family,
                            softer: false,
                            magnitude: None,
                        }),
                    });
                }
                for r in recs
                    .iter_mut()
                    .filter(|r| r.implied.is_some_and(|i| i.family == family))
                {
                    let cur = c
                        .setups
                        .last()
                        .copied()
                        .flatten()
                        .and_then(|b| b.values.get(key)?.parse::<f32>().ok());
                    // Convergence honesty: "optimum" is a bracket
                    // claim, not proof. Quote the fit's own expected gain for
                    // the move; when it is under the campaign's measured
                    // noise floor, holding is equally defensible and the
                    // advice must say so instead of implying a sure win.
                    let floor = c.drift_floor.map_or(0.10, |(_, f)| f.max(0.10));
                    let gain = cur.map(|v| a * (v - vertex) * (v - vertex));
                    if at_optimum {
                        r.suggestion = Some(format!("{phrase}: hold {disp}"));
                        r.advice = format!(
                            "no change asked: the current setting is the \
                             bracketed optimum estimate. Further narrowing is \
                             expected to gain less than the ±{floor:.2}s noise \
                             floor. Any run here tightens the estimate for free."
                        );
                        r.probe = false;
                        r.implied = None;
                        r.apply.clear();
                        r.kind = recommend::Kind::Hold;
                    } else {
                        r.suggestion = Some(format!("{phrase}: {disp}"));
                        r.apply = vec![(key.to_string(), vertex.to_string())];
                        // An expected gain under the noise floor is a data
                        // request, not a move claimed to gain time.
                        r.probe = matches!(gain, Some(g) if g < floor);
                        r.advice = match gain {
                            Some(g) if g < floor => format!(
                                "estimated optimum, but minimal predicted gain: \
                                {g:.2}s, which is within the ±{floor:.2}s noise floor. \
                                Set {phrase} to {disp}."
                            ),
                            _ => format!("estimated optimum. Set {phrase} to {disp}."),
                        };
                        r.implied = Some(journal::Change {
                            family,
                            softer: vertex < cur.unwrap_or(vertex),
                            magnitude: None,
                        });
                        r.kind = recommend::Kind::Hone;
                    }
                    r.confidence = recommend::Confidence::Medium;
                    r.evidence.push(format!(
                        "measured landscape ({phrase}): {landscape} (cumulative \
                         verdict delta vs first tried value; lower = faster)"
                    ));
                }
            }
        }
        // The best MEASURED node beats the current setting with no fitted
        // optimum to point elsewhere: returning to a measured state outranks
        // bisection between it and here. (The fit path owns interior optima.)
        if vertex_out.is_none()
            && let Some(key) = key.as_deref()
            && let Some(cur) = c
                .setups
                .last()
                .copied()
                .flatten()
                .and_then(|b| b.values.get(key)?.parse::<f32>().ok())
            && let Some(best) = nodes.iter().min_by(|a, b| a.1.total_cmp(&b.1)).copied()
            && let Some(cur_node) = nodes.iter().find(|n| (n.0 - cur).abs() < 1e-3)
            && (best.0 - cur).abs() > 1e-3
            && cur_node.1 - best.1 >= c.drift_floor.map_or(0.10, |(_, f)| f.max(0.10))
        {
            let phrase = crate::advice::tuning::field_phrase(key);
            let gap = cur_node.1 - best.1;
            let bdisp =
                crate::advice::tuning::display_value(key, &best.0.to_string(), &session.facts);
            for r in recs
                .iter_mut()
                .filter(|r| r.implied.is_some_and(|i| i.family == family))
            {
                r.suggestion = Some(format!("{phrase}: {bdisp}"));
                r.apply = vec![(key.to_string(), best.0.to_string())];
                r.advice = format!(
                    "return to the best measured setting: {phrase} {bdisp} beat \
                     the current value by {gap:.2}s."
                );
                r.confidence = recommend::Confidence::Medium;
                r.probe = false;
                r.kind = recommend::Kind::Hone;
                r.implied = Some(journal::Change {
                    family,
                    softer: best.0 < cur,
                    magnitude: None,
                });
                r.evidence.push(format!(
                    "measured landscape ({phrase}): {} (cumulative verdict delta; \
                     lower = faster)",
                    nodes_summary(&disp_nodes),
                ));
                r.evidence.push(
                    "an optimum may sit between the two; a midpoint run is the \
                     exploratory alternative"
                        .into(),
                );
            }
        }

        // No interior optimum mapped: the workflow's data ask. One stint at
        // a specific value past the good edge extends the landscape where it
        // matters. Not an optimization claim; explicitly a probe.
        if vertex_out.is_none()
            && let Some(key) = key.as_deref()
            && let Some(v) = probe_value(
                &nodes,
                crate::advice::tuning::limit_of(&session.facts, key),
                crate::advice::tuning::slider_step(key),
            )
        {
            let phrase = crate::advice::tuning::field_phrase(key);
            let best = nodes
                .iter()
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|n| n.0)
                .unwrap_or(v);
            let vdisp = crate::advice::tuning::display_value(key, &v.to_string(), &session.facts);
            recs.push(recommend::Recommendation {
                kind: recommend::Kind::Hone,
                apply: vec![(key.to_string(), v.to_string())],
                area: "probe",
                suggestion: Some(format!("{phrase}: {vdisp}")),
                advice: format!(
                    "mapping unfinished. Set {phrase} to {vdisp} to \
                    progress exploration."
                ),
                evidence: vec![format!(
                    "mapped so far: {} (cumulative verdict delta; lower = faster)",
                    nodes_summary(&disp_nodes),
                )],
                confidence: recommend::Confidence::Low,
                probe: true,
                implied: Some(journal::Change {
                    family,
                    softer: v < best,
                    magnitude: None,
                }),
            });
        }
        landscapes.push(LandscapeView {
            area: journal::family_area(family),
            phrase: key
                .as_deref()
                .map(|k| crate::advice::tuning::field_phrase(k).to_string())
                .unwrap_or_else(|| journal::family_area(family).to_string()),
            key,
            nodes: disp_nodes,
            provisional: disp_provisional,
            fit: disp_fit,
            vertex: vertex_out.map(dround),
            measurements: mviews,
        });
    }

    // ---- effect-map prior: untried levers ----
    // The cross-campaign map (tuners map) is a PRIOR: families this campaign
    // has measured are owned by the local evidence above and never touched.
    // For the rest, estimate which behavioural direction has been profitable
    // here (per-field pace correlation) and surface the best-aligned map
    // cell as one Low-confidence experiment suggestion.
    if let Some(met) = c.stints.last().and_then(|s| s.met.as_ref()) {
        let emap = std::fs::read_to_string(crate::util::data_path("effect-map.tsv"))
            .ok()
            .and_then(|t| crate::advice::effectmap::parse(&t).ok())
            .unwrap_or_default();
        let pairs: Vec<(effects::Effects, f32)> = c
            .measurements
            .iter()
            .filter(|m| !m.weak)
            .filter_map(|m| Some((m.effects.clone(), m.outcome.delta_s()?)))
            .collect();
        let mut trends = crate::advice::effectmap::pace_trends(&pairs, Some(&c.effect_floor));
        // Cold start / thin campaigns: the driver's pooled trends from OTHER
        // cars' campaigns fill fields this campaign can't speak on yet. The
        // campaign's own trend always wins per field; the current car is
        // excluded from history wholesale (its campaigns would double-count,
        // and other builds of it are invalidated by upgrades anyway).
        let car = c.stints.last().and_then(|s| s.car());
        for t in crate::advice::effectmap::driver_trends(&emap, "local", car) {
            if !trends.iter().any(|c| c.key == t.key) {
                trends.push(t);
            }
        }
        if std::env::var_os("TUNERS_MAP_TRACE").is_some() {
            eprintln!("map-prior trace: {} pairs, trends:", pairs.len());
            for t in &trends {
                eprintln!(
                    "  {} r={:+.2} n={}{}",
                    t.key,
                    t.r,
                    t.n,
                    if t.history { " (history)" } else { "" }
                );
            }
        }
        let ctx = crate::advice::effectmap::MapContext {
            drivetrain: met.drivetrain_type,
            surface_loose: met.surface_loose,
            aero: rule_context(&session).aero_tunable,
        };
        let (cells, landscapes) =
            crate::advice::priors::merged_view(&emap, met.surface_loose, met.drivetrain_type);
        if let Some(rec) = map_prior(
            &cells,
            &landscapes,
            &trends,
            &ctx,
            &recs,
            &enrich::PriorInputs {
                measurements: &c.measurements,
                baseline: session.latest(),
                facts: &session.facts,
            },
            c.stints.last().map(|s| s.fx()),
        ) {
            recs.push(rec);
        }
    }

    if let Some(rec) = composition_proposal(&c.latest(), &c.setups, &session.facts) {
        recs.push(rec);
    }

    // Drift-aware honesty: a High-confidence conclusion resting on a single
    // comparison whose margin is under the measured same-setup drift gets
    // capped and labeled. Multi-point landscapes are less exposed (averaged
    // nodes), so curve-based Medium suggestions stand with a note.
    if let Some((pairs, floor)) = c.drift_floor {
        for r in recs.iter_mut() {
            let Some(implied) = r.implied else { continue };
            let Some(m) = latest.iter().find(|m| m.change.family == implied.family) else {
                continue;
            };
            let Some(margin) = m.outcome.delta_s().map(f32::abs) else {
                continue;
            };
            if margin < floor {
                r.evidence.push(format!(
                    "provisional: the deciding margin ({margin:.2}s) is under the \
                     measured same-setup drift (±{floor:.2}s over {pairs} repeat \
                     pair{}); corroborate before trusting it",
                    if pairs == 1 { "" } else { "s" },
                ));
                if r.confidence == recommend::Confidence::High {
                    r.confidence = recommend::Confidence::Medium;
                }
            }
        }
    }

    // History-only recs arrive unsorted; tier first (fix > hone > explore >
    // hold — a hold must never headline over an actionable ask), confidence
    // within tier. Lints join below and get the same order at the end.
    recs.sort_by_key(|r| (r.kind.rank_group(), std::cmp::Reverse(r.confidence)));
    // Cite tune absolutes only when the journal's stints are the session
    // car's; an explicitly passed foreign journal must not quote this car's
    // sliders as if they were its own.
    let current_tune = if last.car() == session.car {
        let lints = setup_lints(&session, &c.measurements, &recs, last.met.as_ref());
        recs.extend(lints);
        let tune = enrich_with_tune(&mut recs, &session);
        // The scale was measured on the LAST stint; resolve it against the
        // final drive that stint was actually driven on, not the latest
        // saved revision (which may already hold the applied fix).
        let driven_fd = c
            .setups
            .last()
            .copied()
            .flatten()
            .and_then(|rev| rev.values.get("final_drive"))
            .and_then(|v| v.parse::<f32>().ok());
        enrich::apply_fd_scale(&mut recs, &session, fd_scale, driven_fd);
        tune
    } else {
        Vec::new()
    };

    // The suggestion headline: the concrete setting to try. With the last
    // stint's setup on file, an ABSOLUTE value ("front arb: 17.5"), clamped
    // to the slider's range; blind, the DELTA to apply ("front arb: +0.5").
    // Values base on the last stint's setup (the state the advice was
    // judged against), never a saved-but-undriven revision.
    let round = |v: f32| (v * 1e4).round() / 1e4;
    let base = c.setups.last().copied().flatten();
    for r in recs.iter_mut() {
        if r.suggestion.is_some() {
            continue;
        }
        let Some(implied) = r.implied else { continue };
        let Some(delta) = implied.magnitude else {
            continue;
        };
        let Some(key) = latest
            .iter()
            .find(|m| m.change.family == implied.family)
            .and_then(|m| m.key.as_deref())
        else {
            continue;
        };
        let phrase = crate::advice::tuning::field_phrase(key);
        let step = crate::advice::tuning::slider_step(key);
        // The headline now carries the value; the relative phrasing from
        // blind-mode reconciliation becomes redundant.
        r.advice = r
            .advice
            .replace(&journal::slider_units_phrase(implied.family, delta), "");
        r.suggestion = match base.and_then(|b| b.values.get(key)?.parse::<f32>().ok()) {
            Some(cur) => {
                let mut target = cur + delta;
                if let Some(lim) = crate::advice::tuning::limit_of(&session.facts, key) {
                    target = target.clamp(lim.0, lim.1);
                }
                // Whole-unit sliders (diff lock, brakes) only take integer
                // positions: the target must land on one.
                if step >= 1.0 {
                    target = (target / step).round() * step;
                }
                // Clamping can land the target back on the current value
                // (slider already at the bound): asking for no change is
                // not a suggestion, and the apply would be a no-op save.
                if (target - cur).abs() < 1e-3 {
                    continue;
                }
                r.apply = vec![(key.to_string(), round(target).to_string())];
                Some(format!(
                    "{phrase}: {}",
                    crate::advice::tuning::display_value(
                        key,
                        &round(target).to_string(),
                        &session.facts
                    ),
                ))
            }
            None => Some(format!(
                "{phrase}: {}{}",
                if delta > 0.0 { "+" } else { "" },
                crate::advice::tuning::display_value(
                    key,
                    &round(delta).to_string(),
                    &session.facts
                ),
            )),
        };
    }

    // Final order: reconciliation and lints may have re-tiered entries
    // after the earlier sort (holds emerge from rewrites).
    recs.sort_by_key(|r| (r.kind.rank_group(), std::cmp::Reverse(r.confidence)));

    Ok(AdviseView {
        journal: Some(journal_path.to_string()),
        steps,
        anchor,
        aba,
        in_progress: c.in_progress.clone(),
        missing: c.missing.clone(),
        no_laps: c.no_laps.clone(),
        landscapes,
        drift_floor: c.drift_floor,
        effect_floor: c.effect_floor.clone(),
        advice_for: last.entry.path.clone(),
        recommendations: recs,
        current_tune,
    })
}
