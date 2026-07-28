//! Cross-campaign effect map: pool every campaign's harvested
//! measurements — local journals plus ingested tester bundles — into
//! per family × direction distributions of behavioural movement, keyed by
//! build context (drivetrain, surface, aero). Two tiers by construction:
//! the pooled file is the global prior, and the per-campaign measurements
//! advise already uses are the local reinforcement that overrides it.
//!
//! The file stores RAW SAMPLES, not aggregates, so re-keying, per-driver
//! pooling, and floor policy stay decidable at read time. Per-campaign
//! noise floors ride along as their own rows: a sender's floors come from
//! their own same-setup pairs, never inherited from ours.

use crate::analysis::{effects, journal};
use crate::tuning::TuningSession;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// One measurement contributed to the map: a campaign measurement plus the
/// build context it was taken in.
#[derive(Debug, Clone)]
pub struct Sample {
    /// "local", or the sender id for ingested tester data.
    pub driver: String,
    /// Campaign label (journal file name / bundle group).
    pub campaign: String,
    pub car: i32,
    /// Packet drivetrain code: 0 FWD, 1 RWD, 2 AWD.
    pub drivetrain: i32,
    pub surface_loose: bool,
    /// Whether the build has tunable aero (None = unknown, e.g. blind).
    pub aero: Option<bool>,
    /// Fine-grained family name (journal::family_key): front/rear and
    /// accel/decel kept apart — opposite-end interventions must not pool.
    pub family: String,
    /// The single slider moved, when identifiable.
    pub key: Option<String>,
    pub softer: bool,
    pub magnitude: Option<f32>,
    /// The measurement's judged time delta (channel delta for attributed
    /// compound clauses, ideal delta otherwise; negative = faster).
    pub delta_s: f32,
    /// (entry, exit, straights) split of the pair's delta.
    pub split: Option<(f32, f32, f32)>,
    pub weak: bool,
    pub direct: bool,
    pub clean: bool,
    /// Behavioural movement of the underlying stint pair.
    pub effects: effects::Effects,
}

/// A campaign's own measured noise floors (same-setup pairs).
#[derive(Debug, Clone)]
pub struct CampaignFloor {
    pub driver: String,
    pub campaign: String,
    /// Largest same-setup |ideal delta| (drift).
    pub drift_s: Option<f32>,
    pub effects: effects::Effects,
}

#[derive(Debug, Clone, Default)]
pub struct EffectMap {
    pub samples: Vec<Sample>,
    pub floors: Vec<CampaignFloor>,
}

/// Extract map samples from a loaded campaign. Cross-surface stint pairs are
/// dropped (their behavioural delta measures the surface, not the setup), as
/// are pairs without overall metrics on both sides (no context, no effects).
pub(crate) fn harvest_campaign(
    c: &crate::advise::Campaign,
    driver: &str,
    campaign: &str,
) -> (Vec<Sample>, CampaignFloor) {
    let mut samples = Vec::new();
    for m in &c.measurements {
        let (si, sj) = (&c.stints[m.i], &c.stints[m.j]);
        let (Some(mi), Some(mj)) = (&si.met, &sj.met) else {
            continue;
        };
        if mi.surface_loose != mj.surface_loose {
            continue;
        }
        let Some(delta_s) = m.outcome.delta_s() else {
            continue;
        };
        let Some(car) = crate::advise::car_of(&sj.stint) else {
            continue;
        };
        samples.push(Sample {
            driver: driver.to_string(),
            campaign: campaign.to_string(),
            car,
            drivetrain: mj.drivetrain_type,
            surface_loose: mj.surface_loose,
            aero: c.setups[m.j].map(|rev| rev.values.keys().any(|k| k.starts_with("aero_"))),
            family: journal::family_key(m.change.family).to_string(),
            key: m.key.clone(),
            softer: m.change.softer,
            magnitude: m.change.magnitude,
            delta_s,
            split: m.split,
            weak: m.weak,
            direct: m.direct,
            clean: m.clean,
            effects: m.effects.clone(),
        });
    }
    let floor = CampaignFloor {
        driver: driver.to_string(),
        campaign: campaign.to_string(),
        drift_s: c.drift_floor.map(|(_, f)| f),
        effects: c.effect_floor.clone(),
    };
    (samples, floor)
}

/// A campaign pair discovered on disk.
pub struct CampaignSource {
    pub journal: PathBuf,
    pub session: Option<PathBuf>,
    /// Display/serialization label (the journal file name).
    pub label: String,
}

/// Every campaign pair under the data root: the active session's journal,
/// named-session archives (tune-journal-<car>-<stamp>.txt), legacy per-car
/// files (tune-journal-<car>.txt), and the blind base journal.
pub fn local_campaigns(root: &Path) -> Vec<CampaignSource> {
    let mut out = Vec::new();
    let active = TuningSession::load(&root.join("tune-session.txt"));
    let Ok(rd) = std::fs::read_dir(root) else {
        return out;
    };
    let mut names: Vec<String> = rd
        .flatten()
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            (n.starts_with("tune-journal") && n.ends_with(".txt")).then_some(n)
        })
        .collect();
    names.sort();
    for name in names {
        let stem = name.trim_end_matches(".txt");
        let suffix = stem.strip_prefix("tune-journal").unwrap_or_default();
        let session = if suffix.is_empty() {
            // The blind base journal has no setups on file.
            None
        } else {
            let parts: Vec<&str> = suffix
                .strip_prefix('-')
                .unwrap_or_default()
                .split('-')
                .collect();
            match parts[..] {
                // tune-journal-<car>-<YYYYMMDD>-<HHMMSS>.txt: archived pair.
                [car, d8, t6] if car.parse::<i32>().is_ok() && d8.len() == 8 && t6.len() == 6 => {
                    let s = root.join(format!("tune-session-{car}-{d8}-{t6}.txt"));
                    s.exists().then_some(s)
                }
                // tune-journal-<car>.txt: the live journal (active session's
                // car) or a legacy car-switch pair.
                [car] if car.parse::<i32>().is_ok() => {
                    if active.car == car.parse::<i32>().ok() {
                        Some(root.join("tune-session.txt"))
                    } else {
                        let s = root.join(format!("tune-session-{car}.txt"));
                        s.exists().then_some(s)
                    }
                }
                _ => continue,
            }
        };
        out.push(CampaignSource {
            journal: root.join(&name),
            session,
            label: name,
        });
    }
    out
}

/// A campaign's identity across copies: the car plus the first journaled
/// stint's stamp. A sender's ingested echo of a campaign we harvested
/// locally (sharing enabled on this machine) carries the same key.
type CampaignKey = (i32, String);

fn campaign_key(car: Option<i32>, entries: &[journal::Entry]) -> Option<CampaignKey> {
    let stamp = entries
        .first()
        .and_then(|e| crate::advise::stint_stamp(&e.path))?;
    Some((car?, stamp.to_string()))
}

/// Harvest one local campaign pair into map rows. Errors are reported as
/// strings so a broken campaign skips instead of killing the whole build.
pub fn harvest_local(
    source: &CampaignSource,
    stints_dir: &str,
) -> Result<(Vec<Sample>, CampaignFloor, Option<CampaignKey>), String> {
    let text =
        std::fs::read_to_string(&source.journal).map_err(|e| format!("{}: {e}", source.label))?;
    let mut entries = journal::parse_journal(&text);
    let session = source
        .session
        .as_deref()
        .map(TuningSession::load)
        .unwrap_or_default();
    crate::advise::implicit_steps(&text, &mut entries, session.car, stints_dir);
    if entries.is_empty() {
        return Err(format!("{}: empty journal", source.label));
    }
    let key = campaign_key(session.car, &entries);
    let c = crate::advise::load_campaign(entries, &session, &source.label)?;
    let (samples, floor) = harvest_campaign(&c, "local", &source.label);
    Ok((samples, floor, key))
}

/// A sender campaign reconstructed from ingested bundles: entries point at
/// stint files extracted under the scratch dir.
pub struct SenderCampaign {
    pub driver: String,
    pub label: String,
    pub car: Option<i32>,
    pub session: TuningSession,
    pub entries: Vec<journal::Entry>,
}

/// Sender directories under the library, sorted.
fn sender_dirs(library: &Path) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(library) else {
        return Vec::new();
    };
    let mut senders: Vec<_> = rd
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    senders.sort();
    senders
}

/// Rebuild one sender's campaigns from `library/<sender>/bundle-*.tar.zst`.
/// Bundles of one campaign share a growing journal; they are grouped by
/// (car, first journal entry) and the longest journal in a group defines the
/// campaign. Each bundle's stint is written under `scratch` and its journal
/// entry is repointed there; journal entries no bundle covers become missing
/// paths, which the campaign loader already skips honestly. Unreadable
/// bundles are reported and skipped.
fn harvest_sender(
    library: &Path,
    sender: &str,
    scratch: &Path,
    report: &mut Vec<String>,
) -> Vec<SenderCampaign> {
    let mut out = Vec::new();
    let dir = library.join(sender);
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return out;
    };
    let mut names: Vec<String> = rd
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".tar.zst"))
        .collect();
    names.sort();
    // (car, first journal entry path) -> bundles of that campaign.
    let mut groups: BTreeMap<(String, String), Vec<(String, crate::bundle::Bundle)>> =
        BTreeMap::new();
    for name in names {
        let path = dir.join(&name);
        let bundle = std::fs::read(&path)
            .map_err(|e| e.to_string())
            .and_then(|bytes| crate::bundle::open(&bytes));
        match bundle {
            Ok(b) => {
                let car = b.manifest.get("car").cloned().unwrap_or_default();
                let first = journal::parse_journal(&b.journal_txt)
                    .first()
                    .map(|e| e.path.clone())
                    .unwrap_or_default();
                groups.entry((car, first)).or_default().push((name, b));
            }
            Err(e) => report.push(format!("{sender}/{name}: {e}")),
        }
    }
    for ((car, first), bundles) in groups {
        // The longest journal sees the whole campaign.
        let Some((_, longest)) = bundles
            .iter()
            .max_by_key(|(_, b)| journal::parse_journal(&b.journal_txt).len())
        else {
            continue;
        };
        let mut entries = journal::parse_journal(&longest.journal_txt);
        let session = TuningSession::parse(&longest.session_txt);
        let group_dir = scratch.join(sender).join(&car);
        let mut covered = 0usize;
        for entry in entries.iter_mut() {
            let file = Path::new(&entry.path)
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_else(|| entry.path.clone());
            let dest = group_dir.join(&file);
            // The bundle whose stint stamp names this entry, if delivered.
            let stamp = crate::advise::stint_stamp(&entry.path);
            let backing = bundles
                .iter()
                .find(|(_, b)| b.manifest.get("stint_stamp").map(String::as_str) == stamp);
            if let Some((name, b)) = backing {
                if std::fs::create_dir_all(&group_dir)
                    .and_then(|_| std::fs::write(&dest, &b.stint))
                    .is_err()
                {
                    report.push(format!("{sender}/{name}: could not extract stint"));
                } else {
                    covered += 1;
                }
            }
            // Repoint unconditionally: uncovered entries must resolve
            // (and fail) under scratch, never against local files.
            entry.path = dest.to_string_lossy().into_owned();
        }
        if covered == 0 {
            continue;
        }
        // The first entry's stamp keeps two campaigns of the same car
        // under one sender apart.
        let first_stamp = crate::advise::stint_stamp(&first).unwrap_or("nostamp");
        out.push(SenderCampaign {
            driver: sender.to_string(),
            label: format!("{sender}:{car}:{first_stamp}"),
            car: car.parse().ok(),
            session,
            entries,
        });
    }
    out
}

// ---- serialization ----

const PREFIX_COLS: &[&str] = &[
    "kind",
    "driver",
    "campaign",
    "car",
    "drivetrain",
    "surface",
    "aero",
    "family",
    "key",
    "softer",
    "magnitude",
    "delta_s",
    "entry",
    "exit",
    "straights",
    "weak",
    "direct",
    "clean",
];

fn fx_cell(fx: &effects::Effects, key: &str) -> String {
    fx.iter()
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v.to_string())
        .unwrap_or_default()
}

/// Render the map as TSV: a header, one `s` row per sample, one `f` row per
/// campaign's floors (floor values in the effect columns, drift in delta_s).
pub fn render(map: &EffectMap) -> String {
    let mut out = String::new();
    let field_keys: Vec<&str> = effects::FIELDS.iter().map(|(k, ..)| *k).collect();
    out.push_str(&PREFIX_COLS.join("\t"));
    out.push('\t');
    out.push_str(&field_keys.join("\t"));
    out.push('\n');
    let opt = |v: Option<f32>| v.map(|v| v.to_string()).unwrap_or_default();
    let flag = |b: bool| if b { "1" } else { "0" };
    for s in &map.samples {
        let (e, x, st) = match s.split {
            Some((e, x, st)) => (Some(e), Some(x), Some(st)),
            None => (None, None, None),
        };
        let mut cols = vec![
            "s".to_string(),
            s.driver.clone(),
            s.campaign.clone(),
            s.car.to_string(),
            s.drivetrain.to_string(),
            if s.surface_loose { "dirt" } else { "tarmac" }.into(),
            match s.aero {
                None => String::new(),
                Some(true) => "yes".into(),
                Some(false) => "no".into(),
            },
            s.family.clone(),
            s.key.clone().unwrap_or_default(),
            flag(s.softer).into(),
            opt(s.magnitude),
            s.delta_s.to_string(),
            opt(e),
            opt(x),
            opt(st),
            flag(s.weak).into(),
            flag(s.direct).into(),
            flag(s.clean).into(),
        ];
        cols.extend(field_keys.iter().map(|k| fx_cell(&s.effects, k)));
        out.push_str(&cols.join("\t"));
        out.push('\n');
    }
    for f in &map.floors {
        let mut cols = vec!["f".to_string(), f.driver.clone(), f.campaign.clone()];
        cols.resize(11, String::new());
        cols.push(opt(f.drift_s));
        cols.resize(PREFIX_COLS.len(), String::new());
        cols.extend(field_keys.iter().map(|k| fx_cell(&f.effects, k)));
        out.push_str(&cols.join("\t"));
        out.push('\n');
    }
    out
}

/// Parse a rendered map. Effect columns are matched by header name, so field
/// additions stay compatible; unknown columns are ignored.
pub fn parse(text: &str) -> Result<EffectMap, String> {
    let mut lines = text.lines();
    let header = lines.next().ok_or("empty map file")?;
    let cols: Vec<&str> = header.split('\t').collect();
    if cols.first() != Some(&"kind") {
        return Err("not an effect map (missing header)".into());
    }
    let fx_cols: Vec<(usize, &'static str)> = cols
        .iter()
        .enumerate()
        .skip(PREFIX_COLS.len())
        .filter_map(|(i, name)| effects::key_of(name).map(|k| (i, k)))
        .collect();
    let mut map = EffectMap::default();
    for (ln, line) in lines.enumerate() {
        if line.is_empty() {
            continue;
        }
        let cells: Vec<&str> = line.split('\t').collect();
        let cell = |i: usize| cells.get(i).copied().unwrap_or_default();
        let num = |i: usize| cell(i).parse::<f32>().ok();
        let bad = |what: &str| format!("line {}: bad {what}", ln + 2);
        let fx: effects::Effects = fx_cols
            .iter()
            .filter_map(|(i, k)| Some((*k, num(*i)?)))
            .collect();
        match cell(0) {
            "s" => map.samples.push(Sample {
                driver: cell(1).to_string(),
                campaign: cell(2).to_string(),
                car: cell(3).parse().map_err(|_| bad("car"))?,
                drivetrain: cell(4).parse().map_err(|_| bad("drivetrain"))?,
                surface_loose: cell(5) == "dirt",
                aero: match cell(6) {
                    "yes" => Some(true),
                    "no" => Some(false),
                    _ => None,
                },
                family: cell(7).to_string(),
                key: (!cell(8).is_empty()).then(|| cell(8).to_string()),
                softer: cell(9) == "1",
                magnitude: num(10),
                delta_s: num(11).ok_or_else(|| bad("delta_s"))?,
                split: match (num(12), num(13), num(14)) {
                    (Some(e), Some(x), Some(st)) => Some((e, x, st)),
                    _ => None,
                },
                weak: cell(15) == "1",
                direct: cell(16) == "1",
                clean: cell(17) == "1",
                effects: fx,
            }),
            "f" => map.floors.push(CampaignFloor {
                driver: cell(1).to_string(),
                campaign: cell(2).to_string(),
                drift_s: num(11),
                effects: fx,
            }),
            other => return Err(format!("line {}: unknown row kind '{other}'", ln + 2)),
        }
    }
    Ok(map)
}

// ---- aggregation ----

/// One family × direction × context bucket of the map.
#[derive(Debug, Clone)]
pub struct Cell {
    pub family: String,
    pub softer: bool,
    pub drivetrain: i32,
    pub surface_loose: bool,
    pub aero: Option<bool>,
    /// Non-weak samples aggregated below.
    pub n: usize,
    /// How many of `n` are this machine's own driving (driver "local").
    /// Own data outranks pooled tester data when ranking levers: the
    /// controller effect makes weights driver-dependent (plan 011 phase B).
    pub own_n: usize,
    /// How many of `n` are direct setup A/Bs (the rest are note-based or
    /// channel-attributed compound clauses).
    pub direct_n: usize,
    /// Weak samples matching this bucket (excluded from the stats).
    pub weak_n: usize,
    pub delta_mean: f32,
    pub delta_sd: f32,
    /// Per effect field: (key, samples carrying it, mean, sd).
    pub fields: Vec<(&'static str, usize, f32, f32)>,
}

fn mean_sd(vals: &[f32]) -> (f32, f32) {
    let n = vals.len() as f32;
    let mean = vals.iter().sum::<f32>() / n;
    let var = vals.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / n;
    (mean, var.sqrt())
}

/// Aggregate samples into distribution cells. Weak samples are counted but
/// never pooled: a suspect stint must not shape the prior.
pub fn aggregate(map: &EffectMap) -> Vec<Cell> {
    type Key = (String, bool, i32, bool, Option<bool>);
    let key = |s: &Sample| -> Key {
        (
            s.family.clone(),
            s.softer,
            s.drivetrain,
            s.surface_loose,
            s.aero,
        )
    };
    let mut buckets: BTreeMap<Key, (Vec<&Sample>, usize)> = BTreeMap::new();
    for s in &map.samples {
        let b = buckets.entry(key(s)).or_default();
        if s.weak {
            b.1 += 1;
        } else {
            b.0.push(s);
        }
    }
    let mut out = Vec::new();
    for ((family, softer, drivetrain, surface_loose, aero), (samples, weak_n)) in buckets {
        if samples.is_empty() {
            if weak_n == 0 {
                continue;
            }
            out.push(Cell {
                family,
                softer,
                drivetrain,
                surface_loose,
                aero,
                n: 0,
                own_n: 0,
                direct_n: 0,
                weak_n,
                delta_mean: 0.0,
                delta_sd: 0.0,
                fields: Vec::new(),
            });
            continue;
        }
        let (delta_mean, delta_sd) =
            mean_sd(&samples.iter().map(|s| s.delta_s).collect::<Vec<_>>());
        let mut fields = Vec::new();
        for (fkey, ..) in effects::FIELDS {
            let vals: Vec<f32> = samples
                .iter()
                .filter_map(|s| s.effects.iter().find(|(k, _)| k == fkey).map(|(_, v)| *v))
                .collect();
            if vals.is_empty() {
                continue;
            }
            let (m, sd) = mean_sd(&vals);
            fields.push((*fkey, vals.len(), m, sd));
        }
        out.push(Cell {
            family,
            softer,
            drivetrain,
            surface_loose,
            aero,
            n: samples.len(),
            own_n: samples.iter().filter(|s| s.driver == "local").count(),
            direct_n: samples.iter().filter(|s| s.direct).count(),
            weak_n,
            delta_mean,
            delta_sd,
            fields,
        });
    }
    out
}

// ---- phase D: the map as a prior for untried levers ----

/// A field's measured correlation with pace: within one campaign, or pooled
/// from the driver's history across other cars.
#[derive(Debug, Clone)]
pub struct PaceTrend {
    pub key: &'static str,
    /// Pearson r between the field's movement and the time delta (positive =
    /// the field rising cost time).
    pub r: f32,
    /// Measurement pairs behind it (above-floor movements only).
    pub n: usize,
    /// True when pooled from the driver's past campaigns rather than
    /// measured in the current one.
    pub history: bool,
}

/// Trend gates: a campaign yields ~5-15 measurements against 26 coordinates,
/// so an unconstrained fit is underdetermined; per-field correlation over
/// above-floor movements, with a minimum sample count and a strong-|r| cut,
/// is the honest version.
const TREND_MIN_PAIRS: usize = 4;
const TREND_MIN_R: f32 = 0.7;

fn pearson(pts: &[(f32, f32)]) -> f32 {
    let n = pts.len() as f32;
    let (mx, my) = (
        pts.iter().map(|p| p.0).sum::<f32>() / n,
        pts.iter().map(|p| p.1).sum::<f32>() / n,
    );
    let (mut sxy, mut sxx, mut syy) = (0.0f32, 0.0f32, 0.0f32);
    for (x, y) in pts {
        sxy += (x - mx) * (y - my);
        sxx += (x - mx) * (x - mx);
        syy += (y - my) * (y - my);
    }
    if sxx <= 0.0 || syy <= 0.0 {
        return 0.0;
    }
    sxy / (sxx * syy).sqrt()
}

/// Which effect fields tracked pace across a campaign's measurements:
/// (effect delta, time delta) pairs in, per-field correlations out. Only
/// movements at or above max(library, campaign) floor count, so the trend
/// can't be built out of noise.
pub fn pace_trends(
    pairs: &[(effects::Effects, f32)],
    campaign_floor: Option<&effects::Effects>,
) -> Vec<PaceTrend> {
    let mut out = Vec::new();
    for (key, ..) in effects::FIELDS {
        let base = effects::noise_floor(key);
        let floor = campaign_floor
            .and_then(|f| f.iter().find(|(k, _)| k == key).map(|(_, v)| v.abs()))
            .map_or(base, |c| c.max(base));
        let pts: Vec<(f32, f32)> = pairs
            .iter()
            .filter_map(|(fx, dt)| {
                let v = fx.iter().find(|(k, _)| k == key).map(|(_, v)| *v)?;
                (v.abs() >= floor).then_some((v, *dt))
            })
            .collect();
        if pts.len() < TREND_MIN_PAIRS {
            continue;
        }
        let r = pearson(&pts);
        if r.abs() >= TREND_MIN_R {
            out.push(PaceTrend {
                key,
                r,
                n: pts.len(),
                history: false,
            });
        }
    }
    out
}

/// The driver's pooled pace trends across their OTHER cars' campaigns: the
/// per-driver profitable-direction prior (plan 007), available before the
/// current campaign has measured anything. The current car is excluded
/// wholesale — its campaigns' samples would double-count the local evidence,
/// and same-car other-build campaigns are invalidated by upgrades anyway
/// (named-sessions doctrine). Each sample gates on max(library, its own
/// campaign's) floor.
pub fn driver_trends(map: &EffectMap, driver: &str, exclude_car: Option<i32>) -> Vec<PaceTrend> {
    let floor_of = |campaign: &str, key: &str| -> f32 {
        let base = effects::noise_floor(key);
        map.floors
            .iter()
            .find(|f| f.driver == driver && f.campaign == campaign)
            .and_then(|f| {
                f.effects
                    .iter()
                    .find(|(k, _)| *k == key)
                    .map(|(_, v)| v.abs())
            })
            .map_or(base, |c| c.max(base))
    };
    let mut out = Vec::new();
    for (key, ..) in effects::FIELDS {
        let pts: Vec<(f32, f32)> = map
            .samples
            .iter()
            .filter(|s| s.driver == driver && !s.weak && Some(s.car) != exclude_car)
            .filter_map(|s| {
                let v = s.effects.iter().find(|(k, _)| k == key).map(|(_, v)| *v)?;
                (v.abs() >= floor_of(&s.campaign, key)).then_some((v, s.delta_s))
            })
            .collect();
        if pts.len() < TREND_MIN_PAIRS {
            continue;
        }
        let r = pearson(&pts);
        if r.abs() >= TREND_MIN_R {
            out.push(PaceTrend {
                key,
                r,
                n: pts.len(),
                history: true,
            });
        }
    }
    out
}

/// The build context a map lookup must match.
#[derive(Debug, Clone, Copy)]
pub struct MapContext {
    pub drivetrain: i32,
    pub surface_loose: bool,
    pub aero: Option<bool>,
}

/// Rank map cells by how well their pooled behavioural movement aligns with
/// the campaign's pace trends. Context must match (aero only when known on
/// both sides); cells that measured clearly slower elsewhere are excluded —
/// the map must not recommend known losers on behavioural grounds alone.
/// Positive score = the family-direction moves behaviour the way this
/// campaign has been gaining time.
pub fn rank<'a>(cells: &'a [Cell], trends: &[PaceTrend], ctx: &MapContext) -> Vec<(f32, &'a Cell)> {
    let mut out = Vec::new();
    for cell in cells {
        if cell.n == 0
            || cell.drivetrain != ctx.drivetrain
            || cell.surface_loose != ctx.surface_loose
        {
            continue;
        }
        if let (Some(a), Some(b)) = (cell.aero, ctx.aero)
            && a != b
        {
            continue;
        }
        if cell.delta_mean > 0.10 {
            continue;
        }
        let mut score = 0.0;
        for t in trends {
            let Some((_, _, mean, _)) = cell.fields.iter().find(|(k, ..)| *k == t.key) else {
                continue;
            };
            let floor = effects::noise_floor(t.key);
            if mean.abs() < floor {
                continue;
            }
            let strength = (mean.abs() / floor).min(3.0);
            // r > 0: the field rising cost time, so movement AGAINST the
            // costly direction (mean * r < 0) is what we want more of.
            let aligned = mean * t.r < 0.0;
            score += if aligned {
                strength * t.r.abs()
            } else {
                -strength * t.r.abs()
            };
        }
        if score > 0.0 {
            // Own-driver evidence outranks pooled tester data: an all-own
            // cell counts double, a purely foreign one stands as-is.
            let own_frac = cell.own_n as f32 / cell.n as f32;
            out.push((score * (1.0 + own_frac), cell));
        }
    }
    out.sort_by(|a, b| a.0.total_cmp(&b.0).reverse());
    out
}

/// The journal's generic softer/stiffer flag in each family's own
/// vocabulary (`softer` = the value decreased, per the Family docs).
pub fn direction_word(family: &str, softer: bool) -> &'static str {
    match (family, softer) {
        ("gearing", true) => "longer",
        ("gearing", false) => "shorter",
        ("front aero" | "rear aero", true) => "less",
        ("front aero" | "rear aero", false) => "more",
        ("diff accel" | "diff decel", true) => "less lock",
        ("diff accel" | "diff decel", false) => "more lock",
        ("brakes", true) => "rearward/softer",
        ("brakes", false) => "forward/harder",
        ("tire pressure", true) => "lower",
        ("tire pressure", false) => "higher",
        ("ride height", true) => "lower",
        ("ride height", false) => "higher",
        (_, true) => "softer",
        (_, false) => "stiffer",
    }
}

/// Human summary of the aggregated map: one block per cell, effect fields
/// shown only when the pooled mean clears the library noise floor.
pub fn summary(cells: &[Cell]) -> String {
    let mut out = String::new();
    for c in cells {
        let ctx = format!(
            "{} {}{}",
            if c.surface_loose { "dirt" } else { "tarmac" },
            crate::packet::drivetrain_name(c.drivetrain),
            match c.aero {
                Some(true) => " aero",
                Some(false) => " no-aero",
                None => "",
            },
        );
        let dir = direction_word(&c.family, c.softer);
        if c.n == 0 {
            out.push_str(&format!(
                "{} {dir}  [{ctx}]  weak-only ({} sample{})\n",
                c.family,
                c.weak_n,
                if c.weak_n == 1 { "" } else { "s" },
            ));
            continue;
        }
        out.push_str(&format!(
            "{} {dir}  [{ctx}]  n={} ({} direct{}{})  time {:+.2}s ±{:.2}\n",
            c.family,
            c.n,
            c.direct_n,
            // Own share only worth calling out once foreign data exists.
            if c.own_n < c.n {
                format!(", {} yours", c.own_n)
            } else {
                String::new()
            },
            if c.weak_n > 0 {
                format!(", {} weak excluded", c.weak_n)
            } else {
                String::new()
            },
            c.delta_mean,
            c.delta_sd,
        ));
        let movers: effects::Effects = c
            .fields
            .iter()
            .filter(|(k, _, m, _)| m.abs() >= effects::noise_floor(k))
            .map(|(k, _, m, _)| (*k, *m))
            .collect();
        if !movers.is_empty() {
            out.push_str(&format!(
                "  moved (above library floor): {}\n",
                effects::describe(&movers)
            ));
        }
    }
    out
}

/// A local campaign's identity without loading its stints: parsed journal
/// first entry plus the session car. The cheap form of what `harvest_local`
/// derives, for dedupe keys of campaigns served from cache.
fn source_key(source: &CampaignSource) -> Option<CampaignKey> {
    let text = std::fs::read_to_string(&source.journal).ok()?;
    let entries = journal::parse_journal(&text);
    let car = source
        .session
        .as_deref()
        .map(TuningSession::load)
        .and_then(|s| s.car);
    campaign_key(car, &entries)
}

/// Refresh the map file at `out`, re-harvesting only campaigns whose inputs
/// changed since it was written (mtime-based: the journal, its session file,
/// the stints dir for open campaigns — implicit steps join without a journal
/// write — and each sender's library dir). Unchanged campaigns' rows carry
/// over from the existing file. `force` re-harvests everything. When nothing
/// is stale the existing file is left untouched. Returns the map and a
/// report of what was done.
pub fn refresh(
    root: &Path,
    stints_dir: &str,
    out: &Path,
    scratch: &Path,
    force: bool,
) -> Result<(EffectMap, Vec<String>), String> {
    let mut report = Vec::new();
    let old: Option<EffectMap> = (!force)
        .then(|| {
            std::fs::read_to_string(out)
                .ok()
                .and_then(|t| parse(&t).ok())
        })
        .flatten();
    let map_time = old
        .as_ref()
        .and_then(|_| std::fs::metadata(out).and_then(|m| m.modified()).ok());
    let newer = |p: &Path| -> bool {
        match (
            map_time,
            std::fs::metadata(p).and_then(|m| m.modified()).ok(),
        ) {
            (Some(mt), Some(t)) => t > mt,
            // No baseline or unreadable input: treat as changed.
            _ => true,
        }
    };
    let stints_newer = std::fs::read_dir(stints_dir)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .any(|e| e.path().extension().is_some_and(|x| x == "ftel") && newer(&e.path()));

    let mut map = EffectMap::default();
    let mut seen: Vec<CampaignKey> = Vec::new();
    let mut stale_count = 0usize;
    let carry = |map: &mut EffectMap, old: &EffectMap, driver: &str, label: Option<&str>| {
        let matches = |d: &str, c: &str| d == driver && label.is_none_or(|l| l == c);
        map.samples.extend(
            old.samples
                .iter()
                .filter(|s| matches(&s.driver, &s.campaign))
                .cloned(),
        );
        map.floors.extend(
            old.floors
                .iter()
                .filter(|f| matches(&f.driver, &f.campaign))
                .cloned(),
        );
    };
    for source in local_campaigns(root) {
        // Open campaigns accrue implicit steps from new recordings, so new
        // stint files make them stale even with the journal untouched.
        let open = std::fs::read_to_string(&source.journal)
            .map(|t| !crate::advise::campaign_closed(&t))
            .unwrap_or(true);
        let stale = force
            || newer(&source.journal)
            || source.session.as_deref().is_some_and(&newer)
            || (open && stints_newer);
        let cached = old
            .as_ref()
            .is_some_and(|o| o.floors.iter().any(|f| f.campaign == source.label));
        if !stale && cached {
            carry(
                &mut map,
                old.as_ref().unwrap(),
                "local",
                Some(&source.label),
            );
            seen.extend(source_key(&source));
            continue;
        }
        match harvest_local(&source, stints_dir) {
            Ok((samples, floor, key)) => {
                stale_count += 1;
                report.push(format!("{}: {} samples", source.label, samples.len()));
                map.samples.extend(samples);
                map.floors.push(floor);
                seen.extend(key);
            }
            Err(e) => {
                // A failed harvest only dirties the file when it drops rows
                // the old map carried; a campaign that never harvests (e.g.
                // no completed laps yet) must not force rewrites forever.
                if cached {
                    stale_count += 1;
                }
                report.push(format!("skipped {e}"));
            }
        }
    }
    let library = root.join("library");
    for sender in sender_dirs(&library) {
        // Ingest only ever adds files, so the dir mtime is the staleness
        // signal. Zero carried rows is legitimate: a sender whose campaigns
        // all deduped as local echoes contributes nothing.
        if !force
            && !newer(&library.join(&sender))
            && let Some(old) = old.as_ref()
        {
            carry(&mut map, old, &sender, None);
            continue;
        }
        let had_rows = old.as_ref().is_some_and(|o| {
            o.floors.iter().any(|f| f.driver == sender)
                || o.samples.iter().any(|s| s.driver == sender)
        });
        let rows_before = (map.samples.len(), map.floors.len());
        for sc in harvest_sender(&library, &sender, scratch, &mut report) {
            // A sender's echo of a campaign already harvested from local
            // files (this machine shares its own recordings) must not
            // double-count.
            if let Some(key) = campaign_key(sc.car, &sc.entries)
                && seen.contains(&key)
            {
                report.push(format!(
                    "skipped {}: same campaign already harvested locally",
                    sc.label
                ));
                continue;
            }
            match crate::advise::load_campaign(sc.entries, &sc.session, &sc.label) {
                Ok(c) => {
                    let (samples, floor) = harvest_campaign(&c, &sc.driver, &sc.label);
                    report.push(format!("{}: {} samples", sc.label, samples.len()));
                    map.samples.extend(samples);
                    map.floors.push(floor);
                }
                Err(e) => report.push(format!("skipped {e}")),
            }
        }
        // Dirty only when the harvest changed anything: rows produced now,
        // or old rows this pass dropped.
        if (map.samples.len(), map.floors.len()) != rows_before || had_rows {
            stale_count += 1;
        }
    }
    if stale_count == 0
        && let Some(old) = old
    {
        report.push("up to date".into());
        return Ok((old, report));
    }
    if !map.samples.is_empty() || out.exists() {
        std::fs::write(out, render(&map)).map_err(|e| format!("{}: {e}", out.display()))?;
    }
    Ok((map, report))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(family: &str, softer: bool, delta: f32, weak: bool) -> Sample {
        Sample {
            driver: "local".into(),
            campaign: "tune-journal-1.txt".into(),
            car: 1,
            drivetrain: 2,
            surface_loose: false,
            aero: Some(true),
            family: family.into(),
            key: Some("arb_f".into()),
            softer,
            magnitude: Some(-1.0),
            delta_s: delta,
            split: Some((0.1, -0.2, 0.05)),
            weak,
            direct: true,
            clean: true,
            effects: vec![("balance", -0.05), ("temp_front", -6.0)],
        }
    }

    #[test]
    fn render_parse_round_trip() {
        let map = EffectMap {
            samples: vec![sample("front roll", true, -0.3, false)],
            floors: vec![CampaignFloor {
                driver: "local".into(),
                campaign: "tune-journal-1.txt".into(),
                drift_s: Some(0.25),
                effects: vec![("balance", 0.02)],
            }],
        };
        let text = render(&map);
        let back = parse(&text).unwrap();
        assert_eq!(back.samples.len(), 1);
        let s = &back.samples[0];
        assert_eq!(s.family, "front roll");
        assert!(s.softer && s.direct && s.clean && !s.weak);
        assert_eq!(s.aero, Some(true));
        assert_eq!(s.key.as_deref(), Some("arb_f"));
        assert_eq!(s.magnitude, Some(-1.0));
        assert!((s.delta_s - -0.3).abs() < 1e-6);
        assert_eq!(s.split, Some((0.1, -0.2, 0.05)));
        assert_eq!(s.effects, vec![("balance", -0.05), ("temp_front", -6.0)]);
        let f = &back.floors[0];
        assert_eq!(f.drift_s, Some(0.25));
        assert_eq!(f.effects, vec![("balance", 0.02)]);
    }

    #[test]
    fn aggregate_pools_nonweak_and_counts_weak() {
        let map = EffectMap {
            samples: vec![
                sample("front roll", true, -0.3, false),
                sample("front roll", true, -0.1, false),
                sample("front roll", true, 9.9, true),
                sample("front roll", false, 0.4, false),
            ],
            floors: Vec::new(),
        };
        let cells = aggregate(&map);
        assert_eq!(cells.len(), 2);
        let softer = cells.iter().find(|c| c.softer).unwrap();
        assert_eq!((softer.n, softer.weak_n), (2, 1));
        assert!((softer.delta_mean - -0.2).abs() < 1e-6);
        let bal = softer
            .fields
            .iter()
            .find(|(k, ..)| *k == "balance")
            .unwrap();
        assert_eq!(bal.1, 2);
        assert!((bal.2 - -0.05).abs() < 1e-6);
    }

    #[test]
    fn weak_only_bucket_reports_without_stats() {
        let map = EffectMap {
            samples: vec![sample("brakes", true, 0.5, true)],
            floors: Vec::new(),
        };
        let cells = aggregate(&map);
        assert_eq!(cells.len(), 1);
        assert_eq!((cells[0].n, cells[0].weak_n), (0, 1));
        assert!(summary(&cells).contains("weak-only"));
    }

    #[test]
    fn pace_trends_gate_on_floor_count_and_r() {
        // balance rising costs time, cleanly: r ~ +1 over 4 above-floor pairs.
        let pairs: Vec<(effects::Effects, f32)> = vec![
            (vec![("balance", 0.04)], 0.1),
            (vec![("balance", 0.08)], 0.3),
            (vec![("balance", -0.05)], -0.2),
            (vec![("balance", -0.10)], -0.4),
            // below the 0.03 floor: must not count as a 5th pair
            (vec![("balance", 0.01)], 5.0),
        ];
        let trends = pace_trends(&pairs, None);
        assert_eq!(trends.len(), 1);
        assert_eq!(trends[0].key, "balance");
        assert_eq!(trends[0].n, 4);
        assert!(trends[0].r > 0.9, "r={}", trends[0].r);
        // Only 3 above-floor pairs -> silent.
        assert!(pace_trends(&pairs[..3], None).is_empty());
        // A campaign floor above every movement -> silent.
        let raised = vec![("balance", 0.2)];
        assert!(pace_trends(&pairs, Some(&raised)).is_empty());
    }

    #[test]
    fn rank_scores_aligned_cells_and_filters_context() {
        let trend = PaceTrend {
            key: "balance",
            r: 0.9,
            n: 5,
            history: false,
        };
        let cell = |mean: f32, drivetrain: i32, delta_mean: f32| Cell {
            family: "rear roll".into(),
            softer: true,
            drivetrain,
            surface_loose: false,
            aero: Some(true),
            n: 2,
            own_n: 2,
            direct_n: 1,
            weak_n: 0,
            delta_mean,
            delta_sd: 0.1,
            fields: vec![("balance", 2, mean, 0.01)],
        };
        let ctx = MapContext {
            drivetrain: 2,
            surface_loose: false,
            aero: Some(true),
        };
        let trend = std::slice::from_ref(&trend);
        // Aligned: balance FELL where the campaign says balance-up = slower.
        let aligned = [cell(-0.06, 2, -0.2)];
        let ranked = rank(&aligned, trend, &ctx);
        assert_eq!(ranked.len(), 1);
        assert!(ranked[0].0 > 0.0);
        // Contra movement scores negative -> dropped.
        assert!(rank(&[cell(0.06, 2, -0.2)], trend, &ctx).is_empty());
        // Wrong drivetrain and known-loser cells never rank.
        assert!(rank(&[cell(-0.06, 1, -0.2)], trend, &ctx).is_empty());
        assert!(rank(&[cell(-0.06, 2, 0.5)], trend, &ctx).is_empty());
    }

    #[test]
    fn rank_prefers_own_driver_evidence() {
        let trend = PaceTrend {
            key: "balance",
            r: 0.9,
            n: 5,
            history: false,
        };
        let cell = |own_n: usize, family: &str| Cell {
            family: family.into(),
            softer: true,
            drivetrain: 2,
            surface_loose: false,
            aero: Some(true),
            n: 2,
            own_n,
            direct_n: 1,
            weak_n: 0,
            delta_mean: -0.2,
            delta_sd: 0.1,
            fields: vec![("balance", 2, -0.06, 0.01)],
        };
        let ctx = MapContext {
            drivetrain: 2,
            surface_loose: false,
            aero: Some(true),
        };
        // Identical stats: the all-own cell must outrank the all-foreign one.
        let cells = [cell(0, "rear roll"), cell(2, "damping")];
        let ranked = rank(&cells, std::slice::from_ref(&trend), &ctx);
        assert_eq!(ranked.len(), 2);
        assert_eq!(ranked[0].1.family, "damping");
        assert!((ranked[0].0 - 2.0 * ranked[1].0).abs() < 1e-6);
    }

    #[test]
    fn driver_trends_pool_across_campaigns_and_exclude_the_current_car() {
        let mut s1 = sample("front roll", true, 0.1, false);
        s1.effects = vec![("balance", 0.04)];
        let mut s2 = sample("front roll", true, 0.3, false);
        s2.effects = vec![("balance", 0.08)];
        s2.campaign = "tune-journal-2.txt".into();
        let mut s3 = sample("front roll", true, -0.2, false);
        s3.effects = vec![("balance", -0.05)];
        let mut s4 = sample("front roll", true, -0.4, false);
        s4.effects = vec![("balance", -0.10)];
        // A foreign driver's sample must not join "local" trends.
        let mut foreign = sample("front roll", true, 5.0, false);
        foreign.driver = "abcd".into();
        foreign.effects = vec![("balance", 0.2)];
        let map = EffectMap {
            samples: vec![s1, s2, s3, s4, foreign],
            floors: Vec::new(),
        };
        let trends = driver_trends(&map, "local", None);
        assert_eq!(trends.len(), 1);
        assert_eq!(trends[0].key, "balance");
        assert!(trends[0].history);
        assert_eq!(trends[0].n, 4);
        assert!(trends[0].r > 0.9, "r={}", trends[0].r);
        // Excluding the samples' car (1) removes them all -> silent.
        assert!(driver_trends(&map, "local", Some(1)).is_empty());
        // A campaign floor above a sample's movement gates that sample out.
        let map = EffectMap {
            floors: vec![CampaignFloor {
                driver: "local".into(),
                campaign: "tune-journal-1.txt".into(),
                drift_s: None,
                effects: vec![("balance", 0.06)],
            }],
            ..map
        };
        // s1 (0.04) and s3 (0.05) fall under the raised floor of campaign 1;
        // only 4 - 2 = 2 pairs remain -> under the minimum, silent.
        assert!(driver_trends(&map, "local", None).is_empty());
    }

    #[test]
    fn refresh_carries_unchanged_campaigns_without_harvesting() {
        let root = std::env::temp_dir().join(format!("tuners-refresh-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        // An archived (closed) campaign pair whose stint recording does NOT
        // exist: harvesting it can only fail, so surviving rows prove the
        // cache path was taken.
        let label = "tune-journal-99-20260101-000000.txt";
        std::fs::write(
            root.join("tune-session-99-20260101-000000.txt"),
            "car = 99\n\n[20260101-000000]\narb_f = 20\n",
        )
        .unwrap();
        std::fs::write(
            root.join(label),
            "sessions/stint-20260101-000100.ftel | front arb -1\n# parked 20260102-000000\n",
        )
        .unwrap();
        let out = root.join("effect-map.tsv");
        let mut sample = sample("front roll", true, -0.3, false);
        sample.campaign = label.into();
        let old = EffectMap {
            samples: vec![sample],
            floors: vec![CampaignFloor {
                driver: "local".into(),
                campaign: label.into(),
                drift_s: None,
                effects: Vec::new(),
            }],
        };
        std::fs::write(&out, render(&old)).unwrap();

        let stints = root.join("sessions").to_string_lossy().into_owned();
        let scratch = root.join("scratch");
        let (map, report) = refresh(&root, &stints, &out, &scratch, false).unwrap();
        assert_eq!(map.samples.len(), 1, "{report:?}");
        assert_eq!(map.floors.len(), 1);
        assert!(report.iter().any(|l| l == "up to date"), "{report:?}");

        // Forced: the harvest really runs, fails on the missing recording,
        // and the stale rows drop.
        let (map, report) = refresh(&root, &stints, &out, &scratch, true).unwrap();
        assert!(map.samples.is_empty(), "{report:?}");
        assert!(report.iter().any(|l| l.contains("missing")), "{report:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn summary_gates_movers_on_library_floor() {
        // balance mean -0.05 clears its 0.03 floor; temp_front -6.0 clears
        // 4.0; shrink temp under the floor and it must vanish.
        let mut s = sample("front roll", true, -0.3, false);
        s.effects = vec![("balance", -0.05), ("temp_front", -2.0)];
        let cells = aggregate(&EffectMap {
            samples: vec![s],
            floors: Vec::new(),
        });
        let text = summary(&cells);
        assert!(text.contains("balance index"), "{text}");
        assert!(!text.contains("front tire temp"), "{text}");
    }
}
