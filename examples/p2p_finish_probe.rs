//! Finish-certificate forensics: dump frames around every race-on/race-off
//! edge to see where a run's LastLap lands relative to the race-off at the
//! finish line.
//!
//! VERDICT (2026-08-05, Celica point-to-point sprints): the game goes
//! race-off AT the line before any frame carries the run time; the official
//! time arrives in later results-screen flicker (LapNumber+1, LastLap set,
//! race clock run forward, position garbage), sometimes a single frame,
//! sometimes at the head of the NEXT recording when the idle cut fell in
//! between. driving_segments adopts these certificates (finish_certificate);
//! products::cached recovers the cross-recording case. See telemetry.md.

use tuners::analysis::Stint;

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: p2p_finish_probe <ftel>");
    let stint = Stint::load(path.as_ref()).unwrap();
    let frames = &stint.frames;
    let mut prev_on = true;
    for (i, tf) in frames.iter().enumerate() {
        let f = &tf.frame;
        if f.is_race_on != prev_on {
            let lo = i.saturating_sub(3);
            let hi = (i + 12).min(frames.len());
            println!(
                "--- edge at frame {i} (race_on {} -> {}) ---",
                prev_on, f.is_race_on
            );
            for (j, tf) in frames.iter().enumerate().take(hi).skip(lo) {
                let g = &tf.frame;
                println!(
                    "  [{j}] on={} race_t={:8.2} lap={} cur_lap={:8.2} last_lap={:8.3} dist={:8.1} speed={:5.1}",
                    g.is_race_on as u8,
                    g.current_race_time,
                    g.lap_number,
                    g.current_lap,
                    g.last_lap,
                    g.distance_traveled,
                    g.speed * 2.237,
                );
            }
            prev_on = f.is_race_on;
        }
    }
}
