//! The tuning session: the user-level unit of work. One session = one
//! explicitly chosen car being tuned across stints — car facts telemetry can't
//! see (front weight %, compound, assists), plus the tune itself as absolute
//! slider values, revision by revision (docs/design.md "tune input model").
//!
//! Saving a new revision diffs it against the previous one and produces the
//! journal note automatically — the ideal workflow is "enter the new tune",
//! not "describe what changed". Stored as a human-editable text file
//! (default tune-session.txt), same ethos as tune-journal.txt:
//!
//! ```text
//! # tuners tuning session
//! car = 2352
//! name = Ford GT
//! front_weight_pct = 42.5
//! abs = on
//!
//! [tune 20260719-224500]
//! arb_f = 24
//! arb_r = 30
//! ```

use std::collections::BTreeMap;
use std::path::Path;

/// One saved state of the tuning menu. All fields optional (incremental input
/// model): keys are fixed identifiers (see FIELDS), values kept as text so the
/// format never loses what the user typed; numeric parsing happens at use.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Revision {
    pub stamp: String,
    pub values: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default)]
pub struct TuningSession {
    /// Explicitly chosen car ordinal — stints from other cars are outside the session.
    pub car: Option<i32>,
    /// Car facts telemetry can't provide: front_weight_pct, weight, compound,
    /// abs/tcs/stability, plus anything else the user records. Free key set.
    pub facts: BTreeMap<String, String>,
    pub revisions: Vec<Revision>,
}

/// Tune fields the dashboard form offers, with the journal phrase used when the
/// field changes. Order matches the in-game tuning menu.
///
/// An EMPTY (absent) field means "not tunable on this car" — upgrade choices
/// decide what the game exposes (e.g. only rear diff fields filled = RWD diff;
/// front+rear+center = AWD). Absence is never treated as a reset in diffs.
///
/// VALUES ARE STORED IN CANONICAL IMPERIAL UNITS (what FH6 uses internally):
/// psi, lb/in, in, lb. Unit preferences are a pure display/formatting layer in
/// the dashboard — the file, the diffs, and the journal never change units.
pub const FIELDS: &[(&str, &str)] = &[
    ("tire_pressure_f", "front tire pressure"),
    ("tire_pressure_r", "rear tire pressure"),
    ("final_drive", "final drive"),
    ("gear_1", "1st gear"),
    ("gear_2", "2nd gear"),
    ("gear_3", "3rd gear"),
    ("gear_4", "4th gear"),
    ("gear_5", "5th gear"),
    ("gear_6", "6th gear"),
    ("gear_7", "7th gear"),
    ("gear_8", "8th gear"),
    ("gear_9", "9th gear"),
    ("gear_10", "10th gear"),
    ("camber_f", "front camber"),
    ("camber_r", "rear camber"),
    ("toe_f", "front toe"),
    ("toe_r", "rear toe"),
    ("caster", "caster"),
    ("arb_f", "front arb"),
    ("arb_r", "rear arb"),
    ("springs_f", "front springs"),
    ("springs_r", "rear springs"),
    ("ride_height_f", "front ride height"),
    ("ride_height_r", "rear ride height"),
    ("rebound_f", "front rebound"),
    ("rebound_r", "rear rebound"),
    ("bump_f", "front bump"),
    ("bump_r", "rear bump"),
    ("aero_f", "front aero"),
    ("aero_r", "rear aero"),
    ("brake_balance", "brake balance"),
    ("brake_pressure", "brake pressure"),
    ("diff_accel_f", "front diff accel"),
    ("diff_decel_f", "front diff decel"),
    ("diff_accel_r", "rear diff accel"),
    ("diff_decel_r", "rear diff decel"),
    ("diff_center", "center diff balance"),
];

/// The journal belongs to the session: with a session car set, the journal
/// file is derived per car ("tune-journal.txt" -> "tune-journal-1314.txt") so
/// different cars' trajectories never mix. No car -> the base path (blind mode).
/// Coarse experiment area for a tune field — the granularity at which two
/// setups "differ by one experiment". Setup-state comparison (advise anchor)
/// treats a stint whose setup differs from an ancestor's in exactly one area
/// as a clean single-family A/B, however compound the step notes in between.
pub fn field_area(key: &str) -> &'static str {
    match key {
        "arb_f" | "springs_f" => "front roll",
        "arb_r" | "springs_r" => "rear roll",
        "aero_f" => "front aero",
        "aero_r" => "rear aero",
        "final_drive" => "gearing",
        k if k.starts_with("gear_") => "gearing",
        "diff_accel_f" | "diff_accel_r" | "diff_center" => "diff accel",
        "diff_decel_f" | "diff_decel_r" => "diff decel",
        "brake_balance" | "brake_pressure" => "brakes",
        "rebound_f" | "rebound_r" | "bump_f" | "bump_r" => "damping",
        "ride_height_f" | "ride_height_r" => "ride height",
        "tire_pressure_f" | "tire_pressure_r" => "tire pressure",
        "camber_f" | "camber_r" | "toe_f" | "toe_r" | "caster" => "alignment",
        _ => "other",
    }
}

/// Keys whose values differ between two revisions (beyond per-field noise).
pub fn diff_keys(a: &Revision, b: &Revision) -> Vec<String> {
    let mut keys: Vec<String> = a.values.keys().chain(b.values.keys()).cloned().collect();
    keys.sort();
    keys.dedup();
    keys.retain(|k| {
        let (va, vb) = (a.values.get(k), b.values.get(k));
        match (va, vb) {
            (Some(va), Some(vb)) => match (va.parse::<f32>(), vb.parse::<f32>()) {
                (Ok(fa), Ok(fb)) => (fa - fb).abs() >= diff_epsilon(k),
                _ => va != vb,
            },
            // The form always posts every filled field; absence means "not
            // entered", not a change (mirrors diff_note).
            _ => false,
        }
    });
    keys
}

pub fn journal_path_for(car: Option<i32>, base: &str) -> String {
    match car {
        Some(car) => match base.rsplit_once('.') {
            Some((stem, ext)) => format!("{stem}-{car}.{ext}"),
            None => format!("{base}-{car}"),
        },
        None => base.to_string(),
    }
}

pub fn field_phrase(key: &str) -> &str {
    FIELDS
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, p)| *p)
        .unwrap_or(key)
}

/// Canonical (stored) unit for unit-bearing fields; None for unitless sliders,
/// degrees, percentages, and ratios.
pub fn canonical_unit(key: &str) -> Option<&'static str> {
    match key {
        "tire_pressure_f" | "tire_pressure_r" => Some("psi"),
        "springs_f" | "springs_r" => Some("lb/in"),
        "ride_height_f" | "ride_height_r" => Some("in"),
        "aero_f" | "aero_r" => Some("lb"),
        _ => None,
    }
}

/// Format a canonical value in the session's display units (unit_* facts) —
/// "11591.4436" (lb/in) becomes "207.0 kgf/mm" for a kgfmm session. Fields
/// without units (or unparseable values) pass through unchanged.
pub fn display_value(key: &str, canon: &str, facts: &BTreeMap<String, String>) -> String {
    let dim = match key {
        "tire_pressure_f" | "tire_pressure_r" => "pressure",
        "springs_f" | "springs_r" => "springs",
        "ride_height_f" | "ride_height_r" => "length",
        "aero_f" | "aero_r" => "force",
        "weight" => "mass",
        _ => return canon.to_string(),
    };
    let Ok(v) = canon.parse::<f32>() else { return canon.to_string() };
    let pref = facts.get(&format!("unit_{dim}")).map(String::as_str);
    // (factor canonical -> display, decimals, label); default = canonical unit.
    let (k, dp, label) = match (dim, pref) {
        ("pressure", Some("bar")) => (0.0689476, 2, "bar"),
        ("pressure", _) => (1.0, 1, "psi"),
        ("springs", Some("kgfmm")) => (0.0178580, 1, "kgf/mm"),
        ("springs", _) => (1.0, 0, "lb/in"),
        ("length", Some("cm")) => (2.54, 1, "cm"),
        ("length", _) => (1.0, 1, "in"),
        ("force", Some("kgf")) => (0.453592, 0, "kgf"),
        ("force", _) => (1.0, 0, "lb"),
        ("mass", Some("kg")) => (0.453592, 0, "kg"),
        ("mass", _) => (1.0, 0, "lb"),
        _ => (1.0, 2, ""),
    };
    format!("{:.dp$} {label}", v * k, dp = dp)
}

/// Smallest delta worth journaling, per field, in canonical units — half a
/// sensible display step. Guards against phantom diffs from unit-conversion
/// round-trips (enter 700 lb/in, display 12.5 kgf/mm, re-save → 699.96).
fn diff_epsilon(key: &str) -> f32 {
    match key {
        "tire_pressure_f" | "tire_pressure_r" => 0.05,
        "springs_f" | "springs_r" => 0.5,
        "ride_height_f" | "ride_height_r" => 0.05,
        "aero_f" | "aero_r" => 0.5,
        _ => 1e-4,
    }
}

impl TuningSession {
    pub fn parse(text: &str) -> TuningSession {
        let mut session = TuningSession::default();
        let mut current: Option<Revision> = None;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(rest) = line.strip_prefix("[tune") {
                if let Some(rev) = current.take() {
                    session.revisions.push(rev);
                }
                current = Some(Revision {
                    stamp: rest.trim_end_matches(']').trim().to_string(),
                    values: BTreeMap::new(),
                });
                continue;
            }
            let Some((key, value)) = line.split_once('=') else { continue };
            let (key, value) = (key.trim(), value.trim());
            match &mut current {
                Some(rev) => {
                    rev.values.insert(key.to_string(), value.to_string());
                }
                None if key == "car" => session.car = value.parse().ok(),
                None => {
                    session.facts.insert(key.to_string(), value.to_string());
                }
            }
        }
        if let Some(rev) = current {
            session.revisions.push(rev);
        }
        session
    }

    pub fn load(path: &Path) -> TuningSession {
        Self::parse(&std::fs::read_to_string(path).unwrap_or_default())
    }

    pub fn render(&self) -> String {
        let mut out = String::from("# tuners tuning session\n");
        if let Some(car) = self.car {
            out.push_str(&format!("car = {car}\n"));
        }
        for (k, v) in &self.facts {
            out.push_str(&format!("{k} = {v}\n"));
        }
        for rev in &self.revisions {
            out.push_str(&format!("\n[tune {}]\n", rev.stamp));
            for (k, v) in &rev.values {
                out.push_str(&format!("{k} = {v}\n"));
            }
        }
        out
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        std::fs::write(path, self.render())
    }

    pub fn latest(&self) -> Option<&Revision> {
        self.revisions.last()
    }
}

/// The journal note for a new revision: signed deltas for changed numeric
/// fields ("front arb -2"), old→new for the rest. Multiple changes join with
/// "; " — the advice parser attributes single-family steps only, which is the
/// honest limit (a multi-parameter step's outcome can't be attributed anyway).
/// Empty when nothing changed.
pub fn diff_note(prev: &Revision, next: &Revision) -> String {
    let mut parts = Vec::new();
    for (key, new_val) in &next.values {
        let old_val = prev.values.get(key);
        if old_val == Some(new_val) {
            continue;
        }
        let phrase = field_phrase(key);
        match (
            old_val.and_then(|v| v.parse::<f32>().ok()),
            new_val.parse::<f32>(),
        ) {
            (Some(old), Ok(new)) => {
                // Round away f32 subtraction noise (20.70 - 19.50 = -1.2000008).
                let delta = ((new - old) * 1e4).round() / 1e4;
                if delta.abs() >= diff_epsilon(key) {
                    // Canonical unit suffixed so the journal is self-documenting;
                    // the math stays in one unit no matter how displays are set.
                    match canonical_unit(key) {
                        Some(unit) => parts.push(format!("{phrase} {delta:+} {unit}")),
                        None => parts.push(format!("{phrase} {delta:+}")),
                    }
                }
            }
            (None, _) if old_val.is_none() => parts.push(format!("{phrase} = {new_val}")),
            _ => parts.push(format!(
                "{phrase} {} -> {new_val}",
                old_val.map(String::as_str).unwrap_or("?")
            )),
        }
    }
    // Fields removed in the new revision are ignored: the form always posts
    // every filled field, so absence means "not entered", not "reset".
    parts.join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rev(stamp: &str, pairs: &[(&str, &str)]) -> Revision {
        Revision {
            stamp: stamp.into(),
            values: pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        }
    }

    #[test]
    fn journal_path_is_per_car_when_session_has_one() {
        assert_eq!(journal_path_for(Some(1314), "tune-journal.txt"), "tune-journal-1314.txt");
        assert_eq!(journal_path_for(None, "tune-journal.txt"), "tune-journal.txt");
        assert_eq!(journal_path_for(Some(7), "logs/journal"), "logs/journal-7");
    }

    #[test]
    fn roundtrips_through_text() {
        let mut s = TuningSession {
            car: Some(2352),
            ..Default::default()
        };
        s.facts.insert("front_weight_pct".into(), "42.5".into());
        s.facts.insert("abs".into(), "on".into());
        s.revisions.push(rev("20260719-224500", &[("arb_f", "24"), ("arb_r", "30")]));
        s.revisions.push(rev("20260719-231000", &[("arb_f", "22"), ("arb_r", "30")]));

        let parsed = TuningSession::parse(&s.render());
        assert_eq!(parsed.car, Some(2352));
        assert_eq!(parsed.facts.get("abs").map(String::as_str), Some("on"));
        assert_eq!(parsed.revisions, s.revisions);
    }

    #[test]
    fn diff_note_is_a_parseable_journal_delta() {
        let a = rev("t1", &[("arb_f", "24"), ("arb_r", "30")]);
        let b = rev("t2", &[("arb_f", "22"), ("arb_r", "30")]);
        let note = diff_note(&a, &b);
        assert_eq!(note, "front arb -2");
        // The note round-trips through the journal change parser with magnitude.
        let c = crate::analysis::journal::parse_change(&note).unwrap();
        assert_eq!(c.magnitude, Some(-2.0));
        assert!(c.softer);
    }

    #[test]
    fn multi_field_and_non_numeric_changes_are_recorded_honestly() {
        let a = rev("t1", &[("arb_f", "24"), ("aero_r", "180")]);
        let b = rev("t2", &[("arb_f", "26"), ("aero_r", "200"), ("tire_pressure_f", "28.5")]);
        let note = diff_note(&a, &b);
        assert!(note.contains("front arb +2"), "{note}");
        assert!(note.contains("rear aero +20 lb"), "{note}");
        assert!(note.contains("front tire pressure = 28.5"), "{note}");
        // Compound steps are deliberately unattributable to one family.
        assert_eq!(crate::analysis::journal::parse_change(&note), None, "{note}");
    }

    #[test]
    fn identical_revisions_produce_no_note() {
        let a = rev("t1", &[("arb_f", "24")]);
        assert_eq!(diff_note(&a, &rev("t2", &[("arb_f", "24")])), "");
    }

    #[test]
    fn deltas_are_rounded_not_float_noise() {
        let a = rev("t1", &[("arb_f", "20.70")]);
        let note = diff_note(&a, &rev("t2", &[("arb_f", "19.50")]));
        assert_eq!(note, "front arb -1.2");
    }

    /// The unit round-trip case: 700 lb/in shown as 12.5 kgf/mm re-enters as
    /// 699.96 lb/in — below the display step, so no phantom journal entry.
    #[test]
    fn sub_step_conversion_noise_is_not_a_change() {
        let a = rev("t1", &[("springs_f", "700")]);
        assert_eq!(diff_note(&a, &rev("t2", &[("springs_f", "699.9605")])), "");
        let note = diff_note(&a, &rev("t3", &[("springs_f", "672")]));
        assert_eq!(note, "front springs -28 lb/in");
    }
}
