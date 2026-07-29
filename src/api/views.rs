//! Analysis views over recordings: laps, compare, report, and the
//! advise DTO tree.

use super::*;

/// Per-lap distance-binned speed traces: the lap chart's data.
#[derive(Serialize, Type, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LapView {
    pub lap: u32,
    pub time: f32,
    pub standing: bool,
    pub speeds: Vec<f32>,
}

#[derive(Serialize, Type, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LapsView {
    pub bin_meters: f32,
    pub best_time: f32,
    /// Per-bin corroboration of the spliced ideal: true = a second lap
    /// reproduces this bin's speed within splice tolerance. Drives the confidence
    /// strip under the speed chart.
    pub corroborated: Vec<bool>,
    pub laps: Vec<LapView>,
}

pub fn laps_view(file: &str) -> Result<LapsView, ApiError> {
    let path = checked_session_path(file)?;
    let session = crate::analysis::Stint::load(path)
        .map_err(|e| ApiError::internal(format!("{}: {e}", path.display())))?;
    let profile =
        crate::analysis::profile::stint_profile(&session.frames).map_err(ApiError::internal)?;
    let laps = profile
        .laps
        .iter()
        .map(|lap| LapView {
            lap: lap.lap_number as u32 + 1,
            time: lap.time_s,
            standing: lap.standing_start,
            speeds: lap.bins[..profile.shared_bins]
                .iter()
                .map(|b| b.speed_avg)
                .collect(),
        })
        .collect();
    Ok(LapsView {
        bin_meters: crate::analysis::profile::BIN_METERS,
        best_time: profile.best_lap_time_s,
        corroborated: profile.corroboration().corroborated,
        laps,
    })
}

/// One side of an A/B comparison.
#[derive(Serialize, Type, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CompareSide {
    pub file: String,
    pub laps: u32,
    pub best: f32,
    pub ideal: f32,
    pub standing_only: bool,
}

/// A/B comparison: both composited ideal-lap speed traces plus the per-bin
/// time delta (B − A), for the overlay + segment-delta view.
#[derive(Serialize, Type, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CompareView {
    pub bin_meters: f32,
    pub a: CompareSide,
    pub b: CompareSide,
    pub speeds_a: Vec<f32>,
    pub speeds_b: Vec<f32>,
    pub times_a: Vec<f32>,
    pub delta: Vec<f32>,
    pub unequal_laps: bool,
    pub car_mismatch: bool,
}

pub fn compare_view(a: &str, b: &str) -> Result<CompareView, ApiError> {
    let a_path = checked_session_path(a)?;
    let b_path = checked_session_path(b)?;
    let profile = |path: &Path| -> Result<_, ApiError> {
        let session = crate::analysis::Stint::load(path)
            .map_err(|e| ApiError::internal(format!("{}: {e}", path.display())))?;
        crate::analysis::profile::stint_profile(&session.frames)
            .map_err(|e| ApiError::internal(format!("{}: {e}", path.display())))
    };
    let pa = profile(a_path)?;
    let pb = profile(b_path)?;
    let cmp = crate::analysis::compare::compare(&pa, &pb).map_err(ApiError::internal)?;
    let shared = cmp.bin_delta_s.len();
    let speeds = |p: &crate::analysis::profile::StintProfile| {
        p.composite.bins[..shared]
            .iter()
            .map(|bin| bin.speed_avg)
            .collect()
    };
    let side = |path: &Path, p: &crate::analysis::profile::StintProfile| CompareSide {
        file: path.display().to_string(),
        laps: p.laps.len() as u32,
        best: p.best_lap_time_s,
        ideal: p.composite.time_s,
        standing_only: p.standing_start_only,
    };
    Ok(CompareView {
        bin_meters: crate::analysis::profile::BIN_METERS,
        a: side(a_path, &pa),
        b: side(b_path, &pb),
        speeds_a: speeds(&pa),
        speeds_b: speeds(&pb),
        times_a: pa.composite.bins[..shared]
            .iter()
            .map(|b| b.time_s)
            .collect(),
        delta: cmp.bin_delta_s.clone(),
        unequal_laps: pa.laps.len() != pb.laps.len(),
        car_mismatch: cmp.car_mismatch,
    })
}

/// Full text report for one stint (the per-run report view).
pub fn report_text(file: &str) -> Result<String, ApiError> {
    let path = checked_session_path(file)?;
    crate::analysis::report::full_session_report(path).map_err(ApiError::internal)
}

// -------------------------------------------------------------------- advise

#[derive(Serialize, Type, Debug, Clone)]
#[serde(rename_all_fields = "camelCase", untagged)]
pub enum OutcomeView {
    Measured {
        word: String,
        delta_s: f32,
        unequal_laps: bool,
    },
    NotComparable {
        error: String,
    },
}

#[derive(Serialize, Type, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct StepFamilyView {
    pub area: String,
    /// Where this family's fingerprint is judged: "straights" | "entry" | "corners".
    pub channel: String,
}

#[derive(Serialize, Type, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RowAnchorView {
    pub vs_step: u32,
    pub areas: String,
    pub delta_s: f32,
    pub word: String,
    pub weak: bool,
}

#[derive(Serialize, Type, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct StepView {
    pub path: String,
    pub laps: u32,
    pub best_s: f32,
    pub ideal_s: f32,
    /// (understeer index, front slip frac, rear slip frac).
    pub balance: Option<(f32, f32, f32)>,
    pub note: Option<String>,
    /// Slider positions relative to baseline, when the note trail supports them.
    pub pos: Option<(f32, f32)>,
    pub outcome: Option<OutcomeView>,
    /// Where the time moved vs the previous step: (entry, exit, straights).
    pub split: Option<(f32, f32, f32)>,
    pub anchor: Option<RowAnchorView>,
    /// Families this step's note changed, each with its judged channel.
    pub families: Vec<StepFamilyView>,
}

#[derive(Serialize, Type, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AnchorView {
    pub vs_step: u32,
    pub areas: String,
    pub changes: String,
    pub delta_s: f32,
    pub word: String,
    pub weak: bool,
    pub reconciled: bool,
    pub split: (f32, f32, f32),
    pub effects: BTreeMap<String, f32>,
}

#[derive(Serialize, Type, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AbaView {
    pub families: String,
    pub effect_s: f32,
    pub drift_s: f32,
    pub effects: BTreeMap<String, f32>,
}

#[derive(Serialize, Type, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MeasurementView {
    pub from_step: u32,
    pub to_step: u32,
    pub desc: String,
    pub delta_s: f32,
    pub split: Option<(f32, f32, f32)>,
    pub weak: bool,
    pub direct: bool,
    pub effects: BTreeMap<String, f32>,
}

#[derive(Serialize, Type, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LandscapeView {
    pub area: String,
    pub phrase: String,
    pub key: Option<String>,
    /// (value, cumulative ideal delta s, samples), ascending by value.
    pub nodes: Vec<(f32, f32, u32)>,
    /// y = ax² + bx + c least-squares fit over the nodes (3+ nodes).
    pub fit: Option<(f32, f32, f32)>,
    pub vertex: Option<f32>,
    pub measurements: Vec<MeasurementView>,
}

#[derive(Serialize, Type, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RecommendationView {
    pub confidence: String,
    pub area: String,
    pub suggestion: Option<String>,
    /// Machine-readable accept payload: canonical (key, value) pairs.
    pub apply: Vec<(String, String)>,
    pub advice: String,
    pub evidence: Vec<String>,
}

#[derive(Serialize, Type, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TuneFieldView {
    pub phrase: String,
    pub value: String,
    pub unit: Option<String>,
}

#[derive(Serialize, Type, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AdviseView {
    pub journal: Option<String>,
    pub advice_for: String,
    pub steps: Vec<StepView>,
    pub anchor: Option<AnchorView>,
    pub aba: Option<AbaView>,
    pub landscapes: Vec<LandscapeView>,
    pub drift_floor: Option<(u32, f32)>,
    pub effect_floor: BTreeMap<String, f32>,
    pub in_progress: Option<String>,
    pub missing: Vec<String>,
    pub no_laps: Vec<String>,
    pub recommendations: Vec<RecommendationView>,
    pub current_tune: Vec<TuneFieldView>,
}

fn measurement_view(m: &crate::advice::advise::MeasurementView) -> MeasurementView {
    MeasurementView {
        from_step: m.from_step as u32,
        to_step: m.to_step as u32,
        desc: m.desc.clone(),
        delta_s: m.delta_s,
        split: m.split,
        weak: m.weak,
        direct: m.direct,
        effects: effects_map(&m.effects),
    }
}

pub fn advise_view(v: &crate::advice::advise::AdviseView) -> AdviseView {
    AdviseView {
        journal: v.journal.clone(),
        advice_for: v.advice_for.clone(),
        steps: v
            .steps
            .iter()
            .map(|s| StepView {
                path: s.path.clone(),
                laps: s.laps as u32,
                best_s: s.best_s,
                ideal_s: s.ideal_s,
                balance: s.balance,
                note: s.note.clone(),
                pos: s.pos,
                outcome: s.outcome.as_ref().map(|o| match o {
                    Ok((word, delta, unequal)) => OutcomeView::Measured {
                        word: word.to_string(),
                        delta_s: *delta,
                        unequal_laps: *unequal,
                    },
                    Err(e) => OutcomeView::NotComparable { error: e.clone() },
                }),
                split: s.split,
                families: s
                    .families
                    .iter()
                    .map(|f| StepFamilyView {
                        area: f.area.to_string(),
                        channel: f.channel.to_string(),
                    })
                    .collect(),
                anchor: s.anchor.as_ref().map(|a| RowAnchorView {
                    vs_step: a.vs_step as u32,
                    areas: a.areas.clone(),
                    delta_s: a.delta_s,
                    word: a.word.to_string(),
                    weak: a.weak,
                }),
            })
            .collect(),
        anchor: v.anchor.as_ref().map(|a| AnchorView {
            vs_step: a.vs_step as u32,
            areas: a.areas.clone(),
            changes: a.changes.clone(),
            delta_s: a.delta_s,
            word: a.word.to_string(),
            weak: a.weak,
            reconciled: a.reconciled,
            split: a.split,
            effects: effects_map(&a.effects),
        }),
        aba: v.aba.as_ref().map(|a| AbaView {
            families: a.families.clone(),
            effect_s: a.effect_s,
            drift_s: a.drift_s,
            effects: effects_map(&a.effects),
        }),
        landscapes: v
            .landscapes
            .iter()
            .map(|l| LandscapeView {
                area: l.area.to_string(),
                phrase: l.phrase.clone(),
                key: l.key.clone(),
                nodes: l
                    .nodes
                    .iter()
                    .map(|(v, c, n)| (*v, *c, *n as u32))
                    .collect(),
                fit: l.fit,
                vertex: l.vertex,
                measurements: l.measurements.iter().map(measurement_view).collect(),
            })
            .collect(),
        drift_floor: v.drift_floor.map(|(n, f)| (n as u32, f)),
        effect_floor: effects_map(&v.effect_floor),
        in_progress: v.in_progress.clone(),
        missing: v.missing.clone(),
        no_laps: v.no_laps.clone(),
        recommendations: v
            .recommendations
            .iter()
            .map(|r| RecommendationView {
                confidence: r.confidence.label().to_string(),
                area: r.area.to_string(),
                suggestion: r.suggestion.clone(),
                apply: r.apply.clone(),
                advice: r.advice.clone(),
                evidence: r.evidence.clone(),
            })
            .collect(),
        current_tune: v
            .current_tune
            .iter()
            .map(|(phrase, value, unit)| TuneFieldView {
                phrase: phrase.clone(),
                value: value.clone(),
                unit: unit.map(str::to_string),
            })
            .collect(),
    }
}

/// Advice for the active session: resolves the session car's journal and runs
/// the shared advise engine.
pub fn advise_active(
    session_file: &str,
    journal_base: &str,
    sessions_dir: &str,
) -> Result<AdviseView, ApiError> {
    let session = crate::advice::tuning::TuningSession::load(Path::new(session_file));
    let journal = crate::advice::tuning::journal_path_for(session.car, journal_base);
    crate::advice::advise::advise(&journal, Path::new(session_file), sessions_dir)
        .map(|v| advise_view(&v))
        .map_err(ApiError::internal)
}
