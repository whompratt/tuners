//! Ingest: file received bundles into the per-sender
//! library, in strict mode. Every bundle is untrusted input — `bundle::open`
//! already enforces hashes/members/version, and this layer re-runs the REAL
//! parsers on top: every packet must decode, the session must belong to the
//! manifest's car, and the free-text strip must hold (the export filters are
//! idempotent, so `filter(member) == member` proves nothing slipped through).
//! Anything that fails moves to quarantine with a written reason instead of
//! being deleted — version skew and half-stints are data about failures.
//!
//! The inbox (an rclone mirror of the bucket) is never mutated: ingest COPIES
//! survivors so re-syncs and re-runs are idempotent. Layout:
//!   inbox/<sender>/bundle-*.tar.zst   ->  library/<sender>/...
//!                                     or  quarantine/<sender>/... (+ .reason.txt)

use std::path::{Path, PathBuf};

#[derive(Debug, Default)]
pub struct IngestReport {
    pub ingested: Vec<String>,
    pub skipped: usize,
    pub quarantined: Vec<(String, String)>,
}

pub fn ingest_dir(
    inbox: &Path,
    library: &Path,
    quarantine: &Path,
) -> std::io::Result<IngestReport> {
    let mut report = IngestReport::default();
    for (sender, path) in discover(inbox)? {
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let label = format!("{sender}/{name}");
        let bytes = std::fs::read(&path)?;

        let dest = library.join(&sender).join(&name);
        if dest.exists() {
            if std::fs::read(&dest)? == bytes {
                report.skipped += 1;
            } else {
                // Same name, different content: manual exports have no hash
                // suffix, so a re-cut stint can collide. A human decides.
                park(
                    quarantine,
                    &sender,
                    &name,
                    &bytes,
                    "name collision with different content",
                )?;
                report.quarantined.push((label, "name collision".into()));
            }
            continue;
        }
        if quarantine.join(&sender).join(&name).exists() {
            report.skipped += 1;
            continue;
        }

        match crate::bundle::open(&bytes).and_then(|b| validate(&b)) {
            Ok(()) => {
                std::fs::create_dir_all(dest.parent().unwrap())?;
                std::fs::write(&dest, &bytes)?;
                report.ingested.push(label);
            }
            Err(reason) => {
                park(quarantine, &sender, &name, &bytes, &reason)?;
                report.quarantined.push((label, reason));
            }
        }
    }
    Ok(report)
}

/// `<inbox>/<sender>/*.tar.zst`, plus bare `*.tar.zst` under sender "local"
/// (hand-delivered manual exports).
fn discover(inbox: &Path) -> std::io::Result<Vec<(String, PathBuf)>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(inbox)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            if name == "rejected" {
                continue; // receiver-side parking area, not sender data
            }
            for f in std::fs::read_dir(&path)? {
                let f = f?.path();
                if f.extension().is_some_and(|e| e == "zst")
                    && f.to_string_lossy().ends_with(".tar.zst")
                {
                    out.push((name.clone(), f));
                }
            }
        } else if name.ends_with(".tar.zst") {
            out.push(("local".into(), path));
        }
    }
    out.sort();
    Ok(out)
}

fn park(
    quarantine: &Path,
    sender: &str,
    name: &str,
    bytes: &[u8],
    reason: &str,
) -> std::io::Result<()> {
    let dir = quarantine.join(sender);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join(name), bytes)?;
    std::fs::write(
        dir.join(format!("{name}.reason.txt")),
        format!("{reason}\n"),
    )
}

/// Strict-mode checks beyond `bundle::open`'s structural verification.
fn validate(b: &crate::bundle::Bundle) -> Result<(), String> {
    let car: i32 = b
        .manifest
        .get("car")
        .and_then(|c| c.parse().ok())
        .ok_or("manifest car is not an integer")?;
    let claimed_packets: u64 = b
        .manifest
        .get("packets")
        .and_then(|p| p.parse().ok())
        .ok_or("manifest packets is not a number")?;

    // Every record and every payload must decode with the real parsers.
    let mut packets = 0u64;
    let mut reader = crate::stint::StintReader::open_bytes(&b.stint).map_err(|e| e.to_string())?;
    while let Some((_us, payload)) = reader
        .next_packet()
        .map_err(|e| format!("stint record {packets}: {e}"))?
    {
        crate::packet::decode(&payload).map_err(|e| format!("stint packet {packets}: {e:?}"))?;
        packets += 1;
    }
    if packets != claimed_packets {
        return Err(format!(
            "stint has {packets} packets, manifest claims {claimed_packets}"
        ));
    }

    let session = crate::tuning::TuningSession::parse(&b.session_txt);
    if session.car != Some(car) {
        return Err(format!(
            "session car {:?} != manifest car {car}",
            session.car
        ));
    }
    // The free-text strip must hold: the export filters are idempotent, so a
    // compliant member is a fixed point. Anything else smuggled text.
    let refiltered = crate::bundle::export_session(&session).render();
    if refiltered != b.session_txt {
        return Err("session.txt is not free-text-clean (filter is not a fixed point)".into());
    }
    if crate::bundle::export_journal(&b.journal_txt, car) != b.journal_txt {
        return Err("journal.txt is not free-text-clean (filter is not a fixed point)".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_maps_senders_and_flat_files() {
        let dir = std::env::temp_dir().join(format!("tuners-discover-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("abc123")).unwrap();
        std::fs::create_dir_all(dir.join("rejected")).unwrap();
        std::fs::write(dir.join("abc123/b1.tar.zst"), b"x").unwrap();
        std::fs::write(dir.join("abc123/notes.txt"), b"x").unwrap();
        std::fs::write(dir.join("rejected/bad.tar.zst"), b"x").unwrap();
        std::fs::write(dir.join("hand.tar.zst"), b"x").unwrap();
        let found = discover(&dir).unwrap();
        let labels: Vec<String> = found
            .iter()
            .map(|(s, p)| format!("{s}/{}", p.file_name().unwrap().to_string_lossy()))
            .collect();
        assert_eq!(labels, vec!["abc123/b1.tar.zst", "local/hand.tar.zst"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
