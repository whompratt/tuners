//! Diagnostic: corroboration confidence per session, used to calibrate the
//! dashboard gauge bands (docs/plans/006-dashboard.md).
//!
//!     cargo run --example confidence -- sessions/*.ftel

fn main() {
    println!(
        "{:<44} {:>4} {:>9} {:>7} {:>6}  band",
        "session", "laps", "best", "spread", "conf"
    );
    for path in std::env::args().skip(1) {
        let profile = tuners::analysis::Session::load(path.as_ref())
            .map_err(|e| e.to_string())
            .and_then(|s| tuners::analysis::profile::session_profile(&s.frames));
        let name = path.rsplit('/').next().unwrap_or(&path).to_string();
        match profile {
            Ok(p) => {
                let worst = p.laps.iter().map(|l| l.time_s).fold(0.0f32, f32::max);
                let spread = (worst - p.best_lap_time_s).max(0.0) / p.best_lap_time_s;
                let c = p.corroboration();
                println!(
                    "{:<44} {:>4} {:>9} {:>6.1}% {:>5.0}%  {}",
                    name,
                    p.laps.len(),
                    tuners::util::format_lap_time(p.best_lap_time_s),
                    spread * 100.0,
                    c.score * 100.0,
                    tuners::live::Band::from_score(c.score).as_str(),
                );
            }
            Err(e) => println!("{name:<44} -- {e}"),
        }
    }
}
