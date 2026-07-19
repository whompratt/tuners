//! Guards the session file format and packet layout against accidental changes:
//! the committed fixture must always decode cleanly.

use std::path::Path;
use tuners::{packet, session::SessionReader};

#[test]
fn fixture_decodes_cleanly() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/synthetic-01.ftel");
    let mut reader = SessionReader::open(Path::new(path)).unwrap();

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
