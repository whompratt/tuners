//! Loading a campaign from its journal: entry tolerance (missing,
//! lap-less, in-progress), setup binding, implicit steps, and the
//! measurement harvest.

use super::*;

/// Trailing "YYYYMMDD-HHMMSS" stamp of a stint filename, comparable with
/// tune revision stamps (same fixed format, so string order = time order).
pub(crate) fn stint_stamp(path: &str) -> Option<&str> {
    let name = Path::new(path).file_stem()?.to_str()?;
    let stamp = name.get(name.len().checked_sub(15)?..)?;
    (stamp.as_bytes()[8] == b'-'
        && stamp
            .bytes()
            .enumerate()
            .all(|(i, b)| i == 8 || b.is_ascii_digit()))
    .then_some(stamp)
}

/// Where a journal's campaign stands, from its boundary markers (comment
/// lines the session archive/resume flow appends; the entry parser skips
/// them). Closed = parked in the archive, nothing joins the trajectory;
/// Since = resumed at that stamp, only newer stints join as implicit steps.
pub(super) enum CampaignBound {
    Open,
    Closed,
    Since(String),
}

/// Whether a journal's campaign is parked in the archive (nothing new can
/// join it) — the effect map's staleness check for closed campaigns.
pub(crate) fn campaign_closed(journal_text: &str) -> bool {
    matches!(campaign_bound(journal_text), CampaignBound::Closed)
}

pub(super) fn campaign_bound(journal_text: &str) -> CampaignBound {
    let mut bound = CampaignBound::Open;
    for line in journal_text.lines() {
        let line = line.trim();
        if line.strip_prefix("# parked ").is_some() {
            bound = CampaignBound::Closed;
        } else if let Some(stamp) = line.strip_prefix("# resumed ") {
            bound = CampaignBound::Since(stamp.trim().to_string());
        }
    }
    bound
}

/// A journaled stint whose file was deleted (dashboard delete) is skipped,
/// but its note describes setup changes that really happened, so it merges into
/// the NEXT entry's note so cumulative slider positions stay honest (that
/// step honestly becomes a compound). A trailing deleted entry just drops:
/// its changes have no driven stint. Returns (kept entries, missing paths).
pub(super) fn drop_missing_entries(
    entries: Vec<journal::Entry>,
    exists: impl Fn(&str) -> bool,
) -> (Vec<journal::Entry>, Vec<String>) {
    let mut missing = Vec::new();
    let mut kept: Vec<journal::Entry> = Vec::with_capacity(entries.len());
    let mut carry: Option<String> = None;
    for mut entry in entries {
        if !exists(&entry.path) {
            missing.push(entry.path.clone());
            carry = match (carry.take(), entry.note.take()) {
                (Some(a), Some(b)) => Some(format!("{a}; {b}")),
                (a, b) => a.or(b),
            };
            continue;
        }
        if let Some(c) = carry.take() {
            entry.note = Some(match entry.note.take() {
                Some(n) => format!("{c}; {n}"),
                None => c,
            });
        }
        kept.push(entry);
    }
    (kept, missing)
}

/// Stint recordings in `dir` whose first driving frame matches `car` (any
/// car when None), newest first. Lazy: the car check opens each file only
/// as the iterator is advanced.
pub fn stints_for_car_newest_first(
    dir: &str,
    car: Option<i32>,
) -> impl Iterator<Item = String> + use<> {
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "ftel"))
        .collect();
    paths.sort();
    paths.into_iter().rev().filter_map(move |p| {
        let matches = match car {
            None => true,
            Some(car) => crate::api::stint_car(&p) == Some(car),
        };
        matches.then(|| p.to_string_lossy().into_owned())
    })
}

/// Newest stint recording in `dir` whose first driving frame matches `car`
/// (any car when None).
pub fn latest_stint_for_car(dir: &str, car: Option<i32>) -> Option<String> {
    stints_for_car_newest_first(dir, car).next()
}

/// One measured effect for a family, harvested from the campaign: a stint
/// pair whose setups isolate it (DIRECT: setups differ in exactly one area,
/// a clean A/B regardless of how many steps lie between) or an adjacent-step
/// note reading (single-family notes measure on the total delta, compound
/// notes get channel-attributed per family, capped Medium downstream). The
/// unit of evidence for reconciliation, landscapes, and the cross-campaign
/// effect map.
pub(crate) struct Measurement {
    pub change: journal::Change,
    pub outcome: journal::Outcome,
    pub desc: String,
    pub attributed: Option<String>,
    pub weak: bool,
    /// Stint indices of the underlying pair (from, to).
    pub i: usize,
    pub j: usize,
    pub direct: bool,
    /// The single slider the measurement moved, when identifiable;
    /// lets advice resolve concrete target values.
    pub key: Option<String>,
    /// (entry, exit, straights) split of the pair's delta.
    pub split: Option<(f32, f32, f32)>,
    /// Fit for response-curve building: direct pairs and single-family
    /// notes always; attributed compound clauses only when every sibling
    /// clause is judged on a disjoint channel (a corner-channel sibling
    /// would contaminate the curve).
    pub clean: bool,
    /// Behavioural movement of the stint pair. For an attributed compound
    /// clause this is the WHOLE pair's movement; siblings share it.
    pub effects: effects::Effects,
}

/// One journaled stint, loaded and profiled, with its per-stint analysis
/// products and the comparison against its chronological neighbor.
pub(crate) struct CampaignStint {
    pub entry: journal::Entry,
    pub stint: analysis::Stint,
    pub profile: analysis::profile::StintProfile,
    /// Overall metrics of the longest driving segment (None when too short).
    pub met: Option<analysis::metrics::StintMetrics>,
    /// Effect vector from `met` (empty when metrics are absent).
    pub fx: effects::Effects,
    /// Parsed clauses of the journal note.
    pub changes: Vec<journal::Change>,
    /// "suspect" in the note is the driver's own verdict on the stint
    /// (unfamiliar car, chaotic drive, traffic): every measurement touching
    /// it is weak — kept visible, never trusted alone.
    pub suspect: bool,
    /// Comparison vs the previous stint: (ideal delta, phase attribution),
    /// or why it isn't comparable. None for the first stint.
    pub vs_prev: Option<Result<(f32, analysis::attribution::Attribution), String>>,
}

/// A stint-pair comparison is THIN when either side ran a single flying lap
/// (no corroboration) or failed the splice-trust gate.
pub(super) fn pair_thin(stints: &[CampaignStint], i: usize, j: usize) -> bool {
    stints[i]
        .profile
        .laps
        .len()
        .min(stints[j].profile.laps.len())
        < 2
        || !splice_trusted(&stints[i].profile)
        || !splice_trusted(&stints[j].profile)
}

/// WEAK adds the driver's own suspect verdict on either side.
pub(super) fn pair_weak(stints: &[CampaignStint], i: usize, j: usize) -> bool {
    pair_thin(stints, i, j) || stints[i].suspect || stints[j].suspect
}

/// A stint pair's behavioural movement: per-stint field deltas plus the
/// pair-level corner-matched apex speed (position-matched corner runs on the
/// earlier stint's route — computable because campaign stints share one).
pub(super) fn pair_effects(from: &CampaignStint, to: &CampaignStint) -> effects::Effects {
    let mut d = effects::delta(&from.fx, &to.fx);
    if let Some(v) = analysis::attribution::apex_speed_delta(&from.profile, &to.profile) {
        d.push(("apex_speed", v));
    }
    d
}

/// A campaign loaded for analysis: every journaled stint with its per-stint
/// products, setup states, campaign noise floors, and the harvested
/// measurement set. The shared substrate of `advise` and the cross-campaign
/// effect map.
pub(crate) struct Campaign<'s> {
    pub stints: Vec<CampaignStint>,
    /// Setup state per stint: the latest tune revision saved before it began
    /// (None for foreign cars and blind campaigns).
    pub setups: Vec<Option<&'s crate::advice::tuning::Revision>>,
    /// Slider positions relative to baseline, from the note trail.
    pub positions: Vec<(Option<f32>, Option<f32>)>,
    /// Journaled stint with no completed laps yet (still recording).
    pub in_progress: Option<String>,
    /// Journaled stints whose recordings no longer exist.
    pub missing: Vec<String>,
    /// Mid-campaign stints with no completed laps (an event entered and
    /// immediately abandoned auto-cuts a tiny recording): skipped, any note
    /// merged into the next step.
    pub no_laps: Vec<String>,
    /// (same-setup pair count, largest |ideal delta| across them): the
    /// campaign's own outcome noise floor.
    pub drift_floor: Option<(usize, f32)>,
    /// Per-field campaign noise floor from the same same-setup pairs.
    pub effect_floor: effects::Effects,
    pub measurements: Vec<Measurement>,
}

impl Campaign<'_> {
    pub fn thin(&self, i: usize, j: usize) -> bool {
        pair_thin(&self.stints, i, j)
    }

    pub fn weak_pair(&self, i: usize, j: usize) -> bool {
        pair_weak(&self.stints, i, j)
    }

    /// Latest evidence per family: newest endpoint wins; a direct setup A/B
    /// beats a note-based reading of the same endpoint; nearest ancestor
    /// breaks remaining ties (least drift).
    pub fn latest(&self) -> Vec<&Measurement> {
        let mut latest: Vec<&Measurement> = Vec::new();
        for m in &self.measurements {
            match latest
                .iter_mut()
                .find(|l| l.change.family == m.change.family)
            {
                Some(l) => {
                    if (m.j, m.direct, m.i) > (l.j, l.direct, l.i) {
                        *l = m;
                    }
                }
                None => latest.push(m),
            }
        }
        latest
    }
}

/// Stints of the session car recorded AFTER the last journal entry join the
/// trajectory as implicit no-change steps. Journal lines are written on tune
/// saves, so a stint driven without touching anything (the same-setup repeat
/// that measures pure drift) would otherwise be invisible. Campaign
/// boundaries bound the scan: a parked (archived) journal accrues nothing,
/// and a resumed one only takes stints newer than the resume; stints driven
/// in ANOTHER campaign of the same car while this one was parked must not
/// leak in.
pub(crate) fn implicit_steps(
    journal_text: &str,
    entries: &mut Vec<journal::Entry>,
    session_car: Option<i32>,
    stints_dir: &str,
) {
    let bound = campaign_bound(journal_text);
    if matches!(bound, CampaignBound::Closed) {
        return;
    }
    let Some(last_stamp) = entries.last().and_then(|e| stint_stamp(&e.path)) else {
        return;
    };
    let mut last_stamp = last_stamp.to_string();
    if let CampaignBound::Since(s) = &bound
        && s.as_str() > last_stamp.as_str()
    {
        last_stamp = s.clone();
    }
    let mut extra: Vec<String> = std::fs::read_dir(stints_dir)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            (path.extension().is_some_and(|x| x == "ftel")
                && stint_stamp(&path.to_string_lossy()).is_some_and(|s| s > last_stamp.as_str())
                && session_car.is_some()
                && crate::api::stint_car(&path) == session_car)
                .then(|| format!("{stints_dir}/{}", e.file_name().to_string_lossy()))
        })
        .collect();
    extra.sort();
    entries.extend(
        extra
            .into_iter()
            .map(|path| journal::Entry { path, note: None }),
    );
}

/// Load and profile a campaign's journal entries, in chronological order,
/// and harvest every stint pair as evidence. The LAST entry may still be
/// recording (journaled at the tune save, no completed laps yet): dropped
/// gracefully into `in_progress`. A middle entry failing is real data
/// trouble and stays a hard error. `label` names the campaign in errors
/// (the journal path for advise).
pub(crate) fn load_campaign<'s>(
    entries: Vec<journal::Entry>,
    session: &'s TuningSession,
    label: &str,
) -> Result<Campaign<'s>, String> {
    let (entries, missing) = drop_missing_entries(entries, |p| Path::new(p).exists());
    if entries.is_empty() {
        return Err(format!(
            "{label}: every journaled stint recording is missing; the files \
             were deleted"
        ));
    }

    let mut stints: Vec<CampaignStint> = Vec::new();
    let mut in_progress = None;
    let mut no_laps: Vec<String> = Vec::new();
    // Note of a skipped lap-less stint, merged into the next step so slider
    // positions stay honest (same contract as missing recordings).
    let mut carry: Option<String> = None;
    let last = entries.len() - 1;
    for (i, mut entry) in entries.into_iter().enumerate() {
        if let Some(c) = carry.take() {
            entry.note = Some(match entry.note.take() {
                Some(n) => format!("{c}; {n}"),
                None => c,
            });
        }
        let stint = analysis::Stint::load(entry.path.as_ref())
            .map_err(|e| format!("{}: {e}", entry.path))?;
        let profile = match analysis::profile::stint_profile(&stint.frames) {
            Ok(profile) => profile,
            Err(_) if i == last => {
                in_progress = Some(entry.path.clone());
                continue;
            }
            Err(_) => {
                // A lap-less middle stint (an event entered and abandoned in
                // the pause menu auto-cuts into a tiny recording) is a menu
                // artifact, not data trouble: skip it. Anything unreadable
                // still fails hard at Stint::load above.
                no_laps.push(entry.path.clone());
                carry = entry.note.take();
                continue;
            }
        };
        let met = stint_overall_metrics(&stint);
        let fx = met.as_ref().map(effects::vector).unwrap_or_default();
        let changes = entry
            .note
            .as_deref()
            .map(journal::parse_clauses)
            .unwrap_or_default();
        let suspect = entry
            .note
            .as_deref()
            .is_some_and(|n| n.to_lowercase().contains("suspect"));
        let vs_prev = stints.last().map(|prev: &CampaignStint| {
            analysis::compare::compare(&prev.profile, &profile).map(|cmp| {
                let attr = analysis::attribution::split_delta(&prev.profile, &cmp.bin_delta_s);
                (cmp.ideal_delta_s, attr)
            })
        });
        stints.push(CampaignStint {
            entry,
            stint,
            profile,
            met,
            fx,
            changes,
            suspect,
            vs_prev,
        });
    }
    if stints.is_empty() {
        return Err(format!(
            "{label}: no stints with completed laps in the journal yet; drive a lap first"
        ));
    }
    let n = stints.len();

    let all_changes: Vec<Vec<journal::Change>> = stints.iter().map(|s| s.changes.clone()).collect();
    let positions = journal::track_positions(&all_changes);

    // Setup state per stint: the latest tune revision saved before the stint
    // began. Only bound when the stint really is the session car's, since an
    // explicitly passed foreign journal must not inherit this car's tunes.
    let setups: Vec<Option<&'s crate::advice::tuning::Revision>> = stints
        .iter()
        .map(|cs| {
            let car = car_of(&cs.stint);
            if car.is_none() || car != session.car {
                return None;
            }
            let stamp = stint_stamp(&cs.entry.path)?;
            session
                .revisions
                .iter()
                .rev()
                .find(|r| r.stamp.as_str() < stamp)
        })
        .collect();

    // The campaign's own noise floor: |ideal delta| across SAME-setup stint
    // pairs is pure driver/track drift. Verdicts with margins below the
    // worst observed drift are provisional, and advice must say so.
    let mut drift_obs: Vec<f32> = Vec::new();
    let mut effect_floor: effects::Effects = Vec::new();
    for j in 1..n {
        for i in 0..j {
            let (Some(si), Some(sj)) = (setups[i], setups[j]) else {
                continue;
            };
            if !crate::advice::tuning::diff_keys(si, sj).is_empty() {
                continue;
            }
            if pair_thin(&stints, i, j) {
                continue;
            }
            if let Ok(cmp) = analysis::compare::compare(&stints[i].profile, &stints[j].profile) {
                drift_obs.push(cmp.ideal_delta_s.abs());
                // Same-setup behavioural movement is pure drift too: the
                // campaign's own per-field noise floor.
                effects::fold_floor(&mut effect_floor, &pair_effects(&stints[i], &stints[j]));
            }
        }
    }
    let drift_floor = (!drift_obs.is_empty()).then(|| {
        (
            drift_obs.len(),
            drift_obs.iter().fold(0.0f32, |a, b| a.max(*b)),
        )
    });

    // ------ campaign measurements: every stint pair is evidence ------
    // Reconciliation then uses each family's LATEST measurement, so
    // knowledge from earlier steps keeps tempering advice instead of
    // evaporating when the experiment topic changes.
    let mut measurements: Vec<Measurement> = Vec::new();
    for j in 1..n {
        for i in 0..j {
            let (Some(si), Some(sj)) = (setups[i], setups[j]) else {
                continue;
            };
            let keys = crate::advice::tuning::diff_keys(si, sj);
            if keys.is_empty() {
                continue;
            }
            let [area] = area_list(&keys)[..] else {
                continue;
            };
            let Some(family) = journal::family_for_area(area) else {
                continue;
            };
            let Ok(cmp) = analysis::compare::compare(&stints[i].profile, &stints[j].profile) else {
                continue;
            };
            let mattr = analysis::attribution::split_delta(&stints[i].profile, &cmp.bin_delta_s);
            let vals: Vec<f32> = keys
                .iter()
                .filter_map(|k| {
                    Some(
                        sj.values.get(k)?.parse::<f32>().ok()?
                            - si.values.get(k)?.parse::<f32>().ok()?,
                    )
                })
                .collect();
            measurements.push(Measurement {
                change: journal::Change {
                    family,
                    softer: vals.iter().sum::<f32>() < 0.0,
                    magnitude: (vals.len() == 1).then(|| vals[0]),
                },
                outcome: journal::judge(cmp.ideal_delta_s),
                desc: format!(
                    "{} (steps {}→{})",
                    crate::advice::tuning::diff_note(si, sj),
                    i + 1,
                    j + 1
                ),
                attributed: None,
                weak: pair_weak(&stints, i, j),
                i,
                j,
                direct: true,
                key: Some(keys[0].clone()),
                split: Some((
                    mattr.entry_delta_s,
                    mattr.exit_delta_s,
                    mattr.straight_delta_s,
                )),
                clean: true,
                effects: pair_effects(&stints[i], &stints[j]),
            });
        }
    }
    for j in 1..n {
        let Some(note) = stints[j].entry.note.clone() else {
            continue;
        };
        let Some(Ok((delta, attr))) = stints[j].vs_prev else {
            continue;
        };
        if let Some(change) = journal::parse_change(&note) {
            measurements.push(Measurement {
                change,
                outcome: journal::judge(delta),
                desc: note.clone(),
                attributed: None,
                weak: pair_weak(&stints, j - 1, j),
                i: j - 1,
                j,
                direct: false,
                key: key_from_phrase(&note),
                split: Some((attr.entry_delta_s, attr.exit_delta_s, attr.straight_delta_s)),
                clean: true,
                effects: pair_effects(&stints[j - 1], &stints[j]),
            });
        } else {
            let evidence = format!(
                "outcome attributed from a compound step (\"{note}\"): corner entry \
                 {:+.2}s / exit {:+.2}s / straights {:+.2}s of {delta:+.2}s total \
                 ({:.0}% of lap time is cornering); inferred from where the time \
                 moved, not measured in isolation",
                attr.entry_delta_s,
                attr.exit_delta_s,
                attr.straight_delta_s,
                attr.corner_share * 100.0,
            );
            let clauses: Vec<journal::Change> = journal::parse_clauses(&note);
            let mut seen = Vec::new();
            for clause_text in note.split(';').map(str::trim) {
                let Some(clause) = journal::parse_change(clause_text) else {
                    continue;
                };
                if seen.contains(&clause.family) {
                    continue;
                }
                seen.push(clause.family);
                // Judged on the road the family's fingerprint lives on.
                // Calibrated 2026-07-21: brake bias shows cleanly on entry
                // even inside a compound step; diff lock (accel AND decel)
                // measured SPREAD across phases -> corner total.
                let channel_delta = match clause.family {
                    journal::Family::Gearing => attr.straight_delta_s,
                    journal::Family::Brakes => attr.entry_delta_s,
                    _ => attr.corner_delta_s,
                };
                measurements.push(Measurement {
                    change: clause,
                    outcome: journal::judge(channel_delta),
                    desc: clause_text.to_string(),
                    attributed: Some(evidence.clone()),
                    weak: pair_weak(&stints, j - 1, j),
                    i: j - 1,
                    j,
                    direct: false,
                    key: key_from_phrase(clause_text),
                    split: Some((attr.entry_delta_s, attr.exit_delta_s, attr.straight_delta_s)),
                    effects: pair_effects(&stints[j - 1], &stints[j]),
                    clean: clauses.iter().all(|c| {
                        // Judged-channel overlap: gearing reads straights,
                        // brakes reads entry, everything else the corner
                        // total (entry included). Siblings on a disjoint
                        // channel can't contaminate this clause's reading.
                        let chan = |f: journal::Family| match f {
                            journal::Family::Gearing => 0u8, // straights
                            journal::Family::Brakes => 1,    // entry
                            _ => 2,                          // corner total
                        };
                        let (a, b) = (chan(clause.family), chan(c.family));
                        c.family == clause.family || (a != b && !(a >= 1 && b >= 1)) // entry ⊂ corner
                    }),
                });
            }
        }
    }

    Ok(Campaign {
        stints,
        setups,
        positions,
        in_progress,
        missing,
        no_laps,
        drift_floor,
        effect_floor,
        measurements,
    })
}
