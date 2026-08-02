//! Composition proposer: untested combinations of phase-complementary
//! measured improvements.

use super::*;

/// Corner-phase gain (seconds) a measurement must show in its dominant
/// phase before it can anchor a composition proposal; matches the scale
/// the phase attribution resolves reliably.
const COMPOSE_PHASE_MIN_S: f32 = 0.10;

/// Composition proposer: when two measured single-family improvements
/// concentrated their gains in OPPOSITE corner phases (one entry, one
/// exit) and the current setup still sits at BOTH measurements'
/// from-states, the pairing is an untested experiment with a linear-sum
/// prediction. The applicability guard is strict on purpose: a
/// measurement only transfers to a setup that matches where it started,
/// and reverses are not proposed here (returning to a measured state is
/// the history-revert path's job). Low confidence — interaction between
/// the two changes is exactly what one stint on the combination would
/// measure.
pub(crate) fn composition_proposal(
    latest: &[&Measurement],
    setups: &[Option<&crate::advice::tuning::Revision>],
    facts: &std::collections::BTreeMap<String, String>,
) -> Option<recommend::Recommendation> {
    let current = setups.last().copied().flatten()?;
    let val = |rev: &crate::advice::tuning::Revision, k: &str| -> Option<f32> {
        rev.values.get(k)?.trim().parse::<f32>().ok()
    };
    let near = |a: f32, b: f32| (a - b).abs() <= 1e-3 * a.abs().max(b.abs()).max(1.0);

    struct Cand<'m> {
        m: &'m Measurement,
        key: &'m str,
        to: f32,
        to_raw: String,
        d: f32,
        entry: f32,
        exit: f32,
    }
    let mut cands: Vec<Cand> = Vec::new();
    for m in latest {
        // Single-family, trusted, slider-resolvable improvements only: an
        // attributed compound clause or a weak pair must not seed an
        // experiment ask.
        if m.weak || m.attributed.is_some() {
            continue;
        }
        let Some(key) = m.key.as_deref() else {
            continue;
        };
        let Some((entry, exit, _)) = m.split else {
            continue;
        };
        let journal::Outcome::Improved(d) = m.outcome else {
            continue;
        };
        let (Some(from_rev), Some(to_rev)) = (
            setups.get(m.i).copied().flatten(),
            setups.get(m.j).copied().flatten(),
        ) else {
            continue;
        };
        let (Some(from), Some(to)) = (val(from_rev, key), val(to_rev, key)) else {
            continue;
        };
        if near(from, to) {
            continue;
        }
        // Re-apply guard: the gain is only transferable while the slider
        // still sits where the measurement started.
        if !val(current, key).is_some_and(|cur| near(cur, from)) {
            continue;
        }
        let to_raw = to_rev
            .values
            .get(key)
            .cloned()
            .unwrap_or_else(|| to.to_string());
        cands.push(Cand {
            m,
            key,
            to,
            to_raw,
            d,
            entry,
            exit,
        });
    }

    let mut best: Option<(&Cand, &Cand)> = None;
    for a in &cands {
        for b in &cands {
            if a.m.change.family == b.m.change.family || a.key == b.key {
                continue;
            }
            let entry_led = a.entry <= -COMPOSE_PHASE_MIN_S && a.entry < a.exit;
            let exit_led = b.exit <= -COMPOSE_PHASE_MIN_S && b.exit < b.entry;
            if !entry_led || !exit_led {
                continue;
            }
            // Untested: no bound setup ever held both to-values at once.
            let tested = setups.iter().copied().flatten().any(|rev| {
                val(rev, a.key).is_some_and(|v| near(v, a.to))
                    && val(rev, b.key).is_some_and(|v| near(v, b.to))
            });
            if tested {
                continue;
            }
            if best.is_none_or(|(pa, pb)| a.d + b.d < pa.d + pb.d) {
                best = Some((a, b));
            }
        }
    }
    let (a, b) = best?;
    let label = |k: &str| -> String {
        crate::advice::tuning::FIELDS
            .iter()
            .find(|(fk, _)| *fk == k)
            .map(|(_, l)| (*l).to_string())
            .unwrap_or_else(|| k.to_string())
    };
    let disp = |k: &str, v: &str| crate::advice::tuning::display_value(k, v, facts);
    let dnote = |n: &str| crate::advice::tuning::display_note(n, facts);
    Some(recommend::Recommendation {
        area: "experiment",
        suggestion: Some(format!(
            "{} {} + {} {}",
            label(a.key),
            disp(a.key, &a.to_raw),
            label(b.key),
            disp(b.key, &b.to_raw)
        )),
        apply: vec![
            (a.key.to_string(), a.to_raw.clone()),
            (b.key.to_string(), b.to_raw.clone()),
        ],
        advice: format!(
            "combine \"{}\" and \"{}\". Measured separately, one gained corner \
             entry and the other corner exit. Untested together.",
            dnote(&a.m.desc),
            dnote(&b.m.desc)
        ),
        evidence: vec![
            format!(
                "\"{}\" improved {:+.2}s (entry {:+.2}s / exit {:+.2}s)",
                dnote(&a.m.desc),
                a.d,
                a.entry,
                a.exit
            ),
            format!(
                "\"{}\" improved {:+.2}s (entry {:+.2}s / exit {:+.2}s)",
                dnote(&b.m.desc),
                b.d,
                b.entry,
                b.exit
            ),
            format!(
                "linear sum predicts {:+.2}s; the interaction between them is \
                 exactly what one stint on the combination would measure",
                a.d + b.d
            ),
        ],
        confidence: recommend::Confidence::Low,
        probe: false,
        implied: None,
    })
}
