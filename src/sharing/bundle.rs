//! Stint bundles: the unit of telemetry collection. One
//! bundle = one raw recording plus the session/journal context needed to
//! interpret it, with all free text structurally absent: the filtered files
//! are rebuilt from parsed structures and a strict grammar, never redacted.
//!
//! Layout: `bundle-<car>-<stamp>.tar.zst` = zstd(tar) of
//!   manifest.json   flat JSON: versions, car, stamp, member sha-256 hashes
//!   stint.ftel      the raw recording, byte-identical, untrimmed
//!   session.txt     session minus free text: allowlisted facts + tune revisions
//!   journal.txt     entries whose notes are reduced to machine-grammar deltas
//!
//! The journal filter is DELIBERATELY stricter than `journal::parse_clause`:
//! that parser tolerates prose around a recognizable core ("front arb -2
//! because it felt pushy" parses), so exporting parser-accepted clauses
//! verbatim would leak text. Instead a clause survives only if it matches the
//! exact `tuning::diff_note` grammar the dashboard generates, and it is
//! re-rendered from the parsed parts.
//!
//! `build` self-verifies: the produced archive is reopened, hash-checked, and
//! compared against the sources before it is returned, so a compressor bug can
//! produce a failed export, never a corrupt bundle. `open` is the shared
//! reader used by tests today and `tuners ingest` later.

use crate::advice::tuning::{self, TuningSession};
use crate::util::sha256_hex;
use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;

/// Version written when the recording transposes (v2, columnar stint
/// member). A recording the transpose gate rejects falls back to "1" (raw
/// bytes) — loudly, never silently: a quiet fallback would forfeit the
/// compression win on every stint and hide recording-format drift.
pub const BUNDLE_VERSION: &str = "2";
/// The raw-stint layout; still written by the fallback path and accepted
/// by `open` forever (received v1 bundles never need migrating).
pub const BUNDLE_VERSION_V1: &str = "1";
/// Magic of the transposed stint member: byte-columnar re-layout of a
/// uniform-record .ftel, exactly reversible.
const TRANSPOSED_MAGIC: &[u8; 8] = b"FH6TELT2";
const CONSENT: &str = "collected with informed opt-in consent for tuners development; \
                       free text is stripped before export";
/// Decompression guard for ingest: no legitimate stint approaches this.
const MAX_UNPACKED: usize = 1 << 30;
/// Raw stints measured FLAT across zstd levels (≈1.9x; the interleaved
/// packet layout is the wall). The v2 transpose is what moves the number:
/// whole-library sweep 2026-07-29 (74 recordings, 1.2 GB) measured 1.99x
/// raw vs 3.12x transposed at this level, zero fallbacks, all files
/// byte-identical on round-trip (examples/transpose_scan.rs re-runs it).
const ZSTD_LEVEL: i32 = 9;

#[derive(Debug)]
pub struct Bundle {
    pub manifest: BTreeMap<String, String>,
    pub stint: Vec<u8>,
    pub session_txt: String,
    pub journal_txt: String,
}

/// The bundle file name a recording will get: recordings are
/// stint-<stamp>.ftel and the stamp names the bundle; other .ftel stems
/// (fixtures) pass through as-is, charset-fenced because the name lands in
/// the upload URL.
pub fn bundle_name(car: i32, stint_path: &Path) -> Result<String, String> {
    let file_name = stint_path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("bad stint path")?;
    let stamp = file_name
        .strip_suffix(".ftel")
        .map(|s| s.strip_prefix("stint-").unwrap_or(s))
        .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
        .ok_or_else(|| format!("{file_name}: expected <name>.ftel with a plain-ascii stem"))?;
    Ok(format!("bundle-{car}-{stamp}.tar.zst"))
}

/// Byte-columnar transpose of a uniform-record .ftel: header (record
/// count + record length), the recv_us stream delta-encoded, then payload
/// byte k of every record contiguous. Same bytes, compressor-friendly
/// order (~4.7x vs ~1.9x measured). Err = the recording is not
/// transposable (mixed record lengths, truncated tail) and names why —
/// the caller's fallback must be able to say the reason out loud.
pub fn transpose_recording(raw: &[u8]) -> Result<Vec<u8>, String> {
    let body = raw
        .strip_prefix(crate::telemetry::stint::MAGIC.as_slice())
        .ok_or("bad magic")?;
    let mut off = 0usize;
    let mut count = 0usize;
    let mut rec_len: Option<u32> = None;
    while off < body.len() {
        if body.len() - off < 12 {
            return Err(format!("{} trailing bytes", body.len() - off));
        }
        let len = u32::from_le_bytes(body[off + 8..off + 12].try_into().unwrap());
        match rec_len {
            None => rec_len = Some(len),
            Some(l) if l != len => {
                return Err(format!("mixed record lengths ({l} then {len})"));
            }
            _ => {}
        }
        if body.len() - off - 12 < len as usize {
            return Err("truncated final record".into());
        }
        off += 12 + len as usize;
        count += 1;
    }
    let rec_len = rec_len.ok_or("no records")?;
    if rec_len == 0 {
        return Err("zero-length records".into());
    }
    let l = rec_len as usize;
    let stride = 12 + l;
    let mut out = Vec::with_capacity(raw.len());
    out.extend_from_slice(TRANSPOSED_MAGIC);
    out.extend_from_slice(&(count as u64).to_le_bytes());
    out.extend_from_slice(&rec_len.to_le_bytes());
    let mut prev = 0u64;
    for i in 0..count {
        let t = u64::from_le_bytes(body[i * stride..i * stride + 8].try_into().unwrap());
        out.extend_from_slice(&t.wrapping_sub(prev).to_le_bytes());
        prev = t;
    }
    for k in 0..l {
        for i in 0..count {
            out.push(body[i * stride + 12 + k]);
        }
    }
    Ok(out)
}

/// Exact inverse of `transpose_recording`: reproduces the original .ftel
/// bytes, header included.
pub fn untranspose_recording(blob: &[u8]) -> Result<Vec<u8>, String> {
    let body = blob
        .strip_prefix(TRANSPOSED_MAGIC.as_slice())
        .ok_or("bad transposed magic")?;
    if body.len() < 12 {
        return Err("truncated transpose header".into());
    }
    let count = u64::from_le_bytes(body[0..8].try_into().unwrap()) as usize;
    let rec_len = u32::from_le_bytes(body[8..12].try_into().unwrap());
    let l = rec_len as usize;
    let expected = count
        .checked_mul(8 + l)
        .and_then(|n| n.checked_add(12))
        .ok_or("size overflow")?;
    if body.len() != expected {
        return Err(format!(
            "transposed payload {} bytes != expected {expected}",
            body.len()
        ));
    }
    let total = 8 + count * (12 + l);
    if total > MAX_UNPACKED {
        return Err("reconstructs beyond the sanity limit".into());
    }
    let times = &body[12..12 + count * 8];
    let cols = &body[12 + count * 8..];
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(crate::telemetry::stint::MAGIC);
    let mut prev = 0u64;
    for i in 0..count {
        let d = u64::from_le_bytes(times[i * 8..i * 8 + 8].try_into().unwrap());
        prev = prev.wrapping_add(d);
        out.extend_from_slice(&prev.to_le_bytes());
        out.extend_from_slice(&rec_len.to_le_bytes());
        for col in cols.chunks_exact(count) {
            out.push(col[i]);
        }
    }
    Ok(out)
}

/// Build the bundle for one recording. Returns (file name, bytes).
pub fn build(
    stint_path: &Path,
    session: &TuningSession,
    journal_text: &str,
) -> Result<(String, Vec<u8>), String> {
    let car = session
        .car
        .ok_or("session has no car; export needs an active tuning session")?;
    let name = bundle_name(car, stint_path)?;
    let stamp = name
        .strip_prefix(&format!("bundle-{car}-"))
        .and_then(|s| s.strip_suffix(".tar.zst"))
        .unwrap_or_default()
        .to_string();

    // The recording must decode end-to-end before it ships: a truncated or
    // corrupt stint is caught at the sender, where the original still exists.
    let mut reader =
        crate::telemetry::stint::StintReader::open(stint_path).map_err(|e| e.to_string())?;
    let mut packets = 0u64;
    while reader
        .next_packet()
        .map_err(|e| format!("{name}: {e}"))?
        .is_some()
    {
        packets += 1;
    }
    if packets == 0 {
        return Err(format!(
            "{name}: no packets, refusing to bundle an empty stint"
        ));
    }
    let stint = std::fs::read(stint_path).map_err(|e| e.to_string())?;

    // Columnar member when the recording is uniform (every real recording
    // is); otherwise raw v1 — and the fallback SAYS SO, because a silent
    // one would quietly cost the compression win on every stint.
    let (stint_member, version) = match transpose_recording(&stint) {
        Ok(t) => (t, BUNDLE_VERSION),
        Err(reason) => {
            eprintln!("bundle: {name}: not transposable ({reason}); storing raw as v1");
            (stint.clone(), BUNDLE_VERSION_V1)
        }
    };

    let session_txt = export_session(session).render();
    let journal_txt = export_journal(journal_text, car);

    let mut manifest = BTreeMap::new();
    manifest.insert("bundle_version".into(), version.to_string());
    manifest.insert("tool_version".into(), env!("CARGO_PKG_VERSION").to_string());
    manifest.insert("car".into(), car.to_string());
    manifest.insert("stint_stamp".into(), stamp.to_string());
    manifest.insert("packets".into(), packets.to_string());
    manifest.insert("consent".into(), CONSENT.to_string());
    manifest.insert("sha256_stint".into(), sha256_hex(&stint_member));
    manifest.insert("sha256_session".into(), sha256_hex(session_txt.as_bytes()));
    manifest.insert("sha256_journal".into(), sha256_hex(journal_txt.as_bytes()));

    let mut tar = Vec::new();
    tar_append(
        &mut tar,
        "manifest.json",
        render_manifest(&manifest).as_bytes(),
    );
    tar_append(&mut tar, "stint.ftel", &stint_member);
    tar_append(&mut tar, "session.txt", session_txt.as_bytes());
    tar_append(&mut tar, "journal.txt", journal_txt.as_bytes());
    tar.extend_from_slice(&[0u8; 1024]);

    let bytes = zstd::stream::encode_all(&tar[..], ZSTD_LEVEL).map_err(|e| format!("zstd: {e}"))?;

    // Self-verify: reopen what we just produced and compare against sources.
    let back = open(&bytes)?;
    if back.stint != stint || back.session_txt != session_txt || back.journal_txt != journal_txt {
        return Err("self-verify failed: bundle does not round-trip (not exported)".into());
    }

    Ok((name, bytes))
}

/// Decompress, parse, and hash-verify a bundle. Strict: unknown members,
/// missing members, hash mismatches, and version skew are all errors.
pub fn open(bytes: &[u8]) -> Result<Bundle, String> {
    let decoder = zstd::stream::read::Decoder::new(bytes).map_err(|e| format!("zstd: {e}"))?;
    let mut tar = Vec::new();
    decoder
        .take(MAX_UNPACKED as u64 + 1)
        .read_to_end(&mut tar)
        .map_err(|e| format!("zstd: {e}"))?;
    if tar.len() > MAX_UNPACKED {
        return Err("bundle unpacks beyond the sanity limit".into());
    }

    let mut members: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for (name, data) in tar_entries(&tar)? {
        members.insert(name, data);
    }
    let expected = ["manifest.json", "stint.ftel", "session.txt", "journal.txt"];
    if members.len() != expected.len() || expected.iter().any(|m| !members.contains_key(*m)) {
        return Err(format!(
            "bundle members {:?} != expected {expected:?}",
            members.keys().collect::<Vec<_>>()
        ));
    }

    let manifest = parse_manifest(&String::from_utf8_lossy(&members["manifest.json"]))?;
    let version = manifest
        .get("bundle_version")
        .map(String::as_str)
        .unwrap_or_default()
        .to_string();
    if version != BUNDLE_VERSION && version != BUNDLE_VERSION_V1 {
        return Err(format!(
            "bundle version {version:?} unsupported (expected {BUNDLE_VERSION_V1} or {BUNDLE_VERSION})"
        ));
    }
    for (member, key) in [
        ("stint.ftel", "sha256_stint"),
        ("session.txt", "sha256_session"),
        ("journal.txt", "sha256_journal"),
    ] {
        let claimed = manifest
            .get(key)
            .ok_or_else(|| format!("manifest missing {key}"))?;
        if *claimed != sha256_hex(&members[member]) {
            return Err(format!("{member}: hash mismatch vs manifest"));
        }
    }
    // v2 stores the stint columnar; reconstruct so every consumer (ingest's
    // re-decode, self-verify's byte-compare) sees the original recording.
    let stored = members.remove("stint.ftel").unwrap();
    let stint = if version == BUNDLE_VERSION {
        untranspose_recording(&stored).map_err(|e| format!("stint.ftel: {e}"))?
    } else {
        stored
    };
    if !stint.starts_with(crate::telemetry::stint::MAGIC) {
        return Err("stint.ftel: bad magic".into());
    }

    Ok(Bundle {
        manifest,
        stint,
        session_txt: String::from_utf8_lossy(&members["session.txt"]).into_owned(),
        journal_txt: String::from_utf8_lossy(&members["journal.txt"]).into_owned(),
    })
}

/// Structured facts the dashboard records; everything else a user may have
/// typed into facts is dropped. Allowlist, per the plan: keys in, not text out.
const FACT_ALLOWLIST: &[&str] = &[
    "front_weight_pct",
    "weight",
    "tire_compound",
    "abs",
    "tcs",
    "stability",
];

/// The session minus free text: `name`/`description` and unrecognized fact
/// keys go; allowlisted facts, unit prefs, slider limits, and tune revisions
/// (known fields only) stay.
pub fn export_session(s: &TuningSession) -> TuningSession {
    let known_field = |k: &str| tuning::FIELDS.iter().any(|(f, _)| *f == k);
    let mut out = TuningSession {
        car: s.car,
        ..Default::default()
    };
    for (k, v) in &s.facts {
        let keep = FACT_ALLOWLIST.contains(&k.as_str())
            || k.starts_with("unit_")
            || k.strip_prefix("limit_").is_some_and(known_field);
        if keep {
            out.facts.insert(k.clone(), v.clone());
        }
    }
    for rev in &s.revisions {
        let mut values: BTreeMap<String, String> = rev
            .values
            .iter()
            .filter(|(k, _)| known_field(k))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        // Values must be numeric; a slider value can't smuggle text.
        values.retain(|_, v| v.parse::<f32>().is_ok());
        out.revisions.push(tuning::Revision {
            stamp: rev.stamp.clone(),
            values,
        });
    }
    out
}

/// The journal reduced to machine-parseable content: a regenerated car
/// header, `# parked/# resumed` campaign markers, and per entry the stint
/// path plus only those note clauses matching the strict diff_note grammar.
pub fn export_journal(text: &str, car: i32) -> String {
    let mut out = match crate::cars::car_name(car) {
        Some(name) => format!("# {name} (ordinal {car})\n"),
        None => format!("# ordinal {car}\n"),
    };
    for line in text.lines() {
        let line = line.trim();
        for marker in ["# parked ", "# resumed "] {
            if let Some(stamp) = line.strip_prefix(marker)
                && !stamp.is_empty()
                && stamp.bytes().all(|b| b.is_ascii_digit() || b == b'-')
            {
                out.push_str(line);
                out.push('\n');
            }
        }
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (path, note) = match line.split_once('|') {
            Some((p, n)) => (p.trim(), n.trim()),
            None => (line, ""),
        };
        if path.is_empty()
            || !path
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._/-".contains(&b))
        {
            continue;
        }
        let kept: Vec<String> = note
            .split(';')
            .filter_map(|c| strict_clause(c.trim()))
            .collect();
        if kept.is_empty() {
            out.push_str(path);
            out.push('\n');
        } else {
            out.push_str(&format!("{path} | {}\n", kept.join("; ")));
        }
    }
    out
}

/// Accept exactly the `tuning::diff_note` shapes, re-rendered from parts:
///   "<field phrase> <signed delta> [canonical unit]"
///   "<field phrase> = <numeric value>"
///   "<field phrase> <numeric old> -> <numeric new>"
/// Anything else (trailing words, unknown phrases, non-numeric values) is
/// dropped whole-clause.
fn strict_clause(clause: &str) -> Option<String> {
    let lower = clause.to_lowercase();
    // Longest phrase first so "front tire pressure" wins over any shorter hit.
    let (key, phrase) = tuning::FIELDS
        .iter()
        .filter(|(_, p)| lower.starts_with(*p))
        .max_by_key(|(_, p)| p.len())?;
    let rest = clause[phrase.len()..].trim();
    let num = |s: &str| s.parse::<f32>().is_ok();

    if let Some(value) = rest.strip_prefix('=') {
        let value = value.trim();
        return num(value).then(|| format!("{phrase} = {value}"));
    }
    let toks: Vec<&str> = rest.split_whitespace().collect();
    match toks.as_slice() {
        // "front arb -2" / "rear aero +20 lb" (unit must be the field's own)
        [delta] if delta.starts_with(['+', '-']) && num(delta) => Some(format!("{phrase} {delta}")),
        [delta, unit]
            if delta.starts_with(['+', '-'])
                && num(delta)
                && tuning::canonical_unit(key) == Some(*unit) =>
        {
            Some(format!("{phrase} {delta} {unit}"))
        }
        [old, arrow, new] if *arrow == "->" && num(old) && num(new) => {
            Some(format!("{phrase} {old} -> {new}"))
        }
        _ => None,
    }
}

/// Test-only access to the raw writers, so integration tests can hand-craft
/// hostile bundles (correct hashes, smuggled content) that `build` refuses
/// to produce.
#[doc(hidden)]
pub mod raw {
    pub use super::{render_manifest, tar_append};
}

#[doc(hidden)]
pub fn render_manifest(m: &BTreeMap<String, String>) -> String {
    let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let fields: Vec<String> = m
        .iter()
        .map(|(k, v)| format!("\"{}\":\"{}\"", esc(k), esc(v)))
        .collect();
    format!("{{{}}}\n", fields.join(","))
}

/// Parse the flat string-valued JSON object render_manifest writes.
fn parse_manifest(text: &str) -> Result<BTreeMap<String, String>, String> {
    let mut out = BTreeMap::new();
    let body = text
        .trim()
        .strip_prefix('{')
        .and_then(|t| t.strip_suffix('}'))
        .ok_or("manifest: not a JSON object")?;
    let mut chars = body.chars().peekable();
    loop {
        while matches!(chars.peek(), Some(c) if c.is_whitespace() || *c == ',') {
            chars.next();
        }
        if chars.peek().is_none() {
            return Ok(out);
        }
        let key = json_string(&mut chars)?;
        while matches!(chars.peek(), Some(c) if c.is_whitespace()) {
            chars.next();
        }
        if chars.next() != Some(':') {
            return Err("manifest: expected ':'".into());
        }
        while matches!(chars.peek(), Some(c) if c.is_whitespace()) {
            chars.next();
        }
        let value = json_string(&mut chars)?;
        out.insert(key, value);
    }
}

fn json_string(chars: &mut std::iter::Peekable<std::str::Chars>) -> Result<String, String> {
    if chars.next() != Some('"') {
        return Err("manifest: expected string".into());
    }
    let mut s = String::new();
    loop {
        match chars.next() {
            Some('"') => return Ok(s),
            Some('\\') => match chars.next() {
                Some(c @ ('"' | '\\' | '/')) => s.push(c),
                Some('n') => s.push('\n'),
                Some('t') => s.push('\t'),
                other => return Err(format!("manifest: bad escape {other:?}")),
            },
            Some(c) => s.push(c),
            None => return Err("manifest: unterminated string".into()),
        }
    }
}

/// Minimal ustar writer: enough for flat files with short names.
#[doc(hidden)]
pub fn tar_append(out: &mut Vec<u8>, name: &str, data: &[u8]) {
    let mut header = [0u8; 512];
    header[..name.len()].copy_from_slice(name.as_bytes());
    header[100..108].copy_from_slice(b"0000644\0");
    header[108..116].copy_from_slice(b"0000000\0");
    header[116..124].copy_from_slice(b"0000000\0");
    let size = format!("{:011o}\0", data.len());
    header[124..136].copy_from_slice(size.as_bytes());
    header[136..148].copy_from_slice(b"00000000000\0");
    header[148..156].fill(b' '); // checksum computed over spaces
    header[156] = b'0';
    header[257..263].copy_from_slice(b"ustar\0");
    header[263..265].copy_from_slice(b"00");
    let checksum: u32 = header.iter().map(|&b| b as u32).sum();
    header[148..155].copy_from_slice(format!("{checksum:06o}\0").as_bytes());
    out.extend_from_slice(&header);
    out.extend_from_slice(data);
    let pad = (512 - data.len() % 512) % 512;
    out.extend_from_slice(&vec![0u8; pad]);
}

fn tar_entries(tar: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut out = Vec::new();
    let mut off = 0;
    while off + 512 <= tar.len() {
        let header = &tar[off..off + 512];
        if header.iter().all(|&b| b == 0) {
            return Ok(out);
        }
        let name_end = header[..100].iter().position(|&b| b == 0).unwrap_or(100);
        let name = String::from_utf8_lossy(&header[..name_end]).into_owned();
        let size_str = String::from_utf8_lossy(&header[124..136]);
        let size = usize::from_str_radix(size_str.trim_matches(['\0', ' ']), 8)
            .map_err(|_| format!("tar: bad size for {name}"))?;
        let data_start = off + 512;
        if data_start + size > tar.len() {
            return Err(format!("tar: truncated member {name}"));
        }
        // typeflag '0'/NUL = regular file; anything else has no business here.
        if header[156] != b'0' && header[156] != 0 {
            return Err(format!("tar: unexpected member type for {name}"));
        }
        out.push((name, tar[data_start..data_start + size].to_vec()));
        off = data_start + size.div_ceil(512) * 512;
    }
    Err("tar: missing end-of-archive".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ftel(records: &[(u64, &[u8])]) -> Vec<u8> {
        let mut raw = Vec::from(*crate::telemetry::stint::MAGIC);
        for (t, payload) in records {
            raw.extend_from_slice(&t.to_le_bytes());
            raw.extend_from_slice(&(payload.len() as u32).to_le_bytes());
            raw.extend_from_slice(payload);
        }
        raw
    }

    /// The transpose is an exact inverse pair on uniform recordings and
    /// names its reason on every rejected shape.
    #[test]
    fn transpose_round_trips_and_gates_name_reasons() {
        let uniform = ftel(&[
            (1_000_000, &[1u8, 2, 3, 4]),
            (1_016_667, &[5, 6, 7, 8]),
            (1_033_333, &[9, 10, 11, 12]),
        ]);
        let t = transpose_recording(&uniform).unwrap();
        assert!(t.starts_with(TRANSPOSED_MAGIC));
        assert_eq!(untranspose_recording(&t).unwrap(), uniform);

        let single = ftel(&[(42, &[7u8; 324])]);
        let t = transpose_recording(&single).unwrap();
        assert_eq!(untranspose_recording(&t).unwrap(), single);

        let mixed = ftel(&[(1, &[0u8; 324]), (2, &[0u8; 16])]);
        assert!(
            transpose_recording(&mixed).unwrap_err().contains("mixed"),
            "reason names the shape"
        );

        let mut trailing = ftel(&[(1, &[0u8; 8])]);
        trailing.extend_from_slice(&[0xFF; 5]);
        assert!(
            transpose_recording(&trailing)
                .unwrap_err()
                .contains("trailing")
        );

        let mut truncated = ftel(&[(1, &[0u8; 8])]);
        truncated.truncate(truncated.len() - 3);
        assert!(
            transpose_recording(&truncated)
                .unwrap_err()
                .contains("truncated")
        );

        let empty = ftel(&[]);
        assert!(
            transpose_recording(&empty)
                .unwrap_err()
                .contains("no records")
        );

        // A corrupt transposed blob must never reconstruct silently.
        let good = transpose_recording(&uniform).unwrap();
        let mut short = good.clone();
        short.truncate(good.len() - 1);
        assert!(untranspose_recording(&short).is_err());
    }

    #[test]
    fn tar_roundtrip() {
        let mut tar = Vec::new();
        tar_append(&mut tar, "a.txt", b"hello");
        tar_append(&mut tar, "b.bin", &[0u8; 513]);
        tar.extend_from_slice(&[0u8; 1024]);
        let entries = tar_entries(&tar).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0], ("a.txt".into(), b"hello".to_vec()));
        assert_eq!(entries[1].1.len(), 513);
    }

    #[test]
    fn manifest_roundtrip() {
        let mut m = BTreeMap::new();
        m.insert("car".to_string(), "2793".to_string());
        m.insert(
            "note".to_string(),
            "with \"quotes\" and \\slash".to_string(),
        );
        assert_eq!(parse_manifest(&render_manifest(&m)).unwrap(), m);
    }

    #[test]
    fn strict_clauses_admit_only_machine_grammar() {
        // The exact shapes diff_note generates survive, re-rendered.
        assert_eq!(
            strict_clause("front arb -2").as_deref(),
            Some("front arb -2")
        );
        assert_eq!(
            strict_clause("front springs -28 lb/in").as_deref(),
            Some("front springs -28 lb/in")
        );
        assert_eq!(
            strict_clause("front tire pressure = 28.5").as_deref(),
            Some("front tire pressure = 28.5")
        );
        assert_eq!(
            strict_clause("brake balance 52 -> 20").as_deref(),
            Some("brake balance 52 -> 20")
        );
        // Prose around a parseable core is dropped whole-clause; the loose
        // journal parser would have accepted every one of these.
        for leak in [
            "front arb -2 because it felt pushy",
            "softened the front arb by 2",
            "front arb -2 lb", // unit not this field's canonical unit
            "front arb = my favourite",
            "brake balance 52 -> 20 (much better turn in!)",
            "felt sketchy over the crest",
            "baseline",
            "suspect: deliberate slides",
        ] {
            assert_eq!(strict_clause(leak), None, "{leak}");
        }
    }

    #[test]
    fn journal_filter_strips_prose_keeps_structure() {
        let src = "\
# 1967 Ferrari 330 P4 (ordinal 2793), my notes: gearbox whine at 8k?
sessions/stint-20260724-182140.ftel | baseline
sessions/stint-20260724-231042.ftel | rear arb -1.5
sessions/stint-20260724-232059.ftel | front arb -1.5; felt terrible; rear aero +20 lb
# parked 20260725-111803
# random musing about the weather
# resumed 20260725-111907
bad path with spaces.ftel | front arb -1
sessions/stint-20260725-121955.ftel
";
        let out = export_journal(src, 2793);
        let expect = "\
# 1967 Ferrari 330 P4 (ordinal 2793)
sessions/stint-20260724-182140.ftel
sessions/stint-20260724-231042.ftel | rear arb -1.5
sessions/stint-20260724-232059.ftel | front arb -1.5; rear aero +20 lb
# parked 20260725-111803
# resumed 20260725-111907
sessions/stint-20260725-121955.ftel
";
        assert_eq!(out, expect);
        // Everything kept still parses on the ingest side.
        let entries = crate::advice::journal::parse_journal(&out);
        assert_eq!(entries.len(), 4);
    }

    #[test]
    fn session_filter_is_allowlist() {
        let mut s = TuningSession {
            car: Some(2793),
            ..Default::default()
        };
        for (k, v) in [
            ("name", "rwd build"),
            (
                "description",
                "no aero, testing snap oversteer near my house",
            ),
            ("front_weight_pct", "41.5"),
            ("tire_compound", "semi-slick"),
            ("unit_pressure", "bar"),
            ("limit_springs_f", "100..800"),
            ("limit_bogus", "1..2"),
            ("my_note", "call dave about the diff"),
        ] {
            s.facts.insert(k.into(), v.into());
        }
        s.revisions.push(tuning::Revision {
            stamp: "20260725-1200".into(),
            values: [
                ("arb_f".to_string(), "24".to_string()),
                ("evil".to_string(), "free text".to_string()),
                ("arb_r".to_string(), "not a number".to_string()),
            ]
            .into(),
        });
        let out = export_session(&s);
        let text = out.render();
        for absent in [
            "rwd build",
            "snap oversteer",
            "call dave",
            "free text",
            "not a number",
        ] {
            assert!(!text.contains(absent), "leaked: {absent}\n{text}");
        }
        assert_eq!(
            out.facts.get("front_weight_pct").map(String::as_str),
            Some("41.5")
        );
        assert_eq!(
            out.facts.get("unit_pressure").map(String::as_str),
            Some("bar")
        );
        assert_eq!(
            out.facts.get("limit_springs_f").map(String::as_str),
            Some("100..800")
        );
        assert!(!out.facts.contains_key("limit_bogus"));
        assert_eq!(
            out.revisions[0].values.get("arb_f").map(String::as_str),
            Some("24")
        );
        assert_eq!(out.revisions[0].values.len(), 1);
    }
}
