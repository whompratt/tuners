//! Grip-curve fitting and saturation occupancy (understeer diagnosis).
//!
//! Pools cornering samples into slip-angle vs lateral-G curves, locates each
//! axle's grip plateau (adding slip past it adds no grip), then classifies a
//! stint's cornering time at the limit: PUSH (front saturated, rear with
//! spare — the physical definition of terminal understeer, resistant to
//! driver adaptation) vs SLIDE (both saturated — drifts/oversteer moments,
//! not understeer evidence).
//!
//! The curve is a car property and per-stint fits are unstable, so fits
//! must pool: per campaign (advise), falling back to the car's other
//! recordings, or self-fit from one recording (analyze, labeled as such).
//!
//! Downforce makes the grip ceiling speed-dependent (G scales with speed²),
//! which corrupts a pooled fit on big-aero cars. Handling is self-detecting:
//! curves are also fitted per speed band (boundary `HIGH_SPEED_MPS`); when
//! the bands' grip ceilings agree the car is grip-mechanical and the pooled
//! fit stands, when the high band's peak G is materially higher each sample
//! is judged against its own band's curve. Samples in a band without a
//! supported fit are withheld from classification rather than judged
//! against a curve known to be wrong.

use super::TimedFrame;
use super::metrics::{CORNERING_LAT_ACCEL, HIGH_SPEED_MPS};

/// Slip-angle bin width for the curve fit (game-normalized units, ~1.0 =
/// nominal limit; real fitted plateaus sit at 1.0-1.45 on tarmac).
const BIN_W: f32 = 0.05;
/// Curve range cap: slip beyond this is lumped into the last bin (deep
/// slides carry no curve information).
const ALPHA_MAX: f32 = 2.0;
/// A bin participates in peak/onset-finding only with at least this many
/// samples. Absolute by design: a proportional gate was tried 2026-07-31 and
/// rejected — it re-supported different bins on the 200k+ tester pools and
/// moved every calibrated number. The consequence is that POOL SIZE matters:
/// the validated corpus pools ran ~20k-300k samples, and ~13k pools misread
/// (a healthy 3.7%-push stint as 19.6% — low-slip rear bins lose support and
/// the onset slides up). Pool builders must aim well above FIT_MIN.
const BIN_MIN: usize = 200;
/// Minimum samples for a fit worth trusting (per pool or per speed band).
pub const FIT_MIN: usize = 5000;
/// Minimum band samples for the aero DETECTION test alone: the peak-G
/// estimate is far more stable than an onset, so the test may run on a band
/// too thin to classify against (whose frames are then withheld — a
/// single-recording self-fit of a big-aero car must not silently fall back
/// to the pooled artifact).
const DETECT_MIN: usize = 2000;
/// Saturation onset: first supported bin reaching this share of the smoothed
/// peak mean G (the plateau start; the argmax alone overshoots into the tail).
const PLATEAU_FRAC: f32 = 0.97;
/// High-band front peak G above this multiple of the low band's reads as
/// aero-significant: downforce raises the grip CEILING with speed,
/// mechanical grip does not. Calibrated on the library (2026-07-31): no-aero
/// builds 0.99-1.06, moderate aero 1.10-1.34 (pooled fits proven sound on
/// all of them in phase 1), the Datsun time-attack artifact car 1.54. Onset
/// comparison was tried and rejected: rear onsets are noise-prone and front
/// onsets peg at the range cap on never-saturating cars.
const AERO_PEAK_RATIO: f32 = 1.4;
/// Minimum cornering samples before a stint's occupancy is reported at all.
const OCC_MIN: usize = 200;
/// Pooling target for campaign fits: below this, advise pulls the car's
/// other recordings in (biased across setups, better than an unstable fit).
pub const POOL_TARGET: usize = 2 * FIT_MIN;
/// Sibling-sample target when building a car pool around one recording:
/// the absolute BIN_MIN makes onset estimates size-sensitive, and pools
/// under ~20k misread, so pull siblings until the pool is comfortably in
/// the validated 20k-300k regime (or the era runs out of recordings).
pub const CAR_POOL_SIBLINGS: usize = 50_000;

/// One cornering frame's grip observation.
#[derive(Debug, Clone, Copy)]
pub struct GripSample {
    /// Axle-mean |slip angle|, front.
    pub front: f32,
    /// Axle-mean |slip angle|, rear.
    pub rear: f32,
    /// |lateral acceleration| (m/s²).
    pub lat_g: f32,
    /// Speed (m/s), for band splitting.
    pub speed: f32,
}

/// Cornering samples of a frame span (same gate as the balance metrics).
pub fn cornering_samples(frames: &[TimedFrame]) -> Vec<GripSample> {
    frames
        .iter()
        .filter_map(|tf| {
            let f = &tf.frame;
            let lat = f.acceleration[0];
            (lat.abs() > CORNERING_LAT_ACCEL).then(|| GripSample {
                front: (f.tire_slip_angle.fl.abs() + f.tire_slip_angle.fr.abs()) / 2.0,
                rear: (f.tire_slip_angle.rl.abs() + f.tire_slip_angle.rr.abs()) / 2.0,
                lat_g: lat.abs(),
                speed: f.speed,
            })
        })
        .collect()
}

/// One axle's fitted grip curve.
#[derive(Debug, Clone, Copy)]
pub struct AxleCurve {
    /// Plateau onset: slip angle where mean G first reaches PLATEAU_FRAC of
    /// the peak. Time beyond this adds slip without adding grip.
    pub onset: f32,
    pub peak_g: f32,
    pub n: usize,
}

/// Per-bin (count, mean G) over samples: the raw material of a fit, exposed
/// for calibration dumps.
pub fn bin_means(samples: impl Iterator<Item = (f32, f32)>) -> Vec<(usize, f32)> {
    let n_bins = (ALPHA_MAX / BIN_W) as usize;
    let mut sums = vec![(0usize, 0.0f64); n_bins];
    for (alpha, g) in samples {
        let i = ((alpha / BIN_W) as usize).min(n_bins - 1);
        sums[i].0 += 1;
        sums[i].1 += g as f64;
    }
    sums.iter()
        .map(|&(n, s)| (n, if n > 0 { (s / n as f64) as f32 } else { 0.0 }))
        .collect()
}

/// Bin center of index `i`.
pub fn bin_alpha(i: usize) -> f32 {
    (i as f32 + 0.5) * BIN_W
}

fn fit_axle(samples: impl Iterator<Item = (f32, f32)>, min: usize) -> Option<AxleCurve> {
    let bins = bin_means(samples);
    let n_total: usize = bins.iter().map(|(n, _)| n).sum();
    if n_total < min {
        return None;
    }
    // Sample-weighted 3-bin smoothing before plateau detection: a single
    // noisy bin must not set the peak.
    let smooth: Vec<(usize, f32)> = (0..bins.len())
        .map(|i| {
            let lo = i.saturating_sub(1);
            let hi = (i + 1).min(bins.len() - 1);
            let (mut n, mut s) = (0usize, 0.0f64);
            for &(bn, bg) in &bins[lo..=hi] {
                n += bn;
                s += bn as f64 * bg as f64;
            }
            (n, if n > 0 { (s / n as f64) as f32 } else { 0.0 })
        })
        .collect();
    let g_peak = smooth
        .iter()
        .filter(|(n, _)| *n >= BIN_MIN)
        .map(|(_, g)| *g)
        .max_by(f32::total_cmp)?;
    let onset_i = smooth
        .iter()
        .position(|(n, g)| *n >= BIN_MIN && *g >= PLATEAU_FRAC * g_peak)?;
    Some(AxleCurve {
        onset: bin_alpha(onset_i),
        peak_g: g_peak,
        n: n_total,
    })
}

/// Both axles' curves from one sample pool. Classification needs both ends:
/// an axle that won't fit sinks the pair.
#[derive(Debug, Clone, Copy)]
pub struct AxlePair {
    pub front: AxleCurve,
    pub rear: AxleCurve,
}

fn fit_pair<'a>(samples: impl Iterator<Item = &'a GripSample> + Clone) -> Option<AxlePair> {
    Some(AxlePair {
        front: fit_axle(samples.clone().map(|s| (s.front, s.lat_g)), FIT_MIN)?,
        rear: fit_axle(samples.map(|s| (s.rear, s.lat_g)), FIT_MIN)?,
    })
}

/// A car pool's fitted curves: pooled plus per speed band.
#[derive(Debug, Clone, Copy)]
pub struct CarCurves {
    pub pooled: AxlePair,
    /// Below / at-or-above `HIGH_SPEED_MPS`. None = band unsupported.
    pub low: Option<AxlePair>,
    pub high: Option<AxlePair>,
    /// The high band's grip ceiling sits well above the low band's:
    /// aero-significant car, judge each sample against its own band's
    /// curve. False = pooled fit stands.
    pub banded: bool,
}

pub fn fit_curves(samples: &[GripSample]) -> Option<CarCurves> {
    let pooled = fit_pair(samples.iter())?;
    let low = fit_pair(samples.iter().filter(|s| s.speed < HIGH_SPEED_MPS));
    let high = fit_pair(samples.iter().filter(|s| s.speed >= HIGH_SPEED_MPS));
    // Detection runs on front-only fits at the lower gate; without both
    // bands even there, a single supported band cannot testify about speed
    // dependence.
    let det = |below: bool| {
        fit_axle(
            samples
                .iter()
                .filter(|s| (s.speed < HIGH_SPEED_MPS) == below)
                .map(|s| (s.front, s.lat_g)),
            DETECT_MIN,
        )
    };
    let banded = match (det(true), det(false)) {
        (Some(l), Some(h)) => h.peak_g > AERO_PEAK_RATIO * l.peak_g,
        _ => false,
    };
    Some(CarCurves {
        pooled,
        low,
        high,
        banded,
    })
}

/// Where the curve a stint was judged against came from: the trust label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurveSource {
    /// Pooled over the campaign's own stints.
    Campaign,
    /// Campaign pool was thin; the car's other recordings joined it
    /// (crosses setups, slightly biased).
    CarPool,
    /// Fitted from a single recording (analyze without context): unstable,
    /// indicative only.
    SelfFit,
}

/// A stint's occupancy at the grip limit.
#[derive(Debug, Clone, Copy)]
pub struct GripSaturation {
    /// Share of classified cornering time with the front saturated and the
    /// rear inside its own limit: the understeer signal.
    pub push_frac: f32,
    /// Share with both axles saturated: drifts/oversteer, not understeer.
    pub slide_frac: f32,
    /// Mean rear share-of-onset while pushing (1.0 = no spare). The
    /// magnitude of the problem: how much rear grip goes unused.
    pub rear_use_at_push: Option<f32>,
    /// Share of cornering samples that could be classified (a diverged-band
    /// fit may not cover every speed band).
    pub coverage: f32,
    pub banded: bool,
    pub source: CurveSource,
}

pub fn occupancy(
    samples: &[GripSample],
    curves: &CarCurves,
    source: CurveSource,
) -> Option<GripSaturation> {
    if samples.len() < OCC_MIN {
        return None;
    }
    let mut classified = 0usize;
    let mut push = 0usize;
    let mut slide = 0usize;
    let mut rear_use_sum = 0.0f32;
    for s in samples {
        let pair = if curves.banded {
            match if s.speed >= HIGH_SPEED_MPS {
                &curves.high
            } else {
                &curves.low
            } {
                Some(p) => p,
                None => continue,
            }
        } else {
            &curves.pooled
        };
        classified += 1;
        if s.front < pair.front.onset {
            continue;
        }
        if s.rear < pair.rear.onset {
            push += 1;
            rear_use_sum += s.rear / pair.rear.onset;
        } else {
            slide += 1;
        }
    }
    if classified == 0 {
        return None;
    }
    Some(GripSaturation {
        push_frac: push as f32 / classified as f32,
        slide_frac: slide as f32 / classified as f32,
        rear_use_at_push: (push > 0).then(|| rear_use_sum / push as f32),
        coverage: classified as f32 / samples.len() as f32,
        banded: curves.banded,
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Synthetic pool: G rises linearly with slip to a plateau of `peak` at
    /// `onset`, `n` samples spread over 0..ALPHA_MAX, constant speed.
    fn pool_at(onset_f: f32, onset_r: f32, peak: f32, n: usize, speed: f32) -> Vec<GripSample> {
        (0..n)
            .map(|i| {
                let a = ALPHA_MAX * (i as f32 + 0.5) / n as f32;
                let g = |onset: f32| peak * (a / onset).min(1.0);
                GripSample {
                    front: a,
                    rear: a,
                    lat_g: g(onset_f).min(g(onset_r)),
                    speed,
                }
            })
            .collect()
    }

    fn pool(onset_f: f32, onset_r: f32, n: usize, speed: f32) -> Vec<GripSample> {
        pool_at(onset_f, onset_r, 6.0, n, speed)
    }

    #[test]
    fn fit_finds_plateau_onset() {
        let samples = pool(1.0, 1.0, 20_000, 20.0);
        let c = fit_curves(&samples).unwrap();
        assert!(
            (c.pooled.front.onset - 1.0).abs() < 0.1,
            "onset {}",
            c.pooled.front.onset
        );
        assert!(!c.banded);
    }

    #[test]
    fn fit_needs_samples() {
        let samples = pool(1.0, 1.0, 1_000, 20.0);
        assert!(fit_curves(&samples).is_none());
    }

    #[test]
    fn push_counts_front_only_saturation() {
        let samples = pool(1.0, 1.0, 20_000, 20.0);
        let curves = fit_curves(&samples).unwrap();
        // A stint living at front 1.5 (beyond onset), rear 0.6 (inside).
        let stint: Vec<GripSample> = (0..500)
            .map(|_| GripSample {
                front: 1.5,
                rear: 0.6,
                lat_g: 6.0,
                speed: 20.0,
            })
            .collect();
        let occ = occupancy(&stint, &curves, CurveSource::Campaign).unwrap();
        assert!(occ.push_frac > 0.99);
        assert_eq!(occ.slide_frac, 0.0);
        let ru = occ.rear_use_at_push.unwrap();
        assert!(ru > 0.5 && ru < 0.75, "rear use {ru}");
        assert!((occ.coverage - 1.0).abs() < 1e-6);
    }

    #[test]
    fn slide_counts_both_saturated() {
        let samples = pool(1.0, 1.0, 20_000, 20.0);
        let curves = fit_curves(&samples).unwrap();
        let stint: Vec<GripSample> = (0..500)
            .map(|_| GripSample {
                front: 1.6,
                rear: 1.4,
                lat_g: 6.0,
                speed: 20.0,
            })
            .collect();
        let occ = occupancy(&stint, &curves, CurveSource::SelfFit).unwrap();
        assert_eq!(occ.push_frac, 0.0);
        assert!(occ.slide_frac > 0.99);
        assert!(occ.rear_use_at_push.is_none());
    }

    #[test]
    fn diverging_bands_classify_per_band() {
        // Mechanical low band: peak 6 G at onset 0.8. Downforce high band:
        // ceiling raised to 10 G, onset out at 1.4.
        let mut samples = pool_at(0.8, 0.8, 6.0, 20_000, 20.0);
        samples.extend(pool_at(1.4, 1.4, 10.0, 20_000, 50.0));
        let curves = fit_curves(&samples).unwrap();
        assert!(curves.banded, "bands should diverge");
        // front 1.0 = saturated in the low band, inside in the high band.
        let mk = |speed: f32| -> Vec<GripSample> {
            (0..400)
                .map(|_| GripSample {
                    front: 1.0,
                    rear: 0.4,
                    lat_g: 6.0,
                    speed,
                })
                .collect()
        };
        let low = occupancy(&mk(20.0), &curves, CurveSource::Campaign).unwrap();
        let high = occupancy(&mk(50.0), &curves, CurveSource::Campaign).unwrap();
        assert!(low.push_frac > 0.99, "low-band push {}", low.push_frac);
        assert!(high.push_frac < 0.01, "high-band push {}", high.push_frac);
    }

    #[test]
    fn agreeing_bands_collapse_to_pooled() {
        let mut samples = pool(1.0, 1.0, 20_000, 20.0);
        samples.extend(pool(1.0, 1.0, 20_000, 50.0));
        let curves = fit_curves(&samples).unwrap();
        assert!(!curves.banded);
    }

    #[test]
    fn short_stint_withheld() {
        let samples = pool(1.0, 1.0, 20_000, 20.0);
        let curves = fit_curves(&samples).unwrap();
        let stint = pool(1.0, 1.0, 50, 20.0);
        assert!(occupancy(&stint, &curves, CurveSource::SelfFit).is_none());
    }
}
