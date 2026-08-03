//! Advise cost harness: runs the full advise engine twice in one process
//! (cold product cache, then warm — the app's steady state) and reports
//! wall time plus peak RSS. Run from a data root, like the CLI.

use std::path::Path;

fn rss() -> (String, String) {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let grab = |key: &str| {
        status
            .lines()
            .find(|l| l.starts_with(key))
            .map(|l| l.split_whitespace().skip(1).collect::<Vec<_>>().join(" "))
            .unwrap_or_default()
    };
    (grab("VmRSS"), grab("VmHWM"))
}

fn main() {
    let journal = std::env::args()
        .nth(1)
        .expect("usage: advise_bench <journal-file>");
    for pass in ["cold", "warm"] {
        let t = std::time::Instant::now();
        let view =
            tuners::advice::advise::advise(&journal, Path::new("tune-session.txt"), "sessions")
                .expect("advise");
        let (rss_now, rss_peak) = rss();
        println!(
            "{pass}: {:.2?}, {} steps, rss {rss_now} (peak {rss_peak})",
            t.elapsed(),
            view.steps.len()
        );
    }
}
