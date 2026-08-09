//! Game-mode marker forensics: every transition of IsRaceOn,
//! DistanceTraveled zero/nonzero, LapNumber, CurrentLap resets, and
//! LastLap/BestLap updates, with clocks and speed for context.
//!
//! Verdict from the 2026-08-09 open-world time-attack capture (Ram TRX):
//! time attack is race-on with live DistanceTraveled (so it passes the
//! recorder gate and records) but ALL lap channels stay exactly 0 for the
//! whole event and no finish certificate arrives — runs cannot be timed.
//! Full write-up in docs/telemetry.md ("Open-world time attack").

use tuners::analysis::Stint;

fn main() {
    let path = std::env::args().nth(1).expect("usage: ta_probe <ftel>");
    let stint = Stint::load(path.as_ref()).unwrap();
    let frames = &stint.frames;
    let t0 = frames[0].recv_us;

    let mut prev: Option<&tuners::telemetry::packet::TelemetryFrame> = None;
    for tf in frames.iter() {
        let f = &tf.frame;
        let wall = (tf.recv_us - t0) as f64 / 1e6;
        let ctx = format!(
            "wall {wall:7.1}s | race_t {:8.2} | lap_t {:7.2} | lapno {} | dist {:8.0} | speed {:5.1} | last {:7.2} | best {:7.2}",
            f.current_race_time,
            f.current_lap,
            f.lap_number,
            f.distance_traveled,
            f.speed,
            f.last_lap,
            f.best_lap
        );
        match prev {
            None => println!("first frame        : {ctx}"),
            Some(p) => {
                if p.is_race_on != f.is_race_on {
                    println!("race_on {} -> {} : {ctx}", p.is_race_on, f.is_race_on);
                }
                if (p.distance_traveled == 0.0) != (f.distance_traveled == 0.0) {
                    println!(
                        "dist zero-edge     : {ctx} (was {:.0})",
                        p.distance_traveled
                    );
                }
                if p.lap_number != f.lap_number {
                    println!("lap_number {} -> {} : {ctx}", p.lap_number, f.lap_number);
                }
                if f.current_lap < p.current_lap - 0.5 {
                    println!("lap clock reset    : {ctx} (was {:.2})", p.current_lap);
                }
                if f.last_lap != p.last_lap {
                    println!("last_lap change    : {ctx} (was {:.2})", p.last_lap);
                }
                if f.best_lap != p.best_lap {
                    println!("best_lap change    : {ctx} (was {:.2})", p.best_lap);
                }
            }
        }
        prev = Some(f);
    }
    let (mut max_lap_t, mut max_last, mut max_best, mut race_frames) = (0f32, 0f32, 0f32, 0u32);
    for tf in frames.iter() {
        let f = &tf.frame;
        max_lap_t = max_lap_t.max(f.current_lap);
        max_last = max_last.max(f.last_lap);
        max_best = max_best.max(f.best_lap);
        if f.is_race_on && f.distance_traveled != 0.0 {
            race_frames += 1;
        }
    }
    println!(
        "maxima             : lap_t {max_lap_t:.2} | last {max_last:.2} | best {max_best:.2} | race-mode frames (race_on && dist!=0): {race_frames}"
    );
    let f = &frames.last().unwrap().frame;
    println!(
        "last frame         : race_on {} | race_t {:.2} | lap_t {:.2} | lapno {} | dist {:.0} | last {:.2} | best {:.2}",
        f.is_race_on,
        f.current_race_time,
        f.current_lap,
        f.lap_number,
        f.distance_traveled,
        f.last_lap,
        f.best_lap
    );
}
