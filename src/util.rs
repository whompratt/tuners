//! Small shared helpers.

pub const MPS_TO_MPH: f32 = 2.236_936_3;

/// Format Unix seconds as a UTC `YYYYMMDD-HHMMSS` stamp (for session filenames).
/// Date math is Howard Hinnant's civil_from_days algorithm.
pub fn utc_stamp(unix_secs: u64) -> String {
    let days = (unix_secs / 86_400) as i64;
    let secs = unix_secs % 86_400;

    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);

    format!(
        "{year:04}{month:02}{day:02}-{:02}{:02}{:02}",
        secs / 3600,
        (secs / 60) % 60,
        secs % 60
    )
}

/// Format a lap time the way racers read them: `1:30.791`, or `43.210` when
/// sub-minute. Milliseconds always shown to three places.
pub fn format_lap_time(secs: f32) -> String {
    let total_ms = (f64::from(secs) * 1000.0).round().max(0.0) as u64;
    let minutes = total_ms / 60_000;
    let rem_ms = total_ms % 60_000;
    if minutes > 0 {
        format!("{minutes}:{:02}.{:03}", rem_ms / 1000, rem_ms % 1000)
    } else {
        format!("{}.{:03}", rem_ms / 1000, rem_ms % 1000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lap_times_read_like_lap_times() {
        assert_eq!(format_lap_time(90.791), "1:30.791");
        assert_eq!(format_lap_time(43.21), "43.210");
        assert_eq!(format_lap_time(60.0), "1:00.000");
        assert_eq!(format_lap_time(59.9996), "1:00.000"); // rounds across the minute
        assert_eq!(format_lap_time(605.05), "10:05.050");
        assert_eq!(format_lap_time(0.0), "0.000");
    }

    #[test]
    fn known_dates() {
        assert_eq!(utc_stamp(0), "19700101-000000");
        // 2026-01-01T00:00:00Z
        assert_eq!(utc_stamp(1_767_225_600), "20260101-000000");
        assert_eq!(utc_stamp(1_767_225_600 + 3661), "20260101-010101");
    }
}
