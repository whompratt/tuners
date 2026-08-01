//! Small shared helpers.

pub const MPS_TO_MPH: f32 = 2.236_936_3;

/// Single data root: sessions, tune files, journals, archives,
/// outbox, and collection config all resolve under one directory. Resolution
/// order: `TUNERS_DATA` env, then a root installed by the app shell
/// (app_data_dir), then the current directory, which keeps CLI behavior in
/// a repo checkout identical to before the root existed.
static DATA_ROOT: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

/// Install the fallback root (the desktop app passes its app_data_dir).
/// `TUNERS_DATA` still wins; no-op if the root was already resolved.
pub fn set_data_root(fallback: std::path::PathBuf) {
    let _ = DATA_ROOT.set(
        std::env::var_os("TUNERS_DATA")
            .map(std::path::PathBuf::from)
            .unwrap_or(fallback),
    );
}

pub fn data_root() -> &'static std::path::Path {
    DATA_ROOT.get_or_init(|| {
        std::env::var_os("TUNERS_DATA")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from("."))
    })
}

/// A path under the data root. With the default root the relative name comes
/// back unchanged, so stored references (journal stint paths) never grow a
/// "./" prefix.
pub fn data_path(rel: &str) -> std::path::PathBuf {
    let root = data_root();
    if root == std::path::Path::new(".") {
        std::path::PathBuf::from(rel)
    } else {
        root.join(rel)
    }
}

/// CLI report display units (storage stays canonical: °F, m/s). Thread-local
/// so tests can't pollute each other; defaults imperial, set once by main
/// from the active session's unit prefs or --units.
#[derive(Debug, Clone, Copy, Default)]
pub struct DisplayUnits {
    pub temp_c: bool,
    pub speed_kmh: bool,
}

thread_local! {
    static DISPLAY_UNITS: std::cell::Cell<DisplayUnits> = const { std::cell::Cell::new(DisplayUnits { temp_c: false, speed_kmh: false }) };
}

pub fn set_display_units(u: DisplayUnits) {
    DISPLAY_UNITS.with(|c| c.set(u));
}

/// Canonical °F -> display value.
pub fn temp_val(f: f32) -> f32 {
    if DISPLAY_UNITS.with(|c| c.get()).temp_c {
        (f - 32.0) / 1.8
    } else {
        f
    }
}

/// Canonical °F difference -> display difference (scale only, no offset).
pub fn temp_delta_val(f: f32) -> f32 {
    if DISPLAY_UNITS.with(|c| c.get()).temp_c {
        f / 1.8
    } else {
        f
    }
}

pub fn temp_unit() -> &'static str {
    if DISPLAY_UNITS.with(|c| c.get()).temp_c {
        "°C"
    } else {
        "°F"
    }
}

/// Canonical m/s -> display value.
pub fn speed_val(mps: f32) -> f32 {
    if DISPLAY_UNITS.with(|c| c.get()).speed_kmh {
        mps * 3.6
    } else {
        mps * MPS_TO_MPH
    }
}

pub fn speed_unit() -> &'static str {
    if DISPLAY_UNITS.with(|c| c.get()).speed_kmh {
        "km/h"
    } else {
        "mph"
    }
}

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

/// Streaming SHA-256 (FIPS 180-4), for bundle content hashes.
/// Hand-rolled like everything else; checked against NIST test vectors below.
pub struct Sha256 {
    state: [u32; 8],
    buf: [u8; 64],
    buf_len: usize,
    total_bytes: u64,
}

const SHA256_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

impl Default for Sha256 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha256 {
    pub fn new() -> Self {
        Sha256 {
            state: [
                0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
                0x5be0cd19,
            ],
            buf: [0; 64],
            buf_len: 0,
            total_bytes: 0,
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.total_bytes = self.total_bytes.wrapping_add(data.len() as u64);
        if self.buf_len > 0 {
            let take = (64 - self.buf_len).min(data.len());
            self.buf[self.buf_len..self.buf_len + take].copy_from_slice(&data[..take]);
            self.buf_len += take;
            data = &data[take..];
            if self.buf_len == 64 {
                let block = self.buf;
                self.compress(&block);
                self.buf_len = 0;
            }
        }
        while data.len() >= 64 {
            let mut block = [0u8; 64];
            block.copy_from_slice(&data[..64]);
            self.compress(&block);
            data = &data[64..];
        }
        self.buf[..data.len()].copy_from_slice(data);
        self.buf_len += data.len();
    }

    pub fn finish_hex(mut self) -> String {
        let bit_len = self.total_bytes.wrapping_mul(8);
        self.update(&[0x80]);
        while self.buf_len != 56 {
            self.update(&[0]);
        }
        self.update(&bit_len.to_be_bytes());
        self.state.iter().map(|w| format!("{w:08x}")).collect()
    }

    fn compress(&mut self, block: &[u8; 64]) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                block[4 * i],
                block[4 * i + 1],
                block[4 * i + 2],
                block[4 * i + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = self.state;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = h
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(SHA256_K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (s, v) in self.state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *s = s.wrapping_add(v);
        }
    }
}

/// One-shot SHA-256 of a byte slice, lowercase hex.
pub fn sha256_hex(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    h.finish_hex()
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
    fn sha256_nist_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        // A million 'a's, fed in awkward chunk sizes to exercise buffering.
        let mut h = Sha256::new();
        let chunk = [b'a'; 977];
        let mut left = 1_000_000usize;
        while left > 0 {
            let n = left.min(chunk.len());
            h.update(&chunk[..n]);
            left -= n;
        }
        assert_eq!(
            h.finish_hex(),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    #[test]
    fn known_dates() {
        assert_eq!(utc_stamp(0), "19700101-000000");
        // 2026-01-01T00:00:00Z
        assert_eq!(utc_stamp(1_767_225_600), "20260101-000000");
        assert_eq!(utc_stamp(1_767_225_600 + 3661), "20260101-010101");
    }
}
