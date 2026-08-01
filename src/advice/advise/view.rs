//! The advise result surface: every struct the CLI, API, and app render.

use super::*;

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
    /// Sample sd of the stint's flying-lap times (None under 3 laps).
    /// REPORT-ONLY consistency channel: a change that shrinks scatter at
    /// equal pace made the car easier to drive, but no rule reads this yet
    /// (its same-setup noise floor is uncalibrated).
    pub scatter_s: Option<f32>,
    /// (understeer index, front slip frac, rear slip frac).
    pub balance: Option<(f32, f32, f32)>,
    pub note: Option<String>,
    /// Slider positions relative to baseline, when the note trail supports them.
    pub pos: Option<(f32, f32)>,
    /// Measured outcome vs the previous step: Ok((word, verdict delta,
    /// unequal laps)) or Err(reason) when not comparable. None for the first
    /// step. The delta is the 2-of-3 vote (median of ideal/best/median-lap).
    pub outcome: Option<Result<(&'static str, f32, bool), String>>,
    /// The vote's component deltas vs the previous step (ideal, best,
    /// median lap), for disagreement hedges. Set whenever outcome is Ok.
    pub currencies: Option<(f32, f32, f32)>,
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
    /// Verdict delta anchor -> last (positive = last is slower): the 2-of-3
    /// vote of the component currencies below.
    pub delta_s: f32,
    /// Component deltas (ideal, best, median lap), for disagreement hedges.
    pub currencies: (f32, f32, f32),
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
    /// (value, cumulative verdict delta s, samples), ascending by value.
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
