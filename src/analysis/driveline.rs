//! Drag & driveline model: the aero–gearing interplay, measured.
//!
//! On a straight at full throttle, per unit mass (mass cancels throughout):
//!
//! ```text
//! dv/dt = alpha * P / v  -  beta * v^2  -  g * grade
//! ```
//!
//! with `alpha = eta/m` (drivetrain effectiveness) and `beta = c/m` (quadratic
//! drag). Both are fitted from telemetry (kinematic dv/dt, power channel,
//! height-derived grade; hillclimbs would otherwise poison the fit). An aero
//! change moves `beta`, which moves the reachable top speed, which moves the
//! final drive that puts the rev cut exactly there: the coupling the user hit
//! on the Datsun time attack when dropping downforce made the tuned final
//! drive too short.
//!
//! What gearing should match is not the infinite-straight terminal speed but
//! the speed reachable at the end of THIS track's longest flat-out run,
//! integrated from the fitted model over that run's measured length and entry
//! speed.

use super::TimedFrame;

const AIRBORNE_TRAVEL: f32 = 0.06;
const FULL_THROTTLE: u8 = 250;
const STRAIGHT_LAT: f32 = 1.5;
const MIN_FIT_SPEED: f32 = 20.0;
const MIN_FIT_SAMPLES: usize = 300;
const G: f32 = 9.81;
/// Smoothing half-window (frames) for dv/dt and grade.
const DIFF_HALF_WINDOW: usize = 5;

#[derive(Debug, Clone, Copy)]
pub struct DrivelineFit {
    /// Power effectiveness per mass (eta/m).
    pub alpha: f32,
    /// Quadratic drag per mass (c/m). The aero knob moves this.
    pub beta: f32,
    pub samples: usize,
    /// 95th percentile of full-throttle power (W): the fit's power budget.
    pub p95_power: f32,
    /// Terminal speed on an infinite flat straight (m/s).
    pub vmax_flat: f32,
    /// Speed reached at the end of the longest observed flat-out run (m/s),
    /// integrating the fitted model from that run's entry speed and length.
    pub vmax_track: f32,
    /// rpm per m/s in the top gear used (slip-filtered); scales linearly
    /// with final drive.
    pub k_top: f32,
    pub top_gear: u8,
}

impl DrivelineFit {
    /// Speed (m/s) at which `redline` rpm arrives in top gear.
    pub fn redline_speed(&self, redline: f32) -> f32 {
        if self.k_top > 0.0 {
            redline / self.k_top
        } else {
            0.0
        }
    }

    /// Multiplier for the final drive that puts `redline` exactly at the
    /// track-reachable top speed (> 1 = shorten, < 1 = lengthen).
    pub fn final_drive_scale(&self, redline: f32) -> Option<f32> {
        let rs = self.redline_speed(redline);
        (rs > 0.0 && self.vmax_track > 0.0).then(|| rs / self.vmax_track)
    }
}

pub fn fit(frames: &[TimedFrame]) -> Option<DrivelineFit> {
    // Precompute smoothed dv/dt and grade over the whole (kept) timeline.
    let n = frames.len();
    if n < 2 * DIFF_HALF_WINDOW + 1 {
        return None;
    }
    let mut k_sum = [0.0f64; 11];
    let mut k_n = [0usize; 11];
    let mut top_gear = 0u8;
    let (mut s11, mut s12, mut s22, mut sy1, mut sy2) = (0f64, 0f64, 0f64, 0f64, 0f64);
    let mut fit_n = 0usize;
    let mut powers: Vec<f32> = Vec::new();
    // Longest flat-out run: (entry speed, length in meters).
    let mut best_run = (0.0f32, 0.0f32);
    let mut run: Option<(f32, f32)> = None;

    for i in DIFF_HALF_WINDOW..n - DIFF_HALF_WINDOW {
        let f = &frames[i].frame;
        let (lo, hi) = (
            &frames[i - DIFF_HALF_WINDOW].frame,
            &frames[i + DIFF_HALF_WINDOW].frame,
        );
        let dt = hi.current_race_time - lo.current_race_time;
        // Splice/pause gaps poison differences; a clean window spans ~0.17s.
        let clean_window = dt > 0.0 && dt < 0.5;

        let flat_out = f.is_race_on && f.accel >= FULL_THROTTLE && f.speed > MIN_FIT_SPEED;
        match (&mut run, flat_out && clean_window) {
            (None, true) => run = Some((f.speed, 0.0)),
            (Some((_, len)), true) => {
                *len += f.speed
                    * (f.current_race_time - frames[i - 1].frame.current_race_time).clamp(0.0, 0.1);
            }
            (Some(r), false) => {
                if r.1 > best_run.1 {
                    best_run = *r;
                }
                run = None;
            }
            (None, false) => {}
        }

        if !f.is_race_on {
            continue;
        }
        let travel = f.norm_suspension_travel.to_array();
        if travel.iter().all(|v| *v <= AIRBORNE_TRAVEL) {
            continue;
        }
        if !(1..=10).contains(&f.gear) {
            continue;
        }
        let slip_ok = f.tire_slip_ratio.to_array().iter().all(|r| r.abs() < 0.1);
        if slip_ok && f.speed > 15.0 {
            k_sum[f.gear as usize] += (f.current_engine_rpm / f.speed) as f64;
            k_n[f.gear as usize] += 1;
            if f.gear > top_gear && k_n[f.gear as usize] >= 100 {
                top_gear = f.gear;
            }
        }
        if !(clean_window
            && slip_ok
            && f.accel >= FULL_THROTTLE
            && f.acceleration[0].abs() < STRAIGHT_LAT
            && f.speed > MIN_FIT_SPEED
            && f.power > 1000.0)
        {
            continue;
        }
        let dvdt = (hi.speed - lo.speed) / dt;
        if dvdt.abs() > 15.0 {
            continue;
        }
        let ds: f32 = (hi.current_race_time - lo.current_race_time) * f.speed;
        let grade = if ds > 1.0 {
            (hi.position[1] - lo.position[1]) / ds
        } else {
            0.0
        };
        let y = (dvdt + G * grade.clamp(-0.3, 0.3)) as f64;
        let x1 = (f.power / f.speed) as f64;
        let x2 = -((f.speed * f.speed) as f64);
        s11 += x1 * x1;
        s12 += x1 * x2;
        s22 += x2 * x2;
        sy1 += y * x1;
        sy2 += y * x2;
        fit_n += 1;
        powers.push(f.power);
    }
    if let Some(r) = run
        && r.1 > best_run.1
    {
        best_run = r;
    }

    let det = s11 * s22 - s12 * s12;
    if fit_n < MIN_FIT_SAMPLES || det.abs() < 1e-9 || top_gear == 0 {
        return None;
    }
    let alpha = ((sy1 * s22 - sy2 * s12) / det) as f32;
    let beta = ((s11 * sy2 - s12 * sy1) / det) as f32;
    if alpha <= 0.0 || beta <= 0.0 {
        return None; // fit did not converge to physical values
    }
    powers.sort_by(f32::total_cmp);
    let p95 = powers[(powers.len() as f32 * 0.95) as usize % powers.len()];
    let vmax_flat = (alpha as f64 * p95 as f64 / beta as f64).cbrt() as f32;

    // Integrate dv/ds = a(v)/v over the longest run's length from its entry
    // speed: what this track actually lets the car reach.
    let vmax_track = {
        let (mut v, len) = (best_run.0.max(MIN_FIT_SPEED), best_run.1);
        if len > 0.0 {
            let step = 5.0f32;
            let mut s = 0.0f32;
            while s < len {
                let a = alpha * p95 / v - beta * v * v;
                if a <= 0.01 {
                    break;
                }
                v += (a / v) * step.min(len - s);
                s += step;
            }
            v.min(vmax_flat)
        } else {
            vmax_flat
        }
    };

    let k_top = (k_sum[top_gear as usize] / k_n[top_gear as usize].max(1) as f64) as f32;
    Some(DrivelineFit {
        alpha,
        beta,
        samples: fit_n,
        p95_power: p95,
        vmax_flat,
        vmax_track,
        k_top,
        top_gear,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::packet::TelemetryFrame;

    /// Synthetic full-throttle pull with known alpha/beta on a flat road:
    /// the fit must recover them and the vmax prediction.
    #[test]
    #[allow(clippy::field_reassign_with_default)] // frame setup reads clearer sequentially
    fn fit_recovers_known_drag_model() {
        let (alpha, beta) = (0.0008f32, 0.0006f32);
        let power = 500_000.0f32;
        let mut frames = Vec::new();
        let mut v = 30.0f32;
        let dt = 1.0 / 60.0;
        for i in 0..3000 {
            let a = alpha * power / v - beta * v * v;
            v += a * dt;
            let mut f = TelemetryFrame::default();
            f.is_race_on = true;
            f.speed = v;
            f.power = power;
            f.accel = 255;
            f.gear = 5;
            f.current_engine_rpm = 80.0 * v;
            f.engine_max_rpm = 8000.0;
            f.current_race_time = i as f32 * dt;
            f.norm_suspension_travel = crate::telemetry::packet::Corners {
                fl: 0.4,
                fr: 0.4,
                rl: 0.4,
                rr: 0.4,
            };
            frames.push(crate::analysis::TimedFrame {
                recv_us: 0,
                frame: f,
            });
        }
        let fit = fit(&frames).expect("fit");
        assert!(
            (fit.alpha - alpha).abs() / alpha < 0.05,
            "alpha {}",
            fit.alpha
        );
        assert!((fit.beta - beta).abs() / beta < 0.05, "beta {}", fit.beta);
        let vmax_true = (alpha * power / beta).cbrt();
        assert!(
            (fit.vmax_flat - vmax_true).abs() < 2.0,
            "vmax {} vs {vmax_true}",
            fit.vmax_flat
        );
        assert!((fit.k_top - 80.0).abs() < 0.5, "k {}", fit.k_top);
        // Redline 8000 at K=80 -> 100 m/s; if the track reaches vmax_true
        // (~87 m/s), the final drive should lengthen by ~13%.
        let scale = fit.final_drive_scale(8000.0).unwrap();
        assert!(
            (scale - 100.0 / fit.vmax_track).abs() < 0.02,
            "scale {scale}"
        );
    }
}
