//! Verdict-currency survey over journaled campaigns: for every stint pair,
//! compare the ideal-composite delta against the best-lap and median-lap
//! deltas (the production vote's components). Pairs are labeled same-setup
//! (the drift corpus: true effect is zero), single-area, or multi-area from
//! the bound tune revisions. TSV to stdout; run from the data root.
//!
//! Adjacent journal pairs are emitted plus non-adjacent same-setup pairs
//! (flagged adj=0). Implicit trailing stints (driven after the last journal
//! write) are not scanned; journaled entries only.

use std::path::Path;
use tuners::advice::{journal, tuning};
use tuners::analysis::{self, profile, profile::StintProfile};

fn stamp_of(path: &str) -> Option<String> {
    Path::new(path)
        .file_stem()?
        .to_string_lossy()
        .strip_prefix("stint-")
        .map(str::to_string)
}

struct Row {
    profile: StintProfile,
    stamp: String,
}

fn lap_stats(p: &StintProfile) -> (f32, f32) {
    (p.best_lap_time_s, p.median_lap_time_s())
}

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let root = Path::new(&root);
    println!("journal\ti\tj\tadj\tkind\tareas\tn_i\tn_j\tideal_d\tbest_d\tmedlap_d");

    let mut names: Vec<String> = std::fs::read_dir(root)
        .expect("read root")
        .flatten()
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            (n.starts_with("tune-journal-") && n.ends_with(".txt")).then_some(n)
        })
        .collect();
    names.sort();

    let active = tuning::TuningSession::load(&root.join("tune-session.txt"));
    for name in &names {
        let stem = name.trim_end_matches(".txt");
        let parts: Vec<&str> = stem
            .strip_prefix("tune-journal-")
            .unwrap_or_default()
            .split('-')
            .collect();
        let session_path = match parts[..] {
            [car, d8, t6] if car.parse::<i32>().is_ok() && d8.len() == 8 && t6.len() == 6 => {
                root.join(format!("tune-session-{car}-{d8}-{t6}.txt"))
            }
            [car] if car.parse::<i32>().is_ok() => {
                if active.car == car.parse::<i32>().ok() {
                    root.join("tune-session.txt")
                } else {
                    root.join(format!("tune-session-{car}.txt"))
                }
            }
            _ => continue,
        };
        if !session_path.exists() {
            continue;
        }
        let session = tuning::TuningSession::load(&session_path);
        let text = std::fs::read_to_string(root.join(name)).expect("journal");

        // Profile every journaled stint that still exists, with its bound setup.
        let mut rows: Vec<Row> = Vec::new();
        for e in journal::parse_journal(&text) {
            let Some(stamp) = stamp_of(&e.path) else {
                continue;
            };
            let p = root.join(&e.path);
            let Ok(stint) = analysis::Stint::load(&p) else {
                eprintln!("{name}: {} missing/unreadable, skipped", e.path);
                continue;
            };
            match profile::stint_profile(&stint.frames) {
                Ok(p) if !p.standing_start_only && p.laps.len() >= 2 => {
                    rows.push(Row { profile: p, stamp })
                }
                _ => eprintln!("{name}: {} unprofiled/thin, skipped", e.path),
            }
        }

        let setup = |stamp: &str| -> Option<&tuning::Revision> {
            session
                .revisions
                .iter()
                .rev()
                .find(|r| r.stamp.as_str() < stamp)
        };
        let emit = |i: usize, j: usize, adj: bool| {
            let (kind, areas) = match (setup(&rows[i].stamp), setup(&rows[j].stamp)) {
                (Some(a), Some(b)) => {
                    let mut areas: Vec<String> = tuning::diff_keys(a, b)
                        .iter()
                        .map(|k| tuning::field_area(k).to_string())
                        .collect();
                    areas.sort();
                    areas.dedup();
                    match areas.len() {
                        0 => ("same", String::new()),
                        1 => ("single", areas.join(",")),
                        _ => ("multi", areas.join(",")),
                    }
                }
                _ => ("unbound", String::new()),
            };
            let (bi, mi) = lap_stats(&rows[i].profile);
            let (bj, mj) = lap_stats(&rows[j].profile);
            println!(
                "{name}\t{}\t{}\t{}\t{kind}\t{areas}\t{}\t{}\t{:.3}\t{:.3}\t{:.3}",
                rows[i].stamp,
                rows[j].stamp,
                adj as u8,
                rows[i].profile.laps.len(),
                rows[j].profile.laps.len(),
                rows[j].profile.composite.time_s - rows[i].profile.composite.time_s,
                bj - bi,
                mj - mi,
            );
        };
        for j in 1..rows.len() {
            for i in 0..j {
                let adj = j == i + 1;
                if adj {
                    emit(i, j, true);
                } else if let (Some(a), Some(b)) = (setup(&rows[i].stamp), setup(&rows[j].stamp))
                    && tuning::diff_keys(a, b).is_empty()
                {
                    emit(i, j, false);
                }
            }
        }
    }
}
