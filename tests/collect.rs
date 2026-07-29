//! Phase 2 end-to-end: outbox enqueue + the curl drainer against a
//! real local `tuners receive` (open mode). It is the same protocol the deployed
//! Worker speaks, so a green drain here is the whole sender pipeline working.

use std::net::TcpListener;
use std::path::{Path, PathBuf};
use tuners::advice::tuning::{Revision, TuningSession};
use tuners::sharing::collect::{self, CollectConfig};
use tuners::sharing::receive::{ReceiveConfig, run_listener};

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tuners-collect-e2e-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Open-mode receiver on an ephemeral port; returns (endpoint, storage root).
fn start_receiver(dir: &Path) -> (String, PathBuf) {
    let root = dir.join("inbox");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let cfg = ReceiveConfig {
        root: root.clone(),
        tokens_path: dir.join("no-tokens.txt"),
        blocklist_path: dir.join("no-blocklist.txt"),
        max_bundle_bytes: 64 << 20,
        daily_cap_bytes: 512 << 20,
        global_cap_bytes: u64::MAX,
    };
    std::thread::spawn(move || run_listener(listener, cfg));
    (endpoint, root)
}

fn fixture_session() -> TuningSession {
    let mut s = TuningSession {
        car: Some(4165),
        ..Default::default()
    };
    s.facts.insert("front_weight_pct".into(), "46".into());
    s.revisions.push(Revision {
        stamp: "20260719-224500".into(),
        values: [("arb_f".to_string(), "24".to_string())].into(),
    });
    s
}

fn fixture_stint() -> &'static Path {
    Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/fixtures/real-01.ftel"
    ))
}

#[test]
fn enqueue_then_drain_uploads_and_clears() {
    let dir = temp_dir("drain");
    let outbox = dir.join("outbox");
    let (endpoint, root) = start_receiver(&dir);
    let token = "c".repeat(64);
    let cfg = CollectConfig {
        enabled: true,
        endpoint,
        token: token.clone(),
    };
    assert!(cfg.ready());

    let queued = collect::enqueue(&outbox, fixture_stint(), &fixture_session(), "")
        .unwrap()
        .expect("first enqueue stores");
    assert_eq!(
        queued.file_name().unwrap().to_str().unwrap(),
        "bundle-4165-real-01.tar.zst"
    );
    // Idempotent: the same stint doesn't queue twice.
    assert!(
        collect::enqueue(&outbox, fixture_stint(), &fixture_session(), "")
            .unwrap()
            .is_none()
    );
    assert_eq!(collect::queued(&outbox).len(), 1);

    // Telemetry fresh -> the drainer must not touch the network.
    let log = collect::drain(&outbox, &cfg, &|| true);
    assert_eq!(log, vec!["drain paused: telemetry active".to_string()]);
    assert_eq!(collect::queued(&outbox).len(), 1);

    // Idle -> uploads, deletes locally, and the receiver holds a bundle that
    // reopens cleanly under the derived sender namespace.
    let log = collect::drain(&outbox, &cfg, &|| false);
    assert_eq!(
        log,
        vec!["uploaded bundle-4165-real-01.tar.zst".to_string()]
    );
    assert!(collect::queued(&outbox).is_empty());
    let sender_dir = root.join(collect::sender_id(&token));
    let stored: Vec<_> = std::fs::read_dir(&sender_dir).unwrap().flatten().collect();
    assert_eq!(stored.len(), 1);
    let bundle = tuners::sharing::bundle::open(&std::fs::read(stored[0].path()).unwrap()).unwrap();
    assert_eq!(bundle.manifest.get("car").map(String::as_str), Some("4165"));

    // Draining an empty outbox is a no-op; re-uploading the same content
    // would dedupe server-side anyway.
    assert!(collect::drain(&outbox, &cfg, &|| false).is_empty());

    // The upload landed in the sent ledger (history backfill's memory).
    let sent = std::fs::read_to_string(outbox.join("sent.txt")).unwrap();
    assert_eq!(sent.trim(), "bundle-4165-real-01.tar.zst");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Historic sharing: journaled stints pair with their own campaign's context,
/// dedupe against the ledger/outbox, and unjournaled recordings only count.
#[test]
fn history_plan_is_per_campaign_and_idempotent() {
    let root = temp_dir("history");
    let sessions = root.join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    let outbox = root.join("outbox");
    for name in [
        "stint-20260101-000001.ftel",
        "stint-20260102-000002.ftel",
        "orphan.ftel",
    ] {
        std::fs::copy(fixture_stint(), sessions.join(name)).unwrap();
    }

    // Active campaign (car 4165) journals the first stint...
    std::fs::write(
        root.join("tune-session.txt"),
        "# tuners tuning session\ncar = 4165\n\n[tune t1]\narb_f = 24\n",
    )
    .unwrap();
    std::fs::write(
        root.join("tune-journal-4165.txt"),
        "# ordinal 4165\nsessions/stint-20260101-000001.ftel | front arb -2\n",
    )
    .unwrap();
    // ...an archived campaign (car 9999) journals the second.
    std::fs::write(
        root.join("tune-session-9999-20260101-120000.txt"),
        "# tuners tuning session\ncar = 9999\n\n[tune t1]\narb_r = 30\n",
    )
    .unwrap();
    std::fs::write(
        root.join("tune-journal-9999-20260101-120000.txt"),
        "# ordinal 9999\nsessions/stint-20260102-000002.ftel | rear arb +1\n# parked 20260102-000100\n",
    )
    .unwrap();

    let plan = collect::history_plan(&root, "sessions", &outbox);
    assert_eq!(plan.items.len(), 2, "{plan:?}");
    assert_eq!(plan.campaigns, 2);
    assert_eq!(plan.unjournaled, 1, "orphan.ftel has no campaign");
    assert_eq!(plan.already, 0);
    assert!(plan.bytes > 0);

    // A ledger entry excludes the already-shared stint from the next plan.
    std::fs::create_dir_all(&outbox).unwrap();
    std::fs::write(
        outbox.join("sent.txt"),
        "bundle-4165-20260101-000001.tar.zst\n",
    )
    .unwrap();
    let plan = collect::history_plan(&root, "sessions", &outbox);
    assert_eq!(plan.items.len(), 1, "{plan:?}");
    assert_eq!(plan.already, 1);

    // Enqueue builds the remaining bundle with ITS campaign's car + journal.
    assert_eq!(collect::history_enqueue(plan, &outbox), 1);
    let queued = collect::queued(&outbox);
    assert_eq!(queued.len(), 1);
    let name = queued[0].file_name().unwrap().to_str().unwrap().to_string();
    assert_eq!(name, "bundle-9999-20260102-000002.tar.zst");
    let b = tuners::sharing::bundle::open(&std::fs::read(&queued[0]).unwrap()).unwrap();
    assert_eq!(b.manifest.get("car").map(String::as_str), Some("9999"));
    assert!(b.journal_txt.contains("rear arb +1"), "{}", b.journal_txt);

    // Re-planning now sees everything as shared or queued: fully idempotent.
    let plan = collect::history_plan(&root, "sessions", &outbox);
    assert!(plan.items.is_empty(), "{plan:?}");
    assert_eq!(plan.already, 2);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn permanent_rejection_parks_the_bundle() {
    let dir = temp_dir("reject");
    let outbox = dir.join("outbox");
    let (endpoint, root) = start_receiver(&dir);
    // 'z'*64 is not hex: open mode answers 401, a permanent rejection, so
    // the bundle parks in outbox/rejected instead of retrying forever.
    let cfg = CollectConfig {
        enabled: true,
        endpoint,
        token: "z".repeat(64),
    };
    collect::enqueue(&outbox, fixture_stint(), &fixture_session(), "").unwrap();

    let log = collect::drain(&outbox, &cfg, &|| false);
    assert_eq!(log.len(), 1);
    assert!(
        log[0].contains("401") && log[0].contains("rejected"),
        "{}",
        log[0]
    );
    assert!(collect::queued(&outbox).is_empty());
    assert!(outbox.join("rejected/bundle-4165-real-01.tar.zst").exists());
    assert!(!root.join("inbox").exists() || std::fs::read_dir(&root).unwrap().count() == 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn network_failure_leaves_the_queue_alone() {
    let dir = temp_dir("offline");
    let outbox = dir.join("outbox");
    collect::enqueue(&outbox, fixture_stint(), &fixture_session(), "").unwrap();
    let cfg = CollectConfig {
        enabled: true,
        endpoint: "http://127.0.0.1:9".into(), // discard port: nothing listens
        token: "c".repeat(64),
    };
    let log = collect::drain(&outbox, &cfg, &|| false);
    assert_eq!(log.len(), 1);
    assert!(log[0].contains("will retry later"), "{}", log[0]);
    assert_eq!(
        collect::queued(&outbox).len(),
        1,
        "bundle must survive offline drains"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
