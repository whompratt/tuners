use super::campaign::{CampaignBound, campaign_bound, drop_missing_entries};
use super::*;
use crate::advice::recommend::{Confidence, Recommendation};
use crate::advice::tuning::Revision;

fn balance_rec() -> Recommendation {
    Recommendation {
        apply: Vec::new(),
        area: "balance",
        advice: "reduce front roll stiffness".into(),
        evidence: vec![],
        confidence: Confidence::High,
        suggestion: None,
        probe: false,
        implied: Some(journal::Change {
            family: journal::Family::FrontRoll,
            softer: true,
            magnitude: None,
        }),
    }
}

fn session_with(values: &[(&str, &str)], facts: &[(&str, &str)]) -> TuningSession {
    let mut s = TuningSession::default();
    let mut rev = Revision {
        stamp: "20260721-000000".into(),
        ..Default::default()
    };
    for (k, v) in values {
        rev.values.insert(k.to_string(), v.to_string());
    }
    s.revisions.push(rev);
    for (k, v) in facts {
        s.facts.insert(k.to_string(), v.to_string());
    }
    s
}

/// Front roll advised softer with both front sliders at minimum: the
/// advice flips to stiffening the rear, implied direction included.
#[test]
fn exhausted_front_softening_flips_to_rear() {
    let session = session_with(
        &[
            ("arb_f", "1"),
            ("springs_f", "100"),
            ("arb_r", "30"),
            ("springs_r", "400"),
        ],
        &[("limit_arb_f", "1..65"), ("limit_springs_f", "100..800")],
    );
    let mut recs = vec![balance_rec()];
    enrich_with_tune(&mut recs, &session);
    assert!(
        recs[0].advice.contains("stiffen the rear instead"),
        "{}",
        recs[0].advice
    );
    let implied = recs[0].implied.unwrap();
    assert_eq!(implied.family, journal::Family::RearRoll);
    assert!(!implied.softer);
    assert!(
        recs[0].evidence.iter().any(|e| e.contains("AT MINIMUM")),
        "{:?}",
        recs[0].evidence
    );
    assert!(
        recs[0]
            .evidence
            .iter()
            .any(|e| e.contains("advised direction exhausted"))
    );
}

/// A family whose whole group is absent from the baseline is not adjustable
/// on this build: with a tunable partner the advice redirects, without one
/// it is dropped (plan-014 field report: ARB advice on a car with locked
/// ARBs).
#[test]
fn untunable_family_redirects_or_drops() {
    // Rear roll present: front-roll advice redirects to stiffening the rear.
    let session = session_with(&[("arb_r", "30"), ("springs_r", "400")], &[]);
    let mut recs = vec![balance_rec()];
    enrich_with_tune(&mut recs, &session);
    assert_eq!(recs.len(), 1);
    assert!(
        recs[0]
            .advice
            .contains("no front roll adjustment on this build"),
        "{}",
        recs[0].advice
    );
    let implied = recs[0].implied.unwrap();
    assert_eq!(implied.family, journal::Family::RearRoll);
    assert!(!implied.softer);

    // Neither end tunable: the rec disappears.
    let session = session_with(&[("tire_pressure_f", "28")], &[]);
    let mut recs = vec![balance_rec()];
    enrich_with_tune(&mut recs, &session);
    assert!(
        recs.is_empty(),
        "advice for unfitted parts must not survive"
    );
}

/// Advice with no implied direction still names a system (area): damping
/// advice on a build with no damper adjustment is equally impossible and
/// is dropped by the same gate.
#[test]
fn untunable_area_without_direction_is_dropped() {
    let session = session_with(&[("arb_f", "20"), ("arb_r", "30")], &[]);
    let mut recs = vec![Recommendation {
        apply: Vec::new(),
        area: "damping",
        advice: "reduce front damping".into(),
        evidence: vec![],
        confidence: Confidence::High,
        suggestion: None,
        probe: false,
        implied: None,
    }];
    enrich_with_tune(&mut recs, &session);
    assert!(recs.is_empty());
}

/// A directional rec whose family sits at the advised bound with no
/// flip partner is dropped outright: "reduce X" with X at minimum is
/// impossible advice, not low-confidence advice (plan-014 field report:
/// "reduce rear diff accel" at 0%).
#[test]
fn exhausted_direction_without_partner_is_dropped() {
    let session = session_with(&[("diff_accel_r", "0")], &[]);
    let mut recs = vec![Recommendation {
        apply: Vec::new(),
        area: "traction",
        advice: "reduce differential acceleration lock".into(),
        evidence: vec![],
        confidence: Confidence::High,
        suggestion: None,
        probe: false,
        implied: Some(journal::Change {
            family: journal::Family::DiffAccel,
            softer: true,
            magnitude: None,
        }),
    }];
    enrich_with_tune(&mut recs, &session);
    assert!(recs.is_empty(), "at-minimum reduce advice must not survive");

    // The opposite direction has the whole range: it stands untouched.
    let mut recs = vec![Recommendation {
        probe: false,
        implied: Some(journal::Change {
            family: journal::Family::DiffAccel,
            softer: false,
            magnitude: None,
        }),
        ..Recommendation {
            apply: Vec::new(),
            area: "traction",
            advice: "add rear diff accel lock".into(),
            evidence: vec![],
            confidence: Confidence::Medium,
            suggestion: None,
            probe: false,
            implied: None,
        }
    }];
    enrich_with_tune(&mut recs, &session);
    assert_eq!(recs.len(), 1);
}

/// Only the primary slider pinned: advice stands, evidence points at the
/// secondary slider instead of flipping ends.
#[test]
fn primary_pinned_points_at_secondary() {
    let session = session_with(
        &[("arb_f", "1"), ("springs_f", "300")],
        &[("limit_arb_f", "1..65"), ("limit_springs_f", "100..800")],
    );
    let mut recs = vec![balance_rec()];
    enrich_with_tune(&mut recs, &session);
    assert!(
        recs[0].advice.contains("reduce front roll stiffness"),
        "{}",
        recs[0].advice
    );
    assert_eq!(recs[0].implied.unwrap().family, journal::Family::FrontRoll);
    assert!(
        recs[0]
            .evidence
            .iter()
            .any(|e| e.contains("work with front springs")),
        "{:?}",
        recs[0].evidence
    );
}

/// Springs have no universal range: with no fact recorded, a pinned arb
/// points at the springs but never claims the whole direction exhausted.
#[test]
fn unknown_limits_never_claim_exhaustion() {
    let session = session_with(&[("arb_f", "1"), ("springs_f", "100")], &[]);
    let mut recs = vec![balance_rec()];
    enrich_with_tune(&mut recs, &session);
    assert!(
        recs[0].advice.contains("reduce front roll stiffness"),
        "{}",
        recs[0].advice
    );
    assert!(
        recs[0]
            .evidence
            .iter()
            .any(|e| e.contains("work with front springs")),
        "{:?}",
        recs[0].evidence
    );
    assert!(recs[0].evidence.iter().all(|e| !e.contains("exhausted")));
}

/// The user's worked example: front ARB 10..16 with lap times showing
/// decaying improvement then a slowdown: the fitted optimum sits between
/// 14 and 15, not at the best tried value or a bisection of the last step.
#[test]
fn quad_fit_finds_the_interior_optimum() {
    // Cumulative deltas from lap times 60.0, 59.0, 58.3, 58.0, 58.5.
    let pts = [
        (10.0, 0.0),
        (12.0, -1.0),
        (14.0, -1.7),
        (15.0, -2.0),
        (16.0, -1.5),
    ];
    let (a, b, _) = quad_fit(&pts).unwrap();
    assert!(a > 0.0, "upward curvature (a minimum exists)");
    let vertex = (-b / (2.0 * a)) as f32;
    assert!((14.0..=15.2).contains(&vertex), "vertex {vertex}");

    // Monotonic data has no trustworthy interior minimum.
    let mono = [(10.0, 0.0), (12.0, -1.0), (14.0, -2.0)];
    if let Some((a, b, _)) = quad_fit(&mono) {
        let v = (-b / (2.0 * a)) as f32;
        assert!(
            !(10.0..=14.0).contains(&v) || a <= 0.0,
            "no interior vertex: a={a} v={v}"
        );
    }
    assert!(
        quad_fit(&[(10.0, 0.0), (12.0, -1.0)]).is_none(),
        "2 points fit nothing"
    );
}

/// Probes extend the landscape past the good edge; interior optima and
/// flat landscapes ask for nothing.
#[test]
fn probe_extends_the_mapped_edge() {
    // Better at the low end: probe below it by a quarter span.
    let nodes = [(29.0, -0.21, 1), (100.0, 0.22, 1)];
    let v = probe_value(&nodes, Some((0.0, 100.0)), 0.1).unwrap();
    assert!((v - 11.2).abs() < 0.11, "{v}");
    // Clamped by the slider range but still a new point.
    let nodes = [(12.0, 0.0, 1), (100.0, 0.63, 1)];
    assert_eq!(probe_value(&nodes, Some((0.0, 100.0)), 0.1), Some(0.0));
    // Better at the high end: probe above.
    let nodes = [(20.0, 0.31, 1), (52.0, 0.0, 1)];
    let v = probe_value(&nodes, Some((0.0, 100.0)), 0.1).unwrap();
    assert!((v - 60.0).abs() < 0.11, "{v}");
    // Interior best: the fit's vertex owns it.
    let nodes = [(17.0, -0.16, 1), (18.0, -0.49, 1), (20.7, 0.0, 1)];
    assert_eq!(probe_value(&nodes, None, 0.1), None);
    // One small improving step (the Ferrari final-drive case): a quarter
    // span rounds onto the best value, so probe one slider step out instead.
    let nodes = [(3.95, 0.0, 1), (4.1, -0.27, 1)];
    assert_eq!(probe_value(&nodes, None, 0.1), Some(4.2));
    // Flat landscape: nothing worth a stint.
    let nodes = [(3.35, -0.04, 1), (3.63, 0.03, 1)];
    assert_eq!(probe_value(&nodes, None, 0.1), None);
    // Best pinned at the slider bound: no new point exists.
    let nodes = [(0.0, -0.3, 1), (50.0, 0.2, 1)];
    assert_eq!(probe_value(&nodes, Some((0.0, 100.0)), 0.1), None);
    // Whole-unit slider (diff lock): the probe lands on an integer even
    // when the quarter span is fractional.
    let nodes = [(30.0, 0.31, 1), (65.0, 0.0, 1)];
    let v = probe_value(&nodes, Some((0.0, 100.0)), 1.0).unwrap();
    assert_eq!(v, 74.0, "quarter span 8.75 snaps to a whole unit");
    // ...and the small-span fallback steps a whole unit, not 0.1.
    let nodes = [(50.0, 0.0, 1), (51.0, -0.27, 1)];
    assert_eq!(probe_value(&nodes, None, 1.0), Some(52.0));
}

/// Deleted recordings are skipped but their tune changes carry forward:
/// the next surviving entry's note becomes the honest compound, and a
/// trailing missing entry just drops.
#[test]
fn missing_stints_merge_notes_forward() {
    let e = |path: &str, note: Option<&str>| journal::Entry {
        path: path.to_string(),
        note: note.map(String::from),
    };
    let entries = vec![
        e("a.ftel", Some("baseline")),
        e("gone1.ftel", Some("front arb -0.4")),
        e("gone2.ftel", Some("final drive +0.15")),
        e("b.ftel", Some("rear arb -1")),
        e("gone3.ftel", Some("front camber -1")),
    ];
    let (kept, missing) = drop_missing_entries(entries, |p| !p.starts_with("gone"));
    assert_eq!(missing, vec!["gone1.ftel", "gone2.ftel", "gone3.ftel"]);
    assert_eq!(kept.len(), 2);
    assert_eq!(kept[0].note.as_deref(), Some("baseline"));
    assert_eq!(
        kept[1].note.as_deref(),
        Some("front arb -0.4; final drive +0.15; rear arb -1"),
        "both skipped steps' changes precede the surviving stint"
    );
}

/// Boundary markers gate the implicit-step scan: parked journals accrue
/// nothing, resumed ones only take stints newer than the resume.
#[test]
fn campaign_bound_reads_the_last_marker() {
    assert!(matches!(
        campaign_bound("# car\na.ftel | baseline\n"),
        CampaignBound::Open
    ));
    assert!(matches!(
        campaign_bound("a.ftel | baseline\n# parked 20260724-190000\n"),
        CampaignBound::Closed
    ));
    match campaign_bound("a.ftel | x\n# parked 20260724-190000\n# resumed 20260724-210000\n") {
        CampaignBound::Since(s) => assert_eq!(s, "20260724-210000"),
        _ => panic!("expected Since"),
    }
    // Park after a resume closes it again.
    assert!(matches!(
        campaign_bound("# resumed 20260724-210000\n# parked 20260724-220000\n"),
        CampaignBound::Closed
    ));
}

/// Minimal profileable stint: 3 laps at 30 m/s so lap 1 is a completed
/// flying lap (time from lap 2's LastLap, captured from its start).
fn write_stint_with_laps(path: &Path) {
    let mut w = crate::telemetry::stint::StintWriter::create(path).unwrap();
    for l in 0..3u16 {
        for i in 0..60 {
            let t = (l as f32 * 60.0 + i as f32) * 0.1;
            let f = crate::telemetry::packet::TelemetryFrame {
                is_race_on: true,
                lap_number: l,
                current_lap: i as f32 * 0.1,
                current_race_time: t,
                last_lap: if l > 0 { 6.0 } else { 0.0 },
                distance_traveled: t * 30.0 + 1.0,
                speed: 30.0,
                car_ordinal: 42,
                ..Default::default()
            };
            w.write_packet((t * 1e6) as u64 + 1, &crate::telemetry::packet::encode(&f))
                .unwrap();
        }
    }
}

/// Two single-family improvements with complementary phase splits, both
/// re-applicable from the current setup, propose the untested
/// combination; a tested combination, same-phase pair, or a setup that
/// moved off a from-state all stay silent.
#[test]
fn composition_proposes_untested_phase_complements() {
    use crate::advice::tuning::Revision;
    let rev = |pairs: &[(&str, &str)]| {
        let mut r = Revision::default();
        for (k, v) in pairs {
            r.values.insert(k.to_string(), v.to_string());
        }
        r
    };
    let base = rev(&[("arb_f", "20"), ("diff_accel_r", "60")]);
    let arb = rev(&[("arb_f", "18"), ("diff_accel_r", "60")]);
    let diff = rev(&[("arb_f", "20"), ("diff_accel_r", "45")]);
    let m = |family, key: &str, i, j, d, split| Measurement {
        change: journal::Change {
            family,
            softer: true,
            magnitude: None,
        },
        outcome: journal::Outcome::Improved(d),
        desc: format!("{key} step"),
        attributed: None,
        weak: false,
        i,
        j,
        direct: true,
        key: Some(key.to_string()),
        split: Some(split),
        clean: true,
        effects: Vec::new(),
    };
    let m1 = m(
        journal::Family::FrontRoll,
        "arb_f",
        0,
        1,
        -0.4,
        (-0.30, 0.05, -0.05),
    );
    let m2 = m(
        journal::Family::DiffAccel,
        "diff_accel_r",
        2,
        3,
        -0.3,
        (0.02, -0.28, 0.0),
    );
    let setups: Vec<Option<&Revision>> = vec![
        Some(&base),
        Some(&arb),
        Some(&base),
        Some(&diff),
        Some(&base),
    ];
    let rec = composition_proposal(&[&m1, &m2], &setups, &Default::default())
        .expect("complementary pair");
    assert_eq!(rec.area, "experiment");
    assert!(rec.apply.contains(&("arb_f".into(), "18".into())));
    assert!(rec.apply.contains(&("diff_accel_r".into(), "45".into())));
    assert!(
        rec.evidence.iter().any(|e| e.contains("-0.70s")),
        "linear sum quoted: {:?}",
        rec.evidence
    );

    // A setup that already held BOTH to-values makes it tested.
    let combo = rev(&[("arb_f", "18"), ("diff_accel_r", "45")]);
    let tested: Vec<Option<&Revision>> = vec![
        Some(&base),
        Some(&arb),
        Some(&base),
        Some(&diff),
        Some(&combo),
        Some(&base),
    ];
    assert!(composition_proposal(&[&m1, &m2], &tested, &Default::default()).is_none());

    // Same-phase gains do not compose.
    let m3 = m(
        journal::Family::DiffAccel,
        "diff_accel_r",
        2,
        3,
        -0.3,
        (-0.28, 0.02, 0.0),
    );
    assert!(composition_proposal(&[&m1, &m3], &setups, &Default::default()).is_none());

    // Current setup off a measurement's from-state: not transferable.
    let moved = rev(&[("arb_f", "19"), ("diff_accel_r", "60")]);
    let off: Vec<Option<&Revision>> = vec![
        Some(&base),
        Some(&arb),
        Some(&base),
        Some(&diff),
        Some(&moved),
    ];
    assert!(composition_proposal(&[&m1, &m2], &off, &Default::default()).is_none());
}

/// The pause-menu auto-cut shape: race-on frames, ordinal present, but
/// the car never moves — no driving segment survives the 5s/speed gates.
fn write_stint_menu_only(path: &Path) {
    let mut w = crate::telemetry::stint::StintWriter::create(path).unwrap();
    for i in 0..100 {
        let t = i as f32 * 0.1;
        let f = crate::telemetry::packet::TelemetryFrame {
            is_race_on: true,
            current_race_time: t,
            car_ordinal: 42,
            ..Default::default()
        };
        w.write_packet((t * 1e6) as u64 + 1, &crate::telemetry::packet::encode(&f))
            .unwrap();
    }
}

/// Blind mode (no journal yet) must not die when the NEWEST recording is
/// a menu artifact: it advises on the newest stint with real driving.
#[test]
fn blind_advice_skips_menu_only_newest_stint() {
    let dir = std::env::temp_dir().join(format!("tuners-blindskip-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let driven = dir.join("stint-20260101-000000.ftel");
    write_stint_with_laps(&driven);
    write_stint_menu_only(&dir.join("stint-20260102-000000.ftel"));

    let v = advise(
        &dir.join("no-journal.txt").to_string_lossy(),
        &dir.join("no-session.txt"),
        &dir.to_string_lossy(),
    )
    .expect("menu-only newest stint tolerated in blind mode");
    assert_eq!(v.advice_for, driven.to_string_lossy());
    assert!(v.journal.is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

/// A mid-campaign stint with no completed laps (event entered, abandoned
/// in the pause menu) is skipped with its note merged forward, not a hard
/// error that kills advise.
#[test]
fn lapless_middle_stint_is_skipped_note_merged() {
    let dir = std::env::temp_dir().join(format!("tuners-lapless-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let laps = dir.join("stint-20260101-000000.ftel");
    write_stint_with_laps(&laps);
    let laps = laps.to_string_lossy().into_owned();
    // Real capture with no completed lap: the shape the pause-menu
    // auto-cut produces.
    let no_laps = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/real-01.ftel");
    let entries = vec![
        journal::Entry {
            path: laps.clone(),
            note: None,
        },
        journal::Entry {
            path: no_laps.to_string(),
            note: Some("front arb +1".to_string()),
        },
        journal::Entry {
            path: laps.clone(),
            note: Some("rear arb -1".to_string()),
        },
    ];
    let session = TuningSession::default();
    let c = load_campaign(entries, &session, "test").expect("lap-less middle stint tolerated");
    assert_eq!(c.stints.len(), 2);
    assert_eq!(c.no_laps, vec![no_laps.to_string()]);
    assert_eq!(
        c.stints[1].entry.note.as_deref(),
        Some("front arb +1; rear arb -1")
    );
    assert!(c.in_progress.is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

/// Unjournaled stints between two journal entries (a crash or idle
/// auto-cut: no tune save, so no journal line) join the trajectory as
/// implicit no-change steps in stamp order, so their laps corroborate the
/// setup they were driven on; stints recorded while the campaign was
/// parked belong to whatever campaign was active then and stay out.
#[test]
fn implicit_middle_stints_join_between_entries() {
    let dir = std::env::temp_dir().join(format!("tuners-implicitmid-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for stamp in [
        "20260101-080000", // journaled baseline
        "20260101-083000", // unjournaled middle: joins
        "20260101-100000", // recorded while parked: stays out
        "20260101-120000", // journaled
        "20260101-130000", // unjournaled trailing: joins
    ] {
        write_stint_with_laps(&dir.join(format!("stint-{stamp}.ftel")));
    }
    let sd = dir.to_string_lossy();
    let text = format!(
        "{sd}/stint-20260101-080000.ftel | baseline\n\
         # parked 20260101-090000\n\
         # resumed 20260101-110000\n\
         {sd}/stint-20260101-120000.ftel | front arb -1\n"
    );
    let mut entries = journal::parse_journal(&text);
    implicit_steps(&text, &mut entries, Some(42), &sd);
    let stamps: Vec<_> = entries
        .iter()
        .filter_map(|e| stint_stamp(&e.path))
        .collect();
    assert_eq!(
        stamps,
        vec![
            "20260101-080000",
            "20260101-083000",
            "20260101-120000",
            "20260101-130000",
        ]
    );
    assert!(entries[1].note.is_none(), "implicit middle carries no note");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A parked (archived) journal accrues nothing after the park stamp, but a
/// stint driven while the campaign was still active keeps its place.
#[test]
fn parked_journal_keeps_pre_park_stints_only() {
    let dir = std::env::temp_dir().join(format!("tuners-parked-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    for stamp in ["20260101-080000", "20260101-083000", "20260101-100000"] {
        write_stint_with_laps(&dir.join(format!("stint-{stamp}.ftel")));
    }
    let sd = dir.to_string_lossy();
    let text = format!("{sd}/stint-20260101-080000.ftel | baseline\n# parked 20260101-090000\n");
    let mut entries = journal::parse_journal(&text);
    implicit_steps(&text, &mut entries, Some(42), &sd);
    let stamps: Vec<_> = entries
        .iter()
        .filter_map(|e| stint_stamp(&e.path))
        .collect();
    assert_eq!(stamps, vec!["20260101-080000", "20260101-083000"]);
    let _ = std::fs::remove_dir_all(&dir);
}

/// An implicit (unjournaled) stint whose recording doesn't decode — e.g.
/// truncated by a crash mid-write — is skipped like a lap-less one; it
/// only ever corroborated, and no journal line names it for the user to
/// fix. A noted entry that doesn't decode stays a hard error.
#[test]
fn unreadable_implicit_stint_is_skipped() {
    use std::io::Write;
    let dir = std::env::temp_dir().join(format!("tuners-truncated-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let good = dir.join("stint-20260101-080000.ftel");
    write_stint_with_laps(&good);
    let bad = dir.join("stint-20260101-083000.ftel");
    write_stint_with_laps(&bad);
    std::fs::OpenOptions::new()
        .append(true)
        .open(&bad)
        .unwrap()
        .write_all(&[1, 2, 3, 4, 5]) // partial record header
        .unwrap();
    let good = good.to_string_lossy().into_owned();
    let bad = bad.to_string_lossy().into_owned();
    let entry = |path: &str, note: Option<&str>| journal::Entry {
        path: path.to_string(),
        note: note.map(String::from),
    };
    let session = TuningSession::default();
    let c = load_campaign(
        vec![
            entry(&good, Some("baseline")),
            entry(&bad, None),
            entry(&good, Some("front arb -1")),
        ],
        &session,
        "test",
    )
    .expect("truncated implicit stint tolerated");
    assert_eq!(c.stints.len(), 2);
    assert_eq!(c.no_laps, vec![bad.clone()]);
    let err = load_campaign(
        vec![
            entry(&good, Some("baseline")),
            entry(&bad, Some("front arb -1")),
            entry(&good, Some("rear arb -1")),
        ],
        &session,
        "test",
    );
    assert!(err.is_err(), "noted unreadable entry stays a hard error");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn stint_stamps_parse_from_both_naming_schemes() {
    assert_eq!(
        stint_stamp("sessions/stint-20260720-233644.ftel"),
        Some("20260720-233644")
    );
    assert_eq!(
        stint_stamp("sessions/session-20260719-115355.ftel"),
        Some("20260719-115355")
    );
    assert_eq!(stint_stamp("sessions/other.ftel"), None);
}

fn gearing_rec(softer: bool) -> Recommendation {
    Recommendation {
        apply: Vec::new(),
        area: "gearing",
        advice: "shorten the final drive".into(),
        evidence: vec![],
        confidence: Confidence::Medium,
        suggestion: None,
        probe: false,
        implied: Some(journal::Change {
            family: journal::Family::Gearing,
            softer,
            magnitude: None,
        }),
    }
}

/// The drag model's scale resolves to a concrete, caveated final-drive
/// target on the matching gearing rec (shorten = scale > 1 = softer false).
#[test]
fn fd_scale_resolves_to_caveated_target() {
    let session = session_with(&[("final_drive", "3.4")], &[]);
    let mut recs = vec![gearing_rec(false)];
    enrich::apply_fd_scale(&mut recs, &session, Some(1.2), None);
    let s = recs[0].suggestion.as_deref().unwrap();
    assert!(s.contains("3.4 → 4.08"), "{s}");
    assert!(s.contains("drag-model estimate"), "{s}");
    assert_eq!(
        recs[0].apply,
        vec![("final_drive".to_string(), "4.08".to_string())]
    );
}

/// Direction mismatch, an existing suggestion, or a near-no-op scale must
/// all leave the rec untouched; the target clamps into the slider range.
#[test]
fn fd_scale_respects_direction_ownership_and_limits() {
    let session = session_with(&[("final_drive", "3.4")], &[]);
    // Lengthen rec (softer) with a shorten scale: untouched.
    let mut recs = vec![gearing_rec(true)];
    enrich::apply_fd_scale(&mut recs, &session, Some(1.2), None);
    assert!(recs[0].suggestion.is_none());

    // A measured suggestion already owns the rec: untouched.
    let mut recs = vec![gearing_rec(false)];
    recs[0].suggestion = Some("final drive 3.4 → 4.2 (measured)".into());
    enrich::apply_fd_scale(&mut recs, &session, Some(1.2), None);
    assert!(recs[0].suggestion.as_deref().unwrap().contains("measured"));
    assert!(recs[0].apply.is_empty());

    // Near-1.0 scale = no-op: untouched.
    let mut recs = vec![gearing_rec(false)];
    enrich::apply_fd_scale(&mut recs, &session, Some(1.0004), None);
    assert!(recs[0].suggestion.is_none());

    // Clamped to the slider limit when the estimate overshoots.
    let session = session_with(
        &[("final_drive", "3.4")],
        &[("limit_final_drive", "2.2..4.0")],
    );
    let mut recs = vec![gearing_rec(false)];
    enrich::apply_fd_scale(&mut recs, &session, Some(1.5), None);
    assert!(
        recs[0].suggestion.as_deref().unwrap().contains("3.4 → 4"),
        "{:?}",
        recs[0].suggestion
    );
}

/// The scale applies to the final drive the stint was DRIVEN on, never the
/// latest saved revision: an accepted-but-undriven change must not be
/// re-scaled on top of itself (4.12 × 1.17 saved as 4.82 advising 5.64).
#[test]
fn fd_scale_bases_on_driven_setup() {
    // Latest revision already holds the applied target: pending, not a
    // further step.
    let session = session_with(&[("final_drive", "4.82")], &[]);
    let mut recs = vec![gearing_rec(false)];
    enrich::apply_fd_scale(&mut recs, &session, Some(1.17), Some(4.12));
    let s = recs[0].suggestion.as_deref().unwrap();
    assert!(s.contains("already saved"), "{s}");
    assert_eq!(
        recs[0].apply,
        vec![("final_drive".to_string(), "4.82".to_string())]
    );

    // Latest revision moved somewhere else: the target still comes from the
    // driven value, the arrow from the current one.
    let session = session_with(&[("final_drive", "4.5")], &[]);
    let mut recs = vec![gearing_rec(false)];
    enrich::apply_fd_scale(&mut recs, &session, Some(1.17), Some(4.12));
    let s = recs[0].suggestion.as_deref().unwrap();
    assert!(s.contains("4.5 → 4.82"), "{s}");

    // Latest revision moved PAST the target: an arrow against the rec's
    // shorten direction would read broken, so the estimate stays quiet.
    let session = session_with(&[("final_drive", "5.0")], &[]);
    let mut recs = vec![gearing_rec(false)];
    enrich::apply_fd_scale(&mut recs, &session, Some(1.17), Some(4.12));
    assert!(recs[0].suggestion.is_none());
    assert!(recs[0].apply.is_empty());
}

/// Consecutive same-setup stints group into one state; a setup change, an
/// unbound side, or a standing-start mismatch starts a new group, and a
/// non-consecutive return to an old setup stays separate (A-B-A identity).
#[test]
fn consecutive_same_setup_stints_group() {
    use super::campaign::consecutive_groups;
    let rev = |stamp: &str, arb: &str| Revision {
        stamp: stamp.into(),
        values: [("arb_front".to_string(), arb.to_string())].into(),
    };
    let a = rev("20260801-000000", "30");
    let b = rev("20260801-010000", "25");
    // setups: A A B B A A A None ; standing flips inside the trailing A run
    let setups: Vec<Option<&Revision>> = vec![
        Some(&a),
        Some(&a),
        Some(&b),
        Some(&b),
        Some(&a),
        Some(&a),
        Some(&a),
        None,
    ];
    let standing = vec![false, false, false, false, false, true, true, false];
    let groups = consecutive_groups(&standing, &setups);
    assert_eq!(groups, vec![0, 0, 2, 2, 4, 5, 5, 7]);
}

/// The setup-lint tier (plan 016): bump/rebound band, dampers-mirror-springs
/// inconsistency, and ride-height-above-minimum all fire from tune state,
/// phrase themselves as convention, and defer to existing evidence on the
/// same family.
#[test]
fn setup_lints_fire_from_tune_state_and_defer() {
    use crate::advice::tuning::{Revision, TuningSession};
    let rev = |pairs: &[(&str, &str)]| {
        let mut r = Revision::default();
        for (k, v) in pairs {
            r.values.insert(k.to_string(), v.to_string());
        }
        r
    };
    let mut session = TuningSession::default();
    // In-band bump/rebound (0.62), consistent splits: silent.
    session.revisions.push(rev(&[
        ("rebound_f", "10.0"),
        ("rebound_r", "13.0"),
        ("bump_f", "6.2"),
        ("bump_r", "8.1"),
        ("springs_f", "500"),
        ("springs_r", "600"),
    ]));
    assert!(super::enrich::setup_lints(&session, &[], &[], None).is_empty());

    // Bump above 70% of rebound on one end: the ratio lint fires.
    session.revisions.push(rev(&[
        ("rebound_f", "10.0"),
        ("rebound_r", "13.0"),
        ("bump_f", "9.0"),
        ("bump_r", "8.1"),
        ("springs_f", "500"),
        ("springs_r", "600"),
    ]));
    let lints = super::enrich::setup_lints(&session, &[], &[], None);
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert!(
        lints[0].advice.contains("commonly 40-70%"),
        "{}",
        lints[0].advice
    );
    assert!(lints[0].evidence[0].contains("convention"));

    // Stiffer-sprung rear but stiffer-damped front: the mirror lint fires.
    session.revisions.push(rev(&[
        ("rebound_f", "13.0"),
        ("rebound_r", "10.0"),
        ("bump_f", "8.1"),
        ("bump_r", "6.2"),
        ("springs_f", "500"),
        ("springs_r", "600"),
    ]));
    let lints = super::enrich::setup_lints(&session, &[], &[], None);
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert!(
        lints[0].advice.contains("mirrors the spring split"),
        "{}",
        lints[0].advice
    );

    // An existing damping rec on file silences both damping lints.
    let existing = recommend::Recommendation {
        apply: Vec::new(),
        area: "damping",
        suggestion: None,
        advice: "reduce front rebound".into(),
        evidence: Vec::new(),
        confidence: recommend::Confidence::Medium,
        probe: false,
        implied: None,
    };
    assert!(
        super::enrich::setup_lints(&session, &[], std::slice::from_ref(&existing), None).is_empty()
    );

    // Ride height above recorded minimum with a bottoming-free tarmac stint.
    session
        .facts
        .insert("limit_ride_height_f".into(), "4.0..8.0".into());
    session
        .facts
        .insert("limit_ride_height_r".into(), "4.0..8.0".into());
    session
        .revisions
        .push(rev(&[("ride_height_f", "5.5"), ("ride_height_r", "5.5")]));
    let met = crate::analysis::metrics::StintMetrics::default();
    let lints = super::enrich::setup_lints(&session, &[], &[], Some(&met));
    assert_eq!(lints.len(), 1, "{lints:?}");
    assert!(
        lints[0].advice.contains("free speed"),
        "{}",
        lints[0].advice
    );
    assert_eq!(
        lints[0].implied.unwrap().family,
        journal::Family::RideHeight
    );
    // Dirt: the ride-height lint stays quiet.
    let dirt = crate::analysis::metrics::StintMetrics {
        surface_loose: true,
        ..Default::default()
    };
    assert!(super::enrich::setup_lints(&session, &[], &[], Some(&dirt)).is_empty());
}
