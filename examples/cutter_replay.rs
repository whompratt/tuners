//! Replay recorded sessions through the Cutter to see where today's boundary
//! rules would cut them. Recordings only contain frames the original cutter
//! wrote, so a replay reproduces the original single session unless a newer
//! rule (the eager finish-certificate close) splits it: sessions > 1 means
//! the file holds finished runs that would now finalize immediately.
//!
//! Usage: cutter_replay [--write <out-dir>] <ftel-or-dir> [...]
//! With --write, each replayed session is written out as
//! <stem>-partN.ftel so the analysis tools can run on the splits.

use std::path::{Path, PathBuf};
use tuners::telemetry::record::{Action, Cutter};
use tuners::telemetry::stint::{StintReader, StintWriter};

fn replay(path: &Path, write_dir: Option<&Path>) {
    let Ok(mut reader) = StintReader::open(path) else {
        println!("{}: unreadable", path.display());
        return;
    };
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let mut cutter = Cutter::default();
    let mut sessions = 0u32;
    let mut writes = 0u64;
    let mut boundaries: Vec<String> = Vec::new();
    let mut first_us: Option<u64> = None;
    let mut writer: Option<StintWriter> = None;
    while let Ok(Some((recv_us, payload))) = reader.next_packet() {
        let t0 = *first_us.get_or_insert(recv_us);
        for action in cutter.feed(recv_us, &payload) {
            match action {
                Action::Open { .. } => {
                    sessions += 1;
                    if let Some(dir) = write_dir {
                        let out = dir.join(format!("{stem}-part{sessions}.ftel"));
                        writer = Some(StintWriter::create(&out).unwrap());
                    }
                }
                Action::Write { recv_us, payload } => {
                    writes += 1;
                    if let Some(w) = &mut writer {
                        w.write_packet(recv_us, &payload).unwrap();
                    }
                }
                Action::Close => {
                    writer = None;
                    boundaries.push(format!("close@{:.0}s", (recv_us - t0) as f64 / 1e6));
                }
            }
        }
    }
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    if sessions > 1 {
        println!(
            "{name}: {sessions} sessions ({}), {writes} frames written",
            boundaries.join(", ")
        );
    } else {
        println!("{name}: 1 session, {writes} frames written");
    }
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut write_dir: Option<PathBuf> = None;
    if args.first().is_some_and(|a| a == "--write") {
        args.remove(0);
        let dir = PathBuf::from(args.remove(0));
        std::fs::create_dir_all(&dir).unwrap();
        write_dir = Some(dir);
    }
    if args.is_empty() {
        eprintln!("usage: cutter_replay [--write <out-dir>] <ftel-or-dir> [...]");
        std::process::exit(1);
    }
    let mut files: Vec<PathBuf> = Vec::new();
    for a in &args {
        let p = PathBuf::from(a);
        if p.is_dir() {
            let mut in_dir: Vec<PathBuf> = std::fs::read_dir(&p)
                .unwrap()
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "ftel"))
                .collect();
            in_dir.sort();
            files.extend(in_dir);
        } else {
            files.push(p);
        }
    }
    for f in &files {
        replay(f, write_dir.as_deref());
    }
}
