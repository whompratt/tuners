//! Validate the bundle v2 transpose against real recordings: every file
//! must round-trip byte-identical, and any fallback to the raw v1 layout
//! is reported with its reason (a fallback on healthy data is a bug, not
//! an accident to absorb silently). Also reports the compression ratio
//! both ways so the v2 win stays a measured number.

use tuners::sharing::bundle::{transpose_recording, untranspose_recording};

fn main() {
    let mut transposed = 0usize;
    let mut fallbacks = 0usize;
    let mut mismatches = 0usize;
    let (mut raw_total, mut zv1_total, mut zv2_total) = (0u64, 0u64, 0u64);
    println!("file\traw_mb\tv1_ratio\tv2_ratio\tstatus");
    for path in std::env::args().skip(1) {
        let raw = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("{path}: {e}");
                continue;
            }
        };
        let zv1 = zstd::stream::encode_all(&raw[..], 9).unwrap().len() as u64;
        match transpose_recording(&raw) {
            Ok(t) => {
                let back = untranspose_recording(&t).expect("reconstruct");
                let status = if back == raw {
                    transposed += 1;
                    "ok"
                } else {
                    mismatches += 1;
                    "MISMATCH"
                };
                let zv2 = zstd::stream::encode_all(&t[..], 9).unwrap().len() as u64;
                raw_total += raw.len() as u64;
                zv1_total += zv1;
                zv2_total += zv2;
                println!(
                    "{path}\t{:.1}\t{:.2}\t{:.2}\t{status}",
                    raw.len() as f64 / 1e6,
                    raw.len() as f64 / zv1 as f64,
                    raw.len() as f64 / zv2 as f64,
                );
            }
            Err(reason) => {
                fallbacks += 1;
                println!(
                    "{path}\t{:.1}\t{:.2}\t-\tFALLBACK: {reason}",
                    raw.len() as f64 / 1e6,
                    raw.len() as f64 / zv1 as f64,
                );
            }
        }
    }
    println!("\n{transposed} transposed ok, {fallbacks} fallbacks, {mismatches} mismatches");
    if raw_total > 0 {
        println!(
            "totals: raw {:.1} MB | v1 {:.1} MB ({:.2}x) | v2 {:.1} MB ({:.2}x)",
            raw_total as f64 / 1e6,
            zv1_total as f64 / 1e6,
            raw_total as f64 / zv1_total as f64,
            zv2_total as f64 / 1e6,
            raw_total as f64 / zv2_total as f64,
        );
    }
    if mismatches > 0 || fallbacks > 0 {
        std::process::exit(1);
    }
}
