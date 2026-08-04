//! Text rendering of stint metrics. Observations only, no advice.

use super::metrics::{StintMetrics, stint_metrics};
use super::{GapKind, LapSlice, Stint, classify_gaps, driving_segments, split_laps};
use crate::telemetry::packet::{class_name, drivetrain_name};
use crate::util::{format_lap_time, speed_unit, speed_val, temp_unit, temp_val};
use std::fmt::Write;
use std::path::Path;

/// The complete text report for a session file: rewind/restart notes, per-stint
/// observations, lap tables. Shared by the CLI (`tuners analyze`) and the
/// dashboard's report endpoint.
pub fn full_session_report(path: &Path) -> Result<String, String> {
    let session = Stint::load(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut out = String::new();
    if session.decode_errors > 0 {
        writeln!(
            out,
            "warning: {} packets failed to decode",
            session.decode_errors
        )
        .unwrap();
    }
    let segments = driving_segments(&session.frames, 5.0);
    if segments.is_empty() {
        return Err(format!(
            "no driving stints of 5s or longer found ({} frames total)",
            session.frames.len()
        ));
    }
    writeln!(out, "{}: {} stint(s)\n", path.display(), segments.len()).unwrap();
    for gap in classify_gaps(&session.frames) {
        match gap.kind {
            GapKind::Rewind {
                race_t_before,
                race_t_after,
            } => writeln!(
                out,
                "note: rewind on lap {} (race clock {:.1}s -> {:.1}s): superseded \
                 driving erased, the kept retry counts",
                gap.resume_lap + 1,
                race_t_before,
                race_t_after,
            )
            .unwrap(),
            GapKind::Restart => writeln!(out, "note: session restart detected").unwrap(),
            GapKind::ClockRan { skipped_s } => writeln!(
                out,
                "note: race clock ran {skipped_s:.0}s through a race-off block \
                 (results screen or similar); the stint is split there",
            )
            .unwrap(),
            GapKind::Pause => {}
        }
    }
    writeln!(out).unwrap();
    // Grip curves self-fitted from this recording alone (analyzing a single
    // stint is an intentional act: the label owns the missing campaign
    // context). Dirt curves are their own regime — deferred, tarmac only.
    let per_seg: Vec<Vec<super::grip::GripSample>> = segments
        .iter()
        .map(|s| super::grip::cornering_samples(s))
        .collect();
    let mut metrics: Vec<StintMetrics> = segments.iter().map(|s| stint_metrics(s)).collect();
    let pooled: Vec<super::grip::GripSample> = metrics
        .iter()
        .zip(&per_seg)
        .filter(|(m, _)| !m.surface_loose)
        .flat_map(|(_, s)| s.iter().copied())
        .collect();
    if let Some(curves) = super::grip::fit_curves(&pooled) {
        for (m, samples) in metrics.iter_mut().zip(&per_seg) {
            if !m.surface_loose {
                m.grip_saturation =
                    super::grip::occupancy(samples, &curves, super::grip::CurveSource::SelfFit);
            }
        }
    }
    for (i, (stint, metrics)) in segments.iter().zip(&metrics).enumerate() {
        writeln!(out, "{}", render_stint(i + 1, metrics)).unwrap();
        let laps = split_laps(stint);
        if laps.len() > 1 {
            writeln!(out, "{}", render_laps(&laps)).unwrap();
        }
    }
    Ok(out)
}

pub fn render_recommendations(recs: &[crate::advice::recommend::Recommendation]) -> String {
    let mut out = String::new();
    if recs.is_empty() {
        writeln!(out, "no recommendations: nothing in this session stood out").unwrap();
        return out;
    }
    for r in recs {
        // Probes are data requests, not optimization claims; the tag says so
        // up front instead of burying it in the advice text.
        let tag = if r.probe {
            format!("probe: {}", r.confidence.label())
        } else {
            r.confidence.label().to_string()
        };
        match &r.suggestion {
            Some(sg) => writeln!(out, "[{tag}] {} - {}", sg, r.advice).unwrap(),
            None => writeln!(out, "[{tag}] {}: {}", r.area, r.advice).unwrap(),
        }
        for e in &r.evidence {
            writeln!(out, "    · {e}").unwrap();
        }
    }
    out
}

/// Lap table for a stint. Standing-start laps (rivals out laps) are labelled and
/// excluded from the best-lap comparison; they are not comparable to flying laps.
pub fn render_laps(laps: &[LapSlice]) -> String {
    let mut out = String::new();
    let best_flying = laps
        .iter()
        .filter(|l| !l.standing_start)
        .filter_map(|l| l.time_s)
        .fold(f32::INFINITY, f32::min);

    writeln!(out, "  laps").unwrap();
    for lap in laps {
        let m = stint_metrics(lap.frames);
        let label = format!("lap {}", lap.number + 1);
        let time = match lap.time_s {
            Some(t) => format!("{:>9}", format_lap_time(t)),
            None => format!("({:.1}s, incomplete)", m.duration_s),
        };
        let compare = match (lap.time_s, lap.standing_start) {
            (_, true) if lap.point_to_point => " | point-to-point run".to_string(),
            (_, true) => " | standing start".to_string(),
            (Some(t), false) if t <= best_flying => " | best".to_string(),
            (Some(t), false) => format!(" | +{:.2}s vs best", t - best_flying),
            (None, false) => String::new(),
        };
        let extras = [
            m.wheelspin_frac.map(|w| format!("spin {:.1}%", w * 100.0)),
            m.lockup_frac
                .map(|l| format!("brake-slip {:.1}%", l * 100.0)),
            m.understeer_index.map(|i| format!("balance {i:+.2}")),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" | ");
        writeln!(
            out,
            "    {label}: {time}{compare} | max {:.0} {} | {extras}",
            speed_val(m.max_speed),
            speed_unit(),
        )
        .unwrap();
    }
    out
}

pub fn render_stint(index: usize, m: &StintMetrics) -> String {
    let mut out = String::new();
    let pct = |f: f32| format!("{:.1}%", f * 100.0);

    writeln!(
        out,
        "stint {index}: {:.1}s | {:.2} mi | avg {:.0} {su} | max {:.0} {su}",
        m.duration_s,
        m.distance_m / 1609.34,
        speed_val(m.avg_speed),
        speed_val(m.max_speed),
        su = speed_unit(),
    )
    .unwrap();
    writeln!(
        out,
        "{}: class {} | PI {} | {} | {} cyl | redline {:.0} | surface: {}",
        crate::cars::car_label(m.car_ordinal),
        class_name(m.car_class),
        m.car_performance_index,
        drivetrain_name(m.drivetrain_type),
        m.num_cylinders,
        m.redline,
        if m.surface_loose { "loose" } else { "tarmac" },
    )
    .unwrap();

    let t = &m.tire_temp;
    let tv = temp_val;
    writeln!(out, "\n  tires ({} avg/max)", temp_unit()).unwrap();
    writeln!(
        out,
        "    FL {:>5.0}/{:<5.0}  FR {:>5.0}/{:<5.0}",
        tv(t.fl.avg),
        tv(t.fl.max),
        tv(t.fr.avg),
        tv(t.fr.max)
    )
    .unwrap();
    writeln!(
        out,
        "    RL {:>5.0}/{:<5.0}  RR {:>5.0}/{:<5.0}",
        tv(t.rl.avg),
        tv(t.rl.max),
        tv(t.rr.avg),
        tv(t.rr.max)
    )
    .unwrap();
    let front = (t.fl.avg + t.fr.avg) / 2.0;
    let rear = (t.rl.avg + t.rr.avg) / 2.0;
    let left = (t.fl.avg + t.rl.avg) / 2.0;
    let right = (t.fr.avg + t.rr.avg) / 2.0;
    // Temperature DIFFERENCES scale by 1/1.8 into °C but take no +32 offset.
    let dt = |a: f32, b: f32| (tv(a) - tv(b)).abs();
    writeln!(
        out,
        "    front {} {:.0}{tu} vs rear | {} side {:.0}{tu} hotter",
        if front >= rear {
            "hotter by"
        } else {
            "cooler by"
        },
        dt(front, rear),
        if left >= right { "left" } else { "right" },
        dt(left, right),
        tu = temp_unit(),
    )
    .unwrap();

    writeln!(out, "\n  grip").unwrap();
    let s = &m.slip_frac;
    writeln!(
        out,
        "    time over 100% slip: FL {} FR {} RL {} RR {}",
        pct(s.fl),
        pct(s.fr),
        pct(s.rl),
        pct(s.rr),
    )
    .unwrap();
    if let Some(w) = m.wheelspin_frac {
        writeln!(out, "    wheelspin on throttle: {}", pct(w)).unwrap();
    }
    let ts = &m.traction_spin;
    if ts.samples > 0 {
        writeln!(
            out,
            "    rear spin symmetry (on-throttle cornering): inside-only {} | \
             both rears {} (open diffs spin the unloaded inside wheel alone; \
             locked diffs drag both into breakaway)",
            pct(ts.inside_only_frac),
            pct(ts.both_frac),
        )
        .unwrap();
    }
    if let Some(l) = m.lockup_frac {
        // With ABS on, sustained slip at the limit is normal threshold braking.
        writeln!(out, "    braking at/over slip limit: {}", pct(l)).unwrap();
    }
    let dd = &m.diff_drag;
    if let (Some(ro), Some(rn)) = (dd.rear_off, dd.rear_on) {
        let conv = |c: Option<f32>| c.map(|c| format!("{c:.2}")).unwrap_or_else(|| "-".into());
        writeln!(
            out,
            "    wheel-speed split while cornering (outer-inner): rear {:+.3} \
             off-throttle / {:+.3} on | rear/front ratio {} / {} (the front is a \
             free-rolling reference only when undriven: ~1.0 off = open decel \
             diff, toward 0 = locked; negative on-throttle = inside spinning up)",
            ro,
            rn,
            conv(dd.conv_off()),
            conv(dd.conv_on()),
        )
        .unwrap();
    }
    match (
        m.understeer_index,
        m.cornering_front_slip,
        m.cornering_rear_slip,
    ) {
        (Some(idx), Some(front), Some(rear)) => {
            writeln!(
                out,
                "    balance while cornering ({} of stint): {} {:+.2} \
                 (front at {:.0}% of grip limit, rear {:.0}%)",
                pct(m.cornering_frac),
                if idx > 0.05 {
                    "understeer"
                } else if idx < -0.05 {
                    "oversteer"
                } else {
                    "neutral"
                },
                idx,
                front * 100.0,
                rear * 100.0,
            )
            .unwrap();
            if let Some(ratio) = m.margin_ratio() {
                writeln!(
                    out,
                    "    grip margin: front runs {ratio:.1}x closer to its limit \
                     (drivers settle near 1.5-1.7x; lower = the rear is working \
                     relatively harder)",
                )
                .unwrap();
            }
            if let Some(gs) = &m.grip_saturation {
                let mut line = format!(
                    "    front saturation: push {} of cornering (front at its grip \
                     limit, rear with spare) | slide {} (both at the limit)",
                    pct(gs.push_frac),
                    pct(gs.slide_frac),
                );
                if let Some(u) = gs.rear_use_at_push {
                    write!(
                        line,
                        " | rear at {:.0}% of its limit while pushing",
                        u * 100.0
                    )
                    .unwrap();
                }
                writeln!(out, "{line}").unwrap();
                let source = match gs.source {
                    crate::analysis::grip::CurveSource::Campaign => "campaign-pooled grip curve",
                    crate::analysis::grip::CurveSource::CarPool => {
                        "grip curve pooled across this car's recordings"
                    }
                    crate::analysis::grip::CurveSource::SelfFit => {
                        "grip curve fitted from this recording alone — indicative only"
                    }
                };
                let mut note = format!("      ({source}");
                if gs.banded {
                    write!(note, "; speed-banded: aero-significant car").unwrap();
                }
                if gs.coverage < 0.995 {
                    write!(note, "; covers {} of cornering", pct(gs.coverage)).unwrap();
                }
                writeln!(out, "{note})").unwrap();
            }
        }
        _ => writeln!(out, "    no significant cornering in this stint").unwrap(),
    }
    let band = |b: &crate::analysis::metrics::BandBalance| match b.index {
        Some(idx) => format!("{idx:+.2} ({} samples)", b.samples),
        None => "–".into(),
    };
    // The 85 mph band boundary, in display units (38.02 m/s canonical).
    let band_mph = speed_val(38.02);
    let su = speed_unit();
    if m.understeer_index.is_some() {
        writeln!(
            out,
            "    balance by speed: {} below {band_mph:.0} {su} | {} above",
            band(&m.balance_low_speed),
            band(&m.balance_high_speed),
        )
        .unwrap();
        writeln!(
            out,
            "    balance by throttle: {} on | {} off | {} braking",
            band(&m.balance_on_throttle),
            band(&m.balance_off_throttle),
            band(&m.balance_on_brake),
        )
        .unwrap();
    }
    let os = &m.transient_oversteer;
    if m.understeer_index.is_some() && os.episodes > 0 {
        writeln!(
            out,
            "    oversteer flashes: {} of cornering ({} episodes) | on power {} | \
             >={band_mph:.0} {su} {} | rear-first at limit {}",
            pct(os.clear_frac),
            os.episodes,
            pct(os.on_power_frac),
            pct(os.high_speed_frac),
            pct(os.rear_first_frac),
        )
        .unwrap();
        writeln!(
            out,
            "    counter-steer: {} of cornering ({} episodes), the driver's own \
             slide corrections",
            pct(os.countersteer_frac),
            os.countersteer_episodes,
        )
        .unwrap();
    }
    if let Some(c) = &m.corners {
        writeln!(
            out,
            "    corners: {} events | avg apex {:.0} {su} | balance {} entry | {} exit",
            c.corners,
            speed_val(c.avg_apex_speed),
            band(&c.entry),
            band(&c.exit),
        )
        .unwrap();
        writeln!(
            out,
            "    entry by pedal: {} trail-braking | {} coasting/turn-in",
            band(&c.entry_braking),
            band(&c.entry_coasting),
        )
        .unwrap();
    }

    writeln!(
        out,
        "\n  suspension (normalized travel avg | bottomed | topped | reversals/s | per 100m)"
    )
    .unwrap();
    for (label, s) in [
        ("FL", m.suspension.fl),
        ("FR", m.suspension.fr),
        ("RL", m.suspension.rl),
        ("RR", m.suspension.rr),
    ] {
        writeln!(
            out,
            "    {label} {:.2} | {} | {} | {:.1}/s | {:.1}/100m",
            s.avg,
            pct(s.bottomed_frac),
            pct(s.topped_frac),
            s.reversals_per_sec,
            s.reversals_per_100m,
        )
        .unwrap();
    }

    let dp = &m.damper_phase;
    if let (Some(ef), Some(er), Some(vf), Some(vr)) = (
        dp.ext_share_front,
        dp.ext_share_rear,
        dp.vratio_front,
        dp.vratio_rear,
    ) {
        writeln!(
            out,
            "    damper phase: extension {} of motion front / {} rear | extension \
             speed {:.2}x compression front / {:.2}x rear (healthy tarmac \
             0.76-0.84; maxed rebound measured 0.59)",
            pct(ef),
            pct(er),
            vf,
            vr,
        )
        .unwrap();
    }
    if let (Some(gf), Some(gr)) = (m.roll_use.grad_front, m.roll_use.grad_rear) {
        writeln!(
            out,
            "    roll gradient: {gf:.2} mm/(m/s²) front / {gr:.2} rear | mean \
             compression {:.1} mm (roll responds to large bar/spring changes; \
             compression rises with soft springs or rebound packing)",
            m.roll_use.jounce_mm,
        )
        .unwrap();
    }

    if m.transitions.events + m.transitions.timeouts > 0 {
        write!(
            out,
            "    transitions: {} flick(s)",
            m.transitions.events + m.transitions.timeouts
        )
        .unwrap();
        if let Some(lag) = m.transitions.median_lag_s {
            write!(out, " | steer->yaw crossover median {lag:.2}s").unwrap();
        }
        if m.transitions.timeouts > 0 {
            write!(out, " | {} never crossed", m.transitions.timeouts).unwrap();
        }
        writeln!(out).unwrap();
    }

    if m.kerbs.events > 0 {
        writeln!(
            out,
            "    kerbs: {} strike(s) | {} of stint | while striking: bottomed {} / topped {}",
            m.kerbs.events,
            pct(m.kerbs.time_frac),
            pct(m.kerbs.bottomed_frac),
            pct(m.kerbs.topped_frac),
        )
        .unwrap();
    }

    if let Some(dive) = m.brake_dive_front {
        writeln!(
            out,
            "    brake dive: front travel {dive:+.2} vs on-throttle (uncalibrated \
             measurement; front aero also compresses at braking speeds)",
        )
        .unwrap();
    }
    if m.jumps > 0 {
        writeln!(
            out,
            "    airborne: {} jump/crest event(s); {} landing bottoming sample(s) \
             excluded from the stats above",
            m.jumps, m.landing_bottomed_excluded,
        )
        .unwrap();
    }

    writeln!(out, "\n  gearing").unwrap();
    let per_gear: Vec<String> = m
        .gears
        .time_frac
        .iter()
        .map(|(g, f)| format!("{g}:{}", pct(*f)))
        .collect();
    writeln!(out, "    time per gear: {}", per_gear.join("  ")).unwrap();
    match m.gears.avg_upshift_rpm {
        Some(rpm) => writeln!(
            out,
            "    {} upshifts, avg at {:.0} rpm ({:.0}% of redline)",
            m.gears.upshifts,
            rpm,
            100.0 * rpm / m.redline.max(1.0),
        )
        .unwrap(),
        None => writeln!(out, "    no upshifts").unwrap(),
    }
    writeln!(
        out,
        "    top gear used: {} | {} of top-gear time >=90% redline (max {:.0} rpm) | time on limiter: {} ({} in a held gear)",
        m.gears.top_gear,
        pct(m.gears.top_gear_high_rev_frac),
        m.gears.top_gear_max_rpm,
        pct(m.gears.limiter_frac),
        pct(m.gears.limiter_held_frac),
    )
    .unwrap();
    if m.gears.limiter_detected {
        writeln!(
            out,
            "    detected rev cut at {:.0} rpm ({:.0}% of the reported {:.0} redline); \
             gearing stats use the detected value",
            m.gears.effective_redline,
            100.0 * m.gears.effective_redline / m.redline.max(1.0),
            m.redline,
        )
        .unwrap();
    }
    if let Some(d) = &m.driveline {
        let scale = d
            .final_drive_scale(m.gears.effective_redline)
            .unwrap_or(1.0);
        writeln!(
            out,
            "    drag model: flat-out top speed {:.0} {su} (longest run here reaches \
             ~{:.0} {su}) | rev cut arrives at {:.0} {su} in gear {}: final drive \
             {} for this aero (ideal ≈ current × {:.2})",
            speed_val(d.vmax_flat),
            speed_val(d.vmax_track),
            speed_val(d.redline_speed(m.gears.effective_redline)),
            d.top_gear,
            if scale < 0.95 {
                "SHORT (rev cut before reachable speed)"
            } else if scale > 1.05 {
                "LONG (revs the run never uses)"
            } else {
                "matched"
            },
            scale,
        )
        .unwrap();
    }

    out
}
