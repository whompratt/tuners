//! Stint bundles (plan 009 phase 1): the unit of telemetry collection. One
//! bundle = one raw recording plus the session/journal context needed to
//! interpret it, with all free text structurally absent — the filtered files
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
//! compared against the sources before it is returned — a compressor bug can
//! produce a failed export, never a corrupt bundle. `open` is the shared
//! reader used by tests today and `tuners ingest` later.

use crate::tuning::{self, TuningSession};
use crate::util::sha256_hex;
use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;

pub const BUNDLE_VERSION: &str = "1";
const CONSENT: &str = "collected with informed opt-in consent for tuners development; \
                       free text is stripped before export";
/// Decompression guard for ingest: no legitimate stint approaches this.
const MAX_UNPACKED: usize = 1 << 30;
/// Measured FLAT on real telemetry (levels 3/9/15/19 all ≈1.9x on a 20.8 MB
/// stint, 2026-07-26) — the packet format's entropy is the wall; the plan's
/// byte-columnar transpose is the v2 lever if size ever matters. 9 costs
/// ~0.5s per stint, a hair smaller than default.
const ZSTD_LEVEL: i32 = 9;

#[derive(Debug)]
pub struct Bundle {
    pub manifest: BTreeMap<String, String>,
    pub stint: Vec<u8>,
    pub session_txt: String,
    pub journal_txt: String,
}

/// Build the bundle for one recording. Returns (file name, bytes).
pub fn build(
    stint_path: &Path,
    session: &TuningSession,
    journal_text: &str,
) -> Result<(String, Vec<u8>), String> {
    let car = session
        .car
        .ok_or("session has no car — export needs an active tuning session")?;
    let file_name = stint_path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("bad stint path")?;
    // Recordings are stint-<stamp>.ftel; the stamp names the bundle. Other
    // .ftel stems (fixtures) pass through as-is — charset-fenced because the
    // stamp lands in the upload URL.
    let stamp = file_name
        .strip_suffix(".ftel")
        .map(|s| s.strip_prefix("stint-").unwrap_or(s))
        .filter(|s| {
            !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
        .ok_or_else(|| format!("{file_name}: expected <name>.ftel with a plain-ascii stem"))?;

    // The recording must decode end-to-end before it ships: a truncated or
    // corrupt stint is caught at the sender, where the original still exists.
    let mut reader = crate::stint::StintReader::open(stint_path).map_err(|e| e.to_string())?;
    let mut packets = 0u64;
    while reader.next_packet().map_err(|e| format!("{file_name}: {e}"))?.is_some() {
        packets += 1;
    }
    if packets == 0 {
        return Err(format!("{file_name}: no packets — refusing to bundle an empty stint"));
    }
    let stint = std::fs::read(stint_path).map_err(|e| e.to_string())?;

    let session_txt = export_session(session).render();
    let journal_txt = export_journal(journal_text, car);

    let mut manifest = BTreeMap::new();
    manifest.insert("bundle_version".into(), BUNDLE_VERSION.to_string());
    manifest.insert("tool_version".into(), env!("CARGO_PKG_VERSION").to_string());
    manifest.insert("car".into(), car.to_string());
    manifest.insert("stint_stamp".into(), stamp.to_string());
    manifest.insert("packets".into(), packets.to_string());
    manifest.insert("consent".into(), CONSENT.to_string());
    manifest.insert("sha256_stint".into(), sha256_hex(&stint));
    manifest.insert("sha256_session".into(), sha256_hex(session_txt.as_bytes()));
    manifest.insert("sha256_journal".into(), sha256_hex(journal_txt.as_bytes()));

    let mut tar = Vec::new();
    tar_append(&mut tar, "manifest.json", render_manifest(&manifest).as_bytes());
    tar_append(&mut tar, "stint.ftel", &stint);
    tar_append(&mut tar, "session.txt", session_txt.as_bytes());
    tar_append(&mut tar, "journal.txt", journal_txt.as_bytes());
    tar.extend_from_slice(&[0u8; 1024]);

    let bytes = zstd::stream::encode_all(&tar[..], ZSTD_LEVEL).map_err(|e| format!("zstd: {e}"))?;

    // Self-verify: reopen what we just produced and compare against sources.
    let back = open(&bytes)?;
    if back.stint != stint || back.session_txt != session_txt || back.journal_txt != journal_txt {
        return Err("self-verify failed: bundle does not round-trip (not exported)".into());
    }

    Ok((format!("bundle-{car}-{stamp}.tar.zst"), bytes))
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
    if manifest.get("bundle_version").map(String::as_str) != Some(BUNDLE_VERSION) {
        return Err(format!(
            "bundle version {:?} unsupported (expected {BUNDLE_VERSION})",
            manifest.get("bundle_version")
        ));
    }
    for (member, key) in [
        ("stint.ftel", "sha256_stint"),
        ("session.txt", "sha256_session"),
        ("journal.txt", "sha256_journal"),
    ] {
        let claimed = manifest.get(key).ok_or_else(|| format!("manifest missing {key}"))?;
        if *claimed != sha256_hex(&members[member]) {
            return Err(format!("{member}: hash mismatch vs manifest"));
        }
    }
    if !members["stint.ftel"].starts_with(crate::stint::MAGIC) {
        return Err("stint.ftel: bad magic".into());
    }

    Ok(Bundle {
        manifest,
        stint: members.remove("stint.ftel").unwrap(),
        session_txt: String::from_utf8_lossy(&members["session.txt"]).into_owned(),
        journal_txt: String::from_utf8_lossy(&members["journal.txt"]).into_owned(),
    })
}

/// Structured facts the dashboard records; everything else a user may have
/// typed into facts is dropped. Allowlist, per the plan: keys in, not text out.
const FACT_ALLOWLIST: &[&str] =
    &["front_weight_pct", "weight", "tire_compound", "abs", "tcs", "stability"];

/// The session minus free text: `name`/`description` and unrecognized fact
/// keys go; allowlisted facts, unit prefs, slider limits, and tune revisions
/// (known fields only) stay.
pub fn export_session(s: &TuningSession) -> TuningSession {
    let known_field = |k: &str| tuning::FIELDS.iter().any(|(f, _)| *f == k);
    let mut out = TuningSession { car: s.car, ..Default::default() };
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
        // Values must be numeric — a slider value can't smuggle text.
        values.retain(|_, v| v.parse::<f32>().is_ok());
        out.revisions.push(tuning::Revision { stamp: rev.stamp.clone(), values });
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
            if let Some(stamp) = line.strip_prefix(marker) {
                if !stamp.is_empty() && stamp.bytes().all(|b| b.is_ascii_digit() || b == b'-') {
                    out.push_str(line);
                    out.push('\n');
                }
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
            || !path.bytes().all(|b| b.is_ascii_alphanumeric() || b"._/-".contains(&b))
        {
            continue;
        }
        let kept: Vec<String> = note.split(';').filter_map(|c| strict_clause(c.trim())).collect();
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
/// Anything else — trailing words, unknown phrases, non-numeric values — is
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
        [delta] if delta.starts_with(['+', '-']) && num(delta) => {
            Some(format!("{phrase} {delta}"))
        }
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
    let fields: Vec<String> =
        m.iter().map(|(k, v)| format!("\"{}\":\"{}\"", esc(k), esc(v))).collect();
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
        m.insert("note".to_string(), "with \"quotes\" and \\slash".to_string());
        assert_eq!(parse_manifest(&render_manifest(&m)).unwrap(), m);
    }

    #[test]
    fn strict_clauses_admit_only_machine_grammar() {
        // The exact shapes diff_note generates survive, re-rendered.
        assert_eq!(strict_clause("front arb -2").as_deref(), Some("front arb -2"));
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
        // Prose around a parseable core is dropped whole-clause — the loose
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
# 1967 Ferrari 330 P4 (ordinal 2793) — my notes: gearbox whine at 8k?
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
        let entries = crate::analysis::journal::parse_journal(&out);
        assert_eq!(entries.len(), 4);
    }

    #[test]
    fn session_filter_is_allowlist() {
        let mut s = TuningSession { car: Some(2793), ..Default::default() };
        for (k, v) in [
            ("name", "rwd build"),
            ("description", "no aero, testing snap oversteer near my house"),
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
        for absent in ["rwd build", "snap oversteer", "call dave", "free text", "not a number"] {
            assert!(!text.contains(absent), "leaked: {absent}\n{text}");
        }
        assert_eq!(out.facts.get("front_weight_pct").map(String::as_str), Some("41.5"));
        assert_eq!(out.facts.get("unit_pressure").map(String::as_str), Some("bar"));
        assert_eq!(out.facts.get("limit_springs_f").map(String::as_str), Some("100..800"));
        assert!(!out.facts.contains_key("limit_bogus"));
        assert_eq!(out.revisions[0].values.get("arb_f").map(String::as_str), Some("24"));
        assert_eq!(out.revisions[0].values.len(), 1);
    }
}
