//! Tuning journal: history-aware advice without knowing absolute setup values.
//! Blind mode stays blind to numbers — hill-climbing only needs the *direction* of
//! each past change plus the measured outcome (docs/plans/005-journal.md).

use super::recommend::{Confidence, Recommendation};

/// Parameter families the change-note parser understands (grow as exercised).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    FrontRoll,
    RearRoll,
}

/// A step on a parameter family. Still blind to absolute setup values: the
/// optional magnitude is a SIGNED delta in slider units (negative = softer),
/// which lets positions accumulate relative to baseline (plan 005 v2) without
/// ever knowing where baseline actually sits on the slider.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Change {
    pub family: Family,
    pub softer: bool,
    pub magnitude: Option<f32>,
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub path: String,
    pub note: Option<String>,
}

/// One session per line, chronological: `path | change note`. `#` comments and
/// blank lines are skipped; the note is optional.
pub fn parse_journal(text: &str) -> Vec<Entry> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let (path, note) = match line.split_once('|') {
                Some((p, n)) => {
                    let n = n.trim();
                    (p.trim().to_string(), (!n.is_empty()).then(|| n.to_string()))
                }
                None => (line.to_string(), None),
            };
            Some(Entry { path, note })
        })
        .collect()
}

/// Extract a change from a note: direction words ("front arb softer") or v2
/// slider deltas ("front arb -2", negative = softer). None when it doesn't
/// parse — the note still shows in the trajectory, it just can't modulate
/// advice. A direction word wins over the number's sign ("softened front arb
/// by 2" → -2); a bare unsigned number with no direction word stays unparsed.
pub fn parse_change(note: &str) -> Option<Change> {
    let t = note.to_lowercase();
    let front = t.contains("front");
    let rear = t.contains("rear");
    if front == rear {
        return None;
    }
    let roll = ["arb", "anti-roll", "antiroll", "roll bar", "spring"]
        .iter()
        .any(|k| t.contains(k));
    if !roll {
        return None;
    }
    let number = t
        .split_whitespace()
        .map(|tok| tok.trim_end_matches([',', ')', ':']))
        .find_map(|tok| tok.parse::<f32>().ok().map(|v| (v, tok.starts_with(['+', '-']))));
    let softer = if t.contains("soft") {
        true
    } else if t.contains("stiff") {
        false
    } else if let Some((v, true)) = number {
        v < 0.0
    } else {
        return None;
    };
    let magnitude = number.map(|(v, _)| if softer { -v.abs() } else { v.abs() });
    Some(Change {
        family: if front { Family::FrontRoll } else { Family::RearRoll },
        softer,
        magnitude,
    })
}

/// Slider positions relative to baseline after each entry: (front, rear).
/// A parsed change without a magnitude makes that family's position unknown
/// from there on — direction alone can't say where you ended up.
pub fn track_positions(changes: &[Option<Change>]) -> Vec<(Option<f32>, Option<f32>)> {
    let (mut front, mut rear) = (Some(0.0f32), Some(0.0f32));
    changes
        .iter()
        .map(|c| {
            if let Some(c) = c {
                let slot = match c.family {
                    Family::FrontRoll => &mut front,
                    Family::RearRoll => &mut rear,
                };
                *slot = match (*slot, c.magnitude) {
                    (Some(p), Some(m)) => Some(p + m),
                    _ => None,
                };
            }
            (front, rear)
        })
        .collect()
}

/// Lines to append to the journal when a new session opens after a tune change.
/// If the journal has no entries yet, the session the change was made *after*
/// (when known) is entered first as the baseline.
pub fn append_lines(
    journal_text: &str,
    prev_session: Option<&str>,
    new_session: &str,
    note: &str,
) -> String {
    let mut out = String::new();
    if parse_journal(journal_text).is_empty()
        && let Some(prev) = prev_session
    {
        out.push_str(&format!("{prev} | baseline\n"));
    }
    let note = note.replace(['|', '\n'], " ");
    out.push_str(&format!("{new_session} | {}\n", note.trim()));
    out
}

/// Ideal-lap delta below which a step's outcome is inconclusive (seconds).
const OUTCOME_CLEAR_S: f32 = 0.10;

/// Measured result of one journal step, from the ideal-lap delta between the
/// sessions before and after it (negative = the step made the car faster).
#[derive(Debug, Clone, Copy)]
pub enum Outcome {
    Improved(f32),
    Worsened(f32),
    Unclear(f32),
    NotComparable,
}

pub fn judge(ideal_delta_s: f32) -> Outcome {
    if ideal_delta_s <= -OUTCOME_CLEAR_S {
        Outcome::Improved(ideal_delta_s)
    } else if ideal_delta_s >= OUTCOME_CLEAR_S {
        Outcome::Worsened(ideal_delta_s)
    } else {
        Outcome::Unclear(ideal_delta_s)
    }
}

/// Fold the last step's measured outcome into the blind recommendations. A
/// recommendation pointing the same direction as a step that just LOST time gets
/// replaced by "revert half" — the behavioural signal alone would push past the
/// optimum forever (some understeer is lap-time-optimal).
pub fn reconcile(recs: &mut [Recommendation], change: Change, outcome: Outcome, note: &str) {
    for r in recs.iter_mut() {
        let Some(implied) = r.implied else { continue };
        if implied.family != change.family {
            continue;
        }
        let same_direction = implied.softer == change.softer;
        match (same_direction, outcome) {
            (true, Outcome::Worsened(d)) => {
                r.advice = format!(
                    "revert about half of the last change (\"{note}\"): the behaviour \
                     still points the same way, but that step cost lap time — the \
                     optimum is between the last two setups"
                );
                if let Some(m) = change.magnitude {
                    r.advice
                        .push_str(&format!(" (go {:+.1} slider units from here)", -m / 2.0));
                }
                r.evidence
                    .push(format!("last step in this direction lost {d:.2}s of ideal lap"));
                r.confidence = Confidence::High;
            }
            (true, Outcome::Improved(d)) => {
                r.evidence.push(format!(
                    "last step in this direction gained {:.2}s — a similar or smaller \
                     step is reasonable",
                    -d,
                ));
            }
            (true, Outcome::Unclear(d)) => {
                r.confidence = Confidence::Low;
                r.evidence.push(format!(
                    "last step in this direction was inconclusive ({d:+.2}s ideal) — \
                     match lap counts or run A-B-A before stepping again"
                ));
            }
            // The last step moved AGAINST this advice and gained: the optimum is
            // bracketed. The behavioural signal will keep pointing the same way
            // forever — the residual behaviour is likely the fast setup.
            (false, Outcome::Improved(d)) => {
                r.advice = format!(
                    "hold this setting: the last change (\"{note}\") moved against \
                     this advice and still gained {:.2}s — the optimum is likely \
                     bracketed, and the remaining behaviour may simply be what the \
                     fast setup feels like",
                    -d,
                );
                r.confidence = Confidence::Medium;
                r.evidence.push(
                    "behaviour alone would keep pushing past the optimum; \
                     trust the measured outcomes here"
                        .into(),
                );
            }
            (false, Outcome::Worsened(d)) => {
                r.evidence.push(format!(
                    "last change (\"{note}\") moved against this advice and lost \
                     {d:.2}s — stepping back this way (smaller step) is supported by \
                     both behaviour and history",
                ));
                r.confidence = Confidence::High;
            }
            (false, Outcome::Unclear(_)) | (_, Outcome::NotComparable) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn journal_parses_paths_notes_and_comments() {
        let entries = parse_journal(
            "# my tuning log\n\
             sessions/a.ftel | baseline\n\
             \n\
             sessions/b.ftel | front arb softer\n\
             sessions/c.ftel\n",
        );
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].note.as_deref(), Some("baseline"));
        assert_eq!(entries[1].note.as_deref(), Some("front arb softer"));
        assert!(entries[2].note.is_none());
    }

    #[test]
    fn change_notes_parse_family_and_direction() {
        assert_eq!(
            parse_change("front arb softer"),
            Some(Change { family: Family::FrontRoll, softer: true, magnitude: None })
        );
        assert_eq!(
            parse_change("Stiffened rear springs a bit"),
            Some(Change { family: Family::RearRoll, softer: false, magnitude: None })
        );
        assert_eq!(parse_change("baseline"), None);
        assert_eq!(parse_change("softer springs"), None, "front/rear ambiguous");
        assert_eq!(parse_change("front and rear arb softer"), None);
    }

    #[test]
    fn v2_notes_carry_signed_magnitudes() {
        let c = parse_change("front arb -2").unwrap();
        assert_eq!((c.family, c.softer, c.magnitude), (Family::FrontRoll, true, Some(-2.0)));
        let c = parse_change("rear springs +1.5").unwrap();
        assert_eq!((c.family, c.softer, c.magnitude), (Family::RearRoll, false, Some(1.5)));
        // Direction word wins and signs the unsigned number.
        let c = parse_change("softened front arb by 2").unwrap();
        assert_eq!((c.softer, c.magnitude), (true, Some(-2.0)));
        // Unsigned number with no direction word: still unparseable.
        assert_eq!(parse_change("front arb 2"), None);
        // v1 direction-only notes keep working, without magnitude.
        let c = parse_change("front arb softer").unwrap();
        assert_eq!(c.magnitude, None);
    }

    #[test]
    fn positions_accumulate_until_a_magnitudeless_step() {
        let changes: Vec<Option<Change>> = vec![
            None, // baseline
            parse_change("front arb -2"),
            parse_change("front arb -2"),
            parse_change("rear arb +1"),
            parse_change("front arb stiffer"), // direction only -> front unknown
        ];
        let pos = track_positions(&changes);
        assert_eq!(pos[0], (Some(0.0), Some(0.0)));
        assert_eq!(pos[2], (Some(-4.0), Some(0.0)));
        assert_eq!(pos[3], (Some(-4.0), Some(1.0)));
        assert_eq!(pos[4], (None, Some(1.0)), "front unknown after direction-only step");
    }

    #[test]
    fn append_seeds_baseline_only_on_empty_journal() {
        let lines = append_lines("# comments only\n", Some("sessions/a.ftel"), "sessions/b.ftel", "front arb -2");
        assert_eq!(lines, "sessions/a.ftel | baseline\nsessions/b.ftel | front arb -2\n");

        let lines = append_lines("sessions/a.ftel | baseline\n", Some("sessions/b.ftel"), "sessions/c.ftel", "note | with pipe");
        assert_eq!(lines, "sessions/c.ftel | note   with pipe\n");

        let lines = append_lines("", None, "sessions/b.ftel", "front arb -2");
        assert_eq!(lines, "sessions/b.ftel | front arb -2\n");
    }

    #[test]
    fn revert_half_is_computable_with_magnitude() {
        let mut recs = vec![balance_rec()];
        reconcile(
            &mut recs,
            Change { family: Family::FrontRoll, softer: true, magnitude: Some(-2.0) },
            Outcome::Worsened(0.3),
            "front arb -2",
        );
        assert!(recs[0].advice.contains("go +1.0 slider units from here"), "{}", recs[0].advice);
    }

    #[test]
    fn outcome_thresholds() {
        assert!(matches!(judge(-0.5), Outcome::Improved(_)));
        assert!(matches!(judge(0.3), Outcome::Worsened(_)));
        assert!(matches!(judge(0.05), Outcome::Unclear(_)));
    }

    fn balance_rec() -> Recommendation {
        Recommendation {
            area: "balance",
            advice: "reduce front roll stiffness".into(),
            evidence: vec!["understeer +0.3".into()],
            confidence: Confidence::High,
            implied: Some(Change { family: Family::FrontRoll, softer: true, magnitude: None }),
        }
    }

    #[test]
    fn worse_outcome_flips_advice_to_revert_half() {
        let mut recs = vec![balance_rec()];
        reconcile(
            &mut recs,
            Change { family: Family::FrontRoll, softer: true, magnitude: None },
            Outcome::Worsened(0.3),
            "front arb softer",
        );
        assert!(recs[0].advice.contains("revert about half"), "{}", recs[0].advice);
        assert_eq!(recs[0].confidence, Confidence::High);
    }

    #[test]
    fn improved_outcome_endorses_another_step() {
        let mut recs = vec![balance_rec()];
        reconcile(
            &mut recs,
            Change { family: Family::FrontRoll, softer: true, magnitude: None },
            Outcome::Improved(-1.29),
            "front arb softer",
        );
        assert!(recs[0].advice.contains("reduce front roll stiffness"));
        assert!(recs[0].evidence.iter().any(|e| e.contains("gained 1.29s")));
    }

    /// The convergence case from the real trajectory: advice says soften, the last
    /// step stiffened and GAINED — the optimum is bracketed, so hold.
    #[test]
    fn opposite_direction_gain_means_hold() {
        let mut recs = vec![balance_rec()];
        reconcile(
            &mut recs,
            Change { family: Family::FrontRoll, softer: false, magnitude: None },
            Outcome::Improved(-0.42),
            "front arb stiffer",
        );
        assert!(recs[0].advice.contains("hold this setting"), "{}", recs[0].advice);
        assert_eq!(recs[0].confidence, Confidence::Medium);
    }

    #[test]
    fn opposite_direction_loss_reinforces_advice() {
        let mut recs = vec![balance_rec()];
        reconcile(
            &mut recs,
            Change { family: Family::FrontRoll, softer: false, magnitude: None },
            Outcome::Worsened(0.5),
            "front arb stiffer",
        );
        assert!(recs[0].advice.contains("reduce front roll stiffness"));
        assert_eq!(recs[0].confidence, Confidence::High);
        assert!(recs[0].evidence.iter().any(|e| e.contains("moved against this advice and lost")));
    }

    #[test]
    fn unrelated_change_leaves_recs_untouched() {
        let mut recs = vec![balance_rec()];
        let before = recs[0].advice.clone();
        reconcile(
            &mut recs,
            Change { family: Family::RearRoll, softer: false, magnitude: None },
            Outcome::Worsened(0.3),
            "rear arb stiffer",
        );
        assert_eq!(recs[0].advice, before);
    }
}
