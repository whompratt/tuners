//! Guards the session file format and packet layout against accidental changes:
//! the committed fixture must always decode cleanly.

use std::path::Path;
use tuners::{packet, stint::StintReader};

/// Real FH6 capture (2026-07-19, ordinal 4165, S1 800 AWD): the ground truth for
/// the packet layout. If this breaks, the decoder is wrong, not the fixture.
#[test]
fn real_fixture_decodes_cleanly() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/real-01.ftel");
    let mut reader = StintReader::open(Path::new(path)).unwrap();

    let mut packets = 0u64;
    while let Some((_recv_us, payload)) = reader.next_packet().unwrap() {
        assert_eq!(payload.len(), packet::PACKET_LEN);
        let frame = packet::decode(&payload).unwrap();
        assert!(frame.is_race_on);
        assert_eq!(frame.car_ordinal, 4165);
        assert_eq!(frame.drivetrain_type, 2);
        assert!(
            frame.speed >= 0.0 && frame.speed < 150.0,
            "speed {} m/s",
            frame.speed
        );
        assert!(frame.current_engine_rpm <= frame.engine_max_rpm + 500.0);
        packets += 1;
    }
    assert_eq!(packets, 1200);
}

#[test]
fn fixture_decodes_cleanly() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/synthetic-01.ftel");
    let mut reader = StintReader::open(Path::new(path)).unwrap();

    let mut packets = 0u64;
    let mut first = None;
    while let Some((_recv_us, payload)) = reader.next_packet().unwrap() {
        assert_eq!(payload.len(), packet::PACKET_LEN);
        let frame = packet::decode(&payload).unwrap();
        first.get_or_insert(frame);
        packets += 1;
    }

    assert_eq!(packets, 600);
    let first = first.unwrap();
    assert!(first.is_race_on);
    assert_eq!(first.car_ordinal, 1234);
    assert_eq!(first.drivetrain_type, 1);
    assert_eq!(first.engine_max_rpm, 7500.0);
}
