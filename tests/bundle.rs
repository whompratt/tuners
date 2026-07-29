//! End-to-end bundle round trip: build from the real
//! committed fixture, reopen, and prove the parsers accept every member and
//! that free text is provably absent.

use std::collections::BTreeMap;
use std::path::Path;
use tuners::advice::tuning::{Revision, TuningSession};
use tuners::sharing::bundle;

fn fixture_session() -> TuningSession {
    let mut s = TuningSession {
        car: Some(4165),
        ..Default::default()
    };
    for (k, v) in [
        ("name", "s1 aero test"),
        ("description", "secret prose that must never ship"),
        ("front_weight_pct", "46"),
        ("unit_temp", "c"),
        ("limit_arb_f", "5..40"),
    ] {
        s.facts.insert(k.into(), v.into());
    }
    s.revisions.push(Revision {
        stamp: "20260719-224500".into(),
        values: [("arb_f".to_string(), "24".to_string())].into(),
    });
    s
}

const FIXTURE_JOURNAL: &str = "\
# some prose header the user edited (ordinal 4165)
fixtures/real-01.ftel | baseline, felt loose everywhere
fixtures/real-01.ftel | front arb -2; my hands hurt
";

#[test]
fn bundle_round_trip_from_real_fixture() {
    let stint_path = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fixtures/real-01.ftel"
    ));
    let (name, bytes) = bundle::build(stint_path, &fixture_session(), FIXTURE_JOURNAL).unwrap();
    assert_eq!(name, "bundle-4165-real-01.tar.zst");

    let b = bundle::open(&bytes).unwrap();

    // Manifest describes the stint truthfully.
    assert_eq!(b.manifest.get("car").map(String::as_str), Some("4165"));
    assert_eq!(b.manifest.get("packets").map(String::as_str), Some("1200"));
    assert_eq!(
        b.manifest.get("bundle_version").map(String::as_str),
        Some("2"),
        "the real fixture is uniform: the transpose path must engage, \
         never the fallback"
    );
    assert!(b.manifest.contains_key("consent"));

    // The recording is byte-identical and decodes with the real reader.
    assert_eq!(b.stint, std::fs::read(stint_path).unwrap());

    // Session member: parses, keeps structure, and the prose is ABSENT.
    let session = TuningSession::parse(&b.session_txt);
    assert_eq!(session.car, Some(4165));
    assert_eq!(
        session.facts.get("front_weight_pct").map(String::as_str),
        Some("46")
    );
    assert_eq!(session.revisions.len(), 1);
    for leak in ["s1 aero test", "secret prose", "name", "description"] {
        assert!(
            !b.session_txt.contains(leak),
            "leaked {leak}:\n{}",
            b.session_txt
        );
    }

    // Journal member: parses, structured delta kept, prose absent.
    let entries = tuners::advice::journal::parse_journal(&b.journal_txt);
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[1].note.as_deref(), Some("front arb -2"));
    for leak in ["felt loose", "hands hurt", "prose header"] {
        assert!(
            !b.journal_txt.contains(leak),
            "leaked {leak}:\n{}",
            b.journal_txt
        );
    }
    let change =
        tuners::advice::journal::parse_change(entries[1].note.as_deref().unwrap()).unwrap();
    assert_eq!(change.magnitude, Some(-2.0));
}

#[test]
fn tampered_bundles_are_rejected() {
    let stint_path = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fixtures/real-01.ftel"
    ));
    let (_, bytes) = bundle::build(stint_path, &fixture_session(), FIXTURE_JOURNAL).unwrap();

    // Corrupt a member INSIDE the archive: decompress, flip a byte in the
    // stint payload region, recompress. The manifest hash must catch it.
    let mut tar = zstd::stream::decode_all(&bytes[..]).unwrap();
    let mid = tar.len() / 2;
    tar[mid] ^= 0xff;
    let evil = zstd::stream::encode_all(&tar[..], 1).unwrap();
    let err = bundle::open(&evil).unwrap_err();
    assert!(
        err.contains("hash mismatch") || err.contains("tar:"),
        "{err}"
    );

    // Truncated compressed stream fails loudly too.
    assert!(bundle::open(&bytes[..bytes.len() / 2]).is_err());
}

#[test]
fn export_refuses_without_a_session_car() {
    let stint_path = Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fixtures/real-01.ftel"
    ));
    let mut facts = BTreeMap::new();
    facts.insert("unit_temp".to_string(), "c".to_string());
    let session = TuningSession {
        car: None,
        facts,
        ..Default::default()
    };
    let err = bundle::build(stint_path, &session, "").unwrap_err();
    assert!(err.contains("no car"), "{err}");
}

/// A recording the transpose gate rejects (mixed record lengths) still
/// bundles — as v1, with the raw bytes stored — and opens transparently.
/// The gate exists for damaged files only: every real recording is uniform,
/// so a v1 fallback showing up in practice is a signal to investigate, not
/// normal operation.
#[test]
fn irregular_recording_falls_back_to_v1() {
    let dir = std::env::temp_dir().join(format!("tuners-v1fb-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("stint-19700101-000000.ftel");
    let mut raw = Vec::from(*tuners::telemetry::stint::MAGIC);
    for (t, len) in [(1_000_000u64, 324u32), (1_016_667, 16)] {
        raw.extend_from_slice(&t.to_le_bytes());
        raw.extend_from_slice(&len.to_le_bytes());
        raw.extend(std::iter::repeat_n(0xABu8, len as usize));
    }
    std::fs::write(&path, &raw).unwrap();

    let (_, bytes) = bundle::build(&path, &fixture_session(), "").unwrap();
    let b = bundle::open(&bytes).unwrap();
    assert_eq!(
        b.manifest.get("bundle_version").map(String::as_str),
        Some("1")
    );
    assert_eq!(b.stint, raw, "fallback stores the recording verbatim");
    let _ = std::fs::remove_dir_all(&dir);
}
