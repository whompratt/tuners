//! The advise engine, shared by the CLI (`tuners advise`) and the dashboard
//! (`/api/advise`): journal trajectory with measured step outcomes, blind
//! recommendations reconciled with the last step, and current-tune enrichment.
//! With no journal yet (a session's first stint), falls back to blind
//! recommendations on the latest stint of the session car; the journal
//! starts with the first tune change.

use crate::advice::{journal, recommend, tuning::TuningSession};
use crate::analysis::{self, effects};
use std::path::Path;

/// A changed family on a step, with the road its fingerprint is judged on
/// (attribution's channel: gearing = straights, brakes = entry, everything
/// else the corner total). Feeds the frontend's consequence sentence
/// without prose parsing.
pub struct StepFamily {
    pub area: &'static str,
    pub channel: &'static str,
}

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
    /// Families this step's note changed, each with its judged channel.
    pub families: Vec<StepFamily>,
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
    /// Per-stint drift over the pair: the noise floor for outcome margins.
    pub drift_s: f32,
    /// Drift-corrected behavioural movement of the excursion, per effect
    /// field ((exc − rev)/2).
    pub effects: effects::Effects,
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
    /// Behavioural movement anchor → last stint (effect deltas).
    pub effects: effects::Effects,
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
    /// Behavioural movement of the underlying stint pair. For an
    /// attributed compound clause this is the WHOLE pair's movement; the
    /// vector belongs to the pair, siblings share it.
    pub effects: effects::Effects,
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
    /// Journaled stints whose recordings no longer exist (deleted from the
    /// dashboard): skipped, with their notes merged into the next step so
    /// slider positions stay honest.
    pub missing: Vec<String>,
    /// Mid-campaign stints with no completed laps (menu-pause artifacts):
    /// skipped the same way.
    pub no_laps: Vec<String>,
    /// Per-family measured landscapes (see LandscapeView).
    pub landscapes: Vec<LandscapeView>,
    /// Largest |ideal delta| measured between SAME-setup stints: the
    /// campaign's own noise floor. (count of same-setup pairs, floor s).
    pub drift_floor: Option<(usize, f32)>,
    /// Per-field campaign noise floor: largest |effect delta| across the same
    /// same-setup pairs. Raises (never lowers) the library defaults when
    /// gating which effect movements are worth showing.
    pub effect_floor: effects::Effects,
    /// Stint the recommendations are for.
    pub advice_for: String,
    pub recommendations: Vec<recommend::Recommendation>,
    /// Latest tune revision as (phrase, value, canonical unit), for display.
    pub current_tune: Vec<(String, String, Option<&'static str>)>,
}

/// A composite ideal dramatically faster than the stint's own best flying
/// lap is an UNCORROBORATED splice: rewinds, drafting in a race, or route
/// anomalies stitched segments that never co-occurred in one lap. Such a
/// stint's comparisons cannot be trusted.
fn splice_trusted(p: &analysis::profile::StintProfile) -> bool {
    !p.standing_start_only
        && p.best_lap_time_s.is_finite()
        && p.composite.time_s >= 0.95 * p.best_lap_time_s
}

/// The car driven in a stint: first frame with a car ordinal set.
pub(crate) fn car_of(stint: &analysis::Stint) -> Option<i32> {
    stint
        .frames
        .iter()
        .find(|t| t.frame.car_ordinal != 0)
        .map(|t| t.frame.car_ordinal)
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

/// Metrics of a stint's longest driving segment: the basis for both the
/// step balance display and the plan-011 effect vector.
fn stint_overall_metrics(stint: &analysis::Stint) -> Option<analysis::metrics::StintMetrics> {
    let segments = analysis::driving_segments(&stint.frames, 5.0);
    let longest = segments.iter().max_by_key(|s| s.len())?;
    Some(analysis::metrics::stint_metrics(longest))
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

fn blind_recommendations(
    stint: &analysis::Stint,
    path: &str,
    ctx: &recommend::Context,
) -> Result<Vec<recommend::Recommendation>, String> {
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
    Ok(recommend::recommend(&overall, &per_lap, ctx))
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

/// Attach current-tune absolutes (with slider headroom when limits are on
/// file) to family-matched recommendations and build the display list of the
/// latest revision. Advice whose direction is exhausted flips to the partner
/// end of the car, or is downgraded when no partner exists.
fn enrich_with_tune(
    recs: &mut [recommend::Recommendation],
    session: &TuningSession,
) -> Vec<(String, String, Option<&'static str>)> {
    let Some(rev) = session.latest() else {
        return Vec::new();
    };
    for r in recs.iter_mut() {
        let Some(implied) = r.implied else { continue };
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
            if let Some((pf, ps, text)) = exhausted_flip(implied.family, implied.softer) {
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
            } else {
                r.evidence.push(
                    "every slider on this channel is already at the advised bound: \
                     direction exhausted"
                        .into(),
                );
                r.confidence = recommend::Confidence::Low;
            }
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
    }
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
/// value, away from the worse side, by a quarter of the mapped span,
/// bracketing the optimum from the good side. None when the landscape is
/// flat vs the noise floor, the best value is interior (the curve fit owns
/// that case), or the slider's range allows no new point.
fn probe_value(nodes: &[(f32, f32, usize)], lim: Option<(f32, f32)>) -> Option<f32> {
    let (first, last) = (nodes.first()?, nodes.last()?);
    let (lo, hi) = nodes.iter().fold((f32::MAX, f32::MIN), |(lo, hi), n| {
        (lo.min(n.1), hi.max(n.1))
    });
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
    // A small mapped span must still ask for a NEW point: after a single
    // small improving step, a quarter-span probe rounds back onto the best
    // tried value and the guard below would cancel the ask, so step one
    // display unit outward instead.
    if ((v * 10.0).round() - (best.0 * 10.0).round()).abs() < 0.5 {
        v = best.0 + dir * 0.1;
    }
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
    crate::advice::tuning::FIELDS
        .iter()
        .filter(|(_, phrase)| t.contains(phrase))
        .max_by_key(|(_, phrase)| phrase.len())
        .map(|(k, _)| k.to_string())
}

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
enum CampaignBound {
    Open,
    Closed,
    Since(String),
}

/// Whether a journal's campaign is parked in the archive (nothing new can
/// join it) — the effect map's staleness check for closed campaigns.
pub(crate) fn campaign_closed(journal_text: &str) -> bool {
    matches!(campaign_bound(journal_text), CampaignBound::Closed)
}

fn campaign_bound(journal_text: &str) -> CampaignBound {
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
fn drop_missing_entries(
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
            Some(car) => crate::api::stint_car(p) == Some(car),
        };
        matches.then(|| p.to_string_lossy().into_owned())
    })
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
fn pair_thin(stints: &[CampaignStint], i: usize, j: usize) -> bool {
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
fn pair_weak(stints: &[CampaignStint], i: usize, j: usize) -> bool {
    pair_thin(stints, i, j) || stints[i].suspect || stints[j].suspect
}

/// A stint pair's behavioural movement: per-stint field deltas plus the
/// pair-level corner-matched apex speed (position-matched corner runs on the
/// earlier stint's route — computable because campaign stints share one).
fn pair_effects(from: &CampaignStint, to: &CampaignStint) -> effects::Effects {
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

/// One Low-confidence suggestion from the cross-campaign effect map (built
/// by `tuners map`): the best grounded, context-matched cell whose pooled
/// behavioural movement aligns with the pace trends, for a family without
/// trustworthy local evidence. Graded gating: a family with any NON-WEAK
/// local measurement is owned by that evidence; weak-only local evidence
/// tempers the prior (quoted) instead of silencing it. A cell must also be
/// a distribution, not an anecdote: one attributed clause from one other
/// car (n=1, no direct A/B) never carries a suggestion. None = the map is
/// silent.
fn map_prior(
    emap: &crate::advice::effectmap::EffectMap,
    trends: &[crate::advice::effectmap::PaceTrend],
    ctx: &crate::advice::effectmap::MapContext,
    measurements: &[Measurement],
    recs: &[recommend::Recommendation],
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
        (grounded && !tried && !advised && score >= 1.0).then_some((cell, family))
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
        // No journal yet: blind advice on the session car's latest stint.
        let path = latest_stint_for_car(stints_dir, session.car)
            .ok_or("no stints recorded yet; drive first")?;
        let stint = analysis::Stint::load(path.as_ref()).map_err(|e| format!("{path}: {e}"))?;
        let mut recs = blind_recommendations(&stint, &path, &rule_context(&session))?;
        // Cold start: no journal means no local trends, but the driver's
        // pooled history from other cars can still rank one untried lever
        // from the effect map — the first informed suggestion of a fresh
        // campaign (plan 007).
        if let Ok(text) = std::fs::read_to_string(crate::util::data_path("effect-map.tsv"))
            && let Ok(emap) = crate::advice::effectmap::parse(&text)
            && let Some(met) = stint_overall_metrics(&stint)
        {
            let trends = crate::advice::effectmap::driver_trends(&emap, "local", car_of(&stint));
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
            if let Some(rec) = map_prior(&emap, &trends, &ctx, &[], &recs) {
                recs.push(rec);
            }
        }
        let current_tune = enrich_with_tune(&mut recs, &session);
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
        && let Ok(stint) = analysis::Stint::load(first.path.as_ref())
    {
        let journal_car = car_of(&stint);
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

    let c = load_campaign(entries, &session, journal_path)?;
    let n = c.stints.len();

    let mut steps = Vec::new();
    for (i, cs) in c.stints.iter().enumerate() {
        let mut split = None;
        let outcome = match (i, &cs.vs_prev) {
            (0, _) | (_, None) => None,
            (_, Some(Ok((delta, attr)))) => {
                split = Some((attr.entry_delta_s, attr.exit_delta_s, attr.straight_delta_s));
                Some(Ok((
                    journal::judge(*delta).word(),
                    *delta,
                    c.stints[i - 1].profile.laps.len() != cs.profile.laps.len(),
                )))
            }
            (_, Some(Err(e))) => Some(Err(e.clone())),
        };
        steps.push(StepView {
            path: cs.entry.path.clone(),
            laps: cs.profile.laps.len(),
            best_s: cs.profile.best_lap_time_s,
            ideal_s: cs.profile.composite.time_s,
            balance: cs.met.as_ref().and_then(|m| {
                Some((
                    m.understeer_index?,
                    m.cornering_front_slip?,
                    m.cornering_rear_slip?,
                ))
            }),
            note: cs.entry.note.clone(),
            pos: match c.positions[i] {
                (Some(f), Some(r)) if f != 0.0 || r != 0.0 => Some((f, r)),
                _ => None,
            },
            outcome,
            split,
            anchor: None,
            families: {
                let mut fams: Vec<journal::Family> = cs.changes.iter().map(|c| c.family).collect();
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

    // Per-row honest verdicts: each step compared against its minimal-diff
    // ancestor, shown only when that ancestor is NOT the previous step (the
    // row's own outcome column already covers the neighbor) and not for the
    // last step (the prominent anchor line below covers it).
    for (j, step) in steps
        .iter_mut()
        .enumerate()
        .take(n.saturating_sub(1))
        .skip(1)
    {
        let Some(sj) = c.setups[j] else { continue };
        let Some((i, keys)) = min_diff_ancestor(&c.setups[..j], sj) else {
            continue;
        };
        if i == j - 1 {
            continue;
        }
        let Ok(cmp) = analysis::compare::compare(&c.stints[i].profile, &c.stints[j].profile) else {
            continue;
        };
        step.anchor = Some(RowAnchor {
            vs_step: i + 1,
            areas: area_list(&keys).join(", "),
            delta_s: cmp.ideal_delta_s,
            word: journal::judge(cmp.ideal_delta_s).word(),
            weak: c.thin(i, j),
        });
    }

    // The honest comparison for the last stint is the prior stint whose SETUP
    // differs least (ties -> most recent). Chained experiments ("revert X;
    // try Y") make the chronological neighbor a compound comparison while the
    // shared baseline is a clean single-area A/B.
    let mut anchor = None;
    let mut anchor_change: Option<(journal::Change, journal::Outcome, String, bool)> = None;
    if let Some(Some(last_setup)) = c.setups.last()
        && n >= 2
        && let Some((i, keys)) = min_diff_ancestor(&c.setups[..n - 1], last_setup)
        && let Ok(cmp) = analysis::compare::compare(&c.stints[i].profile, &c.stints[n - 1].profile)
    {
        let attr = analysis::attribution::split_delta(&c.stints[i].profile, &cmp.bin_delta_s);
        let areas = area_list(&keys);
        let changes = crate::advice::tuning::diff_note(c.setups[i].unwrap(), last_setup);
        let weak = c.weak_pair(i, n - 1);
        let outcome = journal::judge(cmp.ideal_delta_s);
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
        anchor = Some(AnchorView {
            vs_step: i + 1,
            areas: areas.join(", "),
            changes,
            delta_s: cmp.ideal_delta_s,
            word: outcome.word(),
            weak,
            reconciled: anchor_change.is_some(),
            split: (attr.entry_delta_s, attr.exit_delta_s, attr.straight_delta_s),
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
            .and_then(|r| r.as_ref().ok().map(|(d, _)| *d))
    };
    let aba = (n >= 3)
        .then(|| {
            let (exc, rev) = (&c.stints[n - 2].changes, &c.stints[n - 1].changes);
            let laps_ok = c.stints[n - 3..].iter().all(|s| s.profile.laps.len() >= 2);
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
    let mut recs = blind_recommendations(&last.stint, &last.entry.path, &rule_context(&session))?;
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
    // its anchor: a past experiment already reverted needs no advice. The
    // suggestion is the anchor's own values: reverting fully means returning
    // to a measured state, not arithmetic.
    if let Some((change, outcome, note, weak)) = &anchor_change
        && !matched_families.contains(&change.family)
        && let Some(mut rec) = journal::history_revert(*change, *outcome, note, None, *weak)
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
                desc: m.desc.clone(),
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
        }

        let pts: Vec<(f32, f32)> = nodes.iter().map(|n| (n.0, n.1)).collect();
        let fit = quad_fit(&pts).map(|(a, b, c)| (a as f32, b as f32, c as f32));
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
                let vertex = (vertex * 10.0).round() / 10.0;
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
                    .is_some_and(|cur| (cur - vertex).abs() < 0.05);
                let landscape = nodes_summary(&nodes);
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
                        apply: Vec::new(),
                        area: journal::family_area(family),
                        suggestion: None,
                        advice: String::new(),
                        evidence: Vec::new(),
                        confidence: recommend::Confidence::Medium,
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
                             estimated optimum (bracketed; further narrowing \
                             is expected to gain less than the ±{floor:.2}s \
                             noise floor, which is not proof of convergence). \
                             Any stint driven here tightens the estimate for \
                             free"
                        );
                        r.implied = None;
                        r.apply.clear();
                    } else {
                        r.suggestion = Some(format!("{phrase}: {disp}"));
                        r.apply = vec![(key.to_string(), vertex.to_string())];
                        r.advice = match gain {
                            Some(g) if g < floor => format!(
                                "probe the estimated optimum: the fit expects \
                                 only {g:.2}s here, within the ±{floor:.2}s \
                                 noise floor, so holding the current value is \
                                 equally defensible and a stint at {vertex} \
                                 mainly tightens the map. Everything else \
                                 unchanged; set {phrase} to {vertex}"
                            ),
                            _ => format!(
                                "set and drive one stint: this is the estimated \
                                 optimum of the mapped response. Everything else \
                                 unchanged; set {phrase} to {vertex}"
                            ),
                        };
                        r.implied = Some(journal::Change {
                            family,
                            softer: vertex < cur.unwrap_or(vertex),
                            magnitude: None,
                        });
                    }
                    r.confidence = recommend::Confidence::Medium;
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
            for r in recs
                .iter_mut()
                .filter(|r| r.implied.is_some_and(|i| i.family == family))
            {
                r.suggestion = Some(format!(
                    "{phrase}: {}",
                    crate::advice::tuning::display_value(key, &best.0.to_string(), &session.facts),
                ));
                r.apply = vec![(key.to_string(), best.0.to_string())];
                r.advice = format!(
                    "return to the best measured setting: {phrase} {} beat the \
                     current value by {gap:.2}s. An interior optimum may exist \
                     between the two; a midpoint stint is the exploratory \
                     alternative",
                    best.0,
                );
                r.confidence = recommend::Confidence::Medium;
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

        // No interior optimum mapped: the workflow's data ask. One stint at
        // a specific value past the good edge extends the landscape where it
        // matters. Not an optimization claim; explicitly a probe.
        if vertex_out.is_none()
            && let Some(key) = key.as_deref()
            && let Some(v) =
                probe_value(&nodes, crate::advice::tuning::limit_of(&session.facts, key))
        {
            let phrase = crate::advice::tuning::field_phrase(key);
            let best = nodes
                .iter()
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|n| n.0)
                .unwrap_or(v);
            recs.push(recommend::Recommendation {
                apply: vec![(key.to_string(), v.to_string())],
                area: "probe",
                suggestion: Some(format!(
                    "{phrase}: {}",
                    crate::advice::tuning::display_value(key, &v.to_string(), &session.facts),
                )),
                advice: format!(
                    "probe: one stint here extends the map where it still \
                     improves. Set {phrase} to {v} with everything else \
                     unchanged; probes are one at a time, two unexplored \
                     changes in one stint cannot be separated"
                ),
                evidence: vec![format!(
                    "mapped so far: {} (cumulative ideal delta; lower = faster)",
                    nodes_summary(&nodes),
                )],
                confidence: recommend::Confidence::Low,
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
            nodes,
            fit,
            vertex: vertex_out,
            measurements: mviews,
        });
    }

    // ---- effect-map prior: untried levers ----
    // The cross-campaign map (tuners map) is a PRIOR: families this campaign
    // has measured are owned by the local evidence above and never touched.
    // For the rest, estimate which behavioural direction has been profitable
    // here (per-field pace correlation) and surface the best-aligned map
    // cell as one Low-confidence experiment suggestion.
    if let Ok(text) = std::fs::read_to_string(crate::util::data_path("effect-map.tsv"))
        && let Ok(emap) = crate::advice::effectmap::parse(&text)
        && let Some(met) = c.stints.last().and_then(|s| s.met.as_ref())
    {
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
        let car = c.stints.last().and_then(|s| car_of(&s.stint));
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
        if let Some(rec) = map_prior(&emap, &trends, &ctx, &c.measurements, &recs) {
            recs.push(rec);
        }
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

    // History-only recs arrive unsorted; keep most-confident-first for display.
    recs.sort_by_key(|r| std::cmp::Reverse(r.confidence));
    // Cite tune absolutes only when the journal's stints are the session
    // car's; an explicitly passed foreign journal must not quote this car's
    // sliders as if they were its own.
    let current_tune = if car_of(&last.stint) == session.car {
        enrich_with_tune(&mut recs, &session)
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
        // The headline now carries the value; the relative phrasing from
        // blind-mode reconciliation becomes redundant.
        r.advice = r
            .advice
            .replace(&format!(" (go {delta:+.1} slider units from here)"), "");
        r.suggestion = match base.and_then(|b| b.values.get(key)?.parse::<f32>().ok()) {
            Some(cur) => {
                let mut target = cur + delta;
                if let Some(lim) = crate::advice::tuning::limit_of(&session.facts, key) {
                    target = target.clamp(lim.0, lim.1);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::advice::recommend::{Confidence, Recommendation};
    use crate::advice::tuning::Revision;

    fn balance_rec() -> Recommendation {
        Recommendation {
            apply: Vec::new(),
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
        let mut rev = Revision {
            stamp: "20260721-000000".into(),
            ..Default::default()
        };
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
        assert!(
            recs[0].advice.contains("stiffen the rear instead"),
            "{}",
            recs[0].advice
        );
        let implied = recs[0].implied.unwrap();
        assert_eq!(implied.family, journal::Family::RearRoll);
        assert!(!implied.softer);
        assert!(
            recs[0].evidence.iter().any(|e| e.contains("AT MINIMUM")),
            "{:?}",
            recs[0].evidence
        );
        assert!(
            recs[0]
                .evidence
                .iter()
                .any(|e| e.contains("advised direction exhausted"))
        );
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
        assert!(
            recs[0].advice.contains("reduce front roll stiffness"),
            "{}",
            recs[0].advice
        );
        assert_eq!(recs[0].implied.unwrap().family, journal::Family::FrontRoll);
        assert!(
            recs[0]
                .evidence
                .iter()
                .any(|e| e.contains("work with front springs")),
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
        assert!(
            recs[0].advice.contains("reduce front roll stiffness"),
            "{}",
            recs[0].advice
        );
        assert!(
            recs[0]
                .evidence
                .iter()
                .any(|e| e.contains("work with front springs")),
            "{:?}",
            recs[0].evidence
        );
        assert!(recs[0].evidence.iter().all(|e| !e.contains("exhausted")));
    }

    /// The user's worked example: front ARB 10..16 with lap times showing
    /// decaying improvement then a slowdown: the fitted optimum sits between
    /// 14 and 15, not at the best tried value or a bisection of the last step.
    #[test]
    fn quad_fit_finds_the_interior_optimum() {
        // Cumulative deltas from lap times 60.0, 59.0, 58.3, 58.0, 58.5.
        let pts = [
            (10.0, 0.0),
            (12.0, -1.0),
            (14.0, -1.7),
            (15.0, -2.0),
            (16.0, -1.5),
        ];
        let (a, b, _) = quad_fit(&pts).unwrap();
        assert!(a > 0.0, "upward curvature (a minimum exists)");
        let vertex = (-b / (2.0 * a)) as f32;
        assert!((14.0..=15.2).contains(&vertex), "vertex {vertex}");

        // Monotonic data has no trustworthy interior minimum.
        let mono = [(10.0, 0.0), (12.0, -1.0), (14.0, -2.0)];
        if let Some((a, b, _)) = quad_fit(&mono) {
            let v = (-b / (2.0 * a)) as f32;
            assert!(
                !(10.0..=14.0).contains(&v) || a <= 0.0,
                "no interior vertex: a={a} v={v}"
            );
        }
        assert!(
            quad_fit(&[(10.0, 0.0), (12.0, -1.0)]).is_none(),
            "2 points fit nothing"
        );
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
        // One small improving step (the Ferrari final-drive case): a quarter
        // span rounds onto the best value, so probe one display step out instead.
        let nodes = [(3.95, 0.0, 1), (4.1, -0.27, 1)];
        assert_eq!(probe_value(&nodes, None), Some(4.2));
        // Flat landscape: nothing worth a stint.
        let nodes = [(3.35, -0.04, 1), (3.63, 0.03, 1)];
        assert_eq!(probe_value(&nodes, None), None);
        // Best pinned at the slider bound: no new point exists.
        let nodes = [(0.0, -0.3, 1), (50.0, 0.2, 1)];
        assert_eq!(probe_value(&nodes, Some((0.0, 100.0))), None);
    }

    /// Deleted recordings are skipped but their tune changes carry forward:
    /// the next surviving entry's note becomes the honest compound, and a
    /// trailing missing entry just drops.
    #[test]
    fn missing_stints_merge_notes_forward() {
        let e = |path: &str, note: Option<&str>| journal::Entry {
            path: path.to_string(),
            note: note.map(String::from),
        };
        let entries = vec![
            e("a.ftel", Some("baseline")),
            e("gone1.ftel", Some("front arb -0.4")),
            e("gone2.ftel", Some("final drive +0.15")),
            e("b.ftel", Some("rear arb -1")),
            e("gone3.ftel", Some("front camber -1")),
        ];
        let (kept, missing) = drop_missing_entries(entries, |p| !p.starts_with("gone"));
        assert_eq!(missing, vec!["gone1.ftel", "gone2.ftel", "gone3.ftel"]);
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0].note.as_deref(), Some("baseline"));
        assert_eq!(
            kept[1].note.as_deref(),
            Some("front arb -0.4; final drive +0.15; rear arb -1"),
            "both skipped steps' changes precede the surviving stint"
        );
    }

    /// Boundary markers gate the implicit-step scan: parked journals accrue
    /// nothing, resumed ones only take stints newer than the resume.
    #[test]
    fn campaign_bound_reads_the_last_marker() {
        assert!(matches!(
            campaign_bound("# car\na.ftel | baseline\n"),
            CampaignBound::Open
        ));
        assert!(matches!(
            campaign_bound("a.ftel | baseline\n# parked 20260724-190000\n"),
            CampaignBound::Closed
        ));
        match campaign_bound("a.ftel | x\n# parked 20260724-190000\n# resumed 20260724-210000\n") {
            CampaignBound::Since(s) => assert_eq!(s, "20260724-210000"),
            _ => panic!("expected Since"),
        }
        // Park after a resume closes it again.
        assert!(matches!(
            campaign_bound("# resumed 20260724-210000\n# parked 20260724-220000\n"),
            CampaignBound::Closed
        ));
    }

    /// Minimal profileable stint: 3 laps at 30 m/s so lap 1 is a completed
    /// flying lap (time from lap 2's LastLap, captured from its start).
    fn write_stint_with_laps(path: &Path) {
        let mut w = crate::telemetry::stint::StintWriter::create(path).unwrap();
        for l in 0..3u16 {
            for i in 0..60 {
                let t = (l as f32 * 60.0 + i as f32) * 0.1;
                let f = crate::telemetry::packet::TelemetryFrame {
                    is_race_on: true,
                    lap_number: l,
                    current_lap: i as f32 * 0.1,
                    current_race_time: t,
                    last_lap: if l > 0 { 6.0 } else { 0.0 },
                    distance_traveled: t * 30.0 + 1.0,
                    speed: 30.0,
                    car_ordinal: 42,
                    ..Default::default()
                };
                w.write_packet((t * 1e6) as u64 + 1, &crate::telemetry::packet::encode(&f))
                    .unwrap();
            }
        }
    }

    /// A mid-campaign stint with no completed laps (event entered, abandoned
    /// in the pause menu) is skipped with its note merged forward, not a hard
    /// error that kills advise.
    #[test]
    fn lapless_middle_stint_is_skipped_note_merged() {
        let dir = std::env::temp_dir().join(format!("tuners-lapless-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let laps = dir.join("stint-20260101-000000.ftel");
        write_stint_with_laps(&laps);
        let laps = laps.to_string_lossy().into_owned();
        // Real capture with no completed lap: the shape the pause-menu
        // auto-cut produces.
        let no_laps = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/real-01.ftel");
        let entries = vec![
            journal::Entry {
                path: laps.clone(),
                note: None,
            },
            journal::Entry {
                path: no_laps.to_string(),
                note: Some("front arb +1".to_string()),
            },
            journal::Entry {
                path: laps.clone(),
                note: Some("rear arb -1".to_string()),
            },
        ];
        let session = TuningSession::default();
        let c = load_campaign(entries, &session, "test").expect("lap-less middle stint tolerated");
        assert_eq!(c.stints.len(), 2);
        assert_eq!(c.no_laps, vec![no_laps.to_string()]);
        assert_eq!(
            c.stints[1].entry.note.as_deref(),
            Some("front arb +1; rear arb -1")
        );
        assert!(c.in_progress.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stint_stamps_parse_from_both_naming_schemes() {
        assert_eq!(
            stint_stamp("sessions/stint-20260720-233644.ftel"),
            Some("20260720-233644")
        );
        assert_eq!(
            stint_stamp("sessions/session-20260719-115355.ftel"),
            Some("20260719-115355")
        );
        assert_eq!(stint_stamp("sessions/other.ftel"), None);
    }
}
