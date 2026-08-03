//! Response-curve fitting over a slider's tried values: quadratic
//! landscapes, vertex/probe suggestions.

/// Least-squares quadratic fit y = ax² + bx + c over the points, solved via
/// normal equations. None when degenerate (needs 3+ distinct x).
pub(super) fn quad_fit(pts: &[(f32, f32)]) -> Option<(f64, f64, f64)> {
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
/// that case), or the slider's range allows no new point. `step` is the
/// slider's granularity (tuning::slider_step): probes land on real slider
/// positions, so whole-unit sliders (diff lock) never get fractional asks.
pub(super) fn probe_value(
    nodes: &[(f32, f32, usize)],
    lim: Option<(f32, f32)>,
    step: f32,
) -> Option<f32> {
    // Snap through the reciprocal: dividing the rounded multiple keeps
    // f32 exactness for tenths ((x/0.1).round()*0.1 drifts, 4.2000003).
    let inv = 1.0 / step;
    let snap = move |x: f32| (x * inv).round() / inv;
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
    // slider unit outward instead.
    if (snap(v) - snap(best.0)).abs() < step * 0.5 {
        v = best.0 + dir * step;
    }
    if let Some((mn, mx)) = lim {
        v = v.clamp(mn, mx);
    }
    let v = snap(v);
    // Compare at slider granularity: clamping to a slider bound must not
    // fabricate a "new" point that rounds to the best tried value.
    ((v - snap(best.0)).abs() > step * 0.5).then_some(v)
}

/// The tune field a note clause is about, matched by field phrase (auto-
/// generated notes use these phrases verbatim). Longest match wins so
/// "front tire pressure" is not mistaken for a shorter overlapping phrase.
pub(super) fn key_from_phrase(text: &str) -> Option<String> {
    let t = text.to_lowercase();
    crate::advice::tuning::FIELDS
        .iter()
        .filter(|(_, phrase)| t.contains(phrase))
        .max_by_key(|(_, phrase)| phrase.len())
        .map(|(k, _)| k.to_string())
}
