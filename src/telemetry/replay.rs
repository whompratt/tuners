//! Replay a recorded session through the decoder and summarize it: the headless
//! check that a capture is intact and the decoder handles every packet in it.

use crate::telemetry::packet::{self, TelemetryFrame};
use crate::telemetry::stint::StintReader;
use crate::util;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

pub fn run(path: &Path) -> Result<(), String> {
    let mut reader = StintReader::open(path).map_err(|e| format!("{}: {e}", path.display()))?;

    let mut packets: u64 = 0;
    let mut errors: u64 = 0;
    let mut race_on: u64 = 0;
    let mut first: Option<TelemetryFrame> = None;
    let mut last: Option<TelemetryFrame> = None;
    let mut first_recv_us: u64 = 0;
    let mut last_recv_us: u64 = 0;
    let mut max_speed: f32 = 0.0;
    let mut max_rpm: f32 = 0.0;
    let mut max_combined_slip: f32 = 0.0;
    let mut max_tire_temp: f32 = 0.0;
    let mut gears: BTreeSet<u8> = BTreeSet::new();
    // ordinal -> (class, pi, drivetrain, cylinders)
    let mut cars: BTreeMap<i32, (i32, i32, i32, i32)> = BTreeMap::new();

    loop {
        let (recv_us, payload) = match reader.next_packet() {
            Ok(Some(record)) => record,
            Ok(None) => break,
            Err(e) => return Err(format!("read error after {packets} packets: {e}")),
        };
        packets += 1;
        if first.is_none() {
            first_recv_us = recv_us;
        }
        last_recv_us = recv_us;

        let f = match packet::decode(&payload) {
            Ok(f) => f,
            Err(e) => {
                errors += 1;
                if errors <= 3 {
                    eprintln!("packet {packets}: {e}");
                }
                continue;
            }
        };
        if first.is_none() {
            first = Some(f);
        }
        last = Some(f);
        race_on += f.is_race_on as u64;
        max_speed = max_speed.max(f.speed);
        max_rpm = max_rpm.max(f.current_engine_rpm);
        max_combined_slip = f
            .tire_combined_slip
            .to_array()
            .iter()
            .fold(max_combined_slip, |m, s| m.max(s.abs()));
        max_tire_temp = f
            .tire_temp
            .to_array()
            .iter()
            .fold(max_tire_temp, |m, t| m.max(*t));
        gears.insert(f.gear);
        cars.insert(
            f.car_ordinal,
            (
                f.car_class,
                f.car_performance_index,
                f.drivetrain_type,
                f.num_cylinders,
            ),
        );
    }

    println!(
        "{}: {packets} packets, {errors} decode errors",
        path.display()
    );
    if let (Some(first), Some(last)) = (first, last) {
        let game_span_ms = last.timestamp_ms.wrapping_sub(first.timestamp_ms);
        let wall_span_us = last_recv_us.saturating_sub(first_recv_us);
        println!(
            "in-game span {:.1}s | wall span {:.1}s | race on {:.0}%",
            game_span_ms as f64 / 1000.0,
            wall_span_us as f64 / 1_000_000.0,
            100.0 * race_on as f64 / packets as f64,
        );
        for (ordinal, (class, pi, dt, cyl)) in &cars {
            println!(
                "car ordinal {ordinal}: class {} | PI {pi} | {} | {cyl} cyl",
                packet::class_name(*class),
                packet::drivetrain_name(*dt),
            );
        }
        let gear_list: Vec<String> = gears.iter().map(u8::to_string).collect();
        println!(
            "max speed {:.1} mph | max rpm {:.0} | gears [{}] | max |combined slip| {:.2} | max tire temp {:.0}",
            max_speed * util::MPS_TO_MPH,
            max_rpm,
            gear_list.join(","),
            max_combined_slip,
            max_tire_temp,
        );
    }

    if packets == 0 {
        return Err("session contains no packets".into());
    }
    if errors > 0 {
        return Err(format!("{errors} of {packets} packets failed to decode"));
    }
    Ok(())
}
