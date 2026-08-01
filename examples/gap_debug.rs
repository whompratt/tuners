//! Per-recording race-off gap forensics: the race clock, lap, distance, and
//! speed on both sides of every gap, plus the resulting driving segments.
//! Used to diagnose gap misclassification (pauses vs rewinds vs restarts vs
//! results-screen clock runs).

use tuners::analysis::{self, Stint};

fn main() {
    let path = std::env::args().nth(1).expect("usage: gap_debug <ftel>");
    let stint = Stint::load(path.as_ref()).unwrap();
    let frames = &stint.frames;
    println!("{} frames total", frames.len());

    let mut last_on: Option<usize> = None;
    let mut in_gap = false;
    for (i, tf) in frames.iter().enumerate() {
        if !tf.frame.is_race_on {
            in_gap = last_on.is_some();
            continue;
        }
        if in_gap {
            let b = &frames[last_on.unwrap()];
            let gap_wall = (tf.recv_us - b.recv_us) as f64 / 1e6;
            println!(
                "gap: wall {:.1}s | race_t {:.2} -> {:.2} (jump {:+.2}) | lap {} -> {} | dist {:.0} -> {:.0} | speed {:.1} -> {:.1}",
                gap_wall,
                b.frame.current_race_time,
                tf.frame.current_race_time,
                tf.frame.current_race_time - b.frame.current_race_time,
                b.frame.lap_number,
                tf.frame.lap_number,
                b.frame.distance_traveled,
                tf.frame.distance_traveled,
                b.frame.speed,
                tf.frame.speed,
            );
            in_gap = false;
        }
        last_on = Some(i);
    }

    for (si, seg) in analysis::driving_segments(frames, 5.0).iter().enumerate() {
        println!(
            "segment {}: {:.1}s race-clock, {} frames, race_t {:.1}..{:.1}",
            si + 1,
            analysis::stint_seconds(seg),
            seg.len(),
            seg.first().unwrap().frame.current_race_time,
            seg.last().unwrap().frame.current_race_time,
        );
    }
}
