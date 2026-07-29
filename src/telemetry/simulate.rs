//! Synthetic telemetry sender: a stand-in for the game so the capture pipeline and,
//! later, the dashboard can be exercised without FH6 running.

use crate::telemetry::packet::{self, Corners, TelemetryFrame};
use std::io;
use std::net::UdpSocket;
use std::time::Duration;

pub struct SimOpts {
    pub addr: String,
    pub port: u16,
    pub packets: u64,
    pub rate: f64,
    /// In-game seconds per wall second (>1 compresses laps for headless tests).
    pub timescale: f64,
}

pub fn run(opts: &SimOpts) -> io::Result<()> {
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    let target = format!("{}:{}", opts.addr, opts.port);
    println!(
        "sending {} synthetic packets to {target} at {:.0} Hz (timescale {:.0}x)",
        opts.packets, opts.rate, opts.timescale
    );
    for i in 0..opts.packets {
        let t = (i as f64 * opts.timescale / opts.rate) as f32;
        socket.send_to(&packet::encode(&synth_frame(t)), &target)?;
        std::thread::sleep(Duration::from_secs_f64(1.0 / opts.rate));
    }
    Ok(())
}

/// Synthetic lap length in in-game seconds.
pub const LAP_S: f32 = 30.0;

/// Distance covered `x` seconds into a lap: integral of the lap speed curve.
fn lap_distance(x: f32) -> f32 {
    30.0 * x - 60.0 * (x / 4.0).cos() + 60.0
}

/// Deterministic, vaguely-plausible car state at time `t` seconds: speed and steering
/// oscillate, gears step, temperatures drift, laps tick over every LAP_S seconds so
/// the analysis pipeline (lap split, profile, quality) sees real lap structure.
/// Not physics, just non-constant values for every field group analysis cares about.
pub fn synth_frame(t: f32) -> TelemetryFrame {
    let lap = (t / LAP_S) as u16;
    let t_lap = t % LAP_S;
    // Speed is a function of lap position, so synthetic laps repeat like a real
    // driver's do (and corroboration/quality sees consistent laps), plus a small
    // wobble within the splice tolerance for lap-to-lap variety.
    let speed = 30.0 + 15.0 * (t_lap / 4.0).sin() + 0.8 * (t * 1.7).sin();
    let rpm = 3800.0 + 2600.0 * (t * 1.5).sin();
    let wheel_speed = speed / 0.35;
    let corner_wave = |phase: f32, base: f32, amp: f32| Corners {
        fl: base + amp * (t + phase).sin(),
        fr: base + amp * (t + phase + 0.7).sin(),
        rl: base + amp * (t + phase + 1.4).sin(),
        rr: base + amp * (t + phase + 2.1).sin(),
    };

    TelemetryFrame {
        is_race_on: true,
        timestamp_ms: (t * 1000.0) as u32,
        engine_max_rpm: 7500.0,
        engine_idle_rpm: 900.0,
        current_engine_rpm: rpm,
        acceleration: [2.0 * (t / 2.0).sin(), -9.6, 3.0 * (t / 3.0).cos()],
        velocity: [0.0, 0.0, speed],
        angular_velocity: [0.0, 0.4 * (t / 2.0).sin(), 0.0],
        yaw: (t / 8.0).sin(),
        norm_suspension_travel: corner_wave(0.0, 0.5, 0.12),
        tire_slip_ratio: corner_wave(1.0, 0.0, 0.3),
        wheel_rotation_speed: Corners {
            fl: wheel_speed,
            fr: wheel_speed,
            rl: wheel_speed * 1.02,
            rr: wheel_speed * 1.02,
        },
        surface_rumble: corner_wave(2.0, 0.1, 0.1),
        tire_slip_angle: corner_wave(3.0, 0.0, 0.4),
        tire_combined_slip: corner_wave(4.0, 0.2, 0.25),
        suspension_travel_meters: corner_wave(0.0, 0.08, 0.02),
        car_ordinal: 1234,
        car_class: 4,
        car_performance_index: 800,
        drivetrain_type: 1,
        num_cylinders: 6,
        car_group: 3,
        position: [30.0 * t, 100.0, 12.0 * t],
        speed,
        power: speed * 4000.0,
        torque: 420.0 + 80.0 * (t * 1.5).sin(),
        tire_temp: corner_wave(0.5, 175.0, 20.0),
        boost: 6.0 + 6.0 * (t * 1.5).sin().max(0.0),
        fuel: (0.95 - t * 0.0002).max(0.0),
        // Integral of the per-lap speed curve (wobble ignored), so distance stays
        // monotonic and every lap covers the same distance.
        distance_traveled: lap as f32 * lap_distance(LAP_S) + lap_distance(t_lap),
        best_lap: if lap >= 1 { LAP_S } else { 0.0 },
        last_lap: if lap >= 1 { LAP_S } else { 0.0 },
        current_lap: t % LAP_S,
        current_race_time: t,
        lap_number: lap,
        race_position: 1,
        accel: 200,
        brake: if (t / 5.0).sin() < -0.8 { 180 } else { 0 },
        gear: 1 + ((t / 3.0) as u8) % 6,
        steer: (100.0 * (t / 2.0).sin()) as i8,
        ..Default::default()
    }
}
