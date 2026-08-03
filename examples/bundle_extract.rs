//! Unpack received telemetry bundles into a plain campaign layout (stint
//! .ftel files + the newest bundle's session/journal texts) so the analysis
//! tools can run on shared data directly.

use std::path::Path;

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: bundle_extract <out-dir> <bundle.tar.zst>...");
        std::process::exit(2);
    }
    let out = std::path::PathBuf::from(args.remove(0));
    std::fs::create_dir_all(&out).unwrap();
    args.sort();
    for path in &args {
        let bytes = std::fs::read(path).unwrap();
        let b = tuners::sharing::bundle::open(&bytes).unwrap_or_else(|e| {
            eprintln!("{path}: {e}");
            std::process::exit(1);
        });
        // bundle-<car>-<stamp>-<hash>.tar.zst -> stint-<stamp>.ftel
        let name = Path::new(path).file_name().unwrap().to_string_lossy();
        let parts: Vec<&str> = name.split('-').collect();
        let stamp = format!("{}-{}", parts[2], parts[3]);
        let stint_path = out.join(format!("stint-{stamp}.ftel"));
        std::fs::write(&stint_path, &b.stint).unwrap();
        // Later bundles carry a superset journal; last write wins.
        std::fs::write(out.join("tune-session.txt"), &b.session_txt).unwrap();
        std::fs::write(out.join("journal.txt"), &b.journal_txt).unwrap();
        println!(
            "{} -> {} ({} bytes, journal {} lines)",
            name,
            stint_path.display(),
            b.stint.len(),
            b.journal_txt.lines().count()
        );
    }
}
