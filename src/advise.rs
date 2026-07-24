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
    /// The step's honest setup-state verdict when its minimal-diff ancestor
    /// is NOT the previous step (chained experiments make the neighbor
    /// comparison compound while a shared baseline is the clean A/B).
    pub anchor: Option<RowAnchor>,
}

/// Compact per-row anchor: comparison against the prior stint with the
/// smallest setup difference. Empty areas = same setup (pure drift).
pub struct RowAnchor {
    pub vs_step: usize,
    pub areas: String,
    pub delta_s: f32,
    pub word: &'static str,
    pub weak: bool,
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

/// One measured effect for a family: a stint pair whose setups isolate it
/// (direct) or a channel-attributed note reading.
pub struct MeasurementView {
    pub from_step: usize,
    pub to_step: usize,
    pub desc: String,
    pub delta_s: f32,
    /// (entry, exit, straights) share of the step's delta, when known.
    pub split: Option<(f32, f32, f32)>,
    pub weak: bool,
    pub direct: bool,
}

/// A family's measured landscape over one slider: every tried value with its
/// cumulative delta, the fitted curve when trustworthy, and the raw
/// measurements behind it. The data behind "view a change's effects
/// historically".
pub struct LandscapeView {
    pub area: &'static str,
    /// Slider label when the axis is a single known key, else the area.
    pub phrase: String,
    pub key: Option<String>,
    /// (value, cumulative ideal delta s, samples), ascending by value.
    pub nodes: Vec<(f32, f32, usize)>,
    /// y = ax² + bx + c least-squares fit over the nodes (3+ nodes).
    pub fit: Option<(f32, f32, f32)>,
    /// Estimated optimum (interior fit vertex with meaningful spread).
    pub vertex: Option<f32>,
    pub measurements: Vec<MeasurementView>,
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
    /// Per-family measured landscapes (see LandscapeView).
    pub landscapes: Vec<LandscapeView>,
    /// Largest |ideal delta| measured between SAME-setup stints — the
    /// campaign's own noise floor. (count of same-setup pairs, floor s).
    pub drift_floor: Option<(usize, f32)>,
    /// Stint the recommendations are for.
    pub advice_for: String,
    pub recommendations: Vec<analysis::recommend::Recommendation>,
    /// Latest tune revision as (phrase, value, canonical unit), for display.
    pub current_tune: Vec<(String, String, Option<&'static str>)>,
}

/// A composite ideal dramatically faster than the stint's own best flying
/// lap is an UNCORROBORATED splice — rewinds, drafting in a race, or route
/// anomalies stitched segments that never co-occurred in one lap. Such a
/// stint's comparisons cannot be trusted.
fn splice_trusted(p: &analysis::profile::StintProfile) -> bool {
    !p.standing_start_only
        && p.best_lap_time_s.is_finite()
        && p.composite.time_s >= 0.95 * p.best_lap_time_s
}

/// The car driven in a stint: first frame with a car ordinal set.
fn car_of(stint: &analysis::Stint) -> Option<i32> {
    stint.frames.iter().find(|t| t.frame.car_ordinal != 0).map(|t| t.frame.car_ordinal)
}

/// The prior stint whose SETUP differs least from `target` (ties -> most
/// recent): the honest comparison partner for a step. Searches the given
/// prefix of the per-step setups.
fn min_diff_ancestor(
    setups: &[Option<&crate::tuning::Revision>],
    target: &crate::tuning::Revision,
) -> Option<(usize, Vec<String>)> {
    let mut best: Option<(usize, Vec<String>)> = None;
    for (i, s) in setups.iter().enumerate() {
        let Some(s) = s else { continue };
        let keys = crate::tuning::diff_keys(s, target);
        if best.as_ref().is_none_or(|(_, bk)| keys.len() <= bk.len()) {
            best = Some((i, keys));
        }
    }
    best
}

/// Distinct tuning areas the changed keys span, sorted for stable display.
fn area_list(keys: &[String]) -> Vec<&'static str> {
    let mut areas: Vec<&'static str> =
        keys.iter().map(|k| crate::tuning::field_area(k)).collect();
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
        journal::Family::TirePressure => &["tire_pressure_f", "tire_pressure_r"],
        journal::Family::Alignment => &["camber_f", "camber_r", "toe_f", "toe_r", "caster"],
        journal::Family::RideHeight => &["ride_height_f", "ride_height_r"],
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
                // Any concrete value suggested for the exhausted end no
                // longer applies to the rewritten advice.
                r.suggestion = None;
                r.apply.clear();
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

/// Least-squares quadratic fit y = ax² + bx + c over the points, solved via
/// normal equations. None when degenerate (needs 3+ distinct x).
fn quad_fit(pts: &[(f32, f32)]) -> Option<(f64, f64, f64)> {
    let mut xs: Vec<f32> = pts.iter().map(|p| p.0).collect();
    xs.sort_by(f32::total_cmp);
    xs.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
    if xs.len() < 3 {
        return None;
    }
    let (mut s1, mut s2, mut s3, mut s4) = (0f64, 0f64, 0f64, 0f64);
    let (mut t0, mut t1, mut t2) = (0f64, 0f64, 0f64);
    let s0 = pts.len() as f64;
    for &(x, y) in pts {
        let (x, y) = (x as f64, y as f64);
        s1 += x;
        s2 += x * x;
        s3 += x * x * x;
        s4 += x * x * x * x;
        t0 += y;
        t1 += x * y;
        t2 += x * x * y;
    }
    let det3 = |m: [[f64; 3]; 3]| {
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
            - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    };
    let d = det3([[s4, s3, s2], [s3, s2, s1], [s2, s1, s0]]);
    if d.abs() < 1e-12 {
        return None;
    }
    let a = det3([[t2, s3, s2], [t1, s2, s1], [t0, s1, s0]]) / d;
    let b = det3([[s4, t2, s2], [s3, t1, s1], [s2, t0, s0]]) / d;
    let c = det3([[s4, s3, t2], [s3, s2, t1], [s2, s1, t0]]) / d;
    Some((a, b, c))
}

/// Where to probe next to extend a mapped landscape: past the best tried
/// value, away from the worse side, by a quarter of the mapped span —
/// bracketing the optimum from the good side. None when the landscape is
/// flat vs the noise floor, the best value is interior (the curve fit owns
/// that case), or the slider's range allows no new point.
fn probe_value(nodes: &[(f32, f32, usize)], lim: Option<(f32, f32)>) -> Option<f32> {
    let (first, last) = (nodes.first()?, nodes.last()?);
    let (lo, hi) = nodes
        .iter()
        .fold((f32::MAX, f32::MIN), |(lo, hi), n| (lo.min(n.1), hi.max(n.1)));
    if nodes.len() < 2 || hi - lo < 0.10 {
        return None;
    }
    let best = nodes.iter().min_by(|a, b| a.1.total_cmp(&b.1))?;
    let dir = if (best.0 - first.0).abs() < 1e-3 {
        -1.0
    } else if (best.0 - last.0).abs() < 1e-3 {
        1.0
    } else {
        return None; // interior best: the fit's vertex is the suggestion
    };
    let mut v = best.0 + dir * (last.0 - first.0) * 0.25;
    if let Some((mn, mx)) = lim {
        v = v.clamp(mn, mx);
    }
    let v = (v * 10.0).round() / 10.0;
    // Compare at display granularity: clamping to a slider bound must not
    // fabricate a "new" point that rounds to the best tried value.
    ((v - (best.0 * 10.0).round() / 10.0).abs() > 0.05).then_some(v)
}

/// The tune field a note clause is about, matched by field phrase (auto-
/// generated notes use these phrases verbatim). Longest match wins so
/// "front tire pressure" is not mistaken for a shorter overlapping phrase.
fn key_from_phrase(text: &str) -> Option<String> {
    let t = text.to_lowercase();
    crate::tuning::FIELDS
        .iter()
        .filter(|(_, phrase)| t.contains(phrase))
        .max_by_key(|(_, phrase)| phrase.len())
        .map(|(k, _)| k.to_string())
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
    let mut session = TuningSession::load(session_path);
    let text = std::fs::read_to_string(journal_path).unwrap_or_default();
    let mut entries = journal::parse_journal(&text);

    // Stints of the session car recorded AFTER the last journal entry join
    // the trajectory as implicit no-change steps. Journal lines are written
    // on tune saves, so a stint driven without touching anything — the
    // same-setup repeat that measures pure drift — would otherwise be
    // invisible to advice.
    if let Some(last_stamp) = entries.last().and_then(|e| stint_stamp(&e.path)) {
        let last_stamp = last_stamp.to_string();
        let mut extra: Vec<String> = std::fs::read_dir(stints_dir)
            .ok()
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let path = e.path();
                (path.extension().is_some_and(|x| x == "ftel")
                    && stint_stamp(&path.to_string_lossy())
                        .is_some_and(|s| s > last_stamp.as_str())
                    && session.car.is_some()
                    && crate::serve::stint_car(&path) == session.car)
                    .then(|| format!("{stints_dir}/{}", e.file_name().to_string_lossy()))
            })
            .collect();
        extra.sort();
        entries.extend(extra.into_iter().map(|path| journal::Entry { path, note: None }));
    }

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
            landscapes: Vec::new(),
            drift_floor: None,
            advice_for: path,
            recommendations: recs,
            current_tune,
        });
    }

    // A journal for another car (explicitly passed while a different session
    // is active) resolves that car's ARCHIVED session file, so its setups,
    // facts, and landscapes work instead of degrading to blind mode.
    if let Some(first) = entries.first()
        && let Ok(stint) = analysis::Stint::load(first.path.as_ref())
    {
        let journal_car = car_of(&stint);
        if journal_car.is_some() && journal_car != session.car {
            let per_car = crate::tuning::journal_path_for(
                journal_car,
                &session_path.to_string_lossy(),
            );
            let archived = TuningSession::load(per_car.as_ref());
            if archived.car == journal_car {
                session = archived;
            }
        }
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

    // "suspect" in a journal note is the driver's own verdict on that stint
    // (unfamiliar car, chaotic drive, traffic): every measurement touching it
    // is treated as weak — kept visible, never trusted alone.
    let suspect: Vec<bool> = loaded
        .iter()
        .map(|(e, _, _)| {
            e.note
                .as_deref()
                .is_some_and(|n| n.to_lowercase().contains("suspect"))
        })
        .collect();
    // A stint-pair comparison is THIN when either side ran a single flying
    // lap (no corroboration) or failed the splice-trust gate; WEAK adds the
    // driver's own suspect verdict on either side.
    let thin = |i: usize, j: usize| {
        loaded[i].2.laps.len().min(loaded[j].2.laps.len()) < 2
            || !splice_trusted(&loaded[i].2)
            || !splice_trusted(&loaded[j].2)
    };
    let weak_pair = |i: usize, j: usize| thin(i, j) || suspect[i] || suspect[j];

    let mut steps = Vec::new();
    // Per-step comparison products vs the previous step, for the measurement
    // harvest and A-B-A decomposition below.
    let mut deltas: Vec<Option<f32>> = Vec::new();
    let mut attrs: Vec<Option<analysis::attribution::Attribution>> = Vec::new();
    for i in 0..loaded.len() {
        let (entry, stint, profile) = &loaded[i];
        let mut split = None;
        deltas.push(None);
        attrs.push(None);
        let outcome = if i == 0 {
            None
        } else {
            let prev = &loaded[i - 1].2;
            match analysis::compare::compare(prev, profile) {
                Ok(cmp) => {
                    let attr = analysis::attribution::split_delta(prev, &cmp.bin_delta_s);
                    split = Some((attr.entry_delta_s, attr.exit_delta_s, attr.straight_delta_s));
                    *deltas.last_mut().unwrap() = Some(cmp.ideal_delta_s);
                    *attrs.last_mut().unwrap() = Some(attr);
                    let word = journal::judge(cmp.ideal_delta_s).word();
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
            anchor: None,
        });
    }

    // Setup state per step: the latest tune revision saved before the stint
    // began. Only bound when the stint really is the session car's — an
    // explicitly passed foreign journal must not inherit this car's tunes.
    let setups: Vec<Option<&crate::tuning::Revision>> = loaded
        .iter()
        .map(|(entry, stint, _)| {
            let car = car_of(stint);
            if car.is_none() || car != session.car {
                return None;
            }
            let stamp = stint_stamp(&entry.path)?;
            session.revisions.iter().rev().find(|r| r.stamp.as_str() < stamp)
        })
        .collect();

    // The campaign's own noise floor: |ideal delta| across SAME-setup stint
    // pairs is pure driver/track drift. Verdicts with margins below the
    // worst observed drift are provisional, and advice must say so.
    let mut drift_obs: Vec<f32> = Vec::new();
    for j in 1..loaded.len() {
        for i in 0..j {
            let (Some(si), Some(sj)) = (setups[i], setups[j]) else { continue };
            if !crate::tuning::diff_keys(si, sj).is_empty() {
                continue;
            }
            if thin(i, j) {
                continue;
            }
            if let Ok(cmp) = analysis::compare::compare(&loaded[i].2, &loaded[j].2) {
                drift_obs.push(cmp.ideal_delta_s.abs());
            }
        }
    }
    let drift_floor = (!drift_obs.is_empty())
        .then(|| (drift_obs.len(), drift_obs.iter().fold(0.0f32, |a, b| a.max(*b))));

    // Per-row honest verdicts: each step compared against its minimal-diff
    // ancestor, shown only when that ancestor is NOT the previous step (the
    // row's own outcome column already covers the neighbor) and not for the
    // last step (the prominent anchor line below covers it).
    let n = loaded.len();
    for j in 1..n.saturating_sub(1) {
        let Some(sj) = setups[j] else { continue };
        let Some((i, keys)) = min_diff_ancestor(&setups[..j], sj) else { continue };
        if i == j - 1 {
            continue;
        }
        let Ok(cmp) = analysis::compare::compare(&loaded[i].2, &loaded[j].2) else { continue };
        steps[j].anchor = Some(RowAnchor {
            vs_step: i + 1,
            areas: area_list(&keys).join(", "),
            delta_s: cmp.ideal_delta_s,
            word: journal::judge(cmp.ideal_delta_s).word(),
            weak: thin(i, j),
        });
    }

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
        if let Some((i, keys)) = min_diff_ancestor(&setups[..n - 1], last_setup)
            && let Ok(cmp) = analysis::compare::compare(&loaded[i].2, &loaded[n - 1].2)
        {
            let attr = analysis::attribution::split_delta(&loaded[i].2, &cmp.bin_delta_s);
            let areas = area_list(&keys);
            let changes = crate::tuning::diff_note(setups[i].unwrap(), last_setup);
            let weak = weak_pair(i, n - 1);
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
                word: outcome.word(),
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

    // ------ campaign measurements: every stint pair is evidence ------
    // DIRECT: ordered pairs whose setups differ in exactly one area — a clean
    // A/B for that family regardless of how many steps lie between. NOTE-
    // BASED: adjacent steps via their journal note; single-family notes
    // measure on the total delta, compound notes get channel-attributed per
    // family (capped Medium downstream). Reconciliation then uses each
    // family's LATEST measurement, so knowledge from earlier steps keeps
    // tempering advice instead of evaporating when the topic changes.
    struct Measurement {
        change: journal::Change,
        outcome: journal::Outcome,
        desc: String,
        attributed: Option<String>,
        weak: bool,
        i: usize,
        j: usize,
        direct: bool,
        /// The single slider the measurement moved, when identifiable —
        /// lets advice resolve concrete target values.
        key: Option<String>,
        /// (entry, exit, straights) split of the pair's delta.
        split: Option<(f32, f32, f32)>,
        /// Fit for response-curve building: direct pairs and single-family
        /// notes always; attributed compound clauses only when every sibling
        /// clause is gearing (judged on straights, orthogonal to the corner
        /// share this clause is judged on). A corner-channel sibling would
        /// contaminate the curve.
        clean: bool,
    }
    let mut measurements: Vec<Measurement> = Vec::new();
    for j in 1..n {
        for i in 0..j {
            let (Some(si), Some(sj)) = (setups[i], setups[j]) else { continue };
            let keys = crate::tuning::diff_keys(si, sj);
            if keys.is_empty() {
                continue;
            }
            let [area] = area_list(&keys)[..] else { continue };
            let Some(family) = journal::family_for_area(area) else { continue };
            let Ok(cmp) = analysis::compare::compare(&loaded[i].2, &loaded[j].2) else {
                continue;
            };
            let mattr = analysis::attribution::split_delta(&loaded[i].2, &cmp.bin_delta_s);
            let vals: Vec<f32> = keys
                .iter()
                .filter_map(|k| {
                    Some(sj.values.get(k)?.parse::<f32>().ok()?
                        - si.values.get(k)?.parse::<f32>().ok()?)
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
                    crate::tuning::diff_note(si, sj),
                    i + 1,
                    j + 1
                ),
                attributed: None,
                weak: weak_pair(i, j),
                i,
                j,
                direct: true,
                key: Some(keys[0].clone()),
                split: Some((mattr.entry_delta_s, mattr.exit_delta_s, mattr.straight_delta_s)),
                clean: true,
            });
        }
    }
    for j in 1..n {
        let Some(note) = &loaded[j].0.note else { continue };
        let (Some(delta), Some(attr)) = (deltas[j], attrs[j]) else { continue };
        if let Some(change) = journal::parse_change(note) {
            measurements.push(Measurement {
                change,
                outcome: journal::judge(delta),
                desc: note.clone(),
                attributed: None,
                weak: weak_pair(j - 1, j),
                i: j - 1,
                j,
                direct: false,
                key: key_from_phrase(note),
                split: Some((attr.entry_delta_s, attr.exit_delta_s, attr.straight_delta_s)),
                clean: true,
            });
        } else {
            let evidence = format!(
                "outcome attributed from a compound step (\"{note}\"): corner entry \
                 {:+.2}s / exit {:+.2}s / straights {:+.2}s of {delta:+.2}s total \
                 ({:.0}% of lap time is cornering) — inferred from where the time \
                 moved, not measured in isolation",
                attr.entry_delta_s,
                attr.exit_delta_s,
                attr.straight_delta_s,
                attr.corner_share * 100.0,
            );
            let clauses: Vec<journal::Change> = journal::parse_clauses(note);
            let mut seen = Vec::new();
            for clause_text in note.split(';').map(str::trim) {
                let Some(clause) = journal::parse_change(clause_text) else { continue };
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
                    weak: weak_pair(j - 1, j),
                    i: j - 1,
                    j,
                    direct: false,
                    key: key_from_phrase(clause_text),
                    split: Some((attr.entry_delta_s, attr.exit_delta_s, attr.straight_delta_s)),
                    clean: clauses.iter().all(|c| {
                        // Judged-channel overlap: gearing reads straights,
                        // brakes reads entry, everything else the corner
                        // total (entry included). Siblings on a disjoint
                        // channel can't contaminate this clause's reading.
                        let chan = |f: journal::Family| match f {
                            journal::Family::Gearing => 0u8,   // straights
                            journal::Family::Brakes => 1,      // entry
                            _ => 2,                            // corner total
                        };
                        let (a, b) = (chan(clause.family), chan(c.family));
                        c.family == clause.family
                            || (a != b && !(a >= 1 && b >= 1)) // entry ⊂ corner
                    }),
                });
            }
        }
    }
    // Latest evidence per family: newest endpoint wins; a direct setup A/B
    // beats a note-based reading of the same endpoint; nearest ancestor
    // breaks remaining ties (least drift).
    let mut latest: Vec<&Measurement> = Vec::new();
    for m in &measurements {
        match latest.iter_mut().find(|l| l.change.family == m.change.family) {
            Some(l) => {
                if (m.j, m.direct, m.i) > (l.j, l.direct, l.i) {
                    *l = m;
                }
            }
            None => latest.push(m),
        }
    }

    let (last_entry, last_stint, _) = loaded.last().unwrap();
    let mut recs = blind_recommendations(last_stint, &last_entry.path)?;
    let mut matched_families: Vec<journal::Family> = Vec::new();
    for m in &latest {
        if journal::reconcile(
            &mut recs,
            m.change,
            m.outcome,
            &m.desc,
            m.attributed.as_deref(),
            m.weak,
        ) {
            matched_families.push(m.change.family);
        }
    }
    // History-only reverts stay scoped to the LAST stint's own deviation from
    // its anchor — a past experiment already reverted needs no advice. The
    // suggestion is the anchor's own values: reverting fully means returning
    // to a measured state, not arithmetic.
    if let Some((change, outcome, note, weak)) = &anchor_change
        && !matched_families.contains(&change.family)
        && let Some(mut rec) = journal::history_revert(*change, *outcome, note, None, *weak)
    {
        if let Some(a) = &anchor
            && let (Some(Some(anchor_setup)), Some(Some(last_setup))) =
                (setups.get(a.vs_step - 1), setups.last())
        {
            let restore: Vec<(String, String)> = crate::tuning::diff_keys(anchor_setup, last_setup)
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
                                crate::tuning::field_phrase(k),
                                crate::tuning::display_value(k, v, &session.facts),
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
        for r in recs
            .iter_mut()
            .filter(|r| r.implied.is_some_and(|i| i.family == change.family))
        {
            r.evidence.push(format!(
                "where the time moved vs step {}: corner entry {e:+.2}s / \
                 exit {x:+.2}s / straights {st:+.2}s",
                a.vs_step,
            ));
        }
    }
    // ---- measured landscapes: the campaign's response per slider ----
    // Chained deltas from non-weak measurements build a cumulative curve over
    // a slider's tried values ("decaying improvement" reads as a curve shape,
    // not a single verdict). With 3+ points, meaningful spread, and an
    // interior minimum, the fit's vertex becomes the suggestion —
    // interpolation over the mapped landscape instead of last-step bisection.
    // Every family's landscape is kept on the view for the history panel.
    let mut curve_fams: Vec<journal::Family> = Vec::new();
    for m in &measurements {
        if !curve_fams.contains(&m.change.family) {
            curve_fams.push(m.change.family);
        }
    }
    let mut landscapes: Vec<LandscapeView> = Vec::new();
    for family in curve_fams {
        let fam_all: Vec<&Measurement> = measurements
            .iter()
            .filter(|m| m.change.family == family)
            .collect();
        let mviews: Vec<MeasurementView> = fam_all
            .iter()
            .map(|m| MeasurementView {
                from_step: m.i + 1,
                to_step: m.j + 1,
                desc: m.desc.clone(),
                delta_s: m.outcome.delta_s().unwrap_or(0.0),
                split: m.split,
                weak: m.weak,
                direct: m.direct,
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
        if let Some(key) = key.as_deref() {
            let value_of = |idx: usize| -> Option<f32> {
                setups.get(idx)?.as_ref()?.values.get(key)?.parse::<f32>().ok()
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
                let Some(cum_f) =
                    nodes.iter().find(|n| (n.0 - vf).abs() < 1e-3).map(|n| n.1)
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
        }

        let pts: Vec<(f32, f32)> = nodes.iter().map(|n| (n.0, n.1)).collect();
        let fit = quad_fit(&pts).map(|(a, b, c)| (a as f32, b as f32, c as f32));
        let (lo, hi) = nodes
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), n| (lo.min(n.1), hi.max(n.1)));
        let mut vertex_out = None;
        if let (Some((a, b, _)), Some(key)) = (fit, key.as_deref())
            && a > 0.0
            && nodes.len() >= 3
            && hi - lo >= 0.10
        {
            let mut vertex = -b / (2.0 * a);
            let (vmin, vmax) = (nodes.first().unwrap().0, nodes.last().unwrap().0);
            if vertex >= vmin && vertex <= vmax {
                if let Some(lim) = crate::tuning::limit_of(&session.facts, key) {
                    vertex = vertex.clamp(lim.0, lim.1);
                }
                let vertex = (vertex * 10.0).round() / 10.0;
                vertex_out = Some(vertex);
                let phrase = crate::tuning::field_phrase(key);
                // Already there? Then the ask is NOTHING — repeats tighten
                // the estimate, but no change is being requested.
                let at_optimum = setups
                    .last()
                    .copied()
                    .flatten()
                    .and_then(|b| b.values.get(key)?.parse::<f32>().ok())
                    .is_some_and(|cur| (cur - vertex).abs() < 0.05);
                let landscape = nodes_summary(&nodes);
                let disp =
                    crate::tuning::display_value(key, &vertex.to_string(), &session.facts);
                // A fitted optimum away from the current setting deserves a
                // recommendation even when no behavioural rule speaks for the
                // family (the pressure rule is blind on cars whose temps
                // never leave the band; the landscape is not).
                if !at_optimum
                    && !recs
                        .iter()
                        .any(|r| r.implied.is_some_and(|i| i.family == family))
                {
                    recs.push(analysis::recommend::Recommendation { apply: Vec::new(),
                        area: journal::family_area(family),
                        suggestion: None,
                        advice: String::new(),
                        evidence: Vec::new(),
                        confidence: analysis::recommend::Confidence::Medium,
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
                    if at_optimum {
                        r.suggestion = Some(format!("{phrase}: hold {disp}"));
                        r.advice = "no change asked: the current setting is the \
                             estimated optimum. Any stint driven here tightens \
                             the estimate for free"
                            .to_string();
                        r.implied = None;
                        r.apply.clear();
                    } else {
                        r.suggestion = Some(format!("{phrase}: {disp}"));
                        r.apply = vec![(key.to_string(), vertex.to_string())];
                        r.advice = format!(
                            "set and drive one stint: this is the estimated \
                             optimum of the mapped response. Everything else \
                             unchanged; set {phrase} to {vertex}"
                        );
                        r.implied = Some(journal::Change {
                            family,
                            softer: vertex
                                < setups
                                    .last()
                                    .copied()
                                    .flatten()
                                    .and_then(|b| {
                                        b.values.get(key)?.parse::<f32>().ok()
                                    })
                                    .unwrap_or(vertex),
                            magnitude: None,
                        });
                    }
                    r.confidence = analysis::recommend::Confidence::Medium;
                    r.evidence.push(format!(
                        "measured landscape ({phrase}): {landscape} (cumulative \
                         ideal delta vs first tried value; lower = faster)"
                    ));
                }
            }
        }
        // The best MEASURED node beats the current setting with no fitted
        // optimum to point elsewhere: returning to a measured state outranks
        // bisection between it and here. (The fit path owns interior optima.)
        if vertex_out.is_none()
            && let Some(key) = key.as_deref()
            && let Some(cur) = setups
                .last()
                .copied()
                .flatten()
                .and_then(|b| b.values.get(key)?.parse::<f32>().ok())
            && let Some(best) = nodes
                .iter()
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .copied()
            && let Some(cur_node) = nodes.iter().find(|n| (n.0 - cur).abs() < 1e-3)
            && (best.0 - cur).abs() > 1e-3
            && cur_node.1 - best.1 >= drift_floor.map_or(0.10, |(_, f)| f.max(0.10))
        {
            let phrase = crate::tuning::field_phrase(key);
            let gap = cur_node.1 - best.1;
            for r in recs
                .iter_mut()
                .filter(|r| r.implied.is_some_and(|i| i.family == family))
            {
                r.suggestion = Some(format!(
                    "{phrase}: {}",
                    crate::tuning::display_value(key, &best.0.to_string(), &session.facts),
                ));
                r.apply = vec![(key.to_string(), best.0.to_string())];
                r.advice = format!(
                    "return to the best measured setting: {phrase} {} beat the \
                     current value by {gap:.2}s. An interior optimum may exist \
                     between the two — a midpoint stint is the exploratory \
                     alternative",
                    best.0,
                );
                r.confidence = analysis::recommend::Confidence::Medium;
                r.implied = Some(journal::Change {
                    family,
                    softer: best.0 < cur,
                    magnitude: None,
                });
                r.evidence.push(format!(
                    "measured landscape ({phrase}): {} (cumulative ideal delta; \
                     lower = faster)",
                    nodes_summary(&nodes),
                ));
            }
        }

        // No interior optimum mapped: the workflow's data ask — one stint at
        // a specific value past the good edge extends the landscape where it
        // matters. Not an optimization claim; explicitly a probe.
        if vertex_out.is_none()
            && let Some(key) = key.as_deref()
            && let Some(v) = probe_value(&nodes, crate::tuning::limit_of(&session.facts, key))
        {
            let phrase = crate::tuning::field_phrase(key);
            let best = nodes
                .iter()
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|n| n.0)
                .unwrap_or(v);
            recs.push(analysis::recommend::Recommendation {
                apply: vec![(key.to_string(), v.to_string())],
                area: "probe",
                suggestion: Some(format!(
                    "{phrase}: {}",
                    crate::tuning::display_value(key, &v.to_string(), &session.facts),
                )),
                advice: format!(
                    "probe: one stint here extends the map where it still \
                     improves. Set {phrase} to {v} with everything else \
                     unchanged — probes are one at a time, two unexplored \
                     changes in one stint cannot be separated"
                ),
                evidence: vec![format!(
                    "mapped so far: {} (cumulative ideal delta; lower = faster)",
                    nodes_summary(&nodes),
                )],
                confidence: analysis::recommend::Confidence::Low,
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
                .map(|k| crate::tuning::field_phrase(k).to_string())
                .unwrap_or_else(|| journal::family_area(family).to_string()),
            key,
            nodes,
            fit,
            vertex: vertex_out,
            measurements: mviews,
        });
    }

    // Drift-aware honesty: a High-confidence conclusion resting on a single
    // comparison whose margin is under the measured same-setup drift gets
    // capped and labeled. Multi-point landscapes are less exposed (averaged
    // nodes), so curve-based Medium suggestions stand with a note.
    if let Some((pairs, floor)) = drift_floor {
        for r in recs.iter_mut() {
            let Some(implied) = r.implied else { continue };
            let Some(m) = latest.iter().find(|m| m.change.family == implied.family) else {
                continue;
            };
            let Some(margin) = m.outcome.delta_s().map(f32::abs) else { continue };
            if margin < floor {
                r.evidence.push(format!(
                    "provisional: the deciding margin ({margin:.2}s) is under the \
                     measured same-setup drift (±{floor:.2}s over {pairs} repeat \
                     pair{}) — corroborate before trusting it",
                    if pairs == 1 { "" } else { "s" },
                ));
                if r.confidence == analysis::recommend::Confidence::High {
                    r.confidence = analysis::recommend::Confidence::Medium;
                }
            }
        }
    }

    // History-only recs arrive unsorted; keep most-confident-first for display.
    recs.sort_by_key(|r| std::cmp::Reverse(r.confidence));
    // Cite tune absolutes only when the journal's stints are the session
    // car's — an explicitly passed foreign journal must not quote this car's
    // sliders as if they were its own.
    let current_tune = if car_of(last_stint) == session.car {
        enrich_with_tune(&mut recs, &session)
    } else {
        Vec::new()
    };

    // The suggestion headline: the concrete setting to try. With the last
    // stint's setup on file, an ABSOLUTE value ("front arb: 17.5"), clamped
    // to the slider's range; blind, the DELTA to apply ("front arb: +0.5").
    // Values base on the last stint's setup — the state the advice was
    // judged against — never a saved-but-undriven revision.
    let round = |v: f32| (v * 1e4).round() / 1e4;
    let base = setups.last().copied().flatten();
    for r in recs.iter_mut() {
        if r.suggestion.is_some() {
            continue;
        }
        let Some(implied) = r.implied else { continue };
        let Some(delta) = implied.magnitude else { continue };
        let Some(key) = latest
            .iter()
            .find(|m| m.change.family == implied.family)
            .and_then(|m| m.key.as_deref())
        else {
            continue;
        };
        let phrase = crate::tuning::field_phrase(key);
        // The headline now carries the value; the relative phrasing from
        // blind-mode reconciliation becomes redundant.
        r.advice = r
            .advice
            .replace(&format!(" (go {delta:+.1} slider units from here)"), "");
        r.suggestion = match base.and_then(|b| b.values.get(key)?.parse::<f32>().ok()) {
            Some(cur) => {
                let mut target = cur + delta;
                if let Some(lim) = crate::tuning::limit_of(&session.facts, key) {
                    target = target.clamp(lim.0, lim.1);
                }
                r.apply = vec![(key.to_string(), round(target).to_string())];
                Some(format!(
                    "{phrase}: {}",
                    crate::tuning::display_value(key, &round(target).to_string(), &session.facts),
                ))
            }
            None => Some(format!(
                "{phrase}: {}{}",
                if delta > 0.0 { "+" } else { "" },
                crate::tuning::display_value(key, &round(delta).to_string(), &session.facts),
            )),
        };
    }

    Ok(AdviseView {
        journal: Some(journal_path.to_string()),
        steps,
        anchor,
        aba,
        in_progress,
        landscapes,
        drift_floor,
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
        Recommendation { apply: Vec::new(),
            area: "balance",
            advice: "reduce front roll stiffness".into(),
            evidence: vec![],
            confidence: Confidence::High,
            suggestion: None,
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

    /// Springs have no universal range: with no fact recorded, a pinned arb
    /// points at the springs but never claims the whole direction exhausted.
    #[test]
    fn unknown_limits_never_claim_exhaustion() {
        let session = session_with(&[("arb_f", "1"), ("springs_f", "100")], &[]);
        let mut recs = vec![balance_rec()];
        enrich_with_tune(&mut recs, &session);
        assert!(recs[0].advice.contains("reduce front roll stiffness"), "{}", recs[0].advice);
        assert!(
            recs[0].evidence.iter().any(|e| e.contains("work with front springs")),
            "{:?}",
            recs[0].evidence
        );
        assert!(recs[0].evidence.iter().all(|e| !e.contains("exhausted")));
    }

    /// The user's worked example: front ARB 10..16 with lap times showing
    /// decaying improvement then a slowdown — the fitted optimum sits between
    /// 14 and 15, not at the best tried value or a bisection of the last step.
    #[test]
    fn quad_fit_finds_the_interior_optimum() {
        // Cumulative deltas from lap times 60.0, 59.0, 58.3, 58.0, 58.5.
        let pts = [(10.0, 0.0), (12.0, -1.0), (14.0, -1.7), (15.0, -2.0), (16.0, -1.5)];
        let (a, b, _) = quad_fit(&pts).unwrap();
        assert!(a > 0.0, "upward curvature (a minimum exists)");
        let vertex = (-b / (2.0 * a)) as f32;
        assert!((14.0..=15.2).contains(&vertex), "vertex {vertex}");

        // Monotonic data has no trustworthy interior minimum.
        let mono = [(10.0, 0.0), (12.0, -1.0), (14.0, -2.0)];
        if let Some((a, b, _)) = quad_fit(&mono) {
            let v = (-b / (2.0 * a)) as f32;
            assert!(!(10.0..=14.0).contains(&v) || a <= 0.0, "no interior vertex: a={a} v={v}");
        }
        assert!(quad_fit(&[(10.0, 0.0), (12.0, -1.0)]).is_none(), "2 points fit nothing");
    }

    /// Probes extend the landscape past the good edge; interior optima and
    /// flat landscapes ask for nothing.
    #[test]
    fn probe_extends_the_mapped_edge() {
        // Better at the low end: probe below it by a quarter span.
        let nodes = [(29.0, -0.21, 1), (100.0, 0.22, 1)];
        let v = probe_value(&nodes, Some((0.0, 100.0))).unwrap();
        assert!((v - 11.2).abs() < 0.11, "{v}");
        // Clamped by the slider range but still a new point.
        let nodes = [(12.0, 0.0, 1), (100.0, 0.63, 1)];
        assert_eq!(probe_value(&nodes, Some((0.0, 100.0))), Some(0.0));
        // Better at the high end: probe above.
        let nodes = [(20.0, 0.31, 1), (52.0, 0.0, 1)];
        let v = probe_value(&nodes, Some((0.0, 100.0))).unwrap();
        assert!((v - 60.0).abs() < 0.11, "{v}");
        // Interior best: the fit's vertex owns it.
        let nodes = [(17.0, -0.16, 1), (18.0, -0.49, 1), (20.7, 0.0, 1)];
        assert_eq!(probe_value(&nodes, None), None);
        // Flat landscape: nothing worth a stint.
        let nodes = [(3.35, -0.04, 1), (3.63, 0.03, 1)];
        assert_eq!(probe_value(&nodes, None), None);
        // Best pinned at the slider bound: no new point exists.
        let nodes = [(0.0, -0.3, 1), (50.0, 0.2, 1)];
        assert_eq!(probe_value(&nodes, Some((0.0, 100.0))), None);
    }

    #[test]
    fn stint_stamps_parse_from_both_naming_schemes() {
        assert_eq!(stint_stamp("sessions/stint-20260720-233644.ftel"), Some("20260720-233644"));
        assert_eq!(stint_stamp("sessions/session-20260719-115355.ftel"), Some("20260719-115355"));
        assert_eq!(stint_stamp("sessions/other.ftel"), None);
    }
}
