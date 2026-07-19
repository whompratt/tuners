//! Analysis pipeline against the real capture: values must stay physically plausible.

use std::path::Path;
use tuners::analysis::{metrics::stint_metrics, split_stints, Session};

#[test]
fn real_fixture_analysis_is_sane() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/real-01.ftel");
    let session = Session::load(Path::new(path)).unwrap();
    assert_eq!(session.decode_errors, 0);

    let stints = split_stints(&session.frames, 5.0);
    assert_eq!(stints.len(), 1, "fixture is one contiguous race-on segment");

    let m = stint_metrics(stints[0]);
    assert_eq!(m.samples, 1200);
    assert!((m.duration_s - 7.2).abs() < 0.5, "duration {}", m.duration_s);
    assert_eq!(m.car_ordinal, 4165);
    assert!(m.max_speed > 60.0 && m.max_speed < 80.0, "max speed {} m/s", m.max_speed);
    // ~7.2s at 50-70 m/s: integrated distance must land in a plausible band
    assert!(m.distance_m > 300.0 && m.distance_m < 600.0, "distance {} m", m.distance_m);
    assert!(m.redline > 9000.0);

    // Tire temps stay in a plausible °F band and every metric is finite.
    for t in m.tire_temp.to_array() {
        assert!(t.avg > 100.0 && t.avg < 250.0, "avg temp {}", t.avg);
        assert!(t.max >= t.avg && t.max < 250.0);
    }
    for s in m.suspension.to_array() {
        assert!(s.avg > 0.0 && s.avg < 1.0);
        assert!(s.bottomed_frac.is_finite() && s.topped_frac.is_finite());
    }
    for (g, f) in &m.gears.time_frac {
        assert!((1..=10).contains(g));
        assert!(*f > 0.0 && *f <= 1.0);
    }
    assert!(m.gears.top_gear >= 7, "reached high gears at 155 mph");
}
