//! Tuning journal: history-aware advice without knowing absolute setup values.
//! Blind mode stays blind to numbers — hill-climbing only needs the *direction* of
//! each past change plus the measured outcome (docs/plans/005-journal.md).

use super::recommend::{Confidence, Recommendation};

/// Parameter families the change-note parser understands (grow as exercised).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    FrontRoll,
    RearRoll,
    /// Final drive and per-gear ratios. Direction semantics: `softer` = the
    /// value decreased = LONGER gearing (higher final drive number = shorter).
    Gearing,
    /// Downforce per end. `softer` = LESS downforce.
    FrontAero,
    RearAero,
    /// Differential acceleration lock, either end (the drive axle is implicit
    /// in the car). `softer` = LESS lock.
    DiffAccel,
    /// Deceleration lock. `softer` = LESS lock. Behaviourally invisible per
    /// the 2026-07-21 A/B (driver masks it) — outcome-led advice only.
    DiffDecel,
    /// Brake balance / pressure. `softer` = the value decreased (balance:
    /// more rearward).
    Brakes,
    /// Rebound / bump, either end. `softer` = less damping.
    Damping,
}

/// The Family a setup-state area (tuning::field_area) reconciles under.
pub fn family_for_area(area: &str) -> Option<Family> {
    Some(match area {
        "front roll" => Family::FrontRoll,
        "rear roll" => Family::RearRoll,
        "gearing" => Family::Gearing,
        "front aero" => Family::FrontAero,
        "rear aero" => Family::RearAero,
        "diff accel" => Family::DiffAccel,
        "diff decel" => Family::DiffDecel,
        "brakes" => Family::Brakes,
        "damping" => Family::Damping,
        _ => return None,
    })
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
    if note.contains(';') {
        // Compound note (multiple changes in one step): the outcome cannot be
        // attributed to any single family. Positions still accumulate via
        // parse_clauses.
        return None;
    }
    parse_clause(note)
}

/// All parseable clauses of a (possibly compound) note — "front arb -1;
/// final drive +0.28" yields the arb change even though attribution refuses it.
pub fn parse_clauses(note: &str) -> Vec<Change> {
    note.split(';').filter_map(|c| parse_clause(c.trim())).collect()
}

fn parse_clause(note: &str) -> Option<Change> {
    let t = note.to_lowercase();
    let number = |t: &str| {
        t.split_whitespace()
            .map(|tok| tok.trim_end_matches([',', ')', ':']))
            .find_map(|tok| tok.parse::<f32>().ok().map(|v| (v, tok.starts_with(['+', '-']))))
    };
    if t.contains("final drive") || t.contains("gear") {
        let num = number(&t);
        let softer = if t.contains("short") {
            false // shorter gearing = higher number
        } else if t.contains("long") {
            true
        } else if let Some((v, true)) = num {
            v < 0.0
        } else {
            return None;
        };
        let magnitude = num.map(|(v, _)| if softer { -v.abs() } else { v.abs() });
        return Some(Change { family: Family::Gearing, softer, magnitude });
    }
    // "softer" here = less: less lock / less downforce.
    let less_more = |t: &str| {
        if ["less", "reduc", "lower", "remov"].iter().any(|k| t.contains(k)) {
            Some(true)
        } else if ["more", "add", "increas", "rais"].iter().any(|k| t.contains(k)) {
            Some(false)
        } else if let Some((v, true)) = number(t) {
            Some(v < 0.0)
        } else {
            None
        }
    };
    if t.contains("diff") && (t.contains("accel") || t.contains("decel")) {
        let family = if t.contains("decel") { Family::DiffDecel } else { Family::DiffAccel };
        let softer = less_more(&t)?;
        let magnitude = number(&t).map(|(v, _)| if softer { -v.abs() } else { v.abs() });
        return Some(Change { family, softer, magnitude });
    }
    if t.contains("brake") {
        let softer = less_more(&t)?;
        let magnitude = number(&t).map(|(v, _)| if softer { -v.abs() } else { v.abs() });
        return Some(Change { family: Family::Brakes, softer, magnitude });
    }
    if ["rebound", "bump", "damping"].iter().any(|k| t.contains(k)) {
        let softer = less_more(&t)?;
        let magnitude = number(&t).map(|(v, _)| if softer { -v.abs() } else { v.abs() });
        return Some(Change { family: Family::Damping, softer, magnitude });
    }
    let front = t.contains("front");
    let rear = t.contains("rear");
    if ["aero", "wing", "downforce"].iter().any(|k| t.contains(k)) {
        if front == rear {
            return None;
        }
        let softer = less_more(&t)?;
        let magnitude = number(&t).map(|(v, _)| if softer { -v.abs() } else { v.abs() });
        return Some(Change {
            family: if front { Family::FrontAero } else { Family::RearAero },
            softer,
            magnitude,
        });
    }
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
/// Each entry may carry several clauses (compound steps still move sliders);
/// a clause without a magnitude makes that family's position unknown from
/// there on — direction alone can't say where you ended up.
pub fn track_positions(changes: &[Vec<Change>]) -> Vec<(Option<f32>, Option<f32>)> {
    let (mut front, mut rear) = (Some(0.0f32), Some(0.0f32));
    changes
        .iter()
        .map(|clauses| {
            for c in clauses {
                let slot = match c.family {
                    Family::FrontRoll => &mut front,
                    Family::RearRoll => &mut rear,
                    _ => continue, // not a tracked slider position
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
/// `attributed`: evidence line when the outcome was channel-attributed out of
/// a compound step (corner/straight split) rather than measured directly — it
/// is attached to every touched recommendation and caps confidence at Medium.
pub fn reconcile(
    recs: &mut [Recommendation],
    change: Change,
    outcome: Outcome,
    note: &str,
    attributed: Option<&str>,
    weak: bool,
) -> bool {
    let mut matched = false;
    for r in recs.iter_mut() {
        let Some(implied) = r.implied else { continue };
        if implied.family != change.family {
            continue;
        }
        matched = true;
        // A single-flying-lap comparison has no corroboration: leave the
        // behavioural advice untouched rather than inflate or flip it on an
        // untrustworthy outcome.
        if weak {
            r.evidence.push(format!(
                "last step (\"{note}\") measured {} but against a single flying \
                 lap — outcome not trusted; drive more laps to corroborate",
                outcome_word(outcome),
            ));
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
                // The advice now points the other way; implied must follow it
                // (limit checks and future reconciliation read the direction),
                // and carries the half-revert delta so a concrete target value
                // can be resolved when the setup is on file.
                r.implied = Some(Change {
                    softer: !implied.softer,
                    magnitude: change.magnitude.map(|m| -m / 2.0),
                    ..implied
                });
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
                // "Hold" advises no direction — clear implied so nothing
                // downstream pushes either way.
                r.implied = None;
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
        if let Some(ev) = attributed {
            r.evidence.push(ev.to_string());
            if r.confidence == Confidence::High {
                r.confidence = Confidence::Medium;
            }
        }
    }
    matched
}

/// Does `b` exactly undo `a`? Same clause families, each with negated
/// magnitude (or just flipped direction when magnitudes are unknown). An
/// excursion-and-revert pair (A-B-A) is special: comparing around it cancels
/// driver/track drift, which cross-stint ideal deltas are otherwise soaked in.
pub fn is_reverse(a: &[Change], b: &[Change]) -> bool {
    if a.is_empty() || a.len() != b.len() {
        return false;
    }
    let mut used = vec![false; b.len()];
    a.iter().all(|ca| {
        b.iter().enumerate().any(|(i, cb)| {
            if used[i] || ca.family != cb.family {
                return false;
            }
            let undone = match (ca.magnitude, cb.magnitude) {
                (Some(ma), Some(mb)) => (ma + mb).abs() < 1e-3,
                (None, None) => ca.softer != cb.softer,
                _ => false,
            };
            if undone {
                used[i] = true;
            }
            undone
        })
    })
}

fn outcome_word(o: Outcome) -> &'static str {
    match o {
        Outcome::Improved(_) => "improved",
        Outcome::Worsened(_) => "worse",
        _ => "inconclusive",
    }
}

pub fn family_area(f: Family) -> &'static str {
    match f {
        Family::FrontRoll | Family::RearRoll => "balance",
        Family::Gearing => "gearing",
        Family::FrontAero | Family::RearAero => "aero",
        Family::DiffAccel | Family::DiffDecel => "differential",
        Family::Brakes => "brakes",
        Family::Damping => "damping",
    }
}

/// History-only advice for a step that measurably LOST time on a family no
/// behavioural rule currently speaks for. Behaviour rules only see problems
/// the driver can't mask (a locked diff reads near-neutral in the throttle
/// bands because the driver adapts) — the outcome measurement catches what
/// behaviour hides, and a loss with no behavioural case for the change means
/// revert it fully, not halfway (there is no signal placing the optimum
/// in between).
pub fn history_revert(
    change: Change,
    outcome: Outcome,
    note: &str,
    attributed: Option<&str>,
    weak: bool,
) -> Option<Recommendation> {
    let Outcome::Worsened(d) = outcome else { return None };
    if weak {
        // No corroboration: asking for data is the advice, not a revert.
        let mut evidence =
            vec![format!("provisional: {d:.2}s slower ideal, measured against a single flying lap")];
        evidence.extend(attributed.map(String::from));
        return Some(Recommendation {
            area: family_area(change.family),
            advice: format!(
                "re-run this setup for more laps before reacting: the last change \
                 (\"{note}\") measured worse, but a single-flying-lap comparison \
                 is not trustworthy"
            ),
            evidence,
            confidence: Confidence::Low,
            implied: None,
        });
    }
    let mut evidence = vec![format!(
        "that step lost {d:.2}s of ideal lap, and the car's behaviour shows no \
         case for keeping it"
    )];
    let mut confidence = Confidence::High;
    if let Some(ev) = attributed {
        evidence.push(ev.to_string());
        confidence = Confidence::Medium;
    }
    Some(Recommendation {
        area: family_area(change.family),
        advice: format!("revert the last change (\"{note}\"): it measurably cost lap time"),
        evidence,
        confidence,
        implied: Some(Change {
            family: change.family,
            softer: !change.softer,
            magnitude: change.magnitude.map(|m| -m),
        }),
    })
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
    fn aero_and_diff_notes_parse() {
        let c = parse_change("front aero +20").unwrap();
        assert_eq!((c.family, c.softer, c.magnitude), (Family::FrontAero, false, Some(20.0)));
        let c = parse_change("reduced rear wing").unwrap();
        assert_eq!((c.family, c.softer, c.magnitude), (Family::RearAero, true, None));
        let c = parse_change("rear diff accel -15").unwrap();
        assert_eq!((c.family, c.softer, c.magnitude), (Family::DiffAccel, true, Some(-15.0)));
        let c = parse_change("more diff accel lock").unwrap();
        assert_eq!((c.family, c.softer), (Family::DiffAccel, false));
        assert_eq!(parse_change("aero changes"), None, "no direction, no end");
        // Aero/diff clauses don't move the roll-position tracker.
        let pos = track_positions(&[parse_clauses("front aero +20; rear diff accel -15")]);
        assert_eq!(pos[0], (Some(0.0), Some(0.0)));
    }

    #[test]
    fn positions_accumulate_until_a_magnitudeless_step() {
        let changes: Vec<Vec<Change>> = vec![
            vec![], // baseline
            parse_clauses("front arb -2"),
            parse_clauses("front arb -2"),
            parse_clauses("rear arb +1"),
            parse_clauses("front arb stiffer"), // direction only -> front unknown
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
            None,
            false,
        );
        assert!(recs[0].advice.contains("go +1.0 slider units from here"), "{}", recs[0].advice);
        // The implied change follows the rewritten advice: flipped direction,
        // half the measured magnitude — resolvable to a concrete target value.
        let implied = recs[0].implied.unwrap();
        assert!(!implied.softer);
        assert_eq!(implied.magnitude, Some(1.0));
    }

    /// A compound step's outcome is unattributable, but its clauses still
    /// move the position tracker.
    #[test]
    fn compound_notes_attribute_nothing_but_track_positions() {
        let note = "front arb -1; final drive +0.28";
        assert_eq!(parse_change(note), None, "no single-family attribution");
        let clauses = parse_clauses(note);
        assert_eq!(clauses.len(), 2, "arb and gearing clauses both parse");
        assert_eq!(clauses[0].magnitude, Some(-1.0));
        assert_eq!(clauses[1].family, Family::Gearing);
        assert_eq!(clauses[1].magnitude, Some(0.28));
        assert!(!clauses[1].softer, "positive final drive delta = shorter gearing");

        let pos = track_positions(&[vec![], parse_clauses(note)]);
        assert_eq!(pos[1], (Some(-1.0), Some(0.0)));
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
            None,
            false,
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
            None,
            false,
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
            None,
            false,
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
            None,
            false,
        );
        assert!(recs[0].advice.contains("reduce front roll stiffness"));
        assert_eq!(recs[0].confidence, Confidence::High);
        assert!(recs[0].evidence.iter().any(|e| e.contains("moved against this advice and lost")));
    }

    /// The McLaren diff A/B: max diff lock measured WORSE but the driver adapted
    /// so no behavioural rec carries DiffAccel — history alone must say revert,
    /// fully (no signal places the optimum in between), with flipped direction
    /// so a follow-up step reconciles against it.
    #[test]
    fn worsened_step_with_no_matching_rec_gets_history_revert() {
        let mut recs = vec![balance_rec()];
        let change = Change { family: Family::DiffAccel, softer: false, magnitude: Some(71.0) };
        let outcome = Outcome::Worsened(0.25);
        assert!(!reconcile(&mut recs, change, outcome, "front diff accel +71", Some("attr"), false));
        let rec = history_revert(change, outcome, "front diff accel +71", Some("attr"), false).unwrap();
        assert_eq!(rec.area, "differential");
        assert!(rec.advice.contains("revert the last change"), "{}", rec.advice);
        assert_eq!(rec.confidence, Confidence::Medium, "attributed caps at Medium");
        let implied = rec.implied.unwrap();
        assert!(implied.softer, "revert of a stiffer step points softer");
        assert_eq!(implied.magnitude, Some(-71.0));

        // Improved or unclear steps get no history-only rec.
        assert!(history_revert(change, Outcome::Improved(-0.3), "n", None, false).is_none());
        assert!(history_revert(change, Outcome::Unclear(0.05), "n", None, false).is_none());
        // Direct (non-attributed) measurement keeps High confidence.
        let direct = history_revert(change, outcome, "n", None, false).unwrap();
        assert_eq!(direct.confidence, Confidence::High);
    }

    /// The single-lap revert stint: a "worse" outcome with no corroboration
    /// must NOT produce revert advice (it told the user to re-lock the diffs);
    /// it asks for more laps instead, and reconcile leaves matched behavioural
    /// recs untouched apart from a hedge line.
    #[test]
    fn single_lap_outcome_asks_for_data_not_reverts() {
        let change = Change { family: Family::DiffAccel, softer: true, magnitude: Some(-71.0) };
        let rec = history_revert(change, Outcome::Worsened(0.69), "front diff accel -71", None, true)
            .unwrap();
        assert!(rec.advice.contains("re-run this setup"), "{}", rec.advice);
        assert_eq!(rec.confidence, Confidence::Low);
        assert!(rec.implied.is_none(), "no directional implication from weak data");

        let mut recs = vec![balance_rec()];
        let before = (recs[0].advice.clone(), recs[0].confidence);
        reconcile(
            &mut recs,
            Change { family: Family::FrontRoll, softer: true, magnitude: None },
            Outcome::Worsened(0.5),
            "front arb softer",
            None,
            true,
        );
        assert_eq!((recs[0].advice.clone(), recs[0].confidence), before, "advice untouched");
        assert!(
            recs[0].evidence.iter().any(|e| e.contains("single flying lap")),
            "{:?}",
            recs[0].evidence
        );
    }

    /// The McLaren diff A-B-A: "front diff accel +71; rear diff accel +40"
    /// followed by "front diff accel -71; rear diff accel -40" is a reverse
    /// pair — same-family clauses must pair off by magnitude, not just family.
    #[test]
    fn reverse_pairs_detected_by_magnitude_and_direction() {
        let a = parse_clauses("front diff accel +71; rear diff accel +40");
        let b = parse_clauses("front diff accel -71; rear diff accel -40");
        assert!(is_reverse(&a, &b));
        let partial = parse_clauses("front diff accel -71; rear diff accel -20");
        assert!(!is_reverse(&a, &partial), "different magnitude is not a revert");
        let short = parse_clauses("front diff accel -71");
        assert!(!is_reverse(&a, &short), "missing clause is not a revert");

        // Direction-only notes revert by flipped direction.
        let a = parse_clauses("front arb softer");
        let b = parse_clauses("front arb stiffer");
        assert!(is_reverse(&a, &b));
        assert!(!is_reverse(&a, &a.clone()));
        assert!(!is_reverse(&[], &[]), "empty steps are not an A-B-A");
    }

    #[test]
    fn reconcile_reports_whether_any_rec_matched() {
        let mut recs = vec![balance_rec()];
        let matched = reconcile(
            &mut recs,
            Change { family: Family::FrontRoll, softer: true, magnitude: None },
            Outcome::Improved(-0.2),
            "front arb softer",
            None,
            false,
        );
        assert!(matched);
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
            None,
            false,
        );
        assert_eq!(recs[0].advice, before);
    }
}
