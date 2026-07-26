//! Strict-mode ingest (plan 009 phase 3): good bundles are filed per sender,
//! re-runs are idempotent, and hostile bundles — including one with CORRECT
//! hashes but smuggled prose — land in quarantine with a written reason.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use tuners::bundle;
use tuners::ingest::ingest_dir;
use tuners::tuning::{Revision, TuningSession};
use tuners::util::sha256_hex;

fn dirs(tag: &str) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
    let base = std::env::temp_dir().join(format!("tuners-ingest-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let (inbox, lib, quar) = (base.join("inbox"), base.join("library"), base.join("quarantine"));
    std::fs::create_dir_all(&inbox).unwrap();
    (base, inbox, lib, quar)
}

fn good_bundle() -> (String, Vec<u8>) {
    let mut s = TuningSession { car: Some(4165), ..Default::default() };
    s.facts.insert("front_weight_pct".into(), "46".into());
    s.revisions.push(Revision {
        stamp: "20260719-224500".into(),
        values: [("arb_f".to_string(), "24".to_string())].into(),
    });
    let stint = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/real-01.ftel"));
    bundle::build(stint, &s, "fixtures/real-01.ftel | front arb -2\n").unwrap()
}

/// A bundle `build` would refuse: internally consistent (hashes correct)
/// but with prose smuggled into the journal member.
fn prose_bundle() -> Vec<u8> {
    let stint = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/real-01.ftel")).unwrap();
    let session_txt = "# tuners tuning session\ncar = 4165\n";
    let journal_txt = "# 2018 Audi TT RS (ordinal 4165)\n\
                       fixtures/real-01.ftel | front arb -2 because it felt awful near my house\n";
    let mut manifest = BTreeMap::new();
    manifest.insert("bundle_version".into(), "1".into());
    manifest.insert("tool_version".into(), "0.1.0".into());
    manifest.insert("car".into(), "4165".into());
    manifest.insert("stint_stamp".into(), "real-01".into());
    manifest.insert("packets".into(), "1200".into());
    manifest.insert("consent".into(), "test".into());
    manifest.insert("sha256_stint".into(), sha256_hex(&stint));
    manifest.insert("sha256_session".into(), sha256_hex(session_txt.as_bytes()));
    manifest.insert("sha256_journal".into(), sha256_hex(journal_txt.as_bytes()));
    let mut tar = Vec::new();
    bundle::tar_append(&mut tar, "manifest.json", bundle::render_manifest(&manifest).as_bytes());
    bundle::tar_append(&mut tar, "stint.ftel", &stint);
    bundle::tar_append(&mut tar, "session.txt", session_txt.as_bytes());
    bundle::tar_append(&mut tar, "journal.txt", journal_txt.as_bytes());
    tar.extend_from_slice(&[0u8; 1024]);
    zstd::stream::encode_all(&tar[..], 1).unwrap()
}

#[test]
fn good_bundles_file_per_sender_and_reruns_skip() {
    let (base, inbox, lib, quar) = dirs("good");
    let (name, bytes) = good_bundle();
    std::fs::create_dir_all(inbox.join("aabbccdd00112233")).unwrap();
    std::fs::write(inbox.join("aabbccdd00112233").join(&name), &bytes).unwrap();
    std::fs::write(inbox.join("hand-delivered.tar.zst"), &bytes).unwrap();

    let report = ingest_dir(&inbox, &lib, &quar).unwrap();
    assert_eq!(report.ingested.len(), 2, "{report:?}");
    assert!(report.quarantined.is_empty(), "{report:?}");
    assert!(lib.join("aabbccdd00112233").join(&name).exists());
    assert!(lib.join("local/hand-delivered.tar.zst").exists());

    // Re-run: nothing new, nothing duplicated, inbox untouched.
    let again = ingest_dir(&inbox, &lib, &quar).unwrap();
    assert!(again.ingested.is_empty());
    assert_eq!(again.skipped, 2);
    assert!(inbox.join("aabbccdd00112233").join(&name).exists(), "inbox is never mutated");
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn bad_bundles_quarantine_with_reasons() {
    let (base, inbox, lib, quar) = dirs("bad");
    let sender = inbox.join("aabbccdd00112233");
    std::fs::create_dir_all(&sender).unwrap();

    // Not even zstd.
    std::fs::write(sender.join("garbage.tar.zst"), b"not a bundle at all").unwrap();
    // Correct hashes, smuggled prose — only strict validation catches this.
    std::fs::write(sender.join("prose.tar.zst"), prose_bundle()).unwrap();

    let report = ingest_dir(&inbox, &lib, &quar).unwrap();
    assert!(report.ingested.is_empty(), "{report:?}");
    assert_eq!(report.quarantined.len(), 2, "{report:?}");
    assert!(!lib.exists() || std::fs::read_dir(&lib).unwrap().count() == 0);

    let reason = std::fs::read_to_string(
        quar.join("aabbccdd00112233/prose.tar.zst.reason.txt"),
    )
    .unwrap();
    assert!(reason.contains("journal.txt is not free-text-clean"), "{reason}");
    assert!(quar.join("aabbccdd00112233/garbage.tar.zst.reason.txt").exists());

    // Quarantined bundles are not re-processed on the next run.
    let again = ingest_dir(&inbox, &lib, &quar).unwrap();
    assert_eq!(again.skipped, 2);
    assert!(again.quarantined.is_empty());
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn name_collision_with_different_content_quarantines() {
    let (base, inbox, lib, quar) = dirs("collide");
    let (name, bytes) = good_bundle();
    std::fs::create_dir_all(inbox.join("s1")).unwrap();
    std::fs::write(inbox.join("s1").join(&name), &bytes).unwrap();
    ingest_dir(&inbox, &lib, &quar).unwrap();

    // Same name arrives again with different bytes (manual re-export of a
    // re-cut stint): must not overwrite the library silently.
    let mut other = bytes.clone();
    other.push(0);
    std::fs::write(inbox.join("s1").join(&name), &other).unwrap();
    let report = ingest_dir(&inbox, &lib, &quar).unwrap();
    assert_eq!(report.quarantined.len(), 1);
    assert!(report.quarantined[0].1.contains("collision"), "{report:?}");
    assert_eq!(std::fs::read(lib.join("s1").join(&name)).unwrap(), bytes, "library untouched");
    let _ = std::fs::remove_dir_all(&base);
}
