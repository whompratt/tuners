//! Per-stint metric computation. All thresholds are named constants so tuning them
//! later is a one-line change with tests to catch regressions.

use super::{TimedFrame, stint_seconds};
use crate::telemetry::packet::Corners;

/// |normalized slip| above this = the tire has lost grip (per the packet spec).
pub const SLIP_LIMIT: f32 = 1.0;
/// Lateral acceleration (m/s²) above which a sample counts as cornering.
const CORNERING_LAT_ACCEL: f32 = 4.0;
/// Instantaneous front−rear slip delta at or below this = a clear oversteer
/// moment (the rear is the sliding end right now, whatever the stint average
/// says).
const OS_CLEAR_DELTA: f32 = -0.15;
/// Slip-angle fraction of the grip limit treated as "at the limit" for the
/// rear-first stat.
const REAR_AT_LIMIT: f32 = 0.9;
/// |steer| (i8 units, full lock = 127) below this is straight-ahead jitter,
/// not a counter-steer input.
const STEER_DEADBAND: f32 = 15.0;
/// Consecutive opposite-lock frames before a counter-steer episode counts.
/// Filters chicane transitions where lat flips before the hands do.
const COUNTERSTEER_MIN_RUN: usize = 3;
/// Pedal inputs are 0–255; above this counts as "on".
const PEDAL_ON: u8 = 128;
/// Normalized suspension travel bounds treated as bottomed / topped out.
const BOTTOMED: f32 = 0.97;
const TOPPED: f32 = 0.03;
/// Fraction of redline treated as "on the limiter".
const LIMITER: f32 = 0.98;
/// Fraction of redline treated as "using the top of the rev range": the
/// numerator of GearStats::top_gear_high_rev_frac.
const HIGH_REV: f32 = 0.90;
/// Gear values outside real forward gears: 0 = reverse, 11 = mid-shift sentinel.
const MAX_REAL_GEAR: u8 = 10;
/// Suspension travel must move this far (meters) since the last extreme for a
/// direction change to count as a reversal; filters sensor jitter.
const OSC_MIN_TRAVEL_M: f32 = 0.002;
/// Mean |SurfaceRumble| above this = loose surface (tarmac reads ~0.00, dirt
/// 0.10-0.15 observed).
const LOOSE_SURFACE_RUMBLE: f32 = 0.05;
/// All four wheels at or under this normalized travel = airborne.
const AIRBORNE_TRAVEL: f32 = 0.06;
/// Minimum airborne time for a jump/crest event (seconds).
const JUMP_MIN_AIR_S: f32 = 0.15;
/// Bottoming inside this window after touchdown is a landing, not a spring
/// problem, excluded from bottomed_frac (one jump must not drive spring advice).
const LANDING_WINDOW_S: f32 = 0.6;
/// Flutter (|d rpm/dt|, |d wheel speed/dt|) sampling: same gear, on throttle,
/// frame gaps up to this many seconds.
const FLUTTER_MAX_DT_S: f32 = 0.2;
/// Cornering at or above this speed (m/s, ~85 mph) is the high-speed balance
/// band, where aero dominates roll stiffness; below it, mechanical grip does.
const HIGH_SPEED_MPS: f32 = 38.0;

#[derive(Debug, Clone, Copy, Default)]
pub struct TempStats {
    pub avg: f32,
    pub max: f32,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SuspensionStats {
    pub avg: f32,
    pub bottomed_frac: f32,
    /// Also the airborne-wheel proxy: no airborne flag exists in the packet, but a
    /// fully-extended wheel is unloaded (matters for rally rebound tuning).
    pub topped_frac: f32,
    /// Amplitude-filtered travel direction reversals per second: the damping
    /// signal. Road texture drives ~5.5/s baseline (observed, tarmac, healthy
    /// damping); underdamped ringing adds 2 reversals per cycle on top.
    pub reversals_per_sec: f32,
    /// The same reversals normalized by DISTANCE (per 100m): bumps are
    /// spatial, so the temporal rate scales with speed within a setup
    /// (measured: same-setup Ferrari stints read 8.0/s at 68 m/s and 3.6/s
    /// at 29 m/s — both ~12 per 100m). Healthy tarmac collapses to 11-16
    /// per 100m across the whole library; the bump-max overdamped exemplar
    /// reads 9.4 while the (correctly invisible) rebound-only-max stint
    /// reads 11.5.
    pub reversals_per_100m: f32,
}

/// Balance measured over a conditioned subset of cornering samples (a speed
/// band, or on/off throttle). Same units as `understeer_index`.
#[derive(Debug, Clone, Copy, Default)]
pub struct BandBalance {
    pub samples: usize,
    /// mean |front slip angle| − mean |rear slip angle| within the band.
    pub index: Option<f32>,
    /// Mean |rear slip angle| within the band: the absolute operating point.
    pub rear_slip: Option<f32>,
}

#[derive(Debug, Clone, Default)]
pub struct GearStats {
    /// (gear, fraction of grounded samples), real forward gears only, ascending.
    /// Airborne frames (all four wheels at full droop) are excluded from every
    /// stat here: unloaded wheels free-rev the engine to the limiter, which is
    /// jump evidence, not gearing evidence.
    pub time_frac: Vec<(u8, f32)>,
    pub top_gear: u8,
    /// Highest rpm reached while grounded in the top gear used. Low vs redline
    /// means the top of the rev range goes unused (final drive likely too long).
    pub top_gear_max_rpm: f32,
    /// Share of grounded top-gear time spent at >= HIGH_REV of redline. The
    /// robust form of the signal above: a single downhill burst can push the
    /// max near redline, but it cannot fake sustained use of the rev range.
    pub top_gear_high_rev_frac: f32,
    pub upshifts: u32,
    pub avg_upshift_rpm: Option<f32>,
    /// Fraction of grounded samples at >= LIMITER of the EFFECTIVE redline.
    pub limiter_frac: f32,
    /// The redline gearing stats are judged against. Some cars' reported
    /// engine_max_rpm sits well above the actual rev cut (Datsun 240Z:
    /// limiter ~7500 vs reported 8000). When 3+ gears max out within 1% of
    /// the same sustained ceiling, that ceiling IS the limiter and becomes
    /// the effective redline. Otherwise the reported value stands.
    pub effective_redline: f32,
    /// True when the effective redline came from an observed multi-gear rev
    /// ceiling rather than the reported engine_max_rpm.
    pub limiter_detected: bool,
}

/// Shares of cornering time spent in momentary oversteer: the transients a
/// stint-length balance average hides. All fractions are of cornering samples.
#[derive(Debug, Clone, Copy, Default)]
pub struct TransientOversteer {
    /// Instantaneous delta <= OS_CLEAR_DELTA.
    pub clear_frac: f32,
    /// Clear moments with the throttle down (power oversteer signature).
    pub on_power_frac: f32,
    /// Clear moments at or above 85 mph (rear grip/aero at speed).
    pub high_speed_frac: f32,
    /// Rear at >=90% of its grip limit while the front is not.
    pub rear_first_frac: f32,
    /// Distinct clear-oversteer episodes (consecutive frames count once).
    pub episodes: usize,
    /// Share of cornering time steering AGAINST the corner (opposite lock).
    /// The driver's own correction labels a slide: understeer never needs
    /// counter-steer, every caught slide does, so this channel sees the
    /// rear-limited moments that time-averaged balance structurally cannot
    /// (oversteer is episodic and corrected; understeer is sustained).
    pub countersteer_frac: f32,
    /// Distinct counter-steer episodes (>= 3 consecutive frames).
    pub countersteer_episodes: usize,
}

#[derive(Debug, Clone)]
pub struct StintMetrics {
    pub samples: usize,
    pub duration_s: f32,
    pub distance_m: f32,
    pub avg_speed: f32,
    pub max_speed: f32,
    pub car_ordinal: i32,
    pub car_class: i32,
    pub car_performance_index: i32,
    pub drivetrain_type: i32,
    pub num_cylinders: i32,
    pub redline: f32,
    pub tire_temp: Corners<TempStats>,
    /// Fraction of samples with |combined slip| > SLIP_LIMIT, per corner.
    pub slip_frac: Corners<f32>,
    /// mean |front slip angle| − mean |rear slip angle| over cornering samples.
    /// Positive = front washes out first (understeer tendency). None if no cornering.
    /// Units: fraction of each tire's grip limit (1.0 = at the limit).
    pub understeer_index: Option<f32>,
    /// Mean |slip angle| per axle over cornering samples: the absolute operating
    /// point the index difference hides (0.85 front vs 0.55 rear reads very
    /// differently from 0.30 vs 0.04, at the same index).
    pub cornering_front_slip: Option<f32>,
    pub cornering_rear_slip: Option<f32>,
    pub cornering_frac: f32,
    pub transient_oversteer: TransientOversteer,
    /// Fitted drag/driveline model (None without enough clean full-throttle
    /// data): the measured aero–gearing coupling.
    pub driveline: Option<super::driveline::DrivelineFit>,
    /// Extra normalized front suspension travel while braking vs on throttle
    /// (nose dive). Measurement only; no rule threshold is calibrated yet, and
    /// front aero confounds it at braking speeds. None without enough braking
    /// and throttle samples.
    pub brake_dive_front: Option<f32>,
    /// Balance split by speed band (boundary HIGH_SPEED_MPS): imbalance that
    /// lives only in the high band points at aero, not bars.
    pub balance_low_speed: BandBalance,
    pub balance_high_speed: BandBalance,
    /// Balance split by throttle state while cornering: the on−off index shift
    /// is the power-on balance signature (diff accel / power understeer).
    pub balance_on_throttle: BandBalance,
    pub balance_off_throttle: BandBalance,
    /// Balance while cornering AND braking: the trail-braking signature
    /// (brake balance / diff decel live here, not in positional phase means:
    /// the 2026-07-21 max-decel A/B cost 0.75s with phase means unmoved).
    pub balance_on_brake: BandBalance,
    /// Corner-event segmentation: entry/exit phase balance across
    /// detected corners. None when the stint has no corner events.
    pub corners: Option<super::corners::CornerSummary>,
    /// Drive-wheel spin as a fraction of on-throttle samples. None if never on throttle.
    pub wheelspin_frac: Option<f32>,
    /// Any-wheel lockup as a fraction of on-brake samples. None if never on brake.
    pub lockup_frac: Option<f32>,
    pub suspension: Corners<SuspensionStats>,
    pub gears: GearStats,
    pub surface_rumble_avg: f32,
    /// Loose surface (dirt/gravel) per SurfaceRumble. Baselines for suspension
    /// activity and slip differ enormously from tarmac.
    pub surface_loose: bool,
    /// Airborne events (all four wheels at full droop >= 0.15s): jumps and crests.
    pub jumps: u32,
    /// Bottoming samples that happened on jump landings, excluded from
    /// bottomed_frac so one jump can't drive spring/ride-height advice.
    pub landing_bottomed_excluded: u32,
    /// Mean |d rpm/dt| on throttle in-gear (rpm/s). On loose surfaces, roughly
    /// doubles when overdamped (skipping wheels); flat tarmac shows nothing.
    pub rpm_flutter: Option<f32>,
    /// Mean |d wheel-speed/dt| on throttle in-gear (rad/s²), same signal.
    pub wheelspeed_flutter: Option<f32>,
}

impl StintMetrics {
    /// Grip-margin ratio: how much closer the front runs to its limit than
    /// the rear while cornering (cornering_front_slip / cornering_rear_slip).
    /// Signature runs (2026-07-25) measured every library driver settling at
    /// front 1.5-1.7x — the driver's preferred operating point; a smaller
    /// ratio means the rear is working relatively harder. None without
    /// cornering or with the rear far from its limit (ratio unstable).
    pub fn margin_ratio(&self) -> Option<f32> {
        let (f, r) = (self.cornering_front_slip?, self.cornering_rear_slip?);
        (r >= 0.05).then(|| f / r)
    }
}

fn band_balance((samples, front, rear): (usize, f32, f32)) -> BandBalance {
    BandBalance {
        samples,
        index: (samples > 0).then(|| (front - rear) / samples as f32),
        rear_slip: (samples > 0).then(|| rear / samples as f32),
    }
}

pub fn stint_metrics(frames: &[TimedFrame]) -> StintMetrics {
    let n = frames.len().max(1);
    let first = frames.first().map(|t| t.frame).unwrap_or_default();

    let mut speed_sum = 0.0f32;
    let mut max_speed = 0.0f32;
    let mut temp_sum = [0.0f32; 4];
    let mut temp_max = [0.0f32; 4];
    let mut slip_count = [0usize; 4];
    let mut susp_sum = [0.0f32; 4];
    let mut susp_bottomed = [0usize; 4];
    let mut susp_topped = [0usize; 4];
    let mut osc_reversals = [0u32; 4];
    let mut osc_extreme = frames
        .first()
        .map(|t| t.frame.suspension_travel_meters.to_array())
        .unwrap_or_default();
    let mut osc_dir = [0i8; 4];
    let mut rumble_sum = 0.0f32;
    let mut air_start: Option<f32> = None;
    let mut landing_until = f32::NEG_INFINITY;
    let mut jumps = 0u32;
    let mut landing_bottomed = 0u32;
    let mut flutter_prev: Option<(u8, u8, f32, f32, f32)> = None; // gear, accel, rpm, wheel avg, race_t
    let mut rpm_flutter_sum = 0.0f32;
    let mut wheel_flutter_sum = 0.0f32;
    let mut flutter_samples = 0usize;
    let mut cornering = 0usize;
    let mut brake_frames = 0usize;
    let mut brake_front_travel = 0.0f32;
    let mut throttle_frames = 0usize;
    let mut throttle_front_travel = 0.0f32;
    let mut os_clear = 0usize;
    let mut os_on_power = 0usize;
    let mut os_high_speed = 0usize;
    let mut os_rear_first = 0usize;
    let mut os_episodes = 0usize;
    let mut os_in_run = false;
    let mut cs_frames = 0usize;
    let mut cs_episodes = 0usize;
    let mut cs_run = 0usize;
    let mut front_slip_sum = 0.0f32;
    let mut rear_slip_sum = 0.0f32;
    // [low speed, high speed, on throttle, off throttle, on brake] cornering
    // bands: (samples, front slip sum, rear slip sum).
    let mut bands = [(0usize, 0.0f32, 0.0f32); 5];
    let mut throttle_samples = 0usize;
    let mut wheelspin = 0usize;
    let mut brake_samples = 0usize;
    let mut lockup = 0usize;
    // Grounded real-gear frames (gear, rpm), kept whole so gearing stats can
    // be judged against the EFFECTIVE redline, which is only known once the
    // whole stint's rev ceiling has been seen.
    let mut gear_frames: Vec<(u8, f32)> = Vec::new();
    let mut upshift_rpm_sum = 0.0f32;
    let mut upshifts = 0u32;
    let mut prev_real_gear: Option<(u8, f32)> = None; // (gear, rpm while in it)
    let mut grounded = 0usize; // frames not airborne: denominator for gear stats
    // DistanceTraveled is always 0 outside races (see telemetry.md), so distance is
    // integrated from speed over the race clock (monotonic in a kept timeline,
    // frozen across pauses).
    let mut distance_m = 0.0f32;
    let mut prev_race_t: Option<f32> = None;

    for tf in frames {
        let f = &tf.frame;
        speed_sum += f.speed;
        max_speed = max_speed.max(f.speed);
        if let Some(prev) = prev_race_t {
            distance_m += f.speed * (f.current_race_time - prev).clamp(0.0, 1.0);
        }
        prev_race_t = Some(f.current_race_time);

        rumble_sum += f
            .surface_rumble
            .to_array()
            .iter()
            .map(|r| r.abs())
            .sum::<f32>()
            / 4.0;

        let temps = f.tire_temp.to_array();
        let slips = f.tire_combined_slip.to_array();
        let travel = f.norm_suspension_travel.to_array();
        let travel_m = f.suspension_travel_meters.to_array();

        // Airborne / landing detection must precede bottoming attribution.
        let airborne = travel.iter().all(|v| *v <= AIRBORNE_TRAVEL);
        let t = f.current_race_time;
        if airborne {
            air_start.get_or_insert(t);
        } else if let Some(start) = air_start.take()
            && t - start >= JUMP_MIN_AIR_S
        {
            jumps += 1;
            landing_until = t + LANDING_WINDOW_S;
        }
        let landing = t < landing_until;

        for i in 0..4 {
            temp_sum[i] += temps[i];
            temp_max[i] = temp_max[i].max(temps[i]);
            slip_count[i] += (slips[i].abs() > SLIP_LIMIT) as usize;
            susp_sum[i] += travel[i];
            if travel[i] >= BOTTOMED {
                if landing {
                    landing_bottomed += 1;
                } else {
                    susp_bottomed[i] += 1;
                }
            }
            susp_topped[i] += (travel[i] <= TOPPED) as usize;

            let delta = travel_m[i] - osc_extreme[i];
            if osc_dir[i] >= 0 && delta < -OSC_MIN_TRAVEL_M {
                osc_reversals[i] += 1;
                osc_dir[i] = -1;
                osc_extreme[i] = travel_m[i];
            } else if osc_dir[i] <= 0 && delta > OSC_MIN_TRAVEL_M {
                osc_reversals[i] += 1;
                osc_dir[i] = 1;
                osc_extreme[i] = travel_m[i];
            } else if (osc_dir[i] >= 0 && delta > 0.0) || (osc_dir[i] <= 0 && delta < 0.0) {
                osc_extreme[i] = travel_m[i];
            }
        }

        if f.acceleration[0].abs() > CORNERING_LAT_ACCEL {
            cornering += 1;
            let front = (f.tire_slip_angle.fl.abs() + f.tire_slip_angle.fr.abs()) / 2.0;
            let rear = (f.tire_slip_angle.rl.abs() + f.tire_slip_angle.rr.abs()) / 2.0;
            front_slip_sum += front;
            rear_slip_sum += rear;
            let speed_band = (f.speed >= HIGH_SPEED_MPS) as usize;
            let throttle_band = 2 + (f.accel < PEDAL_ON) as usize;
            for b in [speed_band, throttle_band] {
                bands[b].0 += 1;
                bands[b].1 += front;
                bands[b].2 += rear;
            }
            if f.brake >= PEDAL_ON {
                bands[4].0 += 1;
                bands[4].1 += front;
                bands[4].2 += rear;
            }
            // Transient oversteer: brief rear-first moments a stint-length
            // average hides entirely (a net-understeer car can still snap).
            let delta = front - rear;
            if delta <= OS_CLEAR_DELTA {
                os_clear += 1;
                os_on_power += (f.accel >= PEDAL_ON) as usize;
                os_high_speed += (f.speed >= HIGH_SPEED_MPS) as usize;
                os_episodes += !os_in_run as usize;
                os_in_run = true;
            } else {
                os_in_run = false;
            }
            if rear >= REAR_AT_LIMIT && front < REAR_AT_LIMIT {
                os_rear_first += 1;
            }
            // Opposite lock: steering against the corner's lateral G.
            let steer = f.steer as f32;
            if steer.abs() >= STEER_DEADBAND && (steer > 0.0) != (f.acceleration[0] > 0.0) {
                cs_frames += 1;
                cs_run += 1;
                if cs_run == COUNTERSTEER_MIN_RUN {
                    cs_episodes += 1;
                }
            } else {
                cs_run = 0;
            }
        } else {
            os_in_run = false;
            cs_run = 0;
        }

        if f.accel >= PEDAL_ON {
            throttle_samples += 1;
            let ratios = f.tire_slip_ratio;
            let spinning = match f.drivetrain_type {
                0 => ratios.fl > SLIP_LIMIT || ratios.fr > SLIP_LIMIT,
                1 => ratios.rl > SLIP_LIMIT || ratios.rr > SLIP_LIMIT,
                _ => ratios.to_array().iter().any(|r| *r > SLIP_LIMIT),
            };
            wheelspin += spinning as usize;
        }
        // Brake dive: how much extra front compression braking adds vs the
        // on-throttle attitude. Measurement only for now: confounded by
        // front aero at braking speeds, and the library has no known
        // dive-problem stint to calibrate a rule threshold against.
        {
            let front = (f.norm_suspension_travel.fl + f.norm_suspension_travel.fr) / 2.0;
            if f.brake >= PEDAL_ON {
                brake_frames += 1;
                brake_front_travel += front;
            } else if f.accel >= PEDAL_ON {
                throttle_frames += 1;
                throttle_front_travel += front;
            }
        }
        if f.brake >= PEDAL_ON {
            brake_samples += 1;
            lockup += f
                .tire_slip_ratio
                .to_array()
                .iter()
                .any(|r| *r < -SLIP_LIMIT) as usize;
        }

        // Airborne revs are free-revving against unloaded wheels (usually pinned
        // on the limiter): jump evidence, not gearing evidence. Gear/limiter
        // stats sample grounded frames only, and an upshift must not span an
        // airborne gap (auto shifts at the mid-air limiter).
        if airborne {
            prev_real_gear = None;
        } else {
            grounded += 1;
            if (1..=MAX_REAL_GEAR).contains(&f.gear) {
                gear_frames.push((f.gear, f.current_engine_rpm));
                if let Some((prev, prev_rpm)) = prev_real_gear
                    && f.gear > prev
                {
                    upshift_rpm_sum += prev_rpm;
                    upshifts += 1;
                }
                prev_real_gear = Some((f.gear, f.current_engine_rpm));
            }
        }

        let wheel_avg = f.wheel_rotation_speed.to_array().iter().sum::<f32>() / 4.0;
        if let Some((pgear, paccel, prpm, pwheel, pt)) = flutter_prev {
            let dt = f.current_race_time - pt;
            if dt > 0.0
                && dt <= FLUTTER_MAX_DT_S
                && f.gear == pgear
                && (1..=MAX_REAL_GEAR).contains(&f.gear)
                && f.accel >= PEDAL_ON
                && paccel >= PEDAL_ON
            {
                rpm_flutter_sum += (f.current_engine_rpm - prpm).abs() / dt;
                wheel_flutter_sum += (wheel_avg - pwheel).abs() / dt;
                flutter_samples += 1;
            }
        }
        flutter_prev = Some((
            f.gear,
            f.accel,
            f.current_engine_rpm,
            wheel_avg,
            f.current_race_time,
        ));
    }

    let frac = |count: usize| count as f32 / n as f32;
    // Gear stats count grounded frames only, so their fractions are over the
    // grounded sample count; a jump-heavy stint must not dilute limiter time.
    let gfrac = |count: usize| count as f32 / grounded.max(1) as f32;
    let corners_from = |vals: [f32; 4]| Corners {
        fl: vals[0],
        fr: vals[1],
        rl: vals[2],
        rr: vals[3],
    };

    let mut gear_counts = [0usize; MAX_REAL_GEAR as usize + 1];
    let mut gear_max_rpm = [0.0f32; MAX_REAL_GEAR as usize + 1];
    for &(g, rpm) in &gear_frames {
        gear_counts[g as usize] += 1;
        gear_max_rpm[g as usize] = gear_max_rpm[g as usize].max(rpm);
    }
    let time_frac: Vec<(u8, f32)> = (1..=MAX_REAL_GEAR)
        .filter(|g| gear_counts[*g as usize] > 0)
        .map(|g| (g, gfrac(gear_counts[g as usize])))
        .collect();
    let top_gear = time_frac.last().map(|(g, _)| *g).unwrap_or(0);

    // Effective redline: the SUSTAINED rev ceiling (highest 25-rpm bucket
    // holding >= 0.25s, so a single downhill over-rev spike can't set it),
    // adopted only when 3+ well-used gears max out within 1% of it: the
    // signature of a rev cut, not of a consistent shift point in one gear.
    let reported_redline = first.engine_max_rpm;
    let ceiling = {
        let mut hist = std::collections::BTreeMap::<i32, u32>::new();
        for &(_, rpm) in &gear_frames {
            *hist.entry((rpm / 25.0) as i32).or_default() += 1;
        }
        hist.iter()
            .rev()
            .find(|(_, count)| **count >= 15)
            .map(|(bucket, _)| (*bucket as f32 + 1.0) * 25.0)
            .unwrap_or(0.0)
    };
    // A real rev cut clusters TIGHT (the Datsun's six gears max within 8 rpm
    // of 7500); a consistent shift habit clusters loose (the Ford GT's within
    // ~90 rpm of 6875, plus a brief 7094 overshoot). The cluster is gears
    // whose max sits AT the sustained ceiling: within 0.5% below, 1% above
    // (momentary overshoot past a cut is a spike, not a cut).
    let gears_at_ceiling = (1..=MAX_REAL_GEAR as usize)
        .filter(|g| {
            gear_counts[*g] >= 100
                && gear_max_rpm[*g] >= 0.995 * ceiling
                && gear_max_rpm[*g] <= 1.01 * ceiling
        })
        .count();
    // Adopt only a MATERIALLY lower cut (<97% of reported): near-reported
    // ceilings are consistent shift points (the Ferrari rides to 97.5% in
    // every gear), and correcting by <3% moves no threshold meaningfully
    // while it would quietly shift calibrated behavior.
    let limiter_detected =
        ceiling > 0.0 && gears_at_ceiling >= 3 && ceiling < 0.97 * reported_redline;
    let effective_redline = if limiter_detected {
        ceiling
    } else {
        reported_redline
    };
    let limiter = gear_frames
        .iter()
        .filter(|(_, rpm)| effective_redline > 0.0 && *rpm >= LIMITER * effective_redline)
        .count();
    let top_gear_high_rev = gear_frames
        .iter()
        .filter(|(g, rpm)| {
            *g == top_gear && effective_redline > 0.0 && *rpm >= HIGH_REV * effective_redline
        })
        .count();

    StintMetrics {
        samples: frames.len(),
        duration_s: stint_seconds(frames),
        distance_m,
        avg_speed: speed_sum / n as f32,
        max_speed,
        car_ordinal: first.car_ordinal,
        car_class: first.car_class,
        car_performance_index: first.car_performance_index,
        drivetrain_type: first.drivetrain_type,
        num_cylinders: first.num_cylinders,
        redline: first.engine_max_rpm,
        tire_temp: Corners {
            fl: TempStats {
                avg: temp_sum[0] / n as f32,
                max: temp_max[0],
            },
            fr: TempStats {
                avg: temp_sum[1] / n as f32,
                max: temp_max[1],
            },
            rl: TempStats {
                avg: temp_sum[2] / n as f32,
                max: temp_max[2],
            },
            rr: TempStats {
                avg: temp_sum[3] / n as f32,
                max: temp_max[3],
            },
        },
        slip_frac: corners_from(std::array::from_fn(|i| frac(slip_count[i]))),
        understeer_index: (cornering > 0)
            .then(|| (front_slip_sum - rear_slip_sum) / cornering as f32),
        cornering_front_slip: (cornering > 0).then(|| front_slip_sum / cornering as f32),
        cornering_rear_slip: (cornering > 0).then(|| rear_slip_sum / cornering as f32),
        cornering_frac: frac(cornering),
        brake_dive_front: (brake_frames >= 200 && throttle_frames >= 200).then(|| {
            brake_front_travel / brake_frames as f32
                - throttle_front_travel / throttle_frames as f32
        }),
        transient_oversteer: {
            let cfrac = |n: usize| n as f32 / cornering.max(1) as f32;
            TransientOversteer {
                clear_frac: cfrac(os_clear),
                on_power_frac: cfrac(os_on_power),
                high_speed_frac: cfrac(os_high_speed),
                rear_first_frac: cfrac(os_rear_first),
                episodes: os_episodes,
                countersteer_frac: cfrac(cs_frames),
                countersteer_episodes: cs_episodes,
            }
        },
        balance_low_speed: band_balance(bands[0]),
        balance_high_speed: band_balance(bands[1]),
        balance_on_throttle: band_balance(bands[2]),
        balance_off_throttle: band_balance(bands[3]),
        balance_on_brake: band_balance(bands[4]),
        corners: super::corners::summarize(frames),
        driveline: super::driveline::fit(frames),
        wheelspin_frac: (throttle_samples > 0).then(|| wheelspin as f32 / throttle_samples as f32),
        lockup_frac: (brake_samples > 0).then(|| lockup as f32 / brake_samples as f32),
        suspension: {
            let duration = stint_seconds(frames).max(0.1);
            let mean_speed = frames.iter().map(|f| f.frame.speed).sum::<f32>() / n as f32;
            let dist_100m = (mean_speed * duration / 100.0).max(0.01);
            let stats = |i: usize| SuspensionStats {
                avg: susp_sum[i] / n as f32,
                bottomed_frac: susp_bottomed[i] as f32 / n as f32,
                topped_frac: susp_topped[i] as f32 / n as f32,
                reversals_per_sec: osc_reversals[i] as f32 / duration,
                reversals_per_100m: osc_reversals[i] as f32 / dist_100m,
            };
            Corners {
                fl: stats(0),
                fr: stats(1),
                rl: stats(2),
                rr: stats(3),
            }
        },
        gears: GearStats {
            time_frac,
            top_gear,
            top_gear_max_rpm: gear_max_rpm[top_gear as usize],
            top_gear_high_rev_frac: top_gear_high_rev as f32
                / gear_counts[top_gear as usize].max(1) as f32,
            upshifts,
            avg_upshift_rpm: (upshifts > 0).then(|| upshift_rpm_sum / upshifts as f32),
            limiter_frac: gfrac(limiter),
            effective_redline,
            limiter_detected,
        },
        surface_rumble_avg: rumble_sum / n as f32,
        surface_loose: rumble_sum / n as f32 > LOOSE_SURFACE_RUMBLE,
        jumps,
        landing_bottomed_excluded: landing_bottomed,
        rpm_flutter: (flutter_samples > 50).then(|| rpm_flutter_sum / flutter_samples as f32),
        wheelspeed_flutter: (flutter_samples > 50)
            .then(|| wheel_flutter_sum / flutter_samples as f32),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::packet::TelemetryFrame;

    fn timed(frames: Vec<TelemetryFrame>) -> Vec<TimedFrame> {
        frames
            .into_iter()
            .enumerate()
            .map(|(i, mut frame)| {
                frame.is_race_on = true;
                frame.timestamp_ms = i as u32 * 100;
                frame.current_race_time = i as f32 * 0.1;
                TimedFrame {
                    recv_us: i as u64 * 100_000,
                    frame,
                }
            })
            .collect()
    }

    #[test]
    fn wheelspin_counts_drive_wheels_only() {
        // RWD car: front slip must not count as wheelspin, rear slip must.
        let mut frames = Vec::new();
        for i in 0..10 {
            frames.push(TelemetryFrame {
                drivetrain_type: 1,
                accel: 255,
                tire_slip_ratio: Corners {
                    fl: 2.0, // front always sliding, irrelevant for RWD
                    fr: 2.0,
                    rl: if i < 4 { 1.5 } else { 0.2 },
                    rr: 0.0,
                },
                ..Default::default()
            });
        }
        let m = stint_metrics(&timed(frames));
        assert_eq!(m.wheelspin_frac, Some(0.4));
    }

    #[test]
    fn understeer_index_positive_when_front_slips_more() {
        let frames: Vec<TelemetryFrame> = (0..10)
            .map(|_| TelemetryFrame {
                acceleration: [6.0, 0.0, 0.0], // cornering
                tire_slip_angle: Corners {
                    fl: 0.8,
                    fr: 0.8,
                    rl: 0.4,
                    rr: 0.4,
                },
                ..Default::default()
            })
            .collect();
        let m = stint_metrics(&timed(frames));
        let idx = m.understeer_index.unwrap();
        assert!((idx - 0.4).abs() < 1e-5, "index {idx}");
        assert_eq!(m.cornering_frac, 1.0);
        assert!((m.cornering_front_slip.unwrap() - 0.8).abs() < 1e-5);
        assert!((m.cornering_rear_slip.unwrap() - 0.4).abs() < 1e-5);
    }

    /// Default frames have zero suspension travel = airborne, which excludes
    /// them from gear stats; grounded travel for tests that need revs counted.
    const GROUNDED: Corners<f32> = Corners {
        fl: 0.5,
        fr: 0.5,
        rl: 0.5,
        rr: 0.5,
    };

    #[test]
    fn upshift_rpm_recorded_across_shift_sentinel() {
        // 2nd at 9000 rpm -> gear 11 (mid-shift) -> 3rd: one upshift at 9000.
        let frames: Vec<TelemetryFrame> = [(2, 8000.0), (2, 9000.0), (11, 8200.0), (3, 6500.0)]
            .into_iter()
            .map(|(gear, current_engine_rpm)| TelemetryFrame {
                gear,
                current_engine_rpm,
                norm_suspension_travel: GROUNDED,
                ..Default::default()
            })
            .collect();
        let m = stint_metrics(&timed(frames));
        assert_eq!(m.gears.upshifts, 1);
        assert_eq!(m.gears.avg_upshift_rpm, Some(9000.0));
        assert_eq!(m.gears.top_gear, 3);
        // gear 11 must not appear in time fractions
        assert!(m.gears.time_frac.iter().all(|(g, _)| *g <= 10));
    }

    /// Mid-air the unloaded wheels free-rev the engine to the limiter: those
    /// frames must not count toward limiter time, per-gear max rpm, or upshifts,
    /// and the grounded fractions must not be diluted by air time.
    #[test]
    fn airborne_revs_excluded_from_gear_stats() {
        let mut frames = Vec::new();
        let planted = |gear, rpm| TelemetryFrame {
            gear,
            current_engine_rpm: rpm,
            engine_max_rpm: 8000.0,
            norm_suspension_travel: GROUNDED,
            ..Default::default()
        };
        let flying = |gear, rpm| TelemetryFrame {
            norm_suspension_travel: Corners {
                fl: 0.03,
                fr: 0.03,
                rl: 0.03,
                rr: 0.03,
            },
            ..planted(gear, rpm)
        };
        for _ in 0..20 {
            frames.push(planted(3, 6000.0));
        }
        // Jump: pinned on the limiter, auto-shifting 3rd -> 4th mid-air.
        for _ in 0..3 {
            frames.push(flying(3, 7950.0));
        }
        for _ in 0..3 {
            frames.push(flying(4, 7950.0));
        }
        for _ in 0..20 {
            frames.push(planted(4, 5000.0));
        }
        let m = stint_metrics(&timed(frames));
        assert_eq!(m.jumps, 1);
        assert_eq!(
            m.gears.limiter_frac, 0.0,
            "airborne limiter time must not count"
        );
        assert_eq!(m.gears.top_gear, 4);
        assert_eq!(
            m.gears.top_gear_max_rpm, 5000.0,
            "mid-air rpm must not count"
        );
        assert_eq!(m.gears.upshifts, 0, "mid-air upshift must not count");
        assert_eq!(m.gears.avg_upshift_rpm, None);
        // Fractions are over the 40 grounded samples, not all 46.
        assert_eq!(m.gears.time_frac, vec![(3, 0.5), (4, 0.5)]);
    }

    #[test]
    fn suspension_bottoming_fraction() {
        let frames: Vec<TelemetryFrame> = (0..10)
            .map(|i| TelemetryFrame {
                norm_suspension_travel: Corners {
                    fl: if i < 3 { 0.99 } else { 0.5 },
                    fr: 0.5,
                    rl: 0.5,
                    rr: if i < 5 { 0.01 } else { 0.5 },
                },
                ..Default::default()
            })
            .collect();
        let m = stint_metrics(&timed(frames));
        assert!((m.suspension.fl.bottomed_frac - 0.3).abs() < 1e-5);
        assert!((m.suspension.rr.topped_frac - 0.5).abs() < 1e-5);
        assert_eq!(m.suspension.fr.bottomed_frac, 0.0);
    }

    /// A 10 Hz sine of 5mm amplitude sampled at 10 samples/cycle: two reversals
    /// per cycle -> ~20/s. A flat signal counts none.
    #[test]
    fn suspension_oscillation_rate() {
        let frames: Vec<TelemetryFrame> = (0..500)
            .map(|i| {
                let t = i as f32 * 0.01; // timed() overrides race_t; keep our own phase
                TelemetryFrame {
                    suspension_travel_meters: Corners {
                        fl: 0.05 + 0.005 * (t * 10.0 * std::f32::consts::TAU).sin(),
                        fr: 0.05,
                        ..Default::default()
                    },
                    ..Default::default()
                }
            })
            .collect();
        // timed() spaces frames 0.1s apart -> 500 frames = 49.9s; our sine phase
        // advances 0.01s/frame, so effective ring frequency in stint time is 1 Hz
        // at 10 samples per cycle -> 2 reversals/s expected on FL.
        let m = stint_metrics(&timed(frames));
        let fl = m.suspension.fl.reversals_per_sec;
        let fr = m.suspension.fr.reversals_per_sec;
        assert!((fl - 2.0).abs() < 0.3, "FL {fl}/s");
        assert!(fr < 0.1, "flat FR must not count reversals: {fr}/s");
    }

    /// A jump (all four wheels at full droop for >=0.15s) followed by a bottoming
    /// landing: the event is counted and the landing bottoming is EXCLUDED from
    /// bottomed_frac; one jump must not drive spring advice.
    #[test]
    fn jump_landing_bottoming_is_excluded() {
        let mut frames = Vec::new();
        let cruise = Corners {
            fl: 0.5,
            fr: 0.5,
            rl: 0.5,
            rr: 0.5,
        };
        let airborne = Corners {
            fl: 0.03,
            fr: 0.03,
            rl: 0.03,
            rr: 0.03,
        };
        let landing = Corners {
            fl: 0.99,
            fr: 0.99,
            rl: 0.5,
            rr: 0.5,
        };
        for _ in 0..20 {
            frames.push(TelemetryFrame {
                norm_suspension_travel: cruise,
                ..Default::default()
            });
        }
        for _ in 0..5 {
            frames.push(TelemetryFrame {
                norm_suspension_travel: airborne,
                ..Default::default()
            });
        }
        for _ in 0..3 {
            frames.push(TelemetryFrame {
                norm_suspension_travel: landing,
                ..Default::default()
            });
        }
        for _ in 0..20 {
            frames.push(TelemetryFrame {
                norm_suspension_travel: cruise,
                ..Default::default()
            });
        }
        // timed() spaces frames 0.1s apart: 5 airborne frames = 0.5s > 0.15s min.
        let m = stint_metrics(&timed(frames));
        assert_eq!(m.jumps, 1);
        assert_eq!(
            m.landing_bottomed_excluded, 6,
            "3 frames x 2 bottomed wheels"
        );
        assert_eq!(m.suspension.fl.bottomed_frac, 0.0, "landing must not count");
    }

    #[test]
    fn surface_classified_from_rumble() {
        let dirt: Vec<TelemetryFrame> = (0..10)
            .map(|_| TelemetryFrame {
                surface_rumble: Corners {
                    fl: 0.15,
                    fr: 0.12,
                    rl: 0.14,
                    rr: 0.13,
                },
                ..Default::default()
            })
            .collect();
        assert!(stint_metrics(&timed(dirt)).surface_loose);
        let tarmac: Vec<TelemetryFrame> = (0..10).map(|_| TelemetryFrame::default()).collect();
        assert!(!stint_metrics(&timed(tarmac)).surface_loose);
    }

    #[test]
    fn lockup_fraction_of_braking_time() {
        let mut frames = Vec::new();
        for i in 0..10 {
            frames.push(TelemetryFrame {
                brake: if i < 5 { 255 } else { 0 },
                tire_slip_ratio: Corners {
                    fl: if i < 2 { -1.5 } else { 0.0 },
                    ..Default::default()
                },
                ..Default::default()
            });
        }
        let m = stint_metrics(&timed(frames));
        assert_eq!(m.lockup_frac, Some(0.4)); // 2 lockup samples of 5 braking
    }
}
