use super::session::{append_line, campaign_start};
use super::*;

/// Switching the session car archives the active campaign to its per-car
/// file and restores it intact when switching back.
#[test]
fn car_switch_archives_and_restores_sessions() {
    let dir = std::env::temp_dir().join(format!("tuners-car-switch-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("tune-session.txt");
    let jb = dir.join("tune-journal.txt").to_string_lossy().into_owned();

    // McLaren session with a revision on file.
    let mut s = crate::advice::tuning::TuningSession {
        car: Some(1314),
        ..Default::default()
    };
    s.facts.insert("abs".into(), "on".into());
    s.revisions.push(crate::advice::tuning::Revision {
        stamp: "20260721-000000".into(),
        values: [("arb_f".to_string(), "18.5".to_string())]
            .into_iter()
            .collect(),
    });
    s.save(&path).unwrap();

    let switch = |car: &str| SessionUpdate {
        reset: false,
        car: Some(car.to_string()),
        facts: Vec::new(),
    };
    // Switch to an RWD car: fresh session, McLaren archived.
    update_session(&switch("227"), &path, &jb).unwrap();
    let now = crate::advice::tuning::TuningSession::load(&path);
    assert_eq!(now.car, Some(227));
    assert!(now.revisions.is_empty(), "fresh session for the new car");
    let archived = crate::advice::tuning::TuningSession::load(
        crate::advice::tuning::journal_path_for(Some(1314), &path.to_string_lossy()).as_ref(),
    );
    assert_eq!(archived.car, Some(1314));
    assert_eq!(archived.revisions.len(), 1, "campaign archived intact");

    // Switch back: the McLaren campaign is restored, revisions included.
    update_session(&switch("1314"), &path, &jb).unwrap();
    let restored = crate::advice::tuning::TuningSession::load(&path);
    assert_eq!(restored.car, Some(1314));
    assert_eq!(restored.revisions.len(), 1);
    assert_eq!(restored.facts.get("abs").map(String::as_str), Some("on"));

    let _ = std::fs::remove_dir_all(&dir);
}

/// Effect vectors serialize as a plain JSON object keyed by field; empty
/// stays a valid (empty) object. Replaces the old effects_json shape test.
#[test]
fn effects_serialize_as_object() {
    assert_eq!(
        serde_json::to_value(effects_map(&Vec::new())).unwrap(),
        serde_json::json!({})
    );
    let fx = vec![("balance", 0.05f32), ("apex_speed", -1.25f32)];
    let v = serde_json::to_value(effects_map(&fx)).unwrap();
    assert_eq!(v["apex_speed"], serde_json::json!(-1.25));
    assert_eq!(v["balance"].as_f64().unwrap(), 0.05f32 as f64);
}

/// The live-state payload keeps the dashboard's camelCase wire names.
/// Replaces the old live_state_json shape test.
#[test]
fn live_state_serializes_with_wire_names() {
    let empty = crate::telemetry::live::LiveState::default();
    let rec = crate::telemetry::record::new_shared();
    let v = serde_json::to_value(live_state_view(&empty, &rec.lock().unwrap())).unwrap();
    assert_eq!(v["file"], serde_json::Value::Null);
    assert_eq!(v["ageMs"], serde_json::Value::Null);
    assert_eq!(v["frame"], serde_json::Value::Null);
    assert!(v["recorder"]["mode"].is_string());

    let state = crate::telemetry::live::LiveState {
        file: Some("sessions/session-x.ftel".into()),
        latest: Some(crate::analysis::TimedFrame {
            recv_us: 0,
            frame: crate::telemetry::simulate::synth_frame(2.5),
        }),
        last_data: Some(std::time::Instant::now()),
        ..Default::default()
    };
    let v = serde_json::to_value(live_state_view(&state, &rec.lock().unwrap())).unwrap();
    assert_eq!(v["file"], serde_json::json!("session-x.ftel"));
    assert_eq!(v["frame"]["raceOn"], serde_json::json!(true));
    for key in ["speedMps", "rpm", "maxRpm", "tireTempF", "currentLapS"] {
        assert!(!v["frame"][key].is_null(), "missing frame key {key}");
    }
    assert!(quality_view(None).is_none());
}

/// Deleting an archived session removes its pair and only the runs no
/// other journal references; the plan previews exactly that split.
#[test]
fn archived_session_delete_and_plan() {
    let dir = std::env::temp_dir().join(format!("tuners-sessdel-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let sessions = dir.join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    let sfile = dir.join("tune-session.txt").to_string_lossy().into_owned();
    let jbase = dir.join("tune-journal.txt").to_string_lossy().into_owned();
    let sdir = sessions.to_string_lossy().into_owned();

    // Archived pair for car 99: one run only it references, one shared
    // with car 55's live journal, one whose recording is already gone.
    let id = "99-20260727-000000";
    std::fs::write(dir.join(format!("tune-session-{id}.txt")), "car = 99\n").unwrap();
    std::fs::write(
        dir.join(format!("tune-journal-{id}.txt")),
        "# parked\nsessions/stint-a.ftel | baseline\nsessions/stint-b.ftel | front arb +1\nsessions/stint-gone.ftel | note\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("tune-journal-55.txt"),
        "sessions/stint-b.ftel | baseline\n",
    )
    .unwrap();
    std::fs::write(sessions.join("stint-a.ftel"), vec![0u8; 100]).unwrap();
    std::fs::write(sessions.join("stint-b.ftel"), b"x").unwrap();

    let plan = session_delete_plan(id, &sfile, &jbase, &sdir).unwrap();
    assert_eq!(
        plan,
        SessionDeletePlan {
            runs: 1,
            mb: 100.0 / 1e6,
            shared: 1,
            missing: 1
        }
    );
    let err = session_delete_plan("nope", &sfile, &jbase, &sdir).unwrap_err();
    assert_eq!(err.kind, ErrorKind::NotFound, "{err}");
    let err = session_delete_plan("../evil", &sfile, &jbase, &sdir).unwrap_err();
    assert_eq!(err.kind, ErrorKind::BadRequest, "{err}");

    delete_session(id, true, &sfile, &jbase, &sdir, None).unwrap();
    assert!(
        !sessions.join("stint-a.ftel").exists(),
        "exclusive run goes"
    );
    assert!(sessions.join("stint-b.ftel").exists(), "shared run stays");
    assert!(!dir.join(format!("tune-session-{id}.txt")).exists());
    assert!(!dir.join(format!("tune-journal-{id}.txt")).exists());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Deleting a journaled stint needs an explicit force; unjournaled stints
/// delete freely. Campaign start is the earlier of first revision and
/// first journaled stint.
#[test]
fn journaled_stint_delete_requires_force() {
    let dir = std::env::temp_dir().join(format!("tuners-delguard-{}", std::process::id()));
    let sessions = dir.join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    let jbase = dir.join("tune-journal.txt").to_string_lossy().into_owned();
    std::fs::write(
        dir.join("tune-journal-99.txt"),
        "# car\nsessions/stint-20260725-100000.ftel | baseline\n",
    )
    .unwrap();
    for f in ["stint-20260725-100000.ftel", "stint-20260725-110000.ftel"] {
        std::fs::write(sessions.join(f), b"x").unwrap();
    }
    let sdir = sessions.to_string_lossy().into_owned();

    let err = delete_stint(&sdir, "stint-20260725-100000.ftel", None, false, &jbase)
        .expect_err("journaled stint must not delete without force");
    assert_eq!(err.kind, ErrorKind::Conflict, "{err}");
    assert!(err.message.contains("tune-journal-99.txt"), "{err}");
    assert!(sessions.join("stint-20260725-100000.ftel").exists());

    delete_stint(&sdir, "stint-20260725-110000.ftel", None, false, &jbase)
        .expect("unjournaled deletes without force");

    delete_stint(&sdir, "stint-20260725-100000.ftel", None, true, &jbase)
        .expect("force overrides the guard");
    assert!(!sessions.join("stint-20260725-100000.ftel").exists());

    // Campaign start: journal baseline stint (100000) predates the first
    // revision save (100500), so the earlier stamp wins.
    let mut s = crate::advice::tuning::TuningSession {
        car: Some(99),
        ..Default::default()
    };
    s.revisions.push(crate::advice::tuning::Revision {
        stamp: "20260725-100500".into(),
        ..Default::default()
    });
    assert_eq!(
        campaign_start(&s, &jbase).as_deref(),
        Some("20260725-100000")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// New-session archives the active pair whole; resume swaps it back, and
/// two campaigns for the SAME car keep separate journals throughout.
#[test]
fn session_new_and_resume_roundtrip_same_car() {
    let dir = std::env::temp_dir().join(format!("tuners-sessions-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let session_file = dir.join("tune-session.txt");
    let journal_base = dir.join("tune-journal.txt");
    let (sf, jb) = (
        session_file.to_string_lossy().into_owned(),
        journal_base.to_string_lossy().into_owned(),
    );

    // Campaign A: car 2793 with a name, one revision, a journal with 2 stints.
    let mut a = crate::advice::tuning::TuningSession {
        car: Some(2793),
        ..Default::default()
    };
    a.facts.insert("name".into(), "awd aero".into());
    a.facts.insert("unit_pressure".into(), "psi".into());
    a.revisions.push(crate::advice::tuning::Revision {
        stamp: "1".into(),
        ..Default::default()
    });
    a.save(&session_file).unwrap();
    let journal_a = crate::advice::tuning::journal_path_for(Some(2793), &jb);
    std::fs::write(
        &journal_a,
        "# car\nsessions/a.ftel | baseline\nsessions/b.ftel | x\n",
    )
    .unwrap();

    // A boundary marker appended to a journal whose last line has no
    // trailing newline (hand-edited) must not glue onto the note.
    let ragged = dir.join("ragged.txt");
    std::fs::write(&ragged, "sessions/a.ftel | front arb stiffer").unwrap();
    append_line(&ragged.to_string_lossy(), "# parked 20260101-000000").unwrap();
    assert_eq!(
        std::fs::read_to_string(&ragged).unwrap(),
        "sessions/a.ftel | front arb stiffer\n# parked 20260101-000000\n"
    );

    // New session: A is archived (session + journal move together), the
    // fresh session keeps unit prefs and takes the posted name.
    let fresh = new_session(None, Some("rwd build".into()), None, &sf, &jb).unwrap();
    assert!(
        !Path::new(&journal_a).exists(),
        "journal A moved to the archive"
    );
    let parked = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .find(|e| {
            e.file_name()
                .to_string_lossy()
                .starts_with("tune-journal-2793-")
        })
        .expect("archived journal");
    assert!(
        std::fs::read_to_string(parked.path())
            .unwrap()
            .contains("# parked "),
        "parked marker closes the campaign"
    );
    assert_eq!(fresh.car, None);
    assert_eq!(fresh.facts.get("name").unwrap(), "rwd build");
    assert_eq!(
        fresh.facts.get("unit_pressure").unwrap(),
        "psi",
        "unit prefs carry"
    );
    assert_eq!(fresh.revisions, 0);

    let list = sessions_view(&sf, &jb);
    let row = &list.archived[0];
    assert_eq!(row.name.as_deref(), Some("awd aero"));
    assert_eq!(row.stints, 2);
    let id = row.id.clone().expect("archived id in listing");

    // Make the fresh session campaign B on the SAME car, with its own journal.
    let mut b = crate::advice::tuning::TuningSession::load(&session_file);
    b.car = Some(2793);
    b.save(&session_file).unwrap();
    std::fs::write(&journal_a, "# car\nsessions/c.ftel | baseline\n").unwrap();

    // Resume A: B is archived in turn, A's session AND journal come back.
    let restored = resume_session(&id, &sf, &jb).unwrap();
    assert_eq!(restored.facts.get("name").unwrap(), "awd aero");
    assert_eq!(restored.revisions, 1);
    let journal = std::fs::read_to_string(&journal_a).unwrap();
    assert!(
        journal.contains("sessions/b.ftel"),
        "campaign A journal restored: {journal}"
    );
    assert!(
        journal.contains("# resumed "),
        "resume marker floors the implicit-step scan"
    );
    let list = sessions_view(&sf, &jb);
    assert!(
        list.archived
            .iter()
            .any(|r| r.name.as_deref() == Some("rwd build") && r.stints == 1),
        "campaign B archived with its own journal"
    );

    // Bad ids are rejected, unknown ids are not found.
    assert_eq!(
        resume_session("../evil", &sf, &jb).unwrap_err().kind,
        ErrorKind::BadRequest
    );
    assert_eq!(
        resume_session("none-19700101-000000", &sf, &jb)
            .unwrap_err()
            .kind,
        ErrorKind::NotFound
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Accepting suggestions saves PARTIAL tunes: posted keys merge onto the
/// latest revision, and multiple accepts before the next stint net into
/// ONE journal note diffed against the last driven revision.
#[test]
fn partial_saves_merge_and_net_into_one_note() {
    let dir = std::env::temp_dir().join(format!("tuners-partial-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("tune-session.txt");
    let mut s = crate::advice::tuning::TuningSession {
        car: Some(2793),
        ..Default::default()
    };
    s.revisions.push(crate::advice::tuning::Revision {
        stamp: "20260724-000000".into(),
        values: [
            ("arb_f".to_string(), "18.3".to_string()),
            ("final_drive".to_string(), "3.95".to_string()),
            ("rebound_f".to_string(), "10.6".to_string()),
        ]
        .into_iter()
        .collect(),
    });
    s.save(&path).unwrap();
    let recorder = crate::telemetry::record::new_shared();
    let post = |pairs: &[(&str, &str)], recorder: &crate::telemetry::record::SharedRecorder| {
        let values: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        save_tune(&values, true, &path, recorder)
    };

    // Accept #1: front arb only. Unposted keys carry over from the latest.
    let out = post(&[("arb_f", "16.8")], &recorder).unwrap();
    assert!(
        out.note.as_deref().unwrap().contains("front arb -1.5"),
        "{out:?}"
    );
    let latest_vals = |p: &Path| {
        crate::advice::tuning::TuningSession::load(p)
            .latest()
            .unwrap()
            .values
            .clone()
    };
    let vals = latest_vals(&path);
    assert_eq!(vals.get("arb_f").unwrap(), "16.8");
    assert_eq!(
        vals.get("final_drive").unwrap(),
        "3.95",
        "unposted keys carry over"
    );
    assert_eq!(vals.get("rebound_f").unwrap(), "10.6");

    // Accept #2 before any stint: chains onto #1 and the pending note nets
    // BOTH changes against the driven baseline.
    post(&[("final_drive", "4.1")], &recorder).unwrap();
    let note = recorder.lock().unwrap().pending_note.clone().unwrap();
    assert!(
        note.contains("front arb -1.5") && note.contains("final drive +0.15"),
        "{note}"
    );
    let vals = latest_vals(&path);
    assert_eq!(
        vals.get("arb_f").unwrap(),
        "16.8",
        "accept #2 chains onto #1"
    );
    assert_eq!(vals.get("final_drive").unwrap(), "4.1");

    // Accepting the original arb back nets the chain to one remaining change.
    post(&[("arb_f", "18.3")], &recorder).unwrap();
    let note = recorder.lock().unwrap().pending_note.clone().unwrap();
    assert!(
        note.contains("final drive") && !note.contains("front arb"),
        "{note}"
    );

    // A partial save with no tune on file is rejected.
    let empty = dir.join("empty-session.txt");
    let err = post_at(&empty, &recorder).unwrap_err();
    assert_eq!(err.kind, ErrorKind::BadRequest);

    let _ = std::fs::remove_dir_all(&dir);
}

fn post_at(
    path: &Path,
    recorder: &crate::telemetry::record::SharedRecorder,
) -> Result<TuneSaveView, ApiError> {
    save_tune(
        &[("arb_f".to_string(), "16.8".to_string())],
        true,
        path,
        recorder,
    )
}

#[test]
fn delete_stint_guards_and_deletes() {
    let dir = std::env::temp_dir().join(format!("tuners-del-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let dir_s = dir.to_string_lossy().into_owned();
    std::fs::write(dir.join("stint-x.ftel"), b"data").unwrap();

    let jb = dir.join("tune-journal.txt").to_string_lossy().into_owned();
    for bad in ["../stint-x.ftel", "sub/stint-x.ftel", "stint-x.txt"] {
        let err = delete_stint(&dir_s, bad, None, false, &jb).unwrap_err();
        assert_eq!(err.kind, ErrorKind::BadRequest, "{bad}");
    }
    let err = delete_stint(
        &dir_s,
        "stint-x.ftel",
        Some(dir.join("stint-x.ftel").as_path()),
        false,
        &jb,
    )
    .unwrap_err();
    assert_eq!(
        err.kind,
        ErrorKind::Conflict,
        "active recording is protected"
    );
    assert!(dir.join("stint-x.ftel").exists());

    delete_stint(&dir_s, "stint-x.ftel", None, false, &jb).unwrap();
    assert!(!dir.join("stint-x.ftel").exists());

    let err = delete_stint(&dir_s, "stint-x.ftel", None, false, &jb).unwrap_err();
    assert_eq!(err.kind, ErrorKind::NotFound);
    std::fs::remove_dir_all(&dir).ok();
}

/// The read guard on stint file arguments: only relative .ftel paths with
/// no traversal reach the filesystem.
#[test]
fn file_args_reject_unsafe_paths() {
    for bad in ["../../etc/passwd", "/etc/passwd", "Cargo.toml", ""] {
        assert_eq!(
            report_text(bad).unwrap_err().kind,
            ErrorKind::BadRequest,
            "report {bad}"
        );
        assert_eq!(
            laps_view(bad).unwrap_err().kind,
            ErrorKind::BadRequest,
            "laps {bad}"
        );
        assert_eq!(
            compare_view(bad, "fixtures/rivals-lap-boundary-01.ftel")
                .unwrap_err()
                .kind,
            ErrorKind::BadRequest,
            "compare {bad}"
        );
    }
    // A safe relative fixture path passes the guard (whatever the load
    // outcome under the current cwd, it is never rejected as unsafe).
    if let Err(e) = report_text("fixtures/rivals-lap-boundary-01.ftel") {
        assert_ne!(e.kind, ErrorKind::BadRequest);
    }
}

/// Fixture-driven: the committed fixtures are short race-on segments
/// with no completed lap, so the laps view must fail with the profile
/// error (Internal, since the decode ran), never a guard rejection. When a
/// real session library is present (dev machine; gitignored elsewhere),
/// the full chart geometry is asserted end to end.
#[test]
fn laps_view_over_fixture() {
    let err = laps_view("fixtures/real-01.ftel").expect_err("no completed laps");
    assert_eq!(err.kind, ErrorKind::Internal);
    assert!(err.message.contains("laps"), "{err}");

    let newest = std::fs::read_dir("sessions").ok().map(|rd| {
        let mut paths: Vec<_> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "ftel"))
            .collect();
        paths.sort();
        paths
    });
    let Some(mut paths) = newest else { return };
    // Auto-cut stints can be short (no completed lap): use the newest
    // one that profiles.
    let Some(v) = paths
        .iter()
        .rev()
        .find_map(|p| laps_view(&p.to_string_lossy()).ok())
    else {
        return;
    };
    paths.clear();
    assert!(v.bin_meters > 0.0 && v.best_time > 0.0);
    assert!(!v.laps.is_empty());
    let bins = v.laps[0].speeds.len();
    assert!(bins > 0, "shared bins present");
    assert!(v.laps.iter().all(|l| l.speeds.len() == bins));
    assert_eq!(v.corroborated.len(), bins, "strip aligns with the bins");
    let j = serde_json::to_value(&v).unwrap();
    assert!(j["binMeters"].is_number() && j["bestTime"].is_number());
}

#[test]
fn stint_list_empty_when_dir_missing() {
    assert!(stint_rows("no-such-dir").is_empty());
}
