//! Crowd prior artifact: the distilled, anonymous form of the effect map
//! that ships to every install. Only the two learned mappings leave the
//! maintainer side — aggregated dictionary cells and fitted behaviour-axis
//! landscapes — plus a provenance header. Raw sample rows, sender ids, car
//! ordinals, campaign floors, and driver trends never do: floors and trends
//! are per-driver by definition (each install derives its own), and sample
//! rows carry per-stint behaviour that the aggregate deliberately forgets.
//!
//! The artifact is content-gated on write: rebuilding from an unchanged map
//! leaves the file byte-identical (stamp included), so a scheduled pipeline
//! can run unconditionally and only publish when new data actually landed.

use crate::advice::effectmap::{self, EffectMap};
use serde::{Deserialize, Serialize};
use std::io::Read as _;
use std::path::Path;

/// Artifact schema this build writes and understands.
pub const SCHEMA: u32 = 1;

/// Pinned verification key for artifacts fetched from the collection
/// worker (hex, ed25519). The private half lives only on the maintainer
/// machine, alongside the updater key.
pub const PUBKEY_HEX: &str = "d3332e4ad0b590c17d960af90d8571ec014610b0fea2f8d16f614eadbe3ed0fd";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Priors {
    pub schema: u32,
    /// Oldest schema a consumer must support to read this artifact; an
    /// install refuses anything newer than it understands. Unknown fields
    /// are ignored on read, so additive changes keep this at 1.
    pub min_app_schema: u32,
    /// UTC stamp of generation. Advisory only: content equality (with this
    /// field masked) decides whether a rebuild rewrites the file.
    pub generated: String,
    /// Corpus provenance for the maintainer/Settings surface. Advice copy
    /// never quotes these — evidence lines say "global trend", not counts.
    pub samples: u32,
    pub campaigns: u32,
    pub senders: u32,
    pub cells: Vec<PriorCell>,
    pub landscapes: Vec<PriorContext>,
}

/// One aggregated dictionary cell (effectmap::Cell minus own_n, which is
/// meaningless off the maintainer machine: each install's own share comes
/// from merging its local map). Weak-only cells (n = 0) are not shipped —
/// they cannot ground advice by design.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PriorCell {
    pub family: String,
    pub softer: bool,
    pub drivetrain: i32,
    pub surface_loose: bool,
    pub aero: Option<bool>,
    pub n: u32,
    pub direct_n: u32,
    pub weak_n: u32,
    pub delta_mean: f32,
    pub delta_sd: f32,
    /// Per effect field: (key, samples carrying it, mean, sd).
    pub fields: Vec<(String, u32, f32, f32)>,
    pub key_mode: Option<String>,
    pub mag_mean: Option<f32>,
}

/// Axis landscapes fitted for one (surface, drivetrain) context — the
/// granularity enrich queries at advise time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PriorContext {
    pub surface_loose: bool,
    pub drivetrain: i32,
    pub axes: Vec<PriorAxis>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PriorAxis {
    pub key: String,
    pub n: u32,
    pub mean_gradient: f32,
    pub sign_share: f32,
    pub r: f32,
    pub alpha: f32,
    pub beta: f32,
    pub lo: f32,
    pub hi: f32,
    pub optimum: Option<f32>,
}

/// Distill the artifact from a built map. Deterministic for a given map:
/// cells arrive in aggregate()'s BTreeMap order, axes in registry order,
/// contexts in a fixed surface x drivetrain sweep.
pub fn derive(map: &EffectMap, generated: String) -> Priors {
    let cells = effectmap::aggregate(map)
        .into_iter()
        .filter(|c| c.n > 0)
        .map(|c| PriorCell {
            family: c.family,
            softer: c.softer,
            drivetrain: c.drivetrain,
            surface_loose: c.surface_loose,
            aero: c.aero,
            n: c.n as u32,
            direct_n: c.direct_n as u32,
            weak_n: c.weak_n as u32,
            delta_mean: c.delta_mean,
            delta_sd: c.delta_sd,
            fields: c
                .fields
                .into_iter()
                .map(|(k, n, m, sd)| (k.to_string(), n as u32, m, sd))
                .collect(),
            key_mode: c.key_mode,
            mag_mean: c.mag_mean,
        })
        .collect();
    let mut landscapes = Vec::new();
    for surface_loose in [false, true] {
        for drivetrain in 0..=2 {
            let axes: Vec<PriorAxis> =
                effectmap::axis_landscapes(map, surface_loose, Some(drivetrain))
                    .into_iter()
                    .map(|a| PriorAxis {
                        key: a.key.to_string(),
                        n: a.n as u32,
                        mean_gradient: a.mean_gradient,
                        sign_share: a.sign_share,
                        r: a.r,
                        alpha: a.alpha,
                        beta: a.beta,
                        lo: a.lo,
                        hi: a.hi,
                        optimum: a.optimum,
                    })
                    .collect();
            if !axes.is_empty() {
                landscapes.push(PriorContext {
                    surface_loose,
                    drivetrain,
                    axes,
                });
            }
        }
    }
    let mut senders: Vec<&str> = map.samples.iter().map(|s| s.driver.as_str()).collect();
    senders.sort_unstable();
    senders.dedup();
    Priors {
        schema: SCHEMA,
        min_app_schema: 1,
        generated,
        samples: map.samples.len() as u32,
        campaigns: map.floors.len() as u32,
        senders: senders.len() as u32,
        cells,
        landscapes,
    }
}

pub fn render(p: &Priors) -> String {
    let mut s = serde_json::to_string(p).expect("priors serialize");
    s.push('\n');
    s
}

pub fn parse(text: &str) -> Result<Priors, String> {
    let p: Priors = serde_json::from_str(text).map_err(|e| format!("bad priors JSON: {e}"))?;
    if p.min_app_schema > SCHEMA {
        return Err(format!(
            "priors artifact needs schema {} but this build supports {SCHEMA}",
            p.min_app_schema
        ));
    }
    Ok(p)
}

/// Write the artifact unless it matches what is already on disk with only
/// the stamp differing. Returns whether the file changed.
pub fn write(path: &Path, p: &Priors) -> Result<bool, String> {
    if let Ok(old_text) = std::fs::read_to_string(path)
        && let Ok(old) = parse(&old_text)
    {
        let mut masked = p.clone();
        masked.generated = old.generated.clone();
        if render(&masked) == old_text {
            return Ok(false);
        }
    }
    std::fs::write(path, render(p)).map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(true)
}

// ---- client side: load, merge, fetch ----

/// Artifact locations under the data root.
pub const ARTIFACT_FILE: &str = "crowd-priors.json";
pub const ETAG_FILE: &str = "crowd-priors.etag";
/// Fetch preference file (independent of the sharing consent in
/// tune-collect.txt: receiving the crowd's priors must not require
/// contributing). Absent file = default ON.
pub const CONFIG_FILE: &str = "tune-priors.txt";

#[derive(Debug, Clone, PartialEq)]
pub struct FetchConfig {
    pub enabled: bool,
    pub endpoint: String,
}

impl Default for FetchConfig {
    fn default() -> Self {
        FetchConfig {
            enabled: true,
            endpoint: crate::sharing::collect::DEFAULT_ENDPOINT.into(),
        }
    }
}

impl FetchConfig {
    pub fn load(path: &Path) -> FetchConfig {
        let mut cfg = FetchConfig::default();
        let Ok(text) = std::fs::read_to_string(path) else {
            return cfg;
        };
        for line in text.lines() {
            match line.split_once('=').map(|(k, v)| (k.trim(), v.trim())) {
                Some(("enabled", v)) => cfg.enabled = v == "true",
                Some(("endpoint", v)) if !v.is_empty() => cfg.endpoint = v.to_string(),
                _ => {}
            }
        }
        cfg
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        std::fs::write(
            path,
            format!(
                "# crowd-prior fetch preference (independent of telemetry sharing)\n\
                 enabled = {}\nendpoint = {}\n",
                self.enabled, self.endpoint,
            ),
        )
    }
}

/// The stored artifact, if present and readable. Never errors: a missing,
/// stale, or unreadable artifact degrades to local-map-only advice.
pub fn load() -> Option<Priors> {
    let text = std::fs::read_to_string(crate::util::data_path(ARTIFACT_FILE)).ok()?;
    parse(&text).ok()
}

/// True when this install's map already contains other senders' data (an
/// ingested library): it IS the artifact's source, and merging the crowd
/// artifact on top would double-count every sample.
pub fn is_source(map: &EffectMap) -> bool {
    map.samples.iter().any(|s| s.driver != "local")
}

fn static_key(key: &str) -> Option<&'static str> {
    crate::analysis::effects::FIELDS
        .iter()
        .find(|(k, ..)| *k == key)
        .map(|(k, ..)| *k)
}

fn crowd_cell(c: &PriorCell) -> effectmap::Cell {
    effectmap::Cell {
        family: c.family.clone(),
        softer: c.softer,
        drivetrain: c.drivetrain,
        surface_loose: c.surface_loose,
        aero: c.aero,
        n: c.n as usize,
        own_n: 0,
        direct_n: c.direct_n as usize,
        weak_n: c.weak_n as usize,
        delta_mean: c.delta_mean,
        delta_sd: c.delta_sd,
        // Unknown field keys (a newer artifact) are dropped, not errors.
        fields: c
            .fields
            .iter()
            .filter_map(|(k, n, m, sd)| Some((static_key(k)?, *n as usize, *m, *sd)))
            .collect(),
        key_mode: c.key_mode.clone(),
        mag_mean: c.mag_mean,
    }
}

/// Pool two (mean, sd, n) summaries as one mixture distribution.
fn pool(a: (f32, f32, usize), b: (f32, f32, usize)) -> (f32, f32) {
    let (n, na, nb) = ((a.2 + b.2) as f32, a.2 as f32, b.2 as f32);
    let mean = (na * a.0 + nb * b.0) / n;
    let var = (na * (a.1 * a.1 + a.0 * a.0) + nb * (b.1 * b.1 + b.0 * b.0)) / n - mean * mean;
    (mean, var.max(0.0).sqrt())
}

/// Merge the local aggregate with the crowd artifact's cells: n-weighted
/// pooling per bucket, own_n = the local side's own count (so the existing
/// own-driver rank boost falls out naturally), key_mode/mag_mean from the
/// higher-n side (step sizes must not average across pools of different
/// character). Local samples always count; the crowd fills and thickens.
pub fn merge_cells(local: Vec<effectmap::Cell>, crowd: Option<&Priors>) -> Vec<effectmap::Cell> {
    let Some(crowd) = crowd else {
        return local;
    };
    let mut out = local;
    for pc in &crowd.cells {
        let cc = crowd_cell(pc);
        let Some(lc) = out.iter_mut().find(|l| {
            l.family == cc.family
                && l.softer == cc.softer
                && l.drivetrain == cc.drivetrain
                && l.surface_loose == cc.surface_loose
                && l.aero == cc.aero
        }) else {
            out.push(cc);
            continue;
        };
        if lc.n == 0 {
            // Weak-only local bucket: the crowd's stats stand alone.
            let weak_n = lc.weak_n + cc.weak_n;
            *lc = effectmap::Cell { weak_n, ..cc };
            continue;
        }
        let (delta_mean, delta_sd) = pool(
            (lc.delta_mean, lc.delta_sd, lc.n),
            (cc.delta_mean, cc.delta_sd, cc.n),
        );
        let mut fields = Vec::new();
        for (key, ..) in crate::analysis::effects::FIELDS {
            let l = lc.fields.iter().find(|(k, ..)| k == key);
            let c = cc.fields.iter().find(|(k, ..)| k == key);
            match (l, c) {
                (Some(&(k, n, m, sd)), None) | (None, Some(&(k, n, m, sd))) => {
                    fields.push((k, n, m, sd));
                }
                (Some(&(k, ln, lm, lsd)), Some(&(_, cn, cm, csd))) => {
                    let (m, sd) = pool((lm, lsd, ln), (cm, csd, cn));
                    fields.push((k, ln + cn, m, sd));
                }
                (None, None) => {}
            }
        }
        if cc.n > lc.n && cc.key_mode.is_some() {
            lc.key_mode = cc.key_mode;
            lc.mag_mean = cc.mag_mean;
        }
        lc.delta_mean = delta_mean;
        lc.delta_sd = delta_sd;
        lc.fields = fields;
        lc.n += cc.n;
        lc.direct_n += cc.direct_n;
        lc.weak_n += cc.weak_n;
    }
    out
}

/// Merge landscapes for one (surface, drivetrain) context: a local
/// landscape for an axis always wins (ordinary installs rarely build one;
/// when they do, it is their own driving); crowd axes fill the gaps,
/// flagged so advice wording can say where they came from.
pub fn merge_landscapes(
    local: Vec<effectmap::AxisLandscape>,
    crowd: Option<&Priors>,
    surface_loose: bool,
    drivetrain: i32,
) -> Vec<effectmap::AxisLandscape> {
    let mut out = local;
    let Some(ctx) = crowd.and_then(|p| {
        p.landscapes
            .iter()
            .find(|c| c.surface_loose == surface_loose && c.drivetrain == drivetrain)
    }) else {
        return out;
    };
    for a in &ctx.axes {
        let Some(key) = static_key(&a.key) else {
            continue;
        };
        if out.iter().any(|l| l.key == key) {
            continue;
        }
        out.push(effectmap::AxisLandscape {
            key,
            n: a.n as usize,
            mean_gradient: a.mean_gradient,
            sign_share: a.sign_share,
            r: a.r,
            alpha: a.alpha,
            beta: a.beta,
            lo: a.lo,
            hi: a.hi,
            optimum: a.optimum,
            crowd: true,
        });
    }
    out
}

/// The advise-time view of the map for one context: local aggregate and
/// landscapes, merged with the stored crowd artifact unless this install
/// is the artifact's source (an ingested library means every crowd sample
/// is already in the local map — merging would double-count).
pub fn merged_view(
    emap: &EffectMap,
    surface_loose: bool,
    drivetrain: i32,
) -> (Vec<effectmap::Cell>, Vec<effectmap::AxisLandscape>) {
    let crowd = (!is_source(emap)).then(load).flatten();
    let cells = merge_cells(effectmap::aggregate(emap), crowd.as_ref());
    let landscapes = merge_landscapes(
        effectmap::axis_landscapes(emap, surface_loose, Some(drivetrain)),
        crowd.as_ref(),
        surface_loose,
        drivetrain,
    );
    (cells, landscapes)
}

#[derive(Debug)]
pub enum FetchOutcome {
    Updated,
    Unchanged,
    /// The endpoint has no artifact published; any local copy is kept.
    Missing,
}

/// Fetch the artifact into the data root, verifying the served signature
/// against the pinned key BEFORE anything is written. Conditional via the
/// ETag sidecar; any failure leaves the stored artifact untouched.
pub fn fetch(endpoint: &str) -> Result<FetchOutcome, String> {
    fetch_to(
        endpoint,
        PUBKEY_HEX,
        &crate::util::data_path(ARTIFACT_FILE),
        &crate::util::data_path(ETAG_FILE),
    )
}

/// 8 MB cap: the artifact is ~100 KB; a response far past that is wrong
/// regardless of what it claims to be.
const FETCH_CAP_BYTES: u64 = 8 * 1024 * 1024;

pub fn fetch_to(
    endpoint: &str,
    pubkey_hex: &str,
    artifact_path: &Path,
    etag_path: &Path,
) -> Result<FetchOutcome, String> {
    let url = format!("{}/v1/priors", endpoint.trim_end_matches('/'));
    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(60))
        .build();
    let mut req = agent.get(&url);
    // Only send the cached etag while the artifact it belongs to exists.
    if artifact_path.exists()
        && let Ok(etag) = std::fs::read_to_string(etag_path)
        && !etag.trim().is_empty()
    {
        req = req.set("If-None-Match", etag.trim());
    }
    let resp = match req.call() {
        Ok(resp) => resp,
        Err(ureq::Error::Status(404, _)) => return Ok(FetchOutcome::Missing),
        Err(ureq::Error::Status(code, _)) => return Err(format!("endpoint says {code}")),
        Err(ureq::Error::Transport(t)) => return Err(format!("no response ({t})")),
    };
    // ureq surfaces only 4xx/5xx as errors; the conditional-GET miss
    // arrives here as a plain 304 response.
    if resp.status() == 304 {
        return Ok(FetchOutcome::Unchanged);
    }
    let etag = resp.header("etag").unwrap_or_default().to_string();
    let sig = resp
        .header("x-priors-signature")
        .ok_or("response carries no signature")?
        .to_string();
    let mut body = String::new();
    resp.into_reader()
        .take(FETCH_CAP_BYTES)
        .read_to_string(&mut body)
        .map_err(|e| format!("read: {e}"))?;
    if !verify(pubkey_hex, body.as_bytes(), &sig) {
        return Err("signature verification failed; artifact discarded".into());
    }
    // Parse before storing so a signed-but-unreadable artifact (schema from
    // the future) never replaces a working one.
    parse(&body)?;
    std::fs::write(artifact_path, &body).map_err(|e| e.to_string())?;
    let _ = std::fs::write(etag_path, etag);
    Ok(FetchOutcome::Updated)
}

// ---- signing ----
//
// Ed25519 via ring (already in the tree under ureq's rustls). The key file
// is the PKCS#8 document generate_pkcs8 emits; the signature ships as a
// detached hex file next to the artifact.

pub fn keygen(key_path: &Path) -> Result<String, String> {
    use ring::signature::KeyPair;
    if key_path.exists() {
        return Err(format!(
            "{} exists; refusing to overwrite a signing key",
            key_path.display()
        ));
    }
    let rng = ring::rand::SystemRandom::new();
    let doc = ring::signature::Ed25519KeyPair::generate_pkcs8(&rng)
        .map_err(|_| "keygen failed".to_string())?;
    if let Some(dir) = key_path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    }
    std::fs::write(key_path, doc.as_ref())
        .map_err(|e| format!("write {}: {e}", key_path.display()))?;
    let pair = ring::signature::Ed25519KeyPair::from_pkcs8(doc.as_ref())
        .map_err(|_| "generated key unreadable".to_string())?;
    Ok(hex(pair.public_key().as_ref()))
}

pub fn sign(key_path: &Path, msg: &[u8]) -> Result<String, String> {
    let doc = std::fs::read(key_path).map_err(|e| format!("read {}: {e}", key_path.display()))?;
    let pair = ring::signature::Ed25519KeyPair::from_pkcs8(&doc)
        .map_err(|_| format!("{} is not an ed25519 PKCS#8 key", key_path.display()))?;
    Ok(hex(pair.sign(msg).as_ref()))
}

pub fn verify(pubkey_hex: &str, msg: &[u8], sig_hex: &str) -> bool {
    let (Some(pk), Some(sig)) = (unhex(pubkey_hex), unhex(sig_hex)) else {
        return false;
    };
    ring::signature::UnparsedPublicKey::new(&ring::signature::ED25519, pk)
        .verify(msg, &sig)
        .is_ok()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::advice::effectmap::{CampaignFloor, Sample};

    fn sample(family: &str, softer: bool, delta_s: f32, direct: bool, weak: bool) -> Sample {
        Sample {
            driver: "local".into(),
            campaign: "tune-journal-1.txt".into(),
            car: 1,
            drivetrain: 1,
            surface_loose: false,
            aero: Some(false),
            family: family.into(),
            key: Some("arb_front".into()),
            softer,
            magnitude: Some(2.0),
            delta_s,
            split: None,
            weak,
            direct,
            clean: true,
            attributed: false,
            lap_s: Some(90.0),
            from: format!("a-{delta_s}"),
            to: format!("b-{delta_s}"),
            effects: vec![("front_slip", 0.1)],
            position: vec![("front_slip", 0.5)],
        }
    }

    fn map() -> EffectMap {
        EffectMap {
            samples: vec![
                sample("front roll", true, -0.3, true, false),
                sample("front roll", true, -0.1, false, false),
                sample("front roll", true, 0.2, false, true),
                sample("rear roll", false, 0.4, true, false),
            ],
            floors: vec![CampaignFloor {
                driver: "local".into(),
                campaign: "tune-journal-1.txt".into(),
                drift_s: Some(0.1),
                effects: vec![("front_slip", 0.02)],
            }],
        }
    }

    #[test]
    fn derive_round_trips() {
        let p = derive(&map(), "20260808-000000".into());
        assert_eq!(p.samples, 4);
        assert_eq!(p.campaigns, 1);
        assert_eq!(p.senders, 1);
        // Weak sample counted, not pooled; own_n never serialized.
        let cell = p
            .cells
            .iter()
            .find(|c| c.family == "front roll")
            .expect("front roll cell");
        assert_eq!((cell.n, cell.direct_n, cell.weak_n), (2, 1, 1));
        let text = render(&p);
        assert!(!text.contains("own"), "own_n must not ship: {text}");
        assert_eq!(parse(&text).unwrap(), p);
    }

    #[test]
    fn newer_schema_refused() {
        let mut p = derive(&map(), "s".into());
        p.min_app_schema = SCHEMA + 1;
        let err = parse(&render(&p)).unwrap_err();
        assert!(err.contains("schema"), "{err}");
    }

    #[test]
    fn write_is_content_gated() {
        let dir = std::env::temp_dir().join(format!("tuners-priors-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("crowd-priors.json");
        let p1 = derive(&map(), "20260808-000000".into());
        assert!(write(&path, &p1).unwrap());
        let bytes1 = std::fs::read(&path).unwrap();
        // Same content, later stamp: untouched.
        let p2 = derive(&map(), "20260809-000000".into());
        assert!(!write(&path, &p2).unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), bytes1);
        // New data: rewritten with the new stamp.
        let mut m = map();
        m.samples.push(sample("brakes", true, -0.5, true, false));
        let p3 = derive(&m, "20260810-000000".into());
        assert!(write(&path, &p3).unwrap());
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("20260810-000000"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn merge_cells_pools_and_fills() {
        let p = derive(&map(), "s".into());
        // Disjoint local bucket: crowd cells append, local survives whole.
        let local_only = vec![effectmap::Cell {
            family: "brakes".into(),
            softer: true,
            drivetrain: 1,
            surface_loose: false,
            aero: Some(false),
            n: 2,
            own_n: 2,
            direct_n: 1,
            weak_n: 0,
            delta_mean: -0.4,
            delta_sd: 0.1,
            fields: vec![("front_slip", 2, 0.2, 0.05)],
            key_mode: Some("brake_balance".into()),
            mag_mean: Some(5.0),
        }];
        let merged = merge_cells(local_only.clone(), Some(&p));
        assert_eq!(merged.len(), 1 + p.cells.len());
        let brakes = merged.iter().find(|c| c.family == "brakes").unwrap();
        assert_eq!((brakes.n, brakes.own_n), (2, 2));

        // Overlapping bucket: n-weighted pooling, own_n stays local, the
        // higher-n side's step sizes win.
        let local = vec![effectmap::Cell {
            family: "front roll".into(),
            softer: true,
            drivetrain: 1,
            surface_loose: false,
            aero: Some(false),
            n: 1,
            own_n: 1,
            direct_n: 1,
            weak_n: 0,
            delta_mean: -0.5,
            delta_sd: 0.0,
            fields: vec![("front_slip", 1, 0.3, 0.0)],
            key_mode: Some("springs_front".into()),
            mag_mean: Some(9.9),
        }];
        let merged = merge_cells(local, Some(&p));
        let cell = merged
            .iter()
            .find(|c| c.family == "front roll" && c.softer)
            .unwrap();
        let crowd = p
            .cells
            .iter()
            .find(|c| c.family == "front roll" && c.softer)
            .unwrap();
        assert_eq!(cell.n, 1 + crowd.n as usize);
        assert_eq!(cell.own_n, 1);
        // Pooled mean sits between the sides, weighted by n.
        let expect = (-0.5 + crowd.delta_mean * crowd.n as f32) / (1.0 + crowd.n as f32);
        assert!(
            (cell.delta_mean - expect).abs() < 1e-5,
            "{}",
            cell.delta_mean
        );
        // Crowd n=2 > local 1: its key_mode/mag_mean win.
        assert_eq!(cell.key_mode.as_deref(), crowd.key_mode.as_deref());

        // No crowd: identity.
        let merged = merge_cells(local_only.clone(), None);
        assert_eq!(merged.len(), 1);
    }

    #[test]
    fn merge_cells_drops_unknown_fields() {
        let mut p = derive(&map(), "s".into());
        p.cells[0]
            .fields
            .push(("field_from_the_future".into(), 3, 1.0, 0.1));
        let merged = merge_cells(Vec::new(), Some(&p));
        assert!(
            merged
                .iter()
                .all(|c| c.fields.iter().all(|(k, ..)| *k != "field_from_the_future"))
        );
    }

    #[test]
    fn merge_landscapes_local_wins() {
        let mk = |key: &'static str, crowd: bool| effectmap::AxisLandscape {
            key,
            n: 5,
            mean_gradient: if crowd { 1.0 } else { -1.0 },
            sign_share: 0.8,
            r: 0.6,
            alpha: 0.0,
            beta: 0.1,
            lo: 0.0,
            hi: 1.0,
            optimum: None,
            crowd,
        };
        let p = Priors {
            schema: SCHEMA,
            min_app_schema: 1,
            generated: "s".into(),
            samples: 0,
            campaigns: 0,
            senders: 0,
            cells: Vec::new(),
            landscapes: vec![PriorContext {
                surface_loose: false,
                drivetrain: 1,
                axes: vec![
                    PriorAxis {
                        key: "front_slip".into(),
                        n: 40,
                        mean_gradient: 1.0,
                        sign_share: 0.9,
                        r: 0.7,
                        alpha: 0.0,
                        beta: 0.2,
                        lo: 0.0,
                        hi: 1.0,
                        optimum: Some(0.5),
                    },
                    PriorAxis {
                        key: "wheelspin".into(),
                        n: 12,
                        mean_gradient: 0.4,
                        sign_share: 0.8,
                        r: 0.6,
                        alpha: 0.0,
                        beta: 0.1,
                        lo: 0.0,
                        hi: 1.0,
                        optimum: None,
                    },
                    PriorAxis {
                        key: "axis_from_the_future".into(),
                        n: 9,
                        mean_gradient: 0.1,
                        sign_share: 0.5,
                        r: 0.1,
                        alpha: 0.0,
                        beta: 0.0,
                        lo: 0.0,
                        hi: 1.0,
                        optimum: None,
                    },
                ],
            }],
        };
        let merged = merge_landscapes(vec![mk("front_slip", false)], Some(&p), false, 1);
        // Local front_slip wins; crowd wheelspin fills, flagged; unknown
        // axis and wrong-context lookups drop.
        assert_eq!(merged.len(), 2);
        let fs = merged.iter().find(|l| l.key == "front_slip").unwrap();
        assert!(!fs.crowd);
        assert_eq!(fs.mean_gradient, -1.0);
        let ws = merged.iter().find(|l| l.key == "wheelspin").unwrap();
        assert!(ws.crowd);
        assert_eq!(ws.n, 12);
        assert!(merge_landscapes(Vec::new(), Some(&p), true, 1).is_empty());
    }

    #[test]
    fn source_install_detected() {
        let mut m = map();
        assert!(!is_source(&m));
        let mut s = sample("aero", true, -0.1, true, false);
        s.driver = "b1b71d17aaaaaaaa".into();
        m.samples.push(s);
        assert!(is_source(&m));
    }

    #[test]
    fn sign_verify_and_tamper() {
        let dir = std::env::temp_dir().join(format!("tuners-priors-key-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let key = dir.join("test.key");
        let pubkey = keygen(&key).unwrap();
        assert_eq!(pubkey.len(), 64);
        assert!(keygen(&key).is_err(), "must refuse to overwrite");
        let msg = render(&derive(&map(), "s".into()));
        let sig = sign(&key, msg.as_bytes()).unwrap();
        assert!(verify(&pubkey, msg.as_bytes(), &sig));
        assert!(!verify(&pubkey, &msg.as_bytes()[1..], &sig));
        assert!(!verify(&pubkey, msg.as_bytes(), &sig.replace('a', "b")));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
