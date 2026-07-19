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
/// field changes (None = no journal family yet; the change is still recorded).
/// Order matches the in-game tuning menu.
pub const FIELDS: &[(&str, &str)] = &[
    ("tire_psi_f", "front tire psi"),
    ("tire_psi_r", "rear tire psi"),
    ("final_drive", "final drive"),
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
    ("diff_accel", "diff accel"),
    ("diff_decel", "diff decel"),
    ("diff_center", "center diff balance"),
];

pub fn field_phrase(key: &str) -> &str {
    FIELDS
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, p)| *p)
        .unwrap_or(key)
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
                let delta = new - old;
                if delta != 0.0 {
                    parts.push(format!("{phrase} {delta:+}"));
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
        let b = rev("t2", &[("arb_f", "26"), ("aero_r", "200"), ("tire_psi_f", "28.5")]);
        let note = diff_note(&a, &b);
        assert!(note.contains("front arb +2"), "{note}");
        assert!(note.contains("rear aero +20"), "{note}");
        assert!(note.contains("front tire psi = 28.5"), "{note}");
        // Compound steps are deliberately unattributable to one family.
        assert_eq!(crate::analysis::journal::parse_change(&note), None, "{note}");
    }

    #[test]
    fn identical_revisions_produce_no_note() {
        let a = rev("t1", &[("arb_f", "24")]);
        assert_eq!(diff_note(&a, &rev("t2", &[("arb_f", "24")])), "");
    }
}
