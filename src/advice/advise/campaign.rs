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
pub(crate) enum CampaignBound {
    Open,
    Closed,
    Since(String),
}

/// Whether a journal's campaign is parked in the archive (nothing new can
/// join it) — the effect map's staleness check for closed campaigns.
pub(crate) fn campaign_closed(journal_text: &str) -> bool {
    matches!(campaign_bound(journal_text), CampaignBound::Closed)
}

pub(crate) fn campaign_bound(journal_text: &str) -> CampaignBound {
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

/// Grip-curve pooling over the campaign (plan 015): pool every tarmac
/// stint's cornering samples, fit the car's curves once, and stamp each
/// stint's PUSH/SLIDE occupancy into its metrics. A thin campaign pool
/// (< POOL_TARGET) pulls the car's other recordings in, newest first
/// (crosses setups — labeled CarPool so downstream consumers know). Dirt
/// stints neither feed nor receive the fit (its curves are a separate
/// regime, deferred).
pub(crate) fn attach_saturation(stints: &mut [CampaignStint], stints_dir: &str) {
    use crate::analysis::grip;
    // A stint contributes its cached cornering samples unless dirt (its
    // curves are a separate regime) or too short for metrics.
    let contributes = |cs: &CampaignStint| {
        cs.met.as_ref().is_some_and(|m| !m.surface_loose) && !cs.data.samples.is_empty()
    };
    let mut pooled: Vec<grip::GripSample> = stints
        .iter()
        .filter(|cs| contributes(cs))
        .flat_map(|cs| cs.data.samples.iter().copied())
        .collect();
    // Cross-recording stability is the point of pooling: a single-stint
    // campaign is no better than a self-fit (measured push 0.1-19.6% on
    // healthy stints), so at least two recordings must contribute before
    // the pool carries a detection-grade label.
    let contributing = stints.iter().filter(|cs| contributes(cs)).count();
    let mut source = if contributing >= 2 {
        grip::CurveSource::Campaign
    } else {
        grip::CurveSource::SelfFit
    };
    if pooled.len() < grip::POOL_TARGET || contributing < 2 {
        let campaign_car = stints.iter().find_map(|cs| cs.car());
        let have: Vec<Option<std::ffi::OsString>> = stints
            .iter()
            .map(|cs| {
                Path::new(cs.entry.path.as_str())
                    .file_name()
                    .map(Into::into)
            })
            .collect();
        let mut recordings = contributing;
        for path in stints_for_car_newest_first(stints_dir, campaign_car) {
            // Aim well past FIT_MIN: sub-20k pools misread (grip::BIN_MIN
            // is absolute).
            if pooled.len() >= grip::CAR_POOL_SIBLINGS && recordings >= 2 {
                break;
            }
            // Separator-safe exclusion of the campaign's own recordings.
            if have.contains(&Path::new(&path).file_name().map(Into::into)) {
                continue;
            }
            let Ok(sib) = analysis::products::cached(path.as_ref()) else {
                continue;
            };
            if sib.samples.is_empty() || sib.met.as_ref().is_none_or(|m| m.surface_loose) {
                continue;
            }
            recordings += 1;
            pooled.extend(sib.samples.iter().copied());
            source = grip::CurveSource::CarPool;
        }
    }
    let Some(curves) = grip::fit_curves(&pooled) else {
        return;
    };
    for cs in stints.iter_mut() {
        if !contributes(cs) {
            continue;
        }
        let occ = grip::occupancy(&cs.data.samples, &curves, source);
        if let Some(m) = cs.met.as_mut() {
            m.grip_saturation = occ;
        }
    }
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

/// One journaled stint's cached analysis products, with the per-campaign
/// comparison against its chronological neighbor. Frames are never held:
/// `data` is the compact distillation (plan 018 — a campaign of raw
/// recordings in RAM was the analysis-freeze memory hazard).
pub(crate) struct CampaignStint {
    pub entry: journal::Entry,
    pub data: std::sync::Arc<analysis::products::StintData>,
    /// Overall metrics of the longest driving segment (None when too short).
    /// Cloned out of `data`: the campaign stamps its own pooled
    /// grip-saturation into it, which must not leak into the shared cache.
    pub met: Option<analysis::metrics::StintMetrics>,
    /// Parsed clauses of the journal note.
    pub changes: Vec<journal::Change>,
    /// "suspect" in the note is the driver's own verdict on the stint
    /// (unfamiliar car, chaotic drive, traffic): every measurement touching
    /// it is weak — kept visible, never trusted alone.
    pub suspect: bool,
    /// Comparison vs the previous stint, or why it isn't comparable. None
    /// for the first stint.
    pub vs_prev: Option<Result<PairVerdict, String>>,
}

impl CampaignStint {
    /// Campaign stints are only constructed from profiled recordings.
    pub fn profile(&self) -> &analysis::profile::StintProfile {
        self.data
            .profile
            .as_ref()
            .expect("campaign stints are profiled")
    }

    /// Effect vector from the stint's metrics.
    pub fn fx(&self) -> &effects::Effects {
        &self.data.fx
    }

    pub fn car(&self) -> Option<i32> {
        self.data.car
    }
}

/// Outcome of comparing a stint against its predecessor: the 2-of-3 vote
/// verdict (median of ideal/best/median-lap deltas), its component
/// currencies for disagreement hedges, and the phase attribution of the
/// composite delta (spatial decomposition explains the ideal component;
/// the vote has no per-bin form).
#[derive(Clone, Copy)]
pub(crate) struct PairVerdict {
    pub verdict_s: f32,
    pub ideal_s: f32,
    pub best_s: f32,
    pub median_lap_s: f32,
    pub attr: analysis::attribution::Attribution,
}

/// A stint-pair comparison is THIN when either side ran a single flying lap
/// (no corroboration) or failed the splice-trust gate.
pub(super) fn pair_thin(stints: &[CampaignStint], i: usize, j: usize) -> bool {
    stints[i]
        .profile()
        .laps
        .len()
        .min(stints[j].profile().laps.len())
        < 2
        || !splice_trusted(stints[i].profile())
        || !splice_trusted(stints[j].profile())
}

/// A single STATE profile is thin on the same grounds.
pub(super) fn state_thin(p: &analysis::profile::StintProfile) -> bool {
    p.laps.len() < 2 || !splice_trusted(p)
}

/// WEAK adds the driver's own suspect verdict on either side.
pub(super) fn pair_weak(stints: &[CampaignStint], i: usize, j: usize) -> bool {
    pair_thin(stints, i, j) || stints[i].suspect || stints[j].suspect
}

/// Consecutive same-setup group id per stint (the group head's index). A
/// new group starts whenever the bound setup changes, either side is
/// unbound, or the standing-start character differs (their laps are not
/// poolable).
pub(super) fn consecutive_groups(
    standing: &[bool],
    setups: &[Option<&crate::advice::tuning::Revision>],
) -> Vec<usize> {
    let n = standing.len();
    let mut groups = vec![0usize; n];
    for k in 1..n {
        let same = match (setups[k - 1], setups[k]) {
            (Some(a), Some(b)) => {
                crate::advice::tuning::diff_keys(a, b).is_empty() && standing[k - 1] == standing[k]
            }
            _ => false,
        };
        groups[k] = if same { groups[k - 1] } else { k };
    }
    groups
}

/// A stint pair's behavioural movement: per-stint field deltas plus the
/// pair-level corner-matched apex speed (position-matched corner runs on the
/// earlier stint's route — computable because campaign stints share one).
pub(super) fn pair_effects(from: &CampaignStint, to: &CampaignStint) -> effects::Effects {
    let mut d = effects::delta(from.fx(), to.fx());
    if let Some(v) = analysis::attribution::apex_speed_delta(from.profile(), to.profile()) {
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
    /// Mid-campaign stints that couldn't contribute: no completed laps (an
    /// event entered and immediately abandoned auto-cuts a tiny recording),
    /// or an implicit stint whose file doesn't decode. Skipped, any note
    /// merged into the next step.
    pub no_laps: Vec<String>,
    /// Consecutive same-setup group per stint (the group head's index): a
    /// repeat run is CORROBORATION of the same state, not an experiment.
    /// Non-consecutive returns to a setup stay separate groups (drift and
    /// A-B-A depend on stint identity).
    pub groups: Vec<usize>,
    /// Pooled profile per group head, for groups with 2+ members: all
    /// members' laps in one profile. State comparisons (anchors, direct
    /// A/Bs) use these; per-stint rows and the drift floor never do.
    pub pooled: std::collections::HashMap<usize, analysis::profile::StintProfile>,
    /// (same-setup pair count, largest |ideal delta| across them): the
    /// campaign's own outcome noise floor.
    pub drift_floor: Option<(usize, f32)>,
    /// Per-field campaign noise floor from the same same-setup pairs.
    pub effect_floor: effects::Effects,
    pub measurements: Vec<Measurement>,
}

impl Campaign<'_> {
    /// The profile representing stint `k`'s SETUP STATE: the pooled
    /// consecutive same-setup group when one exists, else the stint's own.
    pub fn state_profile(&self, k: usize) -> &analysis::profile::StintProfile {
        self.pooled
            .get(&self.groups[k])
            .unwrap_or_else(|| self.stints[k].profile())
    }

    /// Last member index of `k`'s group.
    fn group_end(&self, k: usize) -> usize {
        let g = self.groups[k];
        (g..self.groups.len())
            .take_while(|&m| self.groups[m] == g)
            .last()
            .unwrap_or(k)
    }

    /// "runs 8-9" label when `k`'s state pools 2+ runs; None for singletons.
    pub fn pooled_runs(&self, k: usize) -> Option<String> {
        let (g, e) = (self.groups[k], self.group_end(k));
        (g != e).then(|| format!("runs {}-{}", g + 1, e + 1))
    }

    fn group_suspect(&self, k: usize) -> bool {
        (self.groups[k]..=self.group_end(k)).any(|m| self.stints[m].suspect)
    }

    /// State-aware thinness: judged on the pooled profiles, so a
    /// corroboration re-run lifts a single-lap side out of weakness.
    pub fn thin(&self, i: usize, j: usize) -> bool {
        state_thin(self.state_profile(i)) || state_thin(self.state_profile(j))
    }

    pub fn weak_pair(&self, i: usize, j: usize) -> bool {
        self.thin(i, j) || self.group_suspect(i) || self.group_suspect(j)
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

/// Parked intervals from the journal's boundary markers, in order: each
/// "# parked <stamp>" opens a window that the next "# resumed <stamp>"
/// closes (a still-parked journal leaves the last window open-ended).
fn parked_windows(journal_text: &str) -> Vec<(String, Option<String>)> {
    let mut windows: Vec<(String, Option<String>)> = Vec::new();
    for line in journal_text.lines() {
        let line = line.trim();
        if let Some(stamp) = line.strip_prefix("# parked ") {
            windows.push((stamp.trim().to_string(), None));
        } else if let Some(stamp) = line.strip_prefix("# resumed ")
            && let Some(w) = windows.last_mut()
            && w.1.is_none()
        {
            w.1 = Some(stamp.trim().to_string());
        }
    }
    windows
}

/// Unjournaled stints of the session car recorded since the campaign began
/// join the trajectory as implicit no-change steps, in stamp order. Journal
/// lines are written on tune saves, so a stint driven without touching
/// anything — the same-setup repeat that measures pure drift, or a
/// crash/idle auto-cut mid-campaign — would otherwise be invisible and its
/// corroboration lost (middle stints pool into their setup's state).
/// Parked windows bound the scan: stints driven while this campaign sat in
/// the archive belong to whatever campaign was active at the time and must
/// not leak in.
pub(crate) fn implicit_steps(
    journal_text: &str,
    entries: &mut Vec<journal::Entry>,
    session_car: Option<i32>,
    stints_dir: &str,
) {
    let Some(first_stamp) = entries.first().and_then(|e| stint_stamp(&e.path)) else {
        return;
    };
    let first_stamp = first_stamp.to_string();
    // Stamp-keyed dedup: journal paths may use foreign separators
    // ("sessions\stint-...", written on Windows), the stamp is
    // separator-robust.
    let journaled: std::collections::HashSet<String> = entries
        .iter()
        .filter_map(|e| stint_stamp(&e.path).map(str::to_string))
        .collect();
    let windows = parked_windows(journal_text);
    let parked = |stamp: &str| {
        windows
            .iter()
            .any(|(p, r)| stamp > p.as_str() && r.as_deref().is_none_or(|r| stamp < r))
    };
    let mut extra: Vec<(String, String)> = std::fs::read_dir(stints_dir)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            if path.extension().is_none_or(|x| x != "ftel") {
                return None;
            }
            let name = path.to_string_lossy();
            let stamp = stint_stamp(&name)?.to_string();
            (stamp.as_str() > first_stamp.as_str()
                && !journaled.contains(&stamp)
                && !parked(&stamp)
                && session_car.is_some()
                && crate::api::stint_car(&path) == session_car)
                .then(|| {
                    (
                        stamp,
                        format!("{stints_dir}/{}", e.file_name().to_string_lossy()),
                    )
                })
        })
        .collect();
    extra.sort();
    for (stamp, path) in extra {
        let pos = entries
            .iter()
            .position(|e| stint_stamp(&e.path).is_some_and(|s| s > stamp.as_str()))
            .unwrap_or(entries.len());
        entries.insert(pos, journal::Entry { path, note: None });
    }
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
    let (entries, missing) = drop_missing_entries(entries, |p| {
        crate::util::resolve_data(Path::new(p)).exists()
    });
    if entries.is_empty() {
        return Err(format!(
            "{label}: every journaled stint recording is missing; the files \
             were deleted"
        ));
    }

    use std::time::{Duration, Instant};
    let trace = std::env::var_os("TUNERS_ADVISE_TRACE").is_some();
    let t0 = Instant::now();
    let (mut t_load, mut t_cmp) = (Duration::ZERO, Duration::ZERO);

    let mut stints: Vec<CampaignStint> = Vec::new();
    let mut in_progress = None;
    let mut no_laps: Vec<String> = Vec::new();
    // Note of a skipped lap-less stint, merged into the next step so slider
    // positions stay honest (same contract as missing recordings).
    let mut carry: Option<String> = None;
    let last = entries.len() - 1;
    for (i, mut entry) in entries.into_iter().enumerate() {
        let implicit = entry.note.is_none();
        if let Some(c) = carry.take() {
            entry.note = Some(match entry.note.take() {
                Some(n) => format!("{c}; {n}"),
                None => c,
            });
        }
        let t = Instant::now();
        let data = match analysis::products::cached(entry.path.as_ref()) {
            Ok(data) => data,
            // An implicit no-change stint that doesn't decode (a recording
            // truncated by a crash mid-write) only ever corroborated: skip
            // it rather than wedge the campaign on a file no journal line
            // names. A noted entry stays a hard error — its changes are
            // setup-position truth.
            Err(_) if implicit => {
                no_laps.push(entry.path.clone());
                carry = entry.note.take();
                continue;
            }
            Err(e) => return Err(format!("{}: {e}", entry.path)),
        };
        t_load += t.elapsed();
        if data.profile.is_err() {
            if i == last {
                in_progress = Some(entry.path.clone());
            } else {
                // A lap-less middle stint (an event entered and abandoned in
                // the pause menu auto-cuts into a tiny recording) is a menu
                // artifact, not data trouble: skip it. Anything unreadable
                // still fails hard at the load above.
                no_laps.push(entry.path.clone());
                carry = entry.note.take();
            }
            continue;
        }
        let met = data.met.clone();
        let changes = entry
            .note
            .as_deref()
            .map(journal::parse_clauses)
            .unwrap_or_default();
        let suspect = entry
            .note
            .as_deref()
            .is_some_and(|n| n.to_lowercase().contains("suspect"));
        let t = Instant::now();
        let profile = data.profile.as_ref().expect("checked above");
        let vs_prev = stints.last().map(|prev: &CampaignStint| {
            analysis::compare::compare(prev.profile(), profile).map(|cmp| PairVerdict {
                verdict_s: cmp.verdict_delta_s,
                ideal_s: cmp.ideal_delta_s,
                best_s: cmp.best_lap_delta_s,
                median_lap_s: cmp.median_lap_delta_s,
                attr: analysis::attribution::split_delta(prev.profile(), &cmp.bin_delta_s),
            })
        });
        t_cmp += t.elapsed();
        stints.push(CampaignStint {
            entry,
            data,
            met,
            changes,
            suspect,
            vs_prev,
        });
    }
    if trace {
        let (hits, misses) = analysis::products::cache_counters();
        eprintln!(
            "advise-trace: {} stints in {:.2?} (products {:.2?}, cache {hits} hits / {misses} misses lifetime, neighbor-compare {:.2?})",
            stints.len(),
            t0.elapsed(),
            t_load,
            t_cmp
        );
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
            let car = cs.car();
            if car.is_none() || car != session.car {
                return None;
            }
            let stamp = stint_stamp(&cs.entry.path)?;
            // <= not <: a tune save cuts the stint, so the new stint can carry
            // the SAME second as the revision that cut it — that revision is
            // what the stint was driven on (seen live: a center diff step
            // stamped equal to its stint read as a same-setup drift run).
            session
                .revisions
                .iter()
                .rev()
                .find(|r| r.stamp.as_str() <= stamp)
        })
        .collect();

    // Consecutive same-setup stints form one STATE: repeats are
    // corroboration runs, so state comparisons pool their laps. The drift
    // floor below deliberately keeps RAW stint pairs (it measures per-stint
    // spread), and A-B-A stays per-stint too.
    let standing: Vec<bool> = stints
        .iter()
        .map(|s| s.profile().standing_start_only)
        .collect();
    let groups = consecutive_groups(&standing, &setups);
    let mut last_member: Vec<usize> = (0..n).collect();
    for k in (0..n.saturating_sub(1)).rev() {
        if groups[k + 1] == groups[k] {
            last_member[k] = last_member[k + 1];
        }
    }
    let t_pool = Instant::now();
    let mut pooled: std::collections::HashMap<usize, analysis::profile::StintProfile> =
        std::collections::HashMap::new();
    for g in 0..n {
        if groups[g] != g || last_member[g] == g {
            continue; // not a group head, or a singleton
        }
        let laps: Vec<analysis::profile::LapProfile> = (g..=last_member[g])
            .flat_map(|k| stints[k].profile().laps.iter().cloned())
            .collect();
        let Some(profile) =
            analysis::profile::StintProfile::from_laps(laps, stints[g].profile().car_ordinal)
        else {
            continue;
        };
        pooled.insert(g, profile);
    }
    if trace {
        eprintln!(
            "advise-trace: {} pooled group profiles in {:.2?}",
            pooled.len(),
            t_pool.elapsed()
        );
    }
    let state_profile = |k: usize| -> &analysis::profile::StintProfile {
        pooled
            .get(&groups[k])
            .unwrap_or_else(|| stints[k].profile())
    };
    let group_suspect = |k: usize| (groups[k]..=last_member[k]).any(|m: usize| stints[m].suspect);
    let state_weak = |i: usize, j: usize| {
        state_thin(state_profile(i))
            || state_thin(state_profile(j))
            || group_suspect(i)
            || group_suspect(j)
    };
    let pooled_runs = |k: usize| -> Option<String> {
        (groups[k] != last_member[k])
            .then(|| format!("runs {}-{}", groups[k] + 1, last_member[k] + 1))
    };

    // The campaign's own noise floor: |ideal delta| across SAME-setup stint
    // pairs is pure driver/track drift. Verdicts with margins below the
    // worst observed drift are provisional, and advice must say so.
    let t_drift = Instant::now();
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
            if let Ok(cmp) = analysis::compare::compare(stints[i].profile(), stints[j].profile()) {
                drift_obs.push(cmp.verdict_delta_s.abs());
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
    if trace {
        eprintln!(
            "advise-trace: drift floor ({} same-setup pairs) in {:.2?}",
            drift_obs.len(),
            t_drift.elapsed()
        );
    }
    let t_meas = Instant::now();
    let mut measurements: Vec<Measurement> = Vec::new();
    for j in 1..n {
        for i in 0..j {
            // One measurement per STATE pair: each consecutive same-setup
            // group is represented by its LAST member (latest-wins keys on
            // j, so a fresh corroboration run refreshes the measurement)
            // and compared through its pooled profile.
            if i != last_member[i] || j != last_member[j] {
                continue;
            }
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
            let (pi, pj) = (state_profile(i), state_profile(j));
            let Ok(cmp) = analysis::compare::compare(pi, pj) else {
                continue;
            };
            let mattr = analysis::attribution::split_delta(pi, &cmp.bin_delta_s);
            let vals: Vec<f32> = keys
                .iter()
                .filter_map(|k| {
                    Some(
                        sj.values.get(k)?.parse::<f32>().ok()?
                            - si.values.get(k)?.parse::<f32>().ok()?,
                    )
                })
                .collect();
            let mut desc = format!(
                "{} (steps {}→{})",
                crate::advice::tuning::diff_note(si, sj),
                i + 1,
                j + 1
            );
            let pools: Vec<String> = [pooled_runs(i), pooled_runs(j)]
                .into_iter()
                .flatten()
                .collect();
            if !pools.is_empty() {
                desc.push_str(&format!(" [{} pooled]", pools.join(" + ")));
            }
            measurements.push(Measurement {
                change: journal::Change {
                    family,
                    softer: vals.iter().sum::<f32>() < 0.0,
                    magnitude: (vals.len() == 1).then(|| vals[0]),
                },
                outcome: journal::judge(cmp.verdict_delta_s),
                desc,
                attributed: None,
                weak: state_weak(i, j),
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
                // Behavioural fingerprint from the latest expression of each
                // state (metrics are per-stint; laps pool, frames do not).
                effects: pair_effects(&stints[i], &stints[j]),
            });
        }
    }
    for j in 1..n {
        let Some(note) = stints[j].entry.note.clone() else {
            continue;
        };
        let Some(Ok(pv)) = stints[j].vs_prev else {
            continue;
        };
        let (delta, attr) = (pv.verdict_s, pv.attr);
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

    if trace {
        eprintln!(
            "advise-trace: {} measurements in {:.2?}; load_campaign total {:.2?}",
            measurements.len(),
            t_meas.elapsed(),
            t0.elapsed()
        );
    }

    Ok(Campaign {
        stints,
        setups,
        positions,
        in_progress,
        missing,
        no_laps,
        groups,
        pooled,
        drift_floor,
        effect_floor,
        measurements,
    })
}
