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
