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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_dates() {
        assert_eq!(utc_stamp(0), "19700101-000000");
        // 2026-01-01T00:00:00Z
        assert_eq!(utc_stamp(1_767_225_600), "20260101-000000");
        assert_eq!(utc_stamp(1_767_225_600 + 3661), "20260101-010101");
    }
}
