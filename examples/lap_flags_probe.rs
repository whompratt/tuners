//! Per-lap classification forensics for a recording: every driving segment's
//! laps with their standing-start/point-to-point flags, first-frame clock and
//! speed, and whether the lap profiled. Answers "why is this run missing from
//! the profile" (e.g. a severed launch leaves the lap starting mid-run).

use tuners::analysis::{self, Stint};

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: lap_flags_probe <ftel>");
    let stint = Stint::load(path.as_ref()).unwrap();
    let frames = &stint.frames;
    let segments = analysis::driving_segments(frames, 5.0);
    println!("{} driving segment(s)", segments.len());
    for (si, seg) in segments.iter().enumerate() {
        for lap in analysis::split_laps(seg) {
            let first = &lap.frames.first().unwrap().frame;
            let prof = analysis::profile::lap_profile(&lap);
            println!(
                "seg {si} lap {}: time {:?} standing={} p2p={} first(race_t {:.2} lap_t {:.2} speed {:.1}) profiled={}",
                lap.number,
                lap.time_s,
                lap.standing_start,
                lap.point_to_point,
                first.current_race_time,
                first.current_lap,
                first.speed,
                prof.is_some(),
            );
        }
    }
}
