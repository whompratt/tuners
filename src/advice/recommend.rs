//! Recommendation rules: observations -> directional tune advice with evidence.
//! Blind mode: tune values and slider limits are unknown, so advice is a direction
//! phrased to survive unknown limits.

use super::journal::{Change, Family};
use crate::analysis::grip::CurveSource;
use crate::analysis::metrics::StintMetrics;

/// Balance index magnitudes: mild tendency vs clear problem. Since plan 015
/// the index gates only dirt balance and tarmac net-OVERSTEER means: on
/// tarmac every normally-driven stint reads +0.11..+0.38 (the driver's
/// operating point), so positive means are context, not detection.
const BALANCE_MILD: f32 = 0.05;
const BALANCE_CLEAR: f32 = 0.10;
/// Front-saturation occupancy gates (plan 015): share of cornering time with
/// the front pinned at its fitted grip peak while the rear has spare.
/// Library 2026-07-31: healthy tarmac 0-5%, known pushers 7.5-30 with a
/// clean gap; severe (High confidence) covers the aero-cut exemplar (18.5)
/// and the tester Mustang (14.5-30.5).
const PUSH_PROBLEM: f32 = 0.075;
const PUSH_SEVERE: f32 = 0.15;
/// RWD off-throttle rear/front wheel-speed convergence at or under this
/// reads as decel lock visibly dragging entry (open decel diffs measure
/// ~1.0 against the free-rolling front; the max-decel A/B fell to 0.58).
const CONV_OFF_LOCKED: f32 = 0.8;
/// Entry−exit index gap before an imbalance counts as phase-concentrated and
/// the fix priorities change (forza.guide "Balance & Fix-It" cards). Corpus:
/// converged tarmac tunes read |gap| 0.01-0.06; dirt entry push 0.10-0.28;
/// the exit-pushy McLaren stints -0.10..-0.14.
const PHASE_GAP: f32 = 0.10;
/// Braking-band index confirming the front burns its grip budget under
/// braking (entry-understeer card: brake bias is a lever). Healthy tarmac
/// reads <= +0.25; tarmac-only, since dirt trail-brake rotation is technique and
/// reads +0.22..+0.67 even on reference tunes.
const BRAKE_PUSH: f32 = 0.30;
/// Braking-band index at or below this = the car rotates while braking:
/// bias too far rear. Healthy tarmac stints read +0.07..+0.30 (8 cars, 40+
/// stints; weight transfer leaves push under trail braking even on converged
/// tunes); every deliberate rear-bias state measured at or below +0.03
/// across three cars (GT-R 0% -0.06, McLaren 20% +0.01, doubted 570S +0.03,
/// FWD Integra -0.21 under the sample gate). Set below the deliberate-slide
/// provocation stint (+0.026), which must stay out.
const BRAKE_REAR_ROTATE: f32 = 0.02;
/// Transient-oversteer counter-signal gates (tarmac only; dirt reads 12-23%
/// on-power everywhere because rotation is technique). Corpus: healthy AWD
/// tarmac tunes read on-power <= 3.9% and rear-first <= 2.2%; the RWD cars
/// that FEEL snappy read 4.3-8.2% on-power / 1.8-4.2% rear-first.
const OS_ON_POWER_FRAC: f32 = 0.04;
const OS_REAR_FIRST_FRAC: f32 = 0.03;
/// Counter-steer share gate (tarmac): the driver's own corrections label
/// slides, so this is the strongest of the three. Corpus: AWD understeer
/// tunes 0.8-1.4%; the rear-limited RWD cars 4.7-5.3%; dirt 13%+ (technique,
/// excluded by the loose-surface gate like the rest).
const OS_COUNTERSTEER_FRAC: f32 = 0.03;
/// Grip-margin ratio (cornering front/rear share of limit) at or below this
/// = the rear works as hard as the front. Library: drivers settle at
/// 1.36-1.84 on tarmac (one quiet 1.26 stint with zero oversteer events);
/// both deliberate oversteer exemplars collapsed to 0.99/1.03. Corroborator
/// only — a low ratio without oversteer events is just a stiff-rear platform
/// choice (the rear-margin-surplus finding).
const MARGIN_COLLAPSE: f32 = 1.2;
/// Working tire temperature band (°F) per compound; outside it pressures
/// likely need adjusting. The slick band is the in-game-validated anchor
/// (160-210°F reads right on real FH6 sessions); the rest are offset from it
/// using real-world relative operating windows: street compounds run their
/// best grip cooler than race rubber, loose-surface compounds far cooler,
/// snow coldest. Heuristic: the game publishes no per-compound numbers, so
/// these are extrapolations, not measurements.
fn temp_band(compound: Option<&str>) -> (f32, f32, &'static str) {
    match compound.unwrap_or("") {
        "slick" => (160.0, 210.0, "slick"),
        "semi-slick" => (150.0, 200.0, "semi-slick"),
        "drag" => (150.0, 200.0, "drag"),
        "drift" => (140.0, 210.0, "drift"),
        "sport" => (140.0, 190.0, "sport"),
        "street" => (130.0, 180.0, "street"),
        "stock" => (120.0, 175.0, "stock"),
        "vintage" => (120.0, 175.0, "vintage"),
        "rally" => (110.0, 160.0, "rally"),
        "offroad" => (100.0, 150.0, "offroad"),
        "snow" => (60.0, 110.0, "snow"),
        // No compound on file (blind mode): the legacy band, flagged as such.
        _ => (160.0, 210.0, ""),
    }
}
/// °F beyond the band edge before the pressure rule speaks with medium confidence.
const TEMP_CLEAR_MARGIN_F: f32 = 20.0;
/// Wheelspin as a fraction of on-throttle time.
const WHEELSPIN_MED: f32 = 0.08;
const WHEELSPIN_HIGH: f32 = 0.15;
/// Rear spin-symmetry gates (tarmac, rear-driven; plan-008 R12 sweep). The
/// inside-only share reads the OPEN failure mode: 11.0% at 0% lock vs
/// 2.0-2.8% at 100/50 on the sweep car, healthy tarmac library tops out ~6%
/// (one 8.4% candidate, likely also too open). The both-rears share reads
/// the LOCKED mode: 3.2% at 100% vs 1.5% at 50% and 0.2% open; healthy
/// tarmac <= 1.5%. Dirt reads both-rears 17-35% everywhere (technique) and
/// never sees these gates.
const INSIDE_ONLY_SPIN: f32 = 0.08;
const BOTH_REAR_SPIN: f32 = 0.025;
/// Time on the rev limiter worth reacting to.
const LIMITER_FRAC: f32 = 0.02;
/// Minimum cornering samples in a conditioned band (speed / throttle) before
/// band rules speak (~8s of cornering at 60 Hz).
const BAND_MIN_SAMPLES: usize = 500;
/// High-speed-only imbalance (aero signature): the high band must read at
/// least this while the low band stays under BALANCE_MILD. Calibrated on the
/// real library: converged tarmac tunes show band gaps ~0.05 with BOTH bands
/// imbalanced; only a genuinely speed-dependent car separates like this.
const AERO_HIGH_INDEX: f32 = 0.10;
const AERO_BAND_GAP: f32 = 0.10;
/// On−off throttle index shift marking a power-on balance change. Healthy
/// tarmac reads -0.02..-0.09 on the real library.
const POWER_SHIFT: f32 = 0.10;
/// The on-throttle index itself must clear this (in the shift direction): the
/// power end genuinely works harder, not merely less pushy than entry.
const POWER_INDEX: f32 = 0.08;
/// Loose surfaces: throttle rotation is driving technique (dirt shifts read
/// -0.13..-0.62 on well-tuned rally cars with on-throttle index ~0), so the
/// on-throttle index gate is stricter before the diff rule speaks.
const POWER_INDEX_LOOSE: f32 = 0.12;
/// Gearing is judged as the drag-race tradeoff: too short = the engine climbs
/// to redline quickly and camps there (limiter dwell); too long = the car lives
/// in top gear without ever climbing into the top of the rev range. The optimum
/// between them is route-dependent, so only the extremes are flagged.
/// Top gear must carry at least this share of grounded time before the "too
/// long" side speaks: a route that barely reaches top gear says nothing about
/// the stack (converged Ford GT tune: 3-4.6%; the long-geared Ferrari: 17-50%).
const TOP_GEAR_TIME_FRAC: f32 = 0.10;
/// "Too long" fires when less than this share of top-gear time is spent above
/// 90% of redline. Healthy tunes that rev out read 5-11% on the library
/// (Fiesta GRC 8.7%, Audi 10.7%); long stacks read 0.0-0.3%.
const HIGH_REV_SHARE_MIN: f32 = 0.02;
/// Suspension travel fractions.
const BOTTOMING_FRAC: f32 = 0.03;
const TOPPING_FRAC: f32 = 0.10;
/// Damping calibration from deliberate min/max A/Bs. Baselines
/// are SURFACE-driven: healthy reads ~5.5-6 reversals/s on tarmac but 12-16 on
/// dirt, so thresholds are per-surface.
const UNDERDAMPED_REV_TARMAC: f32 = 7.0;
/// Raised from 0.05 after the McLaren F1 measured HEALTHY at 8.2 rev/s with
/// 4.1% topped; healthy reversal rates span 4.1-8.4/s across cars/tracks,
/// so the topped conjunct carries the discrimination.
const UNDERDAMPED_TOPPED_TARMAC: f32 = 0.08;
/// Loose-surface underdamping calibrated from a dirt min-damping capture:
/// healthy axle averages read 11.8-16.2 rev/s, min damping 17.6-19.5. Tight
/// separation (one car), hence Medium confidence when it fires.
const UNDERDAMPED_REV_LOOSE: f32 = 17.0;
const UNDERDAMPED_TOPPED_LOOSE: f32 = 0.15;
/// Overdamping, strong form (any surface): the wheel lives at full extension
/// instead of tracking the surface. Dirt max-damping read 66-84%; healthy dirt
/// 3-26%, healthy tarmac 1-7%.
const OVERDAMPED_TOPPED_FRAC: f32 = 0.40;
/// Overdamping, mild tarmac form: suspension barely articulates.
const OVERDAMPED_REV_TARMAC: f32 = 3.0;
/// Bump-heavy overdamping (measured on the bump-only-max A/B): articulation
/// suppressed AND the wheel parked at full extension a large share of the
/// time. Both conjuncts required: healthy low-rev cars (Acura 3.5-4.5/s,
/// topped <3%) must stay silent.
const OVERDAMPED_BUMP_REV: f32 = 5.5;
const OVERDAMPED_BUMP_TOPPED: f32 = 0.10;
/// Speed-robust form of the same gate: reversals are per BUMP, and bumps are
/// spatial, so per-100m rates hold across driving speed where the raw /s
/// gate is blind (a setup reading 4.8/s at 51 m/s reads ~6.1/s at 65 m/s —
/// past OVERDAMPED_BUMP_REV — but ~9.4 per 100m either way). Healthy tarmac
/// reads 11-16 per 100m across all ten library cars; the bump-max exemplar
/// 9.4; the rebound-only-max stint 11.5 (invisible HERE — the damper-phase
/// vratio gate below is that stint's own channel since plan 015 phase 3).
const OVERDAMPED_SPATIAL_TARMAC: f32 = 10.0;
/// Extension/compression mean-speed ratio at or under this = rebound
/// overdamped (tarmac; per axle). Healthy tarmac 0.76-0.84 across every
/// library car and both drivers; the maxed-rebound A/B read 0.59 front /
/// 0.69 rear against an otherwise-identical setup.
const REBOUND_VRATIO_LOW: f32 = 0.72;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Confidence {
    Low,
    Medium,
    High,
}

impl Confidence {
    pub fn label(self) -> &'static str {
        match self {
            Confidence::High => "high",
            Confidence::Medium => "medium",
            Confidence::Low => "low",
        }
    }
}

#[derive(Debug)]
pub struct Recommendation {
    pub area: &'static str,
    /// The concrete setting to try, shown as the headline when resolvable:
    /// "front arb: 17.5" (absolute, setup on file) or "front arb: +0.5"
    /// (delta, blind mode). Filled by the advise layer.
    pub suggestion: Option<String>,
    /// Machine-applyable form of the suggestion: canonical (key, value)
    /// pairs an accept would save. Empty when the suggestion is not a
    /// concrete absolute (blind deltas, holds, prose-only advice).
    pub apply: Vec<(String, String)>,
    pub advice: String,
    pub evidence: Vec<String>,
    pub confidence: Confidence,
    /// The journal-comparable direction this advice implies, when it maps to a
    /// parameter family the journal tracks. Lets history reconcile with advice.
    pub implied: Option<Change>,
}

/// What the car's setup tells us beyond telemetry: rules must not suggest
/// sliders the build doesn't have. None = unknown (blind mode); rules keep
/// their default phrasing.
#[derive(Debug, Clone, Copy, Default)]
pub struct Context<'a> {
    pub compound: Option<&'a str>,
    /// Whether aero is tunable on this build (absent aero fields in the tune
    /// = the upgrade isn't fitted).
    pub aero_tunable: Option<bool>,
}

/// `overall` covers the whole stint; `per_lap` holds metrics for each completed
/// flying lap (used to judge consistency). Sorted most-confident first.
pub fn recommend(
    overall: &StintMetrics,
    per_lap: &[StintMetrics],
    ctx: &Context,
) -> Vec<Recommendation> {
    let mut recs = Vec::new();

    let balance_sign = balance_rule(overall, per_lap, &mut recs);
    aero_rule(overall, ctx, &mut recs);
    power_balance_rule(overall, &mut recs);
    brake_rule(overall, &mut recs);
    stability_rule(overall, ctx, &mut recs);
    tire_pressure_rule(overall, balance_sign, ctx.compound, &mut recs);
    traction_rule(overall, &mut recs);
    gearing_rule(overall, &mut recs);
    suspension_rule(overall, &mut recs);
    damping_rule(overall, &mut recs);

    recs.sort_by_key(|r| std::cmp::Reverse(r.confidence));
    recs
}

/// Returns the balance direction sign (+1 understeer, -1 oversteer) when the rule
/// fired, so the tire rule can attribute axle heat to scrub.
///
/// Two-stage since plan 015. Tarmac UNDERSTEER detection is saturation-led:
/// share of cornering time with the front pinned at its fitted grip peak
/// while the rear has spare — the physical definition of terminal push, and
/// an adapted driver still operates pinned there, they just stop asking for
/// more. The averaged index is demoted to evidence context (it measures the
/// driver's operating point and clears the old gates on every tarmac stint
/// ever recorded). Net-OVERSTEER means stay index-gated (rare; the episodic
/// side belongs to stability_rule). Dirt keeps the legacy index gate whole:
/// grip curves are deferred there.
fn balance_rule(
    overall: &StintMetrics,
    per_lap: &[StintMetrics],
    recs: &mut Vec<Recommendation>,
) -> Option<f32> {
    let idx = overall.understeer_index;
    // Detection requires a POOLED curve: single-recording self-fits measured
    // push 0.1-19.6% on known-healthy stints (2026-07-31) — display-only.
    let sat = if overall.surface_loose {
        None
    } else {
        overall
            .grip_saturation
            .filter(|g| g.source != CurveSource::SelfFit && g.push_frac >= PUSH_PROBLEM)
    };
    let mean_fired = idx.is_some_and(|i| {
        if overall.surface_loose {
            i.abs() >= BALANCE_MILD
        } else {
            i <= -BALANCE_MILD
        }
    });
    let idx_v = idx.unwrap_or(0.0);
    let (understeer, dir) = if sat.is_some() {
        (true, 1.0f32)
    } else if mean_fired {
        (idx_v > 0.0, idx_v.signum())
    } else {
        return None;
    };
    let lap_indices: Vec<f32> = per_lap.iter().filter_map(|m| m.understeer_index).collect();
    let consistent = lap_indices.len() >= 2
        && lap_indices
            .iter()
            .all(|i| i.signum() == idx_v.signum() && i.abs() >= BALANCE_MILD);

    let confidence = if let Some(gs) = &sat {
        // Saturation-led: severity from occupancy (pooled sources only).
        if gs.push_frac >= PUSH_SEVERE {
            Confidence::High
        } else {
            Confidence::Medium
        }
    } else {
        match (idx_v.abs() >= BALANCE_CLEAR, consistent) {
            (true, true) => Confidence::High,
            (true, false) => Confidence::Medium,
            (false, _) => Confidence::Low,
        }
    };

    // Where in the corner the imbalance lives decides WHICH end/system to
    // touch (forza.guide fix-it cards): entry and mid-corner imbalance are
    // roll-stiffness problems, but exit imbalance under power is a driveline/
    // load-transfer problem: softening the loaded end would dull turn-in
    // without fixing it. Uniform imbalance keeps the classic bar advice.
    let phase_split =
        overall
            .corners
            .as_ref()
            .and_then(|c| match supported_pair(&c.entry, &c.exit) {
                (Some(e), Some(x)) => Some((e, x)),
                _ => None,
            });
    let lean = phase_split
        .map(|(e, x)| {
            let gap = (e - x) * dir; // + = entry-concentrated for this sign
            if gap >= PHASE_GAP && e.abs() >= BALANCE_MILD {
                PhaseLean::Entry
            } else if gap <= -PHASE_GAP && x.abs() >= BALANCE_MILD {
                PhaseLean::Exit
            } else {
                PhaseLean::Uniform
            }
        })
        .unwrap_or(PhaseLean::Uniform);
    let on_power = overall
        .balance_on_throttle
        .index
        .filter(|_| overall.balance_on_throttle.samples >= BAND_MIN_SAMPLES)
        .is_some_and(|on| on * dir >= BALANCE_MILD);

    // Entry-lever signatures (plan 015): which second lever the entry push
    // implicates is read from its own channel, not a fixed ordering.
    let brake_bound = !overall.surface_loose
        && overall
            .balance_on_brake
            .index
            .filter(|_| overall.balance_on_brake.samples >= BAND_MIN_SAMPLES)
            .is_some_and(|b| b >= BRAKE_PUSH);
    let conv_off = overall.diff_drag.conv_off();
    let decel_locked = overall.drivetrain_type == 1 // rear-driven reference only
        && conv_off.is_some_and(|c| c <= CONV_OFF_LOCKED);

    let front_driven = overall.drivetrain_type != 1; // FWD or AWD
    let (advice, family, softer): (String, _, _) = match (understeer, lean) {
        (true, PhaseLean::Entry) => {
            let mut advice = String::from(
                "reduce front roll stiffness (soften the front anti-roll bar \
                 or springs): the push concentrates at corner entry",
            );
            if brake_bound {
                advice.push_str(
                    "; the braking band implicates bias too — shift brake \
                     balance rearward next",
                );
            } else if decel_locked {
                advice.push_str(
                    "; the decel-locked rear diff is resisting turn-in — \
                     reduce rear diff decel next",
                );
            }
            (advice, Family::FrontRoll, true)
        }
        (true, PhaseLean::Exit) if on_power && front_driven => (
            "reduce front diff accel lock: the front washes out under power on \
             corner exit, so softening the front end would dull turn-in without \
             fixing the power-on push. Stiffening the rear (springs/arb) is \
             the second lever"
                .into(),
            Family::DiffAccel,
            true,
        ),
        (true, PhaseLean::Exit) if on_power => (
            "stiffen the rear (anti-roll bar or springs) to shift grip \
             forward: the push appears under power on corner exit, not at \
             turn-in; softening the front would dull entry without fixing it"
                .into(),
            Family::RearRoll,
            false,
        ),
        (false, PhaseLean::Entry) => (
            "increase rear diff decel lock: the rear steps out into corners \
             (braking / lift-off), and decel lock stabilises entry. Softening \
             the rear arb or springs is the second lever, brake balance \
             forward the third"
                .into(),
            Family::DiffDecel,
            false,
        ),
        (false, PhaseLean::Exit) if on_power => (
            "reduce rear roll stiffness: soften the rear anti-roll bar first \
             (springs second). The slide concentrates on corner exit under \
             throttle; if softer rear roll doesn't clear it, reduce rear \
             diff accel next"
                .into(),
            Family::RearRoll,
            true,
        ),
        (true, _) => (
            "reduce front roll stiffness: soften the front anti-roll bar or \
             springs (which of the two is not separable from this data yet: \
             bars move roll only, springs also jounce and pitch)"
                .into(),
            Family::FrontRoll,
            true,
        ),
        (false, _) => (
            "reduce rear roll stiffness: soften the rear anti-roll bar or \
             springs"
                .into(),
            Family::RearRoll,
            true,
        ),
    };
    // Phase-redirected driveline primaries rest on the newer corner-phase
    // signal and on families whose behavioural evidence is historically weak
    // (deliberate min/max A/Bs): never High on the split alone.
    let mut confidence = confidence;
    if matches!(family, Family::DiffAccel | Family::DiffDecel) {
        confidence = confidence.min(Confidence::Medium);
    }
    // Transient counter-signal: a net-understeer car that flashes real
    // oversteer in bursts (power-on, at speed) is compromise-limited: the
    // average says soften the front, but that sharpens the snaps. Cap the
    // averaged advice and note why.
    let os = &overall.transient_oversteer;
    let snappy = !overall.surface_loose
        && understeer
        && (os.on_power_frac >= OS_ON_POWER_FRAC
            || os.rear_first_frac >= OS_REAR_FIRST_FRAC
            || os.countersteer_frac >= OS_COUNTERSTEER_FRAC);
    if snappy {
        confidence = confidence.min(Confidence::Medium);
    }

    let mut evidence = Vec::new();
    if let Some(gs) = &sat {
        let mut line = format!(
            "front saturation: pushing {:.1}% of cornering — front pinned at \
             its fitted grip peak with the rear inside its own (healthy \
             tarmac <= 5%)",
            gs.push_frac * 100.0,
        );
        if let Some(u) = gs.rear_use_at_push {
            line.push_str(&format!(
                "; rear at {:.0}% of its limit while pushing ({:.0}% spare \
                 grip unused)",
                u * 100.0,
                (1.0 - u) * 100.0,
            ));
        }
        evidence.push(line);
        evidence.push(match gs.source {
            CurveSource::Campaign => "grip curve: campaign-pooled".into(),
            CurveSource::CarPool => {
                "grip curve: pooled across this car's recordings (crosses setups)".into()
            }
            CurveSource::SelfFit => {
                "grip curve: fitted from this recording alone — indicative only".into()
            }
        });
        if idx.is_some() {
            evidence.push(format!(
                "averaged balance index {idx_v:+.2}: the driver's operating \
                 point (every tarmac stint reads positive) — context, not the \
                 trigger",
            ));
        }
        if understeer && decel_locked && matches!(lean, PhaseLean::Entry) {
            evidence.push(format!(
                "off-throttle rear/front wheel-speed convergence {:.2} (open \
                 decel diffs read ~1.0): the locked rear drags against turn-in",
                conv_off.unwrap_or(0.0),
            ));
        }
    } else {
        evidence.push(format!(
            "{} tendency: front−rear slip angle delta {idx_v:+.2} while cornering",
            if understeer {
                "understeer"
            } else {
                "oversteer"
            },
        ));
    }
    if !lap_indices.is_empty() {
        let laps_fmt: Vec<String> = lap_indices.iter().map(|i| format!("{i:+.2}")).collect();
        evidence.push(format!(
            "{} across {} flying lap(s): {}",
            if consistent {
                "consistent"
            } else {
                "not consistent"
            },
            lap_indices.len(),
            laps_fmt.join(", "),
        ));
    }
    let (front_t, rear_t) = axle_temps(overall);
    if (front_t - rear_t).abs() >= 5.0 {
        evidence.push(format!(
            "{} tires run {:.0}°F hotter",
            if front_t > rear_t { "front" } else { "rear" },
            (front_t - rear_t).abs(),
        ));
    }
    let front_slip = (overall.slip_frac.fl + overall.slip_frac.fr) / 2.0;
    let rear_slip = (overall.slip_frac.rl + overall.slip_frac.rr) / 2.0;
    evidence.push(format!(
        "time over slip limit: front {:.0}% vs rear {:.0}%",
        front_slip * 100.0,
        rear_slip * 100.0,
    ));
    if let Some(ratio) = overall.margin_ratio() {
        evidence.push(format!(
            "grip-margin ratio {ratio:.1}x (front vs rear share of limit; \
             drivers settle near 1.5-1.7x — the gap is the rear's spare grip)",
        ));
    }
    if let (Some(lo), Some(hi)) =
        supported_pair(&overall.balance_low_speed, &overall.balance_high_speed)
    {
        evidence.push(if (lo - hi) * dir >= AERO_BAND_GAP / 2.0 {
            format!(
                "concentrated at low speed ({lo:+.2} below 85 mph vs {hi:+.2} above): \
                 mechanical grip, so bars/springs over aero"
            )
        } else {
            format!("by speed: {lo:+.2} below 85 mph, {hi:+.2} above")
        });
    }
    if let Some(c) = &overall.corners
        && let (Some(entry), Some(exit)) = supported_pair(&c.entry, &c.exit)
    {
        let mark = match lean {
            PhaseLean::Entry => ", concentrated at entry",
            PhaseLean::Exit => ", concentrated at exit",
            PhaseLean::Uniform => "",
        };
        evidence.push(format!(
            "by corner phase: {entry:+.2} into corners, {exit:+.2} out \
             ({} corners){mark}",
            c.corners,
        ));
    }
    if matches!(lean, PhaseLean::Exit) && on_power {
        evidence.push(format!(
            "on-throttle index {:+.2}: the imbalance rides the power",
            overall.balance_on_throttle.index.unwrap_or(0.0),
        ));
    }

    if snappy {
        evidence.push(format!(
            "counter-signal: {:.1}% of cornering time flashes clear oversteer \
             ({} episodes; {:.1}% on power, {:.1}% at speed, rear-first at \
             limit {:.1}%) and the driver counter-steers {:.1}% of cornering \
             ({} corrections); softening the front sharpens these moments",
            os.clear_frac * 100.0,
            os.episodes,
            os.on_power_frac * 100.0,
            os.high_speed_frac * 100.0,
            os.rear_first_frac * 100.0,
            os.countersteer_frac * 100.0,
            os.countersteer_episodes,
        ));
    }
    recs.push(Recommendation {
        apply: Vec::new(),
        area: "balance",
        advice,
        evidence,
        confidence,
        suggestion: None,
        implied: Some(Change {
            family,
            softer,
            magnitude: None,
        }),
    });

    // Entry-understeer corroborated by the braking band (tarmac only; dirt
    // trail-brake rotation is technique): brake balance is its own lever on
    // the entry card, worth a separate journalable recommendation. Low
    // because the front direction never separates cleanly: even the GT-R's
    // 100%-front exemplar read only +0.23 (the adapting driver caps it), so
    // clearing +0.30 means something genuinely extreme.
    if understeer
        && matches!(lean, PhaseLean::Entry)
        && !overall.surface_loose
        && let Some(brake) = overall
            .balance_on_brake
            .index
            .filter(|_| overall.balance_on_brake.samples >= BAND_MIN_SAMPLES)
        && brake >= BRAKE_PUSH
    {
        recs.push(Recommendation {
            apply: Vec::new(),
            area: "brakes",
            advice: "shift brake balance rearward a step: the front spends its \
                     grip on braking while still turning in"
                .into(),
            evidence: vec![format!(
                "braking-band balance {brake:+.2} vs {BRAKE_PUSH:+.2} threshold \
                 (healthy tarmac reads +0.07..+0.30; even a 100%-front exemplar \
                 read only +0.23)"
            )],
            confidence: Confidence::Low,
            suggestion: None,
            implied: Some(Change {
                family: Family::Brakes,
                softer: true,
                magnitude: None,
            }),
        });
    }
    Some(dir)
}

/// Episodic oversteer detected through EVENT channels (flashes, rear-first
/// moments, the driver's own counter-steer corrections), independent of the
/// averaged balance index: oversteer is episodic-and-corrected, so a car can
/// slide every lap while its cornering mean reads neutral (both deliberate
/// oversteer exemplars read -0.01/+0.02 net). Net-OVERSTEER means are the
/// balance card's job; this card covers what the average hides. Tarmac only
/// (dirt rotation is technique, 12-23% everywhere).
fn stability_rule(overall: &StintMetrics, ctx: &Context, recs: &mut Vec<Recommendation>) {
    if overall.surface_loose {
        return;
    }
    let os = &overall.transient_oversteer;
    if os.on_power_frac < OS_ON_POWER_FRAC
        && os.rear_first_frac < OS_REAR_FIRST_FRAC
        && os.countersteer_frac < OS_COUNTERSTEER_FRAC
    {
        return;
    }
    // A net-oversteer average already gets the balance card's rear levers.
    if overall.understeer_index.is_some_and(|i| i <= -BALANCE_MILD) {
        return;
    }
    // A collapsed braking band names the mechanical cause (bias too far
    // rear, brake_rule's card); a generic rear-grip lever would be noise.
    if overall
        .balance_on_brake
        .index
        .filter(|_| overall.balance_on_brake.samples >= BAND_MIN_SAMPLES)
        .is_some_and(|b| b <= BRAKE_REAR_ROTATE)
    {
        return;
    }
    // Corroboration: the grip-margin ratio collapsing to ~1 means the rear
    // works as hard as the front — both deliberate oversteer exemplars read
    // <= 1.03 while healthy stints with any oversteer events sit >= 1.36.
    let margin = overall.margin_ratio();
    let collapsed = margin.is_some_and(|r| r <= MARGIN_COLLAPSE);

    let at_speed = os.high_speed_frac >= os.on_power_frac;
    let aero = at_speed && ctx.aero_tunable != Some(false);
    let mut evidence = vec![format!(
        "momentary oversteer with the average reading {}: {:.1}% of \
         cornering flashes clear oversteer ({:.1}% on power, {:.1}% at \
         >=85 mph), counter-steer {:.1}%; healthy tarmac reads <=3.9% \
         on-power, deliberate oversteer exemplars 15-23% flashes",
        overall
            .understeer_index
            .map(|i| format!("{i:+.2}"))
            .unwrap_or_else(|| "neutral".into()),
        os.clear_frac * 100.0,
        os.on_power_frac * 100.0,
        os.high_speed_frac * 100.0,
        os.countersteer_frac * 100.0,
    )];
    if let Some(r) = margin
        && collapsed
    {
        evidence.push(format!(
            "grip-margin ratio {r:.2}: the rear works as hard as the front \
             (drivers settle near 1.5-1.7x; both deliberate oversteer \
             exemplars read <=1.03)"
        ));
    }
    recs.push(Recommendation {
        apply: Vec::new(),
        area: "stability",
        advice: if aero {
            "add rear aero: the oversteer flashes concentrate at high speed, \
             where the rear runs out of downforce before the front runs out \
             of grip"
                .into()
        } else if at_speed {
            "soften the rear a step (arb or springs) or lower rear ride \
             height: the oversteer flashes concentrate at high speed and \
             no aero is fitted, so mechanical rear grip is the lever"
                .into()
        } else {
            "reduce rear diff accel lock (or soften the rear a step): the \
             oversteer flashes ride the throttle; the rear breaks away \
             under power"
                .into()
        },
        evidence,
        confidence: if collapsed {
            Confidence::Medium
        } else {
            Confidence::Low
        },
        suggestion: None,
        implied: Some(Change {
            family: if aero {
                Family::RearAero
            } else if at_speed {
                Family::RearRoll
            } else {
                Family::DiffAccel
            },
            softer: !aero,
            magnitude: None,
        }),
    });
}

/// Bias too far rear reads directly in the braking-conditioned band: the car
/// rotates while braking-and-cornering, regardless of what the positional
/// phase means say (the McLaren's bias-20 stint kept +0.20 entry push while
/// its braking band collapsed to +0.01 — the band is the detector). Tarmac
/// only: dirt trail-brake rotation is technique and reads +0.25..+0.67 on
/// reference tunes.
fn brake_rule(overall: &StintMetrics, recs: &mut Vec<Recommendation>) {
    let Some(brake) = overall
        .balance_on_brake
        .index
        .filter(|_| overall.balance_on_brake.samples >= BAND_MIN_SAMPLES)
    else {
        return;
    };
    if overall.surface_loose || brake > BRAKE_REAR_ROTATE {
        return;
    }
    let mut evidence = vec![format!(
        "braking-band balance {brake:+.2}: healthy tarmac stints read +0.07..+0.30 \
         across the library; deliberate rear-bias states read -0.21..+0.03 on three cars"
    )];
    if let Some(c) = &overall.corners
        && let (Some(entry), _) = supported_pair(&c.entry, &c.exit)
    {
        evidence.push(format!(
            "corner-entry balance {entry:+.2}{}",
            if entry <= 0.0 {
                " — the rotation reaches the whole entry phase"
            } else {
                " — the rotation lives in the braking zones alone"
            }
        ));
    }
    let os = &overall.transient_oversteer;
    if os.countersteer_frac >= OS_COUNTERSTEER_FRAC {
        evidence.push(format!(
            "counter-steer {:.1}% of cornering: the rotation is costing corrections",
            os.countersteer_frac * 100.0
        ));
    }
    recs.push(Recommendation {
        apply: Vec::new(),
        area: "brakes",
        advice: "shift brake balance forward: the car rotates while braking — \
                 the rears are doing the stopping and letting go first"
            .into(),
        evidence,
        confidence: Confidence::Medium,
        suggestion: None,
        implied: Some(Change {
            family: Family::Brakes,
            softer: false,
            magnitude: None,
        }),
    });
}

/// Which corner phase an imbalance concentrates in, per the sign of the
/// overall index (entry-heavy understeer and entry-heavy oversteer both read
/// Entry).
#[derive(Clone, Copy, PartialEq)]
enum PhaseLean {
    Entry,
    Exit,
    Uniform,
}

/// Both bands' indices, when each has enough cornering samples to trust.
fn supported_pair(
    a: &crate::analysis::metrics::BandBalance,
    b: &crate::analysis::metrics::BandBalance,
) -> (Option<f32>, Option<f32>) {
    let idx = |band: &crate::analysis::metrics::BandBalance| {
        (band.samples >= BAND_MIN_SAMPLES)
            .then_some(band.index)
            .flatten()
    };
    (idx(a), idx(b))
}

/// Imbalance that lives ONLY in the high-speed band is an aero problem, not a
/// bars problem: mechanical imbalance shows at every speed (and on the real
/// library, mechanical understeer reads STRONGER at low speed).
fn aero_rule(overall: &StintMetrics, ctx: &Context, recs: &mut Vec<Recommendation>) {
    let (Some(lo), Some(hi)) =
        supported_pair(&overall.balance_low_speed, &overall.balance_high_speed)
    else {
        return;
    };
    if hi.abs() < AERO_HIGH_INDEX || lo.abs() >= BALANCE_MILD || (hi - lo).abs() < AERO_BAND_GAP {
        return;
    }
    let understeer = hi > 0.0;
    // No aero fitted: the speed-only imbalance is real but downforce isn't a
    // lever; ride height (rake) is the closest one this build has.
    let no_aero = ctx.aero_tunable == Some(false);
    let advice = match (understeer, no_aero) {
        (true, false) => {
            "add front aero (or reduce rear aero): the car only pushes at high \
             speed, where downforce balance outweighs the bars"
        }
        (false, false) => {
            "add rear aero: the car is only loose at high speed, where downforce \
             balance outweighs the bars"
        }
        (true, true) => {
            "lower front ride height (or raise rear) to tip the rake forward: \
             the car only pushes at high speed and no aero is fitted, so ride \
             height is the closest lever"
        }
        (false, true) => {
            "lower rear ride height (or raise front): the car is only loose at \
             high speed and no aero is fitted, so ride height is the closest \
             lever"
        }
    };
    recs.push(Recommendation {
        apply: Vec::new(),
        area: "aero",
        advice: advice.into(),
        evidence: vec![format!(
            "{} at speed only: index {hi:+.2} above 85 mph vs {lo:+.2} below \
             (neutral); a bars change would upset the low-speed balance that is \
             currently fine",
            if understeer {
                "understeer"
            } else {
                "oversteer"
            },
        )],
        confidence: if overall.surface_loose || no_aero {
            Confidence::Low
        } else {
            Confidence::Medium
        },
        suggestion: None,
        implied: Some(Change {
            family: if no_aero {
                Family::RideHeight
            } else if understeer {
                Family::FrontAero
            } else {
                Family::RearAero
            },
            softer: no_aero, // lowering ride height; aero variants add downforce
            magnitude: None,
        }),
    });
}

/// Balance that degrades when the throttle is applied points at the driveline,
/// not the bars: acceleration diff lock (and center torque split on AWD).
fn power_balance_rule(overall: &StintMetrics, recs: &mut Vec<Recommendation>) {
    let (Some(on), Some(off)) =
        supported_pair(&overall.balance_on_throttle, &overall.balance_off_throttle)
    else {
        return;
    };
    let shift = on - off;
    let index_gate = if overall.surface_loose {
        POWER_INDEX_LOOSE
    } else {
        POWER_INDEX
    };
    let rear_drive = overall.drivetrain_type != 0; // RWD or AWD
    let front_drive = overall.drivetrain_type != 1; // FWD or AWD
    let awd = overall.drivetrain_type == 2;

    let (advice, index_evt, alt) = if shift <= -POWER_SHIFT && on <= -index_gate && rear_drive {
        (
            "reduce rear differential acceleration lock: the rear breaks away \
             specifically under power",
            "oversteer",
            awd.then_some("alternative (AWD): shift center torque forward"),
        )
    } else if shift >= POWER_SHIFT && on >= index_gate && front_drive {
        (
            "reduce front differential acceleration lock: the front washes out \
             specifically under power",
            "understeer",
            awd.then_some("alternative (AWD): shift center torque rearward"),
        )
    } else {
        return;
    };
    let advice = advice.to_string();
    let mut evidence: Vec<String> = alt.map(String::from).into_iter().collect();
    evidence.push(format!(
        "{index_evt} appears with throttle: index {on:+.2} on power vs {off:+.2} \
         off power while cornering"
    ));
    if let Some(rear) = overall.balance_on_throttle.rear_slip {
        evidence.push(format!(
            "rear tires at {:.0}% of grip limit while cornering on power",
            rear * 100.0
        ));
    }
    recs.push(Recommendation {
        apply: Vec::new(),
        area: "differential",
        advice,
        evidence,
        confidence: Confidence::Medium,
        suggestion: None,
        implied: Some(Change {
            family: Family::DiffAccel,
            softer: true,
            magnitude: None,
        }),
    });
}

fn tire_pressure_rule(
    overall: &StintMetrics,
    balance_sign: Option<f32>,
    compound: Option<&str>,
    recs: &mut Vec<Recommendation>,
) {
    let (low, high, band_name) = temp_band(compound);
    let band_label = if band_name.is_empty() {
        "working band (no tire compound on file, assuming slick)".to_string()
    } else {
        format!("{band_name} working band")
    };
    let (front, rear) = axle_temps(overall);
    for (axle, avg, scrub_heated) in [
        ("front", front, balance_sign == Some(1.0)),
        ("rear", rear, balance_sign == Some(-1.0)),
    ] {
        let (advice, margin) = if avg > high {
            (
                format!("raise {axle} tire pressures a step: {axle} tires run hot"),
                avg - high,
            )
        } else if avg < low {
            (
                format!("lower {axle} tire pressures a step to build temperature"),
                low - avg,
            )
        } else {
            continue;
        };
        let mut evidence = vec![format!(
            "{axle} avg temp {avg:.0}°F vs {low:.0}-{high:.0}°F {band_label}"
        )];
        let mut confidence = if margin >= TEMP_CLEAR_MARGIN_F {
            Confidence::Medium
        } else {
            Confidence::Low
        };
        if scrub_heated && avg > high {
            confidence = Confidence::Low;
            evidence.push(
                "heat is partly scrub from the balance issue above; fix balance first".into(),
            );
        }
        recs.push(Recommendation {
            apply: Vec::new(),
            area: "tires",
            suggestion: None,
            advice,
            evidence,
            confidence,
            implied: None,
        });
    }
}

fn traction_rule(overall: &StintMetrics, recs: &mut Vec<Recommendation>) {
    let Some(spin) = overall.wheelspin_frac else {
        return;
    };
    if spin < WHEELSPIN_MED {
        return;
    }
    let drive_axle = match overall.drivetrain_type {
        0 => "front",
        1 => "rear",
        _ => "drive",
    };
    let spin_evidence = format!(
        "wheelspin during {:.0}% of on-throttle time ({} drivetrain)",
        spin * 100.0,
        crate::telemetry::packet::drivetrain_name(overall.drivetrain_type),
    );
    let confidence = if spin >= WHEELSPIN_HIGH {
        Confidence::High
    } else {
        Confidence::Medium
    };

    // Direction comes from the spin PATTERN, not the amount: wheelspin falls
    // monotonically with lock (open diffs spin the unloaded inside wheel),
    // while oversteer events rise with it — the R12 sweep measured the two
    // failure modes on opposite channels. Tarmac rear-driven cars with
    // enough on-throttle cornering get the discriminator; dirt (both-rears
    // 17-35% everywhere = technique) and FWD (front symmetry unmeasured)
    // keep the legacy reduce-lock direction.
    let ts = &overall.traction_spin;
    let os = &overall.transient_oversteer;
    let oversteer_events = os.on_power_frac >= OS_ON_POWER_FRAC
        || os.rear_first_frac >= OS_REAR_FIRST_FRAC
        || os.countersteer_frac >= OS_COUNTERSTEER_FRAC;
    if overall.drivetrain_type != 0 && !overall.surface_loose && ts.samples >= BAND_MIN_SAMPLES {
        let symmetry = format!(
            "rear spin symmetry: inside-only {:.1}% / both rears {:.1}% of \
             on-throttle cornering (healthy tarmac reads <=6% / <=1.5%; an \
             open diff measured 11.0% / 0.2%, a locked one 2.0% / 3.2%)",
            ts.inside_only_frac * 100.0,
            ts.both_frac * 100.0,
        );
        if ts.both_frac >= BOTH_REAR_SPIN || oversteer_events {
            let mut evidence = vec![symmetry, spin_evidence];
            if oversteer_events {
                evidence.push(format!(
                    "oversteer events corroborate: {:.1}% on power, rear-first \
                     {:.1}%, counter-steer {:.1}% of cornering",
                    os.on_power_frac * 100.0,
                    os.rear_first_frac * 100.0,
                    os.countersteer_frac * 100.0,
                ));
            }
            recs.push(Recommendation {
                apply: Vec::new(),
                area: "traction",
                advice: format!(
                    "improve {drive_axle}-axle traction: reduce differential \
                     acceleration lock — both rears break away together, the \
                     locked-diff signature"
                ),
                evidence,
                confidence,
                suggestion: None,
                implied: Some(Change {
                    family: Family::DiffAccel,
                    softer: true,
                    magnitude: None,
                }),
            });
        } else if ts.inside_only_frac >= INSIDE_ONLY_SPIN {
            recs.push(Recommendation {
                apply: Vec::new(),
                area: "traction",
                advice: "add rear diff accel lock: the unloaded inside rear spins \
                         alone while the outside grips — an open diff dumps the \
                         torque there; more lock drives both wheels together"
                    .into(),
                evidence: vec![symmetry, spin_evidence],
                confidence: confidence.min(Confidence::Medium),
                suggestion: None,
                implied: Some(Change {
                    family: Family::DiffAccel,
                    softer: false,
                    magnitude: None,
                }),
            });
        } else {
            // Neither signature: the wheelspin is real but does not implicate
            // the diff in either direction.
            recs.push(Recommendation {
                apply: Vec::new(),
                area: "traction",
                advice: format!(
                    "improve {drive_axle}-axle traction: softer {drive_axle} \
                     springs/dampers help put power down; the rear spin pattern \
                     doesn't implicate the diff in either direction"
                ),
                evidence: vec![symmetry, spin_evidence],
                confidence: Confidence::Low,
                suggestion: None,
                implied: None,
            });
        }
        return;
    }

    recs.push(Recommendation {
        apply: Vec::new(),
        area: "traction",
        advice: format!(
            "improve {drive_axle}-axle traction: reduce differential acceleration lock"
        ),
        evidence: vec![
            format!("alternative: softer {drive_axle} springs/dampers also help put power down"),
            spin_evidence,
        ],
        confidence,
        suggestion: None,
        implied: Some(Change {
            family: Family::DiffAccel,
            softer: true,
            magnitude: None,
        }),
    });
}

fn gearing_rule(overall: &StintMetrics, recs: &mut Vec<Recommendation>) {
    let g = &overall.gears;
    if g.limiter_frac >= LIMITER_FRAC {
        let mut evidence = vec![if g.limiter_detected {
            format!(
                "on the rev limiter {:.1}% of the stint. The ACTUAL rev cut sits at \
                 {:.0} rpm ({:.0}% of the reported {:.0} redline; 3+ gears max out \
                 there)",
                g.limiter_frac * 100.0,
                g.effective_redline,
                100.0 * g.effective_redline / overall.redline.max(1.0),
                overall.redline,
            )
        } else {
            format!(
                "on the rev limiter {:.1}% of the stint (redline {:.0} rpm)",
                g.limiter_frac * 100.0,
                g.effective_redline,
            )
        }];
        if let Some(rpm) = g.avg_upshift_rpm {
            evidence.push(format!("average upshift at {rpm:.0} rpm"));
        }
        evidence.push(format!("top gear used: {}", g.top_gear));
        recs.push(Recommendation {
            apply: Vec::new(),
            area: "gearing",
            advice: "lengthen the final drive (or the gears that hit the limiter) so the \
                     engine stays below redline at the route's top speeds"
                .into(),
            evidence,
            confidence: Confidence::Medium,
            suggestion: None,
            implied: Some(Change {
                family: Family::Gearing,
                softer: true,
                magnitude: None,
            }),
        });
        return;
    }

    // The "too long" extreme: the car lives in top gear but never climbs into
    // the top of the rev range, so every gear is longer than the route needs.
    // Judged on the time-share of top-gear frames near redline: a single
    // downhill burst can push the max near redline, but it cannot fake
    // sustained use of the rev range.
    if g.top_gear == 0 || overall.redline <= 0.0 {
        return;
    }
    let top_time = g.time_frac.last().map(|(_, f)| *f).unwrap_or(0.0);
    if top_time >= TOP_GEAR_TIME_FRAC && g.top_gear_high_rev_frac < HIGH_REV_SHARE_MIN {
        recs.push(Recommendation {
            apply: Vec::new(),
            area: "gearing",
            advice: "shorten the final drive: the car lives in top gear but the top of \
                     the rev range goes unused; shorter gearing gives more acceleration \
                     everywhere at no real top-speed cost"
                .into(),
            evidence: vec![
                format!(
                    "{:.1}% of the stint in top gear ({}) but only {:.1}% of that time \
                     above 90% of the {:.0} redline{} (highest seen {:.0} rpm)",
                    top_time * 100.0,
                    g.top_gear,
                    g.top_gear_high_rev_frac * 100.0,
                    g.effective_redline,
                    if g.limiter_detected {
                        " (the car's real rev cut)"
                    } else {
                        ""
                    },
                    g.top_gear_max_rpm,
                ),
                "check the shorter gearing doesn't put the longest straight on the \
                 limiter: that is the opposite extreme of the same tradeoff"
                    .into(),
            ],
            confidence: Confidence::Medium,
            suggestion: None,
            implied: Some(Change {
                family: Family::Gearing,
                softer: false,
                magnitude: None,
            }),
        });
        return;
    }

    // Model-based check (the aero–gearing coupling): the fitted drag curve
    // says where the rev cut SHOULD arrive: the speed the longest flat-out
    // run actually reaches. Behavioural rules above see only gross extremes;
    // this catches the mismatch an aero change silently introduces.
    if let Some(d) = &overall.driveline
        && let Some(scale) = d.final_drive_scale(g.effective_redline)
        && !(0.92..=1.08).contains(&scale)
    {
        let short = scale < 1.0;
        recs.push(Recommendation {
            apply: Vec::new(),
            area: "gearing",
            advice: if short {
                "lengthen the final drive to match this aero: the fitted drag \
                 model says the longest run reaches past the current rev-cut \
                 speed; the engine runs out before the car does"
                    .into()
            } else {
                "shorten the final drive to match this aero: the rev cut sits \
                 well past what the longest run can reach; unused top end \
                 traded for acceleration everywhere"
                    .into()
            },
            evidence: vec![format!(
                "drag model: longest flat-out run reaches ~{:.0} mph, rev cut \
                 arrives at {:.0} mph (gear {}): ideal final drive ≈ current × {:.2}",
                d.vmax_track * crate::util::MPS_TO_MPH,
                d.redline_speed(g.effective_redline) * crate::util::MPS_TO_MPH,
                d.top_gear,
                scale,
            )],
            confidence: Confidence::Low,
            suggestion: None,
            implied: Some(Change {
                family: Family::Gearing,
                softer: short, // lengthen = lower final drive number
                magnitude: None,
            }),
        });
    }
}

fn suspension_rule(overall: &StintMetrics, recs: &mut Vec<Recommendation>) {
    let s = &overall.suspension;
    for (axle, bottomed, topped) in [
        (
            "front",
            s.fl.bottomed_frac.max(s.fr.bottomed_frac),
            s.fl.topped_frac.max(s.fr.topped_frac),
        ),
        (
            "rear",
            s.rl.bottomed_frac.max(s.rr.bottomed_frac),
            s.rl.topped_frac.max(s.rr.topped_frac),
        ),
    ] {
        if bottomed >= BOTTOMING_FRAC {
            recs.push(Recommendation {
                apply: Vec::new(),
                area: "suspension",
                advice: format!(
                    "{axle} suspension bottoms out: stiffen {axle} springs or raise \
                     {axle} ride height"
                ),
                evidence: vec![format!(
                    "{axle} at full compression {:.1}% of the stint",
                    bottomed * 100.0
                )],
                confidence: Confidence::Medium,
                suggestion: None,
                implied: None,
            });
        }
        if topped >= TOPPING_FRAC {
            recs.push(Recommendation {
                apply: Vec::new(),
                area: "suspension",
                advice: format!(
                    "{axle} suspension spends long at full extension: if the route is \
                     smooth this can mean over-stiff {axle} springs or too much rebound; \
                     over crests and jumps it is normal"
                ),
                evidence: vec![format!(
                    "{axle} at full extension {:.1}% of the stint",
                    topped * 100.0
                )],
                confidence: Confidence::Low,
                suggestion: None,
                implied: None,
            });
        }
    }
}

fn damping_rule(overall: &StintMetrics, recs: &mut Vec<Recommendation>) {
    let s = &overall.suspension;
    let loose = overall.surface_loose;
    let baseline = if loose {
        "~12-16 on loose surfaces"
    } else {
        "~5.5-6 on tarmac"
    };
    let (under_rev, under_topped) = if loose {
        (UNDERDAMPED_REV_LOOSE, UNDERDAMPED_TOPPED_LOOSE)
    } else {
        (UNDERDAMPED_REV_TARMAC, UNDERDAMPED_TOPPED_TARMAC)
    };

    for (axle, a, b) in [("front", s.fl, s.fr), ("rear", s.rl, s.rr)] {
        let fired_before = recs.len();
        let rev = (a.reversals_per_sec + b.reversals_per_sec) / 2.0;
        let spatial = (a.reversals_per_100m + b.reversals_per_100m) / 2.0;
        let topped = a.topped_frac.max(b.topped_frac);

        if topped >= OVERDAMPED_TOPPED_FRAC {
            // Strong overdamping: the wheel lives at full extension instead of
            // tracking the surface.
            let mut evidence = vec![format!(
                "{axle} wheel at full extension {:.0}% of the stint (healthy reads \
                 under ~25% even on rough dirt)",
                topped * 100.0,
            )];
            if loose && let Some(flutter) = overall.rpm_flutter {
                evidence.push(format!(
                    "rpm flutter {flutter:.0} rpm/s on throttle: skipping drive \
                     wheels (roughly doubles vs healthy damping on the same surface)"
                ));
            }
            recs.push(Recommendation {
                apply: Vec::new(),
                area: "damping",
                advice: format!(
                    "reduce {axle} damping (rebound first): the {axle} wheels are \
                     held off the surface instead of following it"
                ),
                evidence,
                confidence: Confidence::High,
                suggestion: None,
                implied: None,
            });
        } else if rev >= under_rev && topped >= under_topped {
            recs.push(Recommendation {
                apply: Vec::new(),
                area: "damping",
                advice: format!(
                    "increase {axle} damping (rebound especially): the {axle} wheels \
                     oscillate and spend long stretches at full extension; bouncing \
                     off the surface costs grip everywhere"
                ),
                evidence: vec![
                    format!(
                        "{axle} suspension reverses direction {rev:.1}x/s (healthy \
                         baseline {baseline})"
                    ),
                    format!(
                        "{axle} at full extension {:.1}% of the stint",
                        topped * 100.0
                    ),
                ],
                confidence: if loose {
                    Confidence::Medium
                } else {
                    Confidence::High
                },
                suggestion: None,
                implied: None,
            });
        } else if !loose
            && rev > 0.0
            && (rev <= OVERDAMPED_BUMP_REV || spatial <= OVERDAMPED_SPATIAL_TARMAC)
            && topped >= OVERDAMPED_BUMP_TOPPED
        {
            recs.push(Recommendation {
                apply: Vec::new(),
                area: "damping",
                suggestion: None,
                advice: format!(
                    "reduce {axle} bump damping: articulation is suppressed and \
                     the {axle} wheels spend long stretches at full extension; \
                     the compression stroke is fighting the surface"
                ),
                evidence: vec![
                    format!(
                        "{axle} suspension reverses direction only {rev:.1}x/s \
                         ({spatial:.1} per 100m; healthy tarmac reads 11-16 per \
                         100m at any speed) while at full extension {:.1}% of \
                         the stint",
                        topped * 100.0,
                    ),
                    "signature measured on the bump-only-max A/B: reversals \
                     halved, full-extension time tripled, +0.62s"
                        .into(),
                ],
                confidence: Confidence::Medium,
                implied: Some(Change {
                    family: Family::Damping,
                    softer: true,
                    magnitude: None,
                }),
            });
        } else if !loose && rev > 0.0 && rev <= OVERDAMPED_REV_TARMAC {
            recs.push(Recommendation {
                apply: Vec::new(),
                area: "damping",
                advice: format!(
                    "consider softer {axle} damping: the {axle} suspension barely \
                     articulates. On smooth tarmac this costs little; on bumpy or \
                     off-road surfaces it will cost real grip"
                ),
                evidence: vec![format!(
                    "{axle} suspension reverses direction only {rev:.1}x/s (healthy \
                     baseline {baseline})"
                )],
                confidence: Confidence::Low,
                suggestion: None,
                implied: None,
            });
        }

        // Rebound overdamping straight from the damper phase split (plan 015
        // phase 3): extension speed collapsing against compression is
        // rebound's OWN signature, visible on smooth tarmac where reversal
        // counts stay healthy (the maxed-rebound A/B kept 5-6 reversals/s and
        // never topped out — the channels above were blind to it). Healthy
        // tarmac reads 0.76-0.84 across every library car and driver.
        let vratio = if axle == "front" {
            overall.damper_phase.vratio_front
        } else {
            overall.damper_phase.vratio_rear
        };
        if recs.len() == fired_before
            && !loose
            && let Some(v) = vratio
            && v <= REBOUND_VRATIO_LOW
        {
            recs.push(Recommendation {
                apply: Vec::new(),
                area: "damping",
                advice: format!(
                    "reduce {axle} rebound: the {axle} dampers extend far slower \
                     than they compress, holding the wheels down after every \
                     bump and body movement"
                ),
                evidence: vec![format!(
                    "{axle} extension speed {v:.2}x compression (healthy tarmac \
                     0.76-0.84 on every library car; the maxed-rebound A/B read \
                     0.59 and cost +0.28s)"
                )],
                confidence: Confidence::Medium,
                suggestion: None,
                implied: Some(Change {
                    family: Family::Damping,
                    softer: true,
                    magnitude: None,
                }),
            });
        }
    }
}

fn axle_temps(m: &StintMetrics) -> (f32, f32) {
    (
        (m.tire_temp.fl.avg + m.tire_temp.fr.avg) / 2.0,
        (m.tire_temp.rl.avg + m.tire_temp.rr.avg) / 2.0,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::metrics::{StintMetrics, TempStats, TractionSpin};
    use crate::telemetry::packet::Corners;

    fn base_metrics() -> StintMetrics {
        StintMetrics {
            samples: 1000,
            duration_s: 90.0,
            distance_m: 5000.0,
            avg_speed: 45.0,
            max_speed: 70.0,
            car_ordinal: 1,
            car_class: 4,
            car_performance_index: 800,
            drivetrain_type: 1,
            num_cylinders: 6,
            redline: 8000.0,
            tire_temp: Corners {
                fl: TempStats {
                    avg: 185.0,
                    max: 200.0,
                },
                fr: TempStats {
                    avg: 185.0,
                    max: 200.0,
                },
                rl: TempStats {
                    avg: 185.0,
                    max: 200.0,
                },
                rr: TempStats {
                    avg: 185.0,
                    max: 200.0,
                },
            },
            slip_frac: Corners::default(),
            understeer_index: Some(0.0),
            cornering_front_slip: Some(0.5),
            cornering_rear_slip: Some(0.5),
            cornering_frac: 0.3,
            transient_oversteer: Default::default(),
            brake_dive_front: None,
            driveline: None,
            balance_low_speed: Default::default(),
            balance_high_speed: Default::default(),
            balance_on_throttle: Default::default(),
            balance_off_throttle: Default::default(),
            balance_on_brake: Default::default(),
            corners: None,
            wheelspin_frac: Some(0.02),
            traction_spin: Default::default(),
            lockup_frac: Some(0.5),
            suspension: Corners::default(),
            gears: Default::default(),
            surface_rumble_avg: 0.0,
            kerbs: Default::default(),
            transitions: Default::default(),
            surface_loose: false,
            jumps: 0,
            landing_bottomed_excluded: 0,
            rpm_flutter: None,
            wheelspeed_flutter: None,
            grip_saturation: None,
            diff_drag: Default::default(),
            damper_phase: Default::default(),
            roll_use: Default::default(),
        }
    }

    fn lap_with_index(idx: f32) -> StintMetrics {
        StintMetrics {
            understeer_index: Some(idx),
            ..base_metrics()
        }
    }

    fn sat(push: f32, source: CurveSource) -> Option<crate::analysis::grip::GripSaturation> {
        Some(crate::analysis::grip::GripSaturation {
            push_frac: push,
            slide_frac: 0.0,
            rear_use_at_push: Some(0.6),
            coverage: 1.0,
            banded: false,
            source,
        })
    }

    /// Campaign-pooled front saturation at `push` share of cornering.
    fn push_sat(push: f32) -> Option<crate::analysis::grip::GripSaturation> {
        sat(push, CurveSource::Campaign)
    }

    #[test]
    fn consistent_understeer_is_high_confidence_front_advice() {
        let mut overall = base_metrics();
        overall.understeer_index = Some(0.24);
        overall.grip_saturation = push_sat(0.20);
        let laps = [
            lap_with_index(0.23),
            lap_with_index(0.26),
            lap_with_index(0.24),
        ];
        let recs = recommend(&overall, &laps, &Default::default());
        let balance = recs.iter().find(|r| r.area == "balance").unwrap();
        assert_eq!(balance.confidence, Confidence::High);
        assert!(
            balance.advice.contains("front anti-roll bar"),
            "{}",
            balance.advice
        );
        assert!(
            balance
                .evidence
                .iter()
                .any(|e| e.contains("consistent across 3"))
        );
    }

    /// The plan-015 core change: a positive averaged index alone is the
    /// driver's operating point, not a diagnosis. Every tarmac stint ever
    /// recorded reads +0.11..+0.38, so without front saturation the balance
    /// card stays silent.
    #[test]
    fn tarmac_positive_index_without_saturation_stays_quiet() {
        let mut overall = base_metrics();
        overall.understeer_index = Some(0.24);
        let recs = recommend(&overall, &[], &Default::default());
        assert!(
            recs.iter().all(|r| r.area != "balance"),
            "index alone must not fire: {:?}",
            recs.iter().map(|r| r.area).collect::<Vec<_>>()
        );
    }

    #[test]
    fn push_below_gate_or_selffit_source_stays_quiet() {
        let mut overall = base_metrics();
        overall.understeer_index = Some(0.24);
        overall.grip_saturation = push_sat(0.05); // healthy band
        let recs = recommend(&overall, &[], &Default::default());
        assert!(recs.iter().all(|r| r.area != "balance"));

        // Single-recording fits measured push 0.1-19.6% on healthy stints:
        // display-only, never a detection source.
        overall.grip_saturation = sat(0.30, CurveSource::SelfFit);
        let recs = recommend(&overall, &[], &Default::default());
        assert!(
            recs.iter().all(|r| r.area != "balance"),
            "SelfFit must not fire detection"
        );

        overall.grip_saturation = sat(0.30, CurveSource::CarPool);
        let recs = recommend(&overall, &[], &Default::default());
        let bal = recs.iter().find(|r| r.area == "balance").unwrap();
        assert_eq!(bal.confidence, Confidence::High);
        assert!(
            bal.evidence
                .iter()
                .any(|e| e.contains("pooled across this car")),
            "{:?}",
            bal.evidence
        );
    }

    /// Dirt keeps the legacy index gate whole (saturation curves deferred).
    #[test]
    fn dirt_understeer_still_fires_on_the_index() {
        let mut overall = base_metrics();
        overall.surface_loose = true;
        overall.understeer_index = Some(0.24);
        let recs = recommend(&overall, &[], &Default::default());
        assert!(recs.iter().any(|r| r.area == "balance"));
    }

    /// Entry push on a rear-driven car with the decel diff visibly dragging
    /// (off-throttle convergence collapsed vs the free-rolling front) names
    /// rear diff decel as the next lever instead of brake balance.
    #[test]
    fn entry_push_with_locked_decel_names_the_diff() {
        let mut overall = base_metrics();
        overall.drivetrain_type = 1;
        overall.understeer_index = Some(0.24);
        overall.grip_saturation = push_sat(0.10);
        overall.corners = corners(0.30, 0.10);
        overall.diff_drag = crate::analysis::metrics::DiffDrag {
            rear_off: Some(0.024),
            front_off: Some(0.040), // conv 0.6 <= 0.8 = locked
            rear_on: None,
            front_on: None,
        };
        let recs = recommend(&overall, &[], &Default::default());
        let bal = recs.iter().find(|r| r.area == "balance").unwrap();
        assert!(bal.advice.contains("rear diff decel"), "{}", bal.advice);
        assert!(
            bal.evidence
                .iter()
                .any(|e| e.contains("wheel-speed convergence")),
            "{:?}",
            bal.evidence
        );
    }

    /// Rebound overdamping fires from the damper-phase velocity asymmetry
    /// alone (the maxed-rebound A/B kept healthy reversal counts).
    #[test]
    fn collapsed_extension_speed_fires_reduce_rebound() {
        let mut overall = base_metrics();
        overall.damper_phase = crate::analysis::metrics::DamperPhase {
            ext_share_front: Some(0.63),
            ext_share_rear: Some(0.55),
            vratio_front: Some(0.59),
            vratio_rear: Some(0.80), // healthy: must stay quiet
        };
        let recs = recommend(&overall, &[], &Default::default());
        let damping: Vec<_> = recs.iter().filter(|r| r.area == "damping").collect();
        assert_eq!(damping.len(), 1, "{:?}", damping);
        assert!(
            damping[0].advice.contains("front rebound"),
            "{}",
            damping[0].advice
        );
        assert_eq!(damping[0].confidence, Confidence::Medium);

        // Dirt distributions are unmeasured for this channel: stay quiet.
        overall.surface_loose = true;
        let recs = recommend(&overall, &[], &Default::default());
        assert!(
            recs.iter()
                .all(|r| r.area != "damping" || !r.advice.contains("rebound: the"))
        );
    }

    #[test]
    fn oversteer_points_at_the_rear_and_inconsistency_lowers_confidence() {
        let mut overall = base_metrics();
        overall.understeer_index = Some(-0.15);
        let laps = [lap_with_index(-0.20), lap_with_index(0.02)];
        let recs = recommend(&overall, &laps, &Default::default());
        let balance = recs.iter().find(|r| r.area == "balance").unwrap();
        assert_eq!(balance.confidence, Confidence::Medium);
        assert!(
            balance.advice.contains("rear anti-roll bar"),
            "{}",
            balance.advice
        );
    }

    #[test]
    fn neutral_car_gets_no_balance_advice() {
        let recs = recommend(&base_metrics(), &[], &Default::default());
        assert!(recs.iter().all(|r| r.area != "balance"));
    }

    #[test]
    fn hot_fronts_defer_to_understeer() {
        let mut overall = base_metrics();
        overall.understeer_index = Some(0.24);
        overall.grip_saturation = push_sat(0.10);
        overall.tire_temp.fl.avg = 245.0;
        overall.tire_temp.fr.avg = 245.0;
        let recs = recommend(&overall, &[], &Default::default());
        let tires = recs.iter().find(|r| r.area == "tires").unwrap();
        assert_eq!(
            tires.confidence,
            Confidence::Low,
            "scrub heat defers to balance"
        );
        assert!(
            tires
                .evidence
                .iter()
                .any(|e| e.contains("fix balance first"))
        );
    }

    #[test]
    fn cold_tires_suggest_lower_pressure() {
        let mut overall = base_metrics();
        for t in [
            &mut overall.tire_temp.fl,
            &mut overall.tire_temp.fr,
            &mut overall.tire_temp.rl,
            &mut overall.tire_temp.rr,
        ] {
            t.avg = 120.0;
        }
        let recs = recommend(&overall, &[], &Default::default());
        let tire_recs: Vec<_> = recs.iter().filter(|r| r.area == "tires").collect();
        assert_eq!(tire_recs.len(), 2, "both axles cold");
        assert!(tire_recs.iter().all(|r| r.advice.contains("lower")));
        assert!(tire_recs.iter().all(|r| r.confidence == Confidence::Medium));
    }

    fn corners(entry: f32, exit: f32) -> Option<crate::analysis::corners::CornerSummary> {
        Some(crate::analysis::corners::CornerSummary {
            corners: 20,
            entry: band(5000, entry),
            exit: band(5000, exit),
            entry_braking: Default::default(),
            entry_coasting: Default::default(),
            avg_apex_speed: 40.0,
        })
    }

    /// Entry-concentrated understeer keeps the front-arb primary (entry card
    /// leads with it) but names the entry levers, and a hot braking band adds
    /// the brake-balance secondary (on tarmac only, dirt trail-braking is
    /// technique).
    #[test]
    fn entry_understeer_names_entry_levers_and_brakes() {
        let mut overall = base_metrics();
        overall.understeer_index = Some(0.24);
        overall.grip_saturation = push_sat(0.10);
        overall.corners = corners(0.30, 0.10);
        overall.balance_on_brake = band(1000, 0.40);
        let recs = recommend(&overall, &[], &Default::default());
        let bal = recs.iter().find(|r| r.area == "balance").unwrap();
        assert!(bal.advice.contains("corner entry"), "{}", bal.advice);
        assert_eq!(bal.implied.unwrap().family, Family::FrontRoll);
        let brakes = recs
            .iter()
            .find(|r| r.area == "brakes")
            .expect("brake secondary");
        assert!(brakes.advice.contains("rearward"), "{}", brakes.advice);
        assert_eq!(brakes.implied.unwrap().family, Family::Brakes);

        overall.surface_loose = true;
        let recs = recommend(&overall, &[], &Default::default());
        assert!(
            recs.iter().all(|r| r.area != "brakes"),
            "dirt trail-braking is technique"
        );
    }

    /// A braking band at or below BRAKE_REAR_ROTATE = bias too far rear,
    /// regardless of the positional entry mean (the McLaren bias-20 shape:
    /// entry still pushes, the braking zones rotate).
    #[test]
    fn rear_bias_rotation_fires_brakes_forward() {
        let mut overall = base_metrics();
        overall.balance_on_brake = band(2000, -0.06);
        overall.corners = corners(-0.11, 0.10);
        let recs = recommend(&overall, &[], &Default::default());
        let brakes = recs.iter().find(|r| r.area == "brakes").unwrap();
        assert_eq!(brakes.confidence, Confidence::Medium);
        assert!(brakes.advice.contains("forward"), "{}", brakes.advice);
        let implied = brakes.implied.unwrap();
        assert_eq!(implied.family, Family::Brakes);
        assert!(!implied.softer, "forward = higher %");
    }

    /// The rear-rotation gate stays silent on healthy bands, just above the
    /// threshold (the deliberate-slide provocation stint reads +0.026), and
    /// on dirt where brake rotation is technique.
    #[test]
    fn rear_bias_gate_edges_stay_silent() {
        let mut overall = base_metrics();
        overall.balance_on_brake = band(2000, 0.10);
        let recs = recommend(&overall, &[], &Default::default());
        assert!(recs.iter().all(|r| r.area != "brakes"));

        overall.balance_on_brake = band(2000, 0.026);
        let recs = recommend(&overall, &[], &Default::default());
        assert!(recs.iter().all(|r| r.area != "brakes"));

        overall.balance_on_brake = band(2000, -0.06);
        overall.surface_loose = true;
        let recs = recommend(&overall, &[], &Default::default());
        assert!(recs.iter().all(|r| r.area != "brakes"));

        // Under the sample gate the band is unsupported evidence.
        overall.surface_loose = false;
        overall.balance_on_brake = band(200, -0.21);
        let recs = recommend(&overall, &[], &Default::default());
        assert!(recs.iter().all(|r| r.area != "brakes"));
    }

    /// Episodic oversteer fires the stability card on a NEUTRAL average (the
    /// provocation-stint shape: mean -0.01, events everywhere), and the
    /// collapsed grip-margin ratio upgrades it to Medium.
    #[test]
    fn neutral_car_with_oversteer_events_gets_stability_card() {
        let mut overall = base_metrics();
        overall.understeer_index = Some(0.0);
        overall.transient_oversteer.clear_frac = 0.23;
        overall.transient_oversteer.on_power_frac = 0.15;
        overall.transient_oversteer.countersteer_frac = 0.10;
        overall.transient_oversteer.rear_first_frac = 0.10;
        overall.cornering_front_slip = Some(0.5);
        overall.cornering_rear_slip = Some(0.5);
        let recs = recommend(&overall, &[], &Default::default());
        let stab = recs.iter().find(|r| r.area == "stability").unwrap();
        assert_eq!(
            stab.confidence,
            Confidence::Medium,
            "margin 1.0 corroborates"
        );
        assert!(
            stab.evidence
                .iter()
                .any(|e| e.contains("grip-margin ratio")),
            "{:?}",
            stab.evidence
        );

        // Healthy margin (the rear-limited-RWD shape): events alone stay Low.
        overall.cornering_front_slip = Some(0.55);
        overall.cornering_rear_slip = Some(0.38);
        let recs = recommend(&overall, &[], &Default::default());
        let stab = recs.iter().find(|r| r.area == "stability").unwrap();
        assert_eq!(stab.confidence, Confidence::Low);
    }

    /// The stability card defers when another card owns the cause: a net-
    /// oversteer mean belongs to the balance card, a collapsed braking band
    /// to the brakes card.
    #[test]
    fn stability_card_defers_to_balance_and_brakes() {
        let mut overall = base_metrics();
        overall.understeer_index = Some(-0.21);
        overall.transient_oversteer.countersteer_frac = 0.07;
        let recs = recommend(&overall, &[], &Default::default());
        assert!(recs.iter().any(|r| r.area == "balance"));
        assert!(recs.iter().all(|r| r.area != "stability"));

        // GT-R 0% shape: neutral mean, events, braking band collapsed.
        let mut overall = base_metrics();
        overall.understeer_index = Some(0.02);
        overall.transient_oversteer.countersteer_frac = 0.06;
        overall.balance_on_brake = band(2000, -0.06);
        let recs = recommend(&overall, &[], &Default::default());
        assert!(recs.iter().any(|r| r.area == "brakes"));
        assert!(
            recs.iter().all(|r| r.area != "stability"),
            "brakes card owns the rotation"
        );
    }

    /// Exit-concentrated understeer under power is NOT a front-bar problem:
    /// front-driven cars get the diff-accel redirect, RWD gets rear stiffening.
    #[test]
    fn exit_understeer_redirects_by_drivetrain() {
        let mut overall = base_metrics();
        overall.understeer_index = Some(0.30);
        overall.grip_saturation = push_sat(0.10);
        overall.corners = corners(0.12, 0.35);
        overall.balance_on_throttle = band(5000, 0.30);
        overall.drivetrain_type = 2; // AWD
        let recs = recommend(&overall, &[], &Default::default());
        let bal = recs.iter().find(|r| r.area == "balance").unwrap();
        assert!(bal.advice.contains("front diff accel"), "{}", bal.advice);
        let implied = bal.implied.unwrap();
        assert_eq!(implied.family, Family::DiffAccel);
        assert!(implied.softer);
        assert!(
            bal.confidence <= Confidence::Medium,
            "diff redirect never High"
        );

        overall.drivetrain_type = 1; // RWD: no front diff to open
        let recs = recommend(&overall, &[], &Default::default());
        let bal = recs.iter().find(|r| r.area == "balance").unwrap();
        assert!(bal.advice.contains("stiffen the rear"), "{}", bal.advice);
        let implied = bal.implied.unwrap();
        assert_eq!(implied.family, Family::RearRoll);
        assert!(!implied.softer);
    }

    /// Entry-concentrated oversteer leads with rear diff decel (entry card);
    /// uniform oversteer keeps the classic rear-bar advice.
    #[test]
    fn entry_oversteer_leads_with_decel_lock() {
        let mut overall = base_metrics();
        overall.understeer_index = Some(-0.15);
        overall.corners = corners(-0.28, -0.06);
        let recs = recommend(&overall, &[], &Default::default());
        let bal = recs.iter().find(|r| r.area == "balance").unwrap();
        assert!(bal.advice.contains("rear diff decel"), "{}", bal.advice);
        let implied = bal.implied.unwrap();
        assert_eq!(implied.family, Family::DiffDecel);
        assert!(!implied.softer, "more decel lock = stiffer");

        overall.corners = corners(-0.15, -0.14);
        let recs = recommend(&overall, &[], &Default::default());
        let bal = recs.iter().find(|r| r.area == "balance").unwrap();
        assert!(bal.advice.contains("rear anti-roll bar"), "{}", bal.advice);
        assert_eq!(bal.implied.unwrap().family, Family::RearRoll);
    }

    /// A net-understeer car that flashes momentary oversteer (the RWD Ferrari
    /// signature) gets the averaged front-soften advice CAPPED with a
    /// counter-signal, plus a rear-grip rec: aero when the flashes live at
    /// speed, diff accel when they ride the throttle. Tarmac only.
    #[test]
    fn transient_oversteer_counters_the_average() {
        let mut overall = base_metrics();
        overall.understeer_index = Some(0.16);
        overall.grip_saturation = push_sat(0.20);
        overall.transient_oversteer = crate::analysis::metrics::TransientOversteer {
            clear_frac: 0.06,
            on_power_frac: 0.045,
            high_speed_frac: 0.053,
            rear_first_frac: 0.02,
            episodes: 150,
            countersteer_frac: 0.02,
            countersteer_episodes: 40,
        };
        let laps: Vec<StintMetrics> = (0..3)
            .map(|_| {
                let mut m = base_metrics();
                m.understeer_index = Some(0.16);
                m
            })
            .collect();
        let recs = recommend(&overall, &laps, &Default::default());
        let bal = recs.iter().find(|r| r.area == "balance").unwrap();
        assert_eq!(
            bal.confidence,
            Confidence::Medium,
            "capped from High by the counter-signal"
        );
        assert!(
            bal.evidence.iter().any(|e| e.contains("counter-signal")),
            "{:?}",
            bal.evidence
        );
        let stab = recs
            .iter()
            .find(|r| r.area == "stability")
            .expect("rear-grip rec");
        assert!(
            stab.advice.contains("rear aero"),
            "at-speed flashes -> aero: {}",
            stab.advice
        );
        assert_eq!(stab.implied.unwrap().family, Family::RearAero);

        // Same at-speed shape on a build with NO aero fitted: downforce is
        // not a lever, mechanical rear grip takes its place.
        let no_aero = Context {
            aero_tunable: Some(false),
            ..Default::default()
        };
        let recs = recommend(&overall, &laps, &no_aero);
        let stab = recs.iter().find(|r| r.area == "stability").unwrap();
        assert!(stab.advice.contains("no aero is fitted"), "{}", stab.advice);
        assert_eq!(stab.implied.unwrap().family, Family::RearRoll);

        // Throttle-riding flashes (the 570S shape) point at the diff instead.
        overall.transient_oversteer.on_power_frac = 0.08;
        overall.transient_oversteer.high_speed_frac = 0.05;
        let recs = recommend(&overall, &laps, &Default::default());
        let stab = recs.iter().find(|r| r.area == "stability").unwrap();
        assert!(stab.advice.contains("rear diff accel"), "{}", stab.advice);
        let implied = stab.implied.unwrap();
        assert_eq!(implied.family, Family::DiffAccel);
        assert!(implied.softer);

        // Dirt: rotation is technique, so no counter-signal, no rec.
        overall.surface_loose = true;
        let recs = recommend(&overall, &laps, &Default::default());
        assert!(recs.iter().all(|r| r.area != "stability"));
        assert_eq!(
            recs.iter()
                .find(|r| r.area == "balance")
                .unwrap()
                .confidence,
            Confidence::High
        );

        // Healthy AWD-tarmac levels stay silent.
        overall.surface_loose = false;
        overall.transient_oversteer = crate::analysis::metrics::TransientOversteer {
            clear_frac: 0.05,
            on_power_frac: 0.033,
            high_speed_frac: 0.03,
            rear_first_frac: 0.022,
            episodes: 200,
            countersteer_frac: 0.014,
            countersteer_episodes: 23,
        };
        let recs = recommend(&overall, &laps, &Default::default());
        assert!(recs.iter().all(|r| r.area != "stability"));

        // Counter-steer alone certifies the slides (driver corrections are
        // the strongest label) even when the slip-based gates stay under.
        overall.transient_oversteer.countersteer_frac = 0.05;
        let recs = recommend(&overall, &laps, &Default::default());
        let stab = recs
            .iter()
            .find(|r| r.area == "stability")
            .expect("countersteer gate");
        assert!(
            stab.evidence[0].contains("counter-steers")
                || recs.iter().any(|r| r.area == "balance"
                    && r.evidence.iter().any(|e| e.contains("counter-steers")))
        );
    }

    /// Bands are compound-relative: 155°F is cold for slicks, in-band for
    /// semi-slicks (the real Ferrari campaign temps), and HOT for rally rubber.
    #[test]
    fn temp_band_follows_the_compound() {
        let mut overall = base_metrics();
        for t in [
            &mut overall.tire_temp.fl,
            &mut overall.tire_temp.fr,
            &mut overall.tire_temp.rl,
            &mut overall.tire_temp.rr,
        ] {
            t.avg = 155.0;
        }
        let recs = recommend(
            &overall,
            &[],
            &Context {
                compound: Some("slick"),
                ..Default::default()
            },
        );
        assert!(
            recs.iter()
                .any(|r| r.area == "tires" && r.advice.contains("lower"))
        );
        let recs = recommend(
            &overall,
            &[],
            &Context {
                compound: Some("semi-slick"),
                ..Default::default()
            },
        );
        assert!(
            recs.iter().all(|r| r.area != "tires"),
            "155°F is in the semi-slick band"
        );
        for t in [
            &mut overall.tire_temp.fl,
            &mut overall.tire_temp.fr,
            &mut overall.tire_temp.rl,
            &mut overall.tire_temp.rr,
        ] {
            t.avg = 170.0;
        }
        let recs = recommend(
            &overall,
            &[],
            &Context {
                compound: Some("rally"),
                ..Default::default()
            },
        );
        let tires = recs.iter().find(|r| r.area == "tires").unwrap();
        assert!(
            tires.advice.contains("raise"),
            "170°F overheats rally rubber: {}",
            tires.advice
        );
        assert!(
            tires.evidence[0].contains("rally working band"),
            "{}",
            tires.evidence[0]
        );
        for t in [
            &mut overall.tire_temp.fl,
            &mut overall.tire_temp.fr,
            &mut overall.tire_temp.rl,
            &mut overall.tire_temp.rr,
        ] {
            t.avg = 155.0;
        }
        // No compound on file: legacy band, flagged as an assumption.
        let recs = recommend(&overall, &[], &Default::default());
        let tires = recs.iter().find(|r| r.area == "tires").unwrap();
        assert!(
            tires.evidence[0].contains("no tire compound on file"),
            "{}",
            tires.evidence[0]
        );
    }

    fn band(samples: usize, index: f32) -> crate::analysis::metrics::BandBalance {
        crate::analysis::metrics::BandBalance {
            samples,
            index: Some(index),
            rear_slip: Some(0.6),
        }
    }

    /// Understeer only above 85 mph with neutral low-speed balance = aero, and
    /// the balance rule must stay quiet (overall index diluted below MILD is
    /// not required; here overall is neutral so only aero speaks).
    #[test]
    fn high_speed_only_understeer_fires_aero_not_bars() {
        let mut overall = base_metrics();
        overall.understeer_index = Some(0.04);
        overall.balance_low_speed = band(2000, 0.02);
        overall.balance_high_speed = band(2000, 0.14);
        let recs = recommend(&overall, &[], &Default::default());
        let aero = recs.iter().find(|r| r.area == "aero").unwrap();
        assert!(aero.advice.contains("add front aero"), "{}", aero.advice);
        assert_eq!(aero.confidence, Confidence::Medium);
        assert_eq!(aero.implied.unwrap().family, Family::FrontAero);
        assert!(recs.iter().all(|r| r.area != "balance"));
    }

    #[test]
    fn high_speed_oversteer_asks_for_rear_wing() {
        let mut overall = base_metrics();
        overall.balance_low_speed = band(2000, -0.01);
        overall.balance_high_speed = band(2000, -0.13);
        let recs = recommend(&overall, &[], &Default::default());
        let aero = recs.iter().find(|r| r.area == "aero").unwrap();
        assert!(aero.advice.contains("add rear aero"), "{}", aero.advice);
        assert_eq!(aero.implied.unwrap().family, Family::RearAero);

        // No aero fitted: same signal, but the advice must not name a slider
        // the build doesn't have; ride height (rake) takes its place, Low.
        let no_aero = Context {
            aero_tunable: Some(false),
            ..Default::default()
        };
        let recs = recommend(&overall, &[], &no_aero);
        let aero = recs.iter().find(|r| r.area == "aero").unwrap();
        assert!(aero.advice.contains("ride height"), "{}", aero.advice);
        assert_eq!(aero.confidence, Confidence::Low);
        assert_eq!(aero.implied.unwrap().family, Family::RideHeight);
    }

    /// The real tarmac signature (Ford GT / McLaren): understeer at EVERY speed,
    /// stronger below 85 mph. Mechanical: aero must stay quiet and the balance
    /// rec must say the imbalance is concentrated at low speed.
    #[test]
    fn uniform_understeer_is_mechanical_not_aero() {
        let mut overall = base_metrics();
        overall.understeer_index = Some(0.24);
        overall.grip_saturation = push_sat(0.10);
        overall.balance_low_speed = band(9000, 0.37);
        overall.balance_high_speed = band(30000, 0.20);
        let recs = recommend(&overall, &[], &Default::default());
        assert!(recs.iter().all(|r| r.area != "aero"));
        let balance = recs.iter().find(|r| r.area == "balance").unwrap();
        assert!(
            balance
                .evidence
                .iter()
                .any(|e| e.contains("concentrated at low speed")),
            "{:?}",
            balance.evidence
        );
    }

    #[test]
    fn starved_band_stays_quiet() {
        let mut overall = base_metrics();
        overall.balance_low_speed = band(2000, 0.0);
        overall.balance_high_speed = band(100, 0.30); // too few samples to trust
        let recs = recommend(&overall, &[], &Default::default());
        assert!(recs.iter().all(|r| r.area != "aero"));
    }

    /// Rear breaks away specifically under power (RWD): diff accel advice.
    #[test]
    fn power_oversteer_fires_diff_accel() {
        let mut overall = base_metrics();
        overall.balance_on_throttle = band(2000, -0.12);
        overall.balance_off_throttle = band(2000, 0.02);
        let recs = recommend(&overall, &[], &Default::default());
        let diff = recs.iter().find(|r| r.area == "differential").unwrap();
        assert!(
            diff.advice.contains("rear differential acceleration lock"),
            "{}",
            diff.advice
        );
        assert_eq!(diff.implied.unwrap().family, Family::DiffAccel);
        assert!(diff.implied.unwrap().softer);
    }

    /// AWD power understeer: front diff / center torque advice.
    #[test]
    fn power_understeer_on_awd_names_the_front_diff() {
        let mut overall = base_metrics();
        overall.drivetrain_type = 2;
        overall.balance_on_throttle = band(2000, 0.15);
        overall.balance_off_throttle = band(2000, 0.02);
        let recs = recommend(&overall, &[], &Default::default());
        let diff = recs.iter().find(|r| r.area == "differential").unwrap();
        assert!(
            diff.advice.contains("front differential"),
            "{}",
            diff.advice
        );
        assert!(
            diff.evidence
                .iter()
                .any(|e| e.contains("center torque rearward")),
            "{:?}",
            diff.evidence
        );
    }

    /// The real dirt pattern (Fiesta/Audi): big on-off shift but on-throttle
    /// index near zero: throttle rotation as technique, not a diff problem.
    #[test]
    fn dirt_throttle_rotation_stays_quiet() {
        let mut overall = base_metrics();
        overall.surface_loose = true;
        overall.balance_on_throttle = band(9000, -0.06);
        overall.balance_off_throttle = band(5000, 0.57);
        let recs = recommend(&overall, &[], &Default::default());
        assert!(recs.iter().all(|r| r.area != "differential"), "{recs:?}");
    }

    /// Understeer everywhere with a small on-off shift (the healthy tarmac
    /// signature) must NOT read as power understeer; the shift gate protects it.
    #[test]
    fn uniform_understeer_is_not_power_understeer() {
        let mut overall = base_metrics();
        overall.drivetrain_type = 2;
        overall.understeer_index = Some(0.27);
        overall.balance_on_throttle = band(27000, 0.26);
        overall.balance_off_throttle = band(16000, 0.30);
        let recs = recommend(&overall, &[], &Default::default());
        assert!(recs.iter().all(|r| r.area != "differential"));
    }

    #[test]
    fn rwd_wheelspin_names_the_rear() {
        let mut overall = base_metrics();
        overall.wheelspin_frac = Some(0.2);
        let recs = recommend(&overall, &[], &Default::default());
        let traction = recs.iter().find(|r| r.area == "traction").unwrap();
        assert_eq!(traction.confidence, Confidence::High);
        assert!(traction.advice.contains("rear-axle"), "{}", traction.advice);
    }

    /// Open-diff signature (the R12 0%-lock stint: inside-only 11.0%, both
    /// 0.2%): the direction flips to ADD lock instead of the old blanket
    /// "reduce", which was impossible at 0% and measured the wrong way.
    #[test]
    fn inside_only_spin_flips_traction_to_add_lock() {
        let mut overall = base_metrics();
        overall.wheelspin_frac = Some(0.09);
        overall.traction_spin = TractionSpin {
            samples: 2000,
            inside_only_frac: 0.11,
            both_frac: 0.002,
        };
        let recs = recommend(&overall, &[], &Default::default());
        let traction = recs.iter().find(|r| r.area == "traction").unwrap();
        assert!(
            traction.advice.contains("add rear diff accel"),
            "{}",
            traction.advice
        );
        let implied = traction.implied.unwrap();
        assert_eq!(implied.family, Family::DiffAccel);
        assert!(!implied.softer, "add lock = value up");
        assert_eq!(traction.confidence, Confidence::Medium);
    }

    /// Locked-diff signature (both rears in breakaway together) keeps the
    /// reduce direction; oversteer events are quoted when present.
    #[test]
    fn both_rear_spin_keeps_reduce_direction() {
        let mut overall = base_metrics();
        overall.wheelspin_frac = Some(0.12);
        overall.traction_spin = TractionSpin {
            samples: 2000,
            inside_only_frac: 0.02,
            both_frac: 0.032,
        };
        let recs = recommend(&overall, &[], &Default::default());
        let traction = recs.iter().find(|r| r.area == "traction").unwrap();
        assert!(traction.advice.contains("reduce"), "{}", traction.advice);
        assert!(traction.implied.unwrap().softer);
    }

    /// Wheelspin without either symmetry signature no longer implies a diff
    /// direction: the rec survives (softer springs help regardless) but
    /// carries no journal-reconcilable direction claim.
    #[test]
    fn ambiguous_spin_pattern_is_non_directional() {
        let mut overall = base_metrics();
        overall.wheelspin_frac = Some(0.09);
        overall.traction_spin = TractionSpin {
            samples: 2000,
            inside_only_frac: 0.03,
            both_frac: 0.005,
        };
        let recs = recommend(&overall, &[], &Default::default());
        let traction = recs.iter().find(|r| r.area == "traction").unwrap();
        assert!(traction.implied.is_none());
        assert_eq!(traction.confidence, Confidence::Low);
    }

    /// Dirt never sees the symmetry gates (both-rears reads 17-35% on every
    /// dirt stint regardless of setup): legacy direction stands.
    #[test]
    fn dirt_keeps_legacy_traction_direction() {
        let mut overall = base_metrics();
        overall.surface_loose = true;
        overall.wheelspin_frac = Some(0.35);
        overall.traction_spin = TractionSpin {
            samples: 2000,
            inside_only_frac: 0.02,
            both_frac: 0.25,
        };
        let recs = recommend(&overall, &[], &Default::default());
        let traction = recs.iter().find(|r| r.area == "traction").unwrap();
        assert!(traction.advice.contains("reduce"), "{}", traction.advice);
        assert!(traction.implied.unwrap().softer);
    }

    #[test]
    fn limiter_time_triggers_final_drive_advice() {
        let mut overall = base_metrics();
        overall.gears.limiter_frac = 0.05;
        overall.gears.top_gear = 6;
        overall.gears.top_gear_max_rpm = 7900.0;
        let recs = recommend(&overall, &[], &Default::default());
        let gearing = recs.iter().find(|r| r.area == "gearing").unwrap();
        assert!(
            gearing.advice.contains("lengthen the final drive"),
            "{}",
            gearing.advice
        );
    }

    /// The "too long" extreme caught on real data (Ferrari 330): the car lives
    /// in top gear but the rev-range top goes unused -> shorten the final drive.
    #[test]
    fn unused_rev_range_suggests_shorter_final_drive() {
        let mut overall = base_metrics();
        overall.gears.limiter_frac = 0.0;
        overall.gears.top_gear = 8;
        overall.gears.top_gear_max_rpm = 8928.0; // one downhill burst near redline...
        overall.gears.top_gear_high_rev_frac = 0.001; // ...but no sustained use
        overall.gears.time_frac = vec![(6, 0.15), (7, 0.52), (8, 0.20)];
        let recs = recommend(&overall, &[], &Default::default());
        let gearing = recs.iter().find(|r| r.area == "gearing").unwrap();
        assert!(
            gearing.advice.contains("shorten the final drive"),
            "{}",
            gearing.advice
        );
        assert!(
            gearing.evidence[0].contains("20.0% of the stint"),
            "{}",
            gearing.evidence[0]
        );
    }

    /// A route that barely reaches top gear says nothing about the stack: the
    /// converged Ford GT tune reads 3-4.6% top-gear time at mid revs, so silent.
    #[test]
    fn marginal_top_gear_time_stays_quiet() {
        let mut overall = base_metrics();
        overall.gears.limiter_frac = 0.0;
        overall.gears.top_gear = 6;
        overall.gears.top_gear_max_rpm = 5500.0; // 69% of the 8000 redline
        overall.gears.top_gear_high_rev_frac = 0.0;
        overall.gears.time_frac = vec![(4, 0.5), (5, 0.4), (6, 0.04)];
        let recs = recommend(&overall, &[], &Default::default());
        assert!(recs.iter().all(|r| r.area != "gearing"));
    }

    /// Healthy tunes rev out in top gear (Fiesta GRC: 17.7% of time in top,
    /// 8.7% of it above 90% redline): silent even with heavy top-gear use.
    #[test]
    fn healthy_rev_usage_stays_quiet() {
        let mut overall = base_metrics();
        overall.gears.limiter_frac = 0.0;
        overall.gears.top_gear = 6;
        overall.gears.top_gear_max_rpm = 8211.0;
        overall.gears.top_gear_high_rev_frac = 0.087;
        overall.gears.time_frac = vec![(4, 0.36), (5, 0.34), (6, 0.18)];
        let recs = recommend(&overall, &[], &Default::default());
        assert!(recs.iter().all(|r| r.area != "gearing"));
    }

    #[test]
    fn bottoming_names_the_axle() {
        let mut overall = base_metrics();
        overall.suspension.rl.bottomed_frac = 0.06;
        let recs = recommend(&overall, &[], &Default::default());
        let susp = recs.iter().find(|r| r.area == "suspension").unwrap();
        assert!(susp.advice.contains("rear springs"), "{}", susp.advice);
    }

    fn susp(reversals: f32, topped: f32) -> crate::analysis::metrics::SuspensionStats {
        crate::analysis::metrics::SuspensionStats {
            avg: 0.4,
            bottomed_frac: 0.0,
            topped_frac: topped,
            reversals_per_sec: reversals,
            // 50 m/s equivalent: the library's typical tarmac pace.
            reversals_per_100m: reversals * 2.0,
        }
    }

    /// The spatial gate catches bump overdamping at speeds where the raw /s
    /// gate is blind: 6.1/s clears OVERDAMPED_BUMP_REV, but 9.4 per 100m is
    /// under the healthy tarmac floor (11-16 at any speed).
    #[test]
    fn spatial_reversal_gate_is_speed_robust() {
        let fast_overdamped = crate::analysis::metrics::SuspensionStats {
            avg: 0.3,
            bottomed_frac: 0.0,
            topped_frac: 0.15,
            reversals_per_sec: 6.1,
            reversals_per_100m: 9.4,
        };
        let mut overall = base_metrics();
        overall.suspension = Corners {
            fl: fast_overdamped,
            fr: fast_overdamped,
            rl: susp(7.5, 0.03),
            rr: susp(7.4, 0.03),
        };
        let recs = recommend(&overall, &[], &Default::default());
        let damp = recs.iter().find(|r| r.area == "damping").unwrap();
        assert!(
            damp.advice.contains("front bump damping"),
            "{}",
            damp.advice
        );
        assert!(
            damp.evidence.iter().any(|e| e.contains("per 100m")),
            "{:?}",
            damp.evidence
        );
    }

    #[test]
    fn underdamped_axle_gets_more_damping_advice() {
        let mut overall = base_metrics();
        overall.suspension = Corners {
            fl: susp(7.5, 0.18),
            fr: susp(7.4, 0.19),
            rl: susp(5.6, 0.03),
            rr: susp(5.5, 0.03),
        };
        let recs = recommend(&overall, &[], &Default::default());
        let damping: Vec<_> = recs.iter().filter(|r| r.area == "damping").collect();
        assert_eq!(damping.len(), 1, "only the front fires");
        assert!(
            damping[0].advice.contains("increase front damping"),
            "{}",
            damping[0].advice
        );
        assert_eq!(damping[0].confidence, Confidence::High);
    }

    #[test]
    fn overdamped_axle_gets_low_confidence_softening_advice() {
        let mut overall = base_metrics();
        overall.suspension = Corners {
            fl: susp(2.1, 0.05),
            fr: susp(2.3, 0.06),
            rl: susp(2.0, 0.02),
            rr: susp(2.2, 0.02),
        };
        let recs = recommend(&overall, &[], &Default::default());
        let damping: Vec<_> = recs.iter().filter(|r| r.area == "damping").collect();
        assert_eq!(damping.len(), 2);
        assert!(damping.iter().all(|r| r.confidence == Confidence::Low));
        assert!(damping.iter().all(|r| r.advice.contains("softer")));
    }

    /// Real min-damping-on-dirt numbers must fire underdamped advice (and the
    /// front's 37% topped stays below the 40% strong-overdamped line).
    #[test]
    fn underdamped_dirt_fires_increase_damping() {
        let mut overall = base_metrics();
        overall.surface_loose = true;
        overall.suspension = Corners {
            fl: susp(17.6, 0.37),
            fr: susp(18.7, 0.32),
            rl: susp(19.2, 0.21),
            rr: susp(19.5, 0.20),
        };
        let recs = recommend(&overall, &[], &Default::default());
        let damping: Vec<_> = recs.iter().filter(|r| r.area == "damping").collect();
        assert_eq!(damping.len(), 2, "{damping:?}");
        assert!(damping.iter().all(|r| r.advice.contains("increase")));
        assert!(damping.iter().all(|r| r.confidence == Confidence::Medium));
    }

    /// Regression from the real dirt captures: healthy dirt reads 12-16 rev/s and
    /// up to ~26% topped; tarmac thresholds must not fire on it.
    #[test]
    fn healthy_dirt_damping_stays_quiet() {
        let mut overall = base_metrics();
        overall.surface_loose = true;
        overall.surface_rumble_avg = 0.13;
        overall.suspension = Corners {
            fl: susp(15.6, 0.26),
            fr: susp(16.2, 0.21),
            rl: susp(11.8, 0.10),
            rr: susp(12.0, 0.09),
        };
        let recs = recommend(&overall, &[], &Default::default());
        assert!(recs.iter().all(|r| r.area != "damping"), "{recs:?}");
    }

    /// The real overdamped-dirt signature: wheels at full extension most of the
    /// stint fires the strong rule regardless of surface baseline.
    #[test]
    fn extreme_topped_fires_reduce_damping() {
        let mut overall = base_metrics();
        overall.surface_loose = true;
        overall.rpm_flutter = Some(12000.0);
        overall.suspension = Corners {
            fl: susp(2.6, 0.84),
            fr: susp(3.6, 0.74),
            rl: susp(2.9, 0.77),
            rr: susp(3.1, 0.66),
        };
        let recs = recommend(&overall, &[], &Default::default());
        let damping: Vec<_> = recs.iter().filter(|r| r.area == "damping").collect();
        assert_eq!(damping.len(), 2);
        assert!(damping.iter().all(|r| r.confidence == Confidence::High));
        assert!(damping.iter().all(|r| r.advice.contains("reduce")));
        assert!(
            damping[0]
                .evidence
                .iter()
                .any(|e| e.contains("rpm flutter"))
        );
    }

    /// The bump-only-max signature (McLaren F1 real A/B): suppressed
    /// articulation plus heavy full-extension time fires at Medium even
    /// though the reversal rate alone looks merely low.
    #[test]
    fn bump_overdamping_fires_on_the_compound_signature() {
        let mut overall = base_metrics();
        overall.suspension = Corners {
            fl: susp(4.7, 0.157),
            fr: susp(5.0, 0.143),
            rl: susp(4.4, 0.066),
            rr: susp(4.8, 0.062),
        };
        let recs = recommend(&overall, &[], &Default::default());
        let damping: Vec<_> = recs.iter().filter(|r| r.area == "damping").collect();
        assert_eq!(damping.len(), 1, "front only: {damping:?}");
        assert!(
            damping[0].advice.contains("reduce front bump damping"),
            "{}",
            damping[0].advice
        );
        assert_eq!(damping[0].confidence, Confidence::Medium);
    }

    /// A healthy softly-damped car (real Acura: 3.5-4.5 rev/s, topped <3%)
    /// must not trip the bump-overdamping tier.
    #[test]
    fn healthy_low_rev_car_stays_quiet() {
        let mut overall = base_metrics();
        overall.suspension = Corners {
            fl: susp(4.1, 0.026),
            fr: susp(4.5, 0.022),
            rl: susp(3.5, 0.020),
            rr: susp(4.4, 0.009),
        };
        let recs = recommend(&overall, &[], &Default::default());
        assert!(recs.iter().all(|r| r.area != "damping"), "{recs:?}");
    }

    #[test]
    fn healthy_damping_stays_quiet() {
        let mut overall = base_metrics();
        overall.suspension = Corners {
            fl: susp(5.8, 0.03),
            fr: susp(5.6, 0.03),
            rl: susp(5.9, 0.01),
            rr: susp(5.5, 0.01),
        };
        let recs = recommend(&overall, &[], &Default::default());
        assert!(recs.iter().all(|r| r.area != "damping"));
    }

    #[test]
    fn quiet_session_produces_no_advice() {
        let recs = recommend(&base_metrics(), &[], &Default::default());
        assert!(recs.is_empty(), "nothing wrong -> nothing to say: {recs:?}");
    }
}
