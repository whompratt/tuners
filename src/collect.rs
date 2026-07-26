//! Opt-in telemetry collection, sender side (plan 009 phase 2): consent
//! config, the outbox spool, and the idle-gated drainer.
//!
//! Consent lives in `tune-collect.txt` (default OFF; the dashboard toggle
//! writes it). The token is generated CLIENT-side at first opt-in — identity,
//! not authentication: the endpoint namespaces senders by sha256(token)[..16]
//! and issues nothing. On stint finalization the recorder calls
//! `maybe_enqueue`, which bundles into `outbox/` on a spawned thread; capture
//! is never coupled to the network. The drainer runs in `tuners serve`,
//! uploads oldest-first, and deletes only on confirmed 2xx.
//!
//! Uploads shell out to `curl` (ships with Windows 10+, macOS, and
//! effectively all Linux): TLS without growing the dependency tree, and the
//! exact invocation was validated against the live endpoint. Hard gate: the
//! caller passes a liveness probe and the drainer refuses to touch the
//! network while telemetry is fresh — driving must never compete with
//! uploads.

use crate::tuning::TuningSession;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const CONFIG_PATH: &str = "tune-collect.txt";
pub const OUTBOX_DIR: &str = "outbox";
/// Baked-in default so testers get zero-config opt-in.
pub const DEFAULT_ENDPOINT: &str = "https://tuners-collect.jaker1342.workers.dev";
/// UDP silence required before the drainer will upload.
pub const IDLE_BEFORE_DRAIN: std::time::Duration = std::time::Duration::from_secs(180);

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CollectConfig {
    pub enabled: bool,
    pub endpoint: String,
    pub token: String,
}

impl CollectConfig {
    pub fn parse(text: &str) -> CollectConfig {
        let mut cfg = CollectConfig::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else { continue };
            match (k.trim(), v.trim()) {
                ("enabled", v) => cfg.enabled = v == "on" || v == "true" || v == "1",
                ("endpoint", v) => cfg.endpoint = v.to_string(),
                ("token", v) => cfg.token = v.to_string(),
                _ => {}
            }
        }
        cfg
    }

    pub fn load(path: &Path) -> CollectConfig {
        Self::parse(&std::fs::read_to_string(path).unwrap_or_default())
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        std::fs::write(
            path,
            format!(
                "# tuners telemetry collection (docs/plans/009-data-collection.md)\n\
                 # enabled = on shares stint bundles (raw driving telemetry, setup\n\
                 # values, tune deltas — no names, no free text) for tool development.\n\
                 enabled = {}\nendpoint = {}\ntoken = {}\n",
                if self.enabled { "on" } else { "off" },
                self.endpoint,
                self.token,
            ),
        )
    }

    /// Usable for uploads: enabled with a plausible token and endpoint.
    pub fn ready(&self) -> bool {
        self.enabled && self.endpoint.starts_with("http") && self.token.len() == 64
    }
}

/// The pseudonymous identity the endpoint derives — shown in the dashboard so
/// delete-on-request has something to quote.
pub fn sender_id(token: &str) -> String {
    crate::util::sha256_hex(token.to_ascii_lowercase().as_bytes())[..16].to_string()
}

/// 64-hex client token. /dev/urandom where it exists; elsewhere a hash of
/// volatile process state — weaker, but the token is pseudonymous identity,
/// not a defended credential (worst case is polluting your own namespace).
pub fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    let urandom = std::fs::File::open("/dev/urandom")
        .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut bytes));
    if urandom.is_ok() {
        return bytes.iter().map(|b| format!("{b:02x}")).collect();
    }
    let mut h = crate::util::Sha256::new();
    h.update(format!("{:?}", std::time::SystemTime::now()).as_bytes());
    h.update(format!("{:?}", std::time::Instant::now()).as_bytes());
    h.update(&std::process::id().to_le_bytes());
    for (k, v) in std::env::vars() {
        h.update(k.as_bytes());
        h.update(v.as_bytes());
    }
    h.finish_hex()
}

/// Bundle a finalized stint into the outbox if collection is on and the stint
/// belongs to the session car. Runs on its own thread — the recorder loop
/// must never wait on bundling. Failures are printed, never fatal.
pub fn maybe_enqueue(stint_path: PathBuf, session_file: PathBuf, car: i32) {
    let cfg = CollectConfig::load(CONFIG_PATH.as_ref());
    if !cfg.ready() {
        return;
    }
    std::thread::spawn(move || {
        let session = TuningSession::load(&session_file);
        if session.car != Some(car) {
            println!(
                "collect: {} is car {car}, session car {:?} — not bundled",
                stint_path.display(),
                session.car
            );
            return;
        }
        let journal_path =
            crate::tuning::journal_path_for(session.car, "tune-journal.txt");
        let journal = std::fs::read_to_string(&journal_path).unwrap_or_default();
        match enqueue(OUTBOX_DIR.as_ref(), &stint_path, &session, &journal) {
            Ok(Some(p)) => println!("collect: queued {}", p.display()),
            Ok(None) => {}
            Err(e) => eprintln!("collect: {} not bundled: {e}", stint_path.display()),
        }
    });
}

/// Build the bundle into the outbox. Ok(None) = already queued.
pub fn enqueue(
    outbox: &Path,
    stint_path: &Path,
    session: &TuningSession,
    journal: &str,
) -> Result<Option<PathBuf>, String> {
    let (name, bytes) = crate::bundle::build(stint_path, session, journal)?;
    std::fs::create_dir_all(outbox).map_err(|e| e.to_string())?;
    let path = outbox.join(&name);
    if path.exists() {
        return Ok(None);
    }
    // Write-then-rename so the drainer never sees a half-written bundle.
    let tmp = outbox.join(format!(".part-{name}"));
    std::fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
    Ok(Some(path))
}

/// Queued bundles, oldest first (mtime).
pub fn queued(outbox: &Path) -> Vec<PathBuf> {
    let Ok(rd) = std::fs::read_dir(outbox) else {
        return Vec::new();
    };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = rd
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            let name = p.file_name()?.to_str()?;
            if !name.ends_with(".tar.zst") || name.starts_with('.') {
                return None;
            }
            Some((e.metadata().ok()?.modified().ok()?, p))
        })
        .collect();
    files.sort();
    files.into_iter().map(|(_, p)| p).collect()
}

/// One drain pass. `telemetry_fresh` is re-checked between files so a drive
/// that starts mid-drain stops the uploads. Returns human-readable log lines
/// (also printed by the serve loop).
pub fn drain(
    outbox: &Path,
    cfg: &CollectConfig,
    telemetry_fresh: &dyn Fn() -> bool,
) -> Vec<String> {
    let mut log = Vec::new();
    for path in queued(outbox) {
        if telemetry_fresh() {
            log.push("drain paused: telemetry active".into());
            break;
        }
        let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        match upload(&cfg.endpoint, &cfg.token, &path) {
            Ok(code) if (200..300).contains(&code) => {
                let _ = std::fs::remove_file(&path);
                // Ledger of shared bundles: keeps "share existing recordings"
                // idempotent (uploaded bundles leave the outbox, so the file
                // system alone can't remember them).
                let _ = std::fs::create_dir_all(outbox);
                if let Ok(mut f) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(outbox.join("sent.txt"))
                {
                    use std::io::Write as _;
                    let _ = writeln!(f, "{name}");
                }
                log.push(format!("uploaded {name}"));
            }
            // Permanent rejections: retrying forever would spin — park them
            // where a human can look.
            Ok(code @ (400 | 401 | 403 | 413 | 422)) => {
                let rejected = outbox.join("rejected");
                let _ = std::fs::create_dir_all(&rejected);
                let _ = std::fs::rename(&path, rejected.join(&name));
                log.push(format!("{name}: endpoint says {code} — moved to outbox/rejected"));
            }
            // Caps, server trouble, offline: everything waits for the next pass.
            Ok(code) => {
                log.push(format!("{name}: endpoint says {code} — will retry later"));
                break;
            }
            Err(e) => {
                log.push(format!("{name}: {e} — will retry later"));
                break;
            }
        }
    }
    log
}

/// Bundle names confirmed uploaded (the drainer's ledger): uploaded bundles
/// leave the outbox, so the filesystem alone can't remember them.
fn sent_names(outbox: &Path) -> std::collections::BTreeSet<String> {
    std::fs::read_to_string(outbox.join("sent.txt"))
        .map(|t| {
            t.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect()
        })
        .unwrap_or_default()
}

/// What "share existing recordings" would do (plan 009 addendum): historic
/// stints are shared per CAMPAIGN — each journaled stint pairs with its own
/// campaign's session + journal, so archived campaigns ship with honest
/// context instead of the active session's. Unjournaled recordings have no
/// campaign to interpret them with and are only counted.
#[derive(Debug, Default)]
pub struct HistoryPlan {
    /// (stint, session file, journal file) per bundle to build.
    pub items: Vec<(PathBuf, PathBuf, PathBuf)>,
    pub campaigns: usize,
    /// Raw bytes of the recordings to be bundled (pre-compression).
    pub bytes: u64,
    pub unjournaled: usize,
    /// Skipped because already queued or already confirmed uploaded.
    pub already: usize,
}

/// Campaign pairs under `root`: the active session + its car's journal, plus
/// every archived pair (tune-session-<id>.txt / tune-journal-<id>.txt —
/// covers both the stamped and the legacy car-switch naming).
fn campaign_pairs(root: &Path) -> Vec<(PathBuf, PathBuf)> {
    let mut out = Vec::new();
    let active = root.join("tune-session.txt");
    let session = TuningSession::load(&active);
    if session.car.is_some() {
        let base = root.join("tune-journal.txt");
        let journal =
            crate::tuning::journal_path_for(session.car, &base.to_string_lossy());
        out.push((active, PathBuf::from(journal)));
    }
    let mut archived = Vec::new();
    if let Ok(rd) = std::fs::read_dir(root) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if let Some(id) =
                name.strip_prefix("tune-session-").and_then(|n| n.strip_suffix(".txt"))
            {
                archived.push((e.path(), root.join(format!("tune-journal-{id}.txt"))));
            }
        }
    }
    archived.sort();
    out.extend(archived);
    out
}

pub fn history_plan(root: &Path, sessions_dir: &str, outbox: &Path) -> HistoryPlan {
    let sent = sent_names(outbox);
    let mut plan = HistoryPlan::default();
    let mut seen: std::collections::BTreeSet<PathBuf> = Default::default();
    for (spath, jpath) in campaign_pairs(root) {
        let session = TuningSession::load(&spath);
        let Some(car) = session.car else { continue };
        let Ok(jtext) = std::fs::read_to_string(&jpath) else { continue };
        let mut used = false;
        for entry in crate::analysis::journal::parse_journal(&jtext) {
            let stint = root.join(&entry.path);
            if !seen.insert(stint.clone()) {
                continue; // parked/resumed campaigns can list a stint twice
            }
            let Ok(md) = stint.metadata() else { continue }; // recording deleted
            let Ok(name) = crate::bundle::bundle_name(car, &stint) else { continue };
            if sent.contains(&name) || outbox.join(&name).exists() {
                plan.already += 1;
                continue;
            }
            plan.bytes += md.len();
            plan.items.push((stint, spath.clone(), jpath.clone()));
            used = true;
        }
        if used {
            plan.campaigns += 1;
        }
    }
    if let Ok(rd) = std::fs::read_dir(root.join(sessions_dir)) {
        plan.unjournaled = rd
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "ftel"))
            .filter(|e| !seen.contains(&e.path()))
            .count();
    }
    plan
}

/// Bundle each planned stint into the outbox (worker-thread work: bundling
/// re-decodes every packet). Returns how many were queued; the drainer
/// uploads them on its usual idle-gated schedule.
pub fn history_enqueue(plan: HistoryPlan, outbox: &Path) -> usize {
    let mut cache: std::collections::BTreeMap<PathBuf, (TuningSession, String)> =
        Default::default();
    let mut queued = 0;
    for (stint, spath, jpath) in plan.items {
        let (session, journal) = cache.entry(spath.clone()).or_insert_with(|| {
            (
                TuningSession::load(&spath),
                std::fs::read_to_string(&jpath).unwrap_or_default(),
            )
        });
        match enqueue(outbox, &stint, session, journal) {
            Ok(Some(p)) => {
                println!("collect: queued {}", p.display());
                queued += 1;
            }
            Ok(None) => {}
            Err(e) => eprintln!("collect: {} not bundled: {e}", stint.display()),
        }
    }
    println!("collect: history backfill queued {queued} bundle(s)");
    queued
}

/// PUT one bundle via curl; returns the HTTP status code.
fn upload(endpoint: &str, token: &str, path: &Path) -> Result<u16, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let sha = crate::util::sha256_hex(&bytes);
    let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
    let url = format!("{}/v1/bundle/{name}", endpoint.trim_end_matches('/'));
    let null = if cfg!(windows) { "NUL" } else { "/dev/null" };
    let out = Command::new("curl")
        .args([
            "-s",
            "-o",
            null,
            "-w",
            "%{http_code}",
            "-m",
            "300",
            "-X",
            "PUT",
            "--data-binary",
            &format!("@{}", path.display()),
            "-H",
            &format!("Authorization: Bearer {token}"),
            "-H",
            &format!("X-Bundle-SHA256: {sha}"),
            &url,
        ])
        .output()
        .map_err(|e| format!("curl not runnable: {e}"))?;
    let code = String::from_utf8_lossy(&out.stdout).trim().parse::<u16>().unwrap_or(0);
    if code == 0 {
        return Err(format!(
            "no response ({})",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_round_trip_and_defaults() {
        let cfg = CollectConfig {
            enabled: true,
            endpoint: DEFAULT_ENDPOINT.into(),
            token: "a".repeat(64),
        };
        let dir = std::env::temp_dir().join(format!("tuners-collect-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("collect.txt");
        cfg.save(&path).unwrap();
        assert_eq!(CollectConfig::load(&path), cfg);
        assert!(cfg.ready());

        assert_eq!(CollectConfig::parse(""), CollectConfig::default());
        assert!(!CollectConfig::default().ready(), "default is OFF");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tokens_are_wellformed_and_distinct() {
        let (a, b) = (generate_token(), generate_token());
        for t in [&a, &b] {
            assert_eq!(t.len(), 64);
            assert!(t.bytes().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        }
        assert_ne!(a, b);
        // Same derivation the endpoint uses (pinned in tests/receive.rs too).
        assert_eq!(sender_id(&"a".repeat(64)), "ffe054fe7ae0cb6d");
    }
}
