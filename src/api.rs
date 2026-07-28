//! Typed app-facing API: the view structs and operations behind the desktop
//! app's commands. Transport-agnostic: serialization is
//! serde, and every builder is a pure function over engine state, so commands
//! and tests share the exact same surface. Wire names stay camelCase to match
//! the dashboard's existing JSON contract.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Serialize;
use specta::Type;

use crate::analysis::effects::Effects;

/// Command failure with enough structure for the frontend to distinguish
/// "confirm and retry with force" (Conflict) from plain errors: the typed
/// replacement for the HTTP status codes the dashboard used to branch on.
#[derive(Serialize, Type, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ApiError {
    pub kind: ErrorKind,
    pub message: String,
}

#[derive(Serialize, Type, Debug, Clone, Copy, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum ErrorKind {
    BadRequest,
    Forbidden,
    NotFound,
    Conflict,
    Internal,
}

impl ApiError {
    fn bad(msg: impl Into<String>) -> Self {
        ApiError {
            kind: ErrorKind::BadRequest,
            message: msg.into(),
        }
    }
    fn conflict(msg: impl Into<String>) -> Self {
        ApiError {
            kind: ErrorKind::Conflict,
            message: msg.into(),
        }
    }
    fn not_found(msg: impl Into<String>) -> Self {
        ApiError {
            kind: ErrorKind::NotFound,
            message: msg.into(),
        }
    }
    fn internal(msg: impl ToString) -> Self {
        ApiError {
            kind: ErrorKind::Internal,
            message: msg.to_string(),
        }
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

/// Effect vector on the wire: sparse map keyed by `effects::FIELDS` keys;
/// absent fields are real absences, never zeroes.
fn effects_map(fx: &Effects) -> BTreeMap<String, f32> {
    fx.iter().map(|(k, v)| (k.to_string(), *v)).collect()
}

// ---------------------------------------------------------------- live state

/// Latest telemetry frame, cut down to what the Drive screen shows.
#[derive(Serialize, Type, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct FrameView {
    pub race_on: bool,
    /// Car ordinal from the packet (0 in menus, where everything but the timestamp
    /// is zeroed while race is off). Lets onboarding show "car detected".
    pub car: i32,
    pub car_name: Option<String>,
    pub speed_mps: f32,
    pub rpm: f32,
    pub max_rpm: f32,
    pub gear: u8,
    pub lap_number: u16,
    pub current_lap_s: f32,
    pub last_lap_s: f32,
    pub best_lap_s: f32,
    pub fuel: f32,
    pub tire_temp_f: [f32; 4],
}

#[derive(Serialize, Type, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RecorderView {
    /// "recording" | "waiting" | "external" (port busy or disabled: view-only).
    pub mode: String,
    pub file: Option<String>,
    pub packets: u32,
    /// Milliseconds since ANY datagram hit the socket; menu packets count,
    /// though they are never recorded. The onboarding wiring check: fresh
    /// here + no frame = hooked up, just not driving yet. None in external
    /// mode (another capture owns the socket) or before the first packet.
    pub udp_age_ms: Option<u32>,
    /// Car seen in raw packets (free roam counts; nothing need be recorded).
    pub udp_car: Option<i32>,
    pub udp_car_name: Option<String>,
}

/// The `live-state` event payload (was the SSE `state` event).
#[derive(Serialize, Type, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LiveStateView {
    pub file: Option<String>,
    pub age_ms: Option<u32>,
    pub frame: Option<FrameView>,
    pub recorder: RecorderView,
}

pub fn live_state_view(
    s: &crate::live::LiveState,
    r: &crate::record::RecorderStatus,
) -> LiveStateView {
    let frame = s.latest.as_ref().map(|tf| {
        let f = &tf.frame;
        let t = f.tire_temp;
        FrameView {
            race_on: f.is_race_on,
            car: f.car_ordinal,
            car_name: crate::cars::car_name(f.car_ordinal).map(str::to_string),
            speed_mps: f.speed,
            rpm: f.current_engine_rpm,
            max_rpm: f.engine_max_rpm,
            gear: f.gear,
            lap_number: f.lap_number,
            current_lap_s: f.current_lap,
            last_lap_s: f.last_lap,
            best_lap_s: f.best_lap,
            fuel: f.fuel,
            tire_temp_f: [t.fl, t.fr, t.rl, t.rr],
        }
    });
    LiveStateView {
        file: file_name(s.file.as_deref()),
        age_ms: s
            .last_data
            .map(|t| t.elapsed().as_millis().min(u32::MAX as u128) as u32),
        frame,
        recorder: recorder_view(r),
    }
}

pub fn recorder_view(r: &crate::record::RecorderStatus) -> RecorderView {
    let mode = match &r.mode {
        crate::record::RecorderMode::External(_) => "external",
        crate::record::RecorderMode::Waiting => "waiting",
        crate::record::RecorderMode::Recording => "recording",
    };
    RecorderView {
        mode: mode.to_string(),
        file: file_name(r.file.as_deref()),
        packets: r.packets.min(u32::MAX as u64) as u32,
        udp_age_ms: r
            .last_udp
            .map(|t| t.elapsed().as_millis().min(u32::MAX as u128) as u32),
        udp_car: r.udp_car,
        udp_car_name: r
            .udp_car
            .and_then(crate::cars::car_name)
            .map(str::to_string),
    }
}

fn file_name(p: Option<&Path>) -> Option<String> {
    p.and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
}

/// The `quality` event payload; None until a comparable lap exists.
#[derive(Serialize, Type, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct QualityView {
    pub laps: u32,
    pub standing_only: bool,
    pub best_lap_s: f32,
    pub spread_pct: f32,
    pub shared_km: f32,
    pub confidence_pct: f32,
    pub band: String,
}

pub fn quality_view(q: Option<&crate::live::Quality>) -> Option<QualityView> {
    q.map(|q| QualityView {
        laps: q.laps as u32,
        standing_only: q.standing_only,
        best_lap_s: q.best_lap_s,
        spread_pct: q.spread_frac * 100.0,
        shared_km: q.shared_km,
        confidence_pct: q.confidence * 100.0,
        band: q.band.as_str().to_string(),
    })
}

/// One entry of the effect-field registry: stable key, display label, unit
/// hint ("" = plain number, "frac" = 0..1 shown as %), and the library noise
/// floor. The engine owns this list (`effects::FIELDS`); the frontend must
/// never hand-copy it.
#[derive(Serialize, Type, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct EffectFieldView {
    pub key: String,
    pub label: String,
    pub unit: String,
    pub floor: f32,
}

pub fn effect_fields() -> Vec<EffectFieldView> {
    crate::analysis::effects::FIELDS
        .iter()
        .map(|(key, label, unit)| EffectFieldView {
            key: key.to_string(),
            label: label.to_string(),
            unit: unit.to_string(),
            floor: crate::analysis::effects::noise_floor(key),
        })
        .collect()
}

// ------------------------------------------------------------------- sharing

/// Effect-map state for the Settings screen: what the background refresher
/// last produced. None = no map yet (nothing journaled anywhere).
#[derive(Serialize, Type, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct EffectMapStatus {
    pub samples: u32,
    pub campaigns: u32,
    /// Unix ms of the map file's last write.
    pub updated_ms: f64,
}

pub fn effect_map_status() -> Option<EffectMapStatus> {
    let path = crate::util::data_path("effect-map.tsv");
    let text = std::fs::read_to_string(&path).ok()?;
    let map = crate::effectmap::parse(&text).ok()?;
    let updated_ms = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis() as f64;
    Some(EffectMapStatus {
        samples: map.samples.len() as u32,
        campaigns: map.floors.len() as u32,
        updated_ms,
    })
}

/// Telemetry-collection state: consent flag, pseudonymous sender
/// id, and outbox depth. The token itself never leaves the config file.
#[derive(Serialize, Type, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SharingView {
    pub enabled: bool,
    pub endpoint: String,
    pub sender: Option<String>,
    pub queued: u32,
    pub rejected: u32,
}

pub fn sharing_view(config: &Path, outbox: &Path) -> SharingView {
    let cfg = crate::collect::CollectConfig::load(config);
    let rejected = std::fs::read_dir(outbox.join("rejected"))
        .map(|rd| rd.flatten().count())
        .unwrap_or(0);
    SharingView {
        enabled: cfg.enabled,
        endpoint: cfg.endpoint.clone(),
        sender: (!cfg.token.is_empty()).then(|| crate::collect::sender_id(&cfg.token)),
        queued: crate::collect::queued(outbox).len() as u32,
        rejected: rejected as u32,
    }
}

/// Toggle/configure collection. First enable mints the client token;
/// `discard` (with disable) empties the queue.
pub fn set_sharing(
    config: &Path,
    outbox: &Path,
    enabled: bool,
    endpoint: Option<String>,
    discard: bool,
) -> Result<SharingView, ApiError> {
    let mut cfg = crate::collect::CollectConfig::load(config);
    if enabled {
        cfg.enabled = true;
        if cfg.token.len() != 64 {
            cfg.token = crate::collect::generate_token();
        }
        if let Some(e) = endpoint.as_deref().map(str::trim).filter(|e| !e.is_empty()) {
            cfg.endpoint = e.to_string();
        }
        if cfg.endpoint.is_empty() {
            cfg.endpoint = crate::collect::DEFAULT_ENDPOINT.to_string();
        }
    } else {
        cfg.enabled = false;
        if discard {
            for p in crate::collect::queued(outbox) {
                let _ = std::fs::remove_file(p);
            }
        }
    }
    cfg.save(config).map_err(ApiError::internal)?;
    Ok(sharing_view(config, outbox))
}

/// Preview of a historic backfill: what "share existing
/// recordings" would queue.
#[derive(Serialize, Type, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct HistoryPlanView {
    pub campaigns: u32,
    pub stints: u32,
    pub mb: f64,
    pub unjournaled: u32,
    pub already: u32,
}

pub fn history_plan_view(root: &Path, sessions_dir: &str, outbox: &Path) -> HistoryPlanView {
    let p = crate::collect::history_plan(root, sessions_dir, outbox);
    HistoryPlanView {
        campaigns: p.campaigns as u32,
        stints: p.items.len() as u32,
        mb: p.bytes as f64 / 1e6,
        unjournaled: p.unjournaled as u32,
        already: p.already as u32,
    }
}

/// Queue the historic backfill. Consent guard is server-side: historic
/// sharing is a separate deliberate act, never possible while sharing is off.
/// Returns how many bundles are being queued (on a background thread).
pub fn share_history(
    root: &Path,
    sessions_dir: &str,
    outbox: &Path,
    config: &Path,
) -> Result<u32, ApiError> {
    let cfg = crate::collect::CollectConfig::load(config);
    if !cfg.ready() {
        return Err(ApiError {
            kind: ErrorKind::Forbidden,
            message: "turn on telemetry sharing first".into(),
        });
    }
    let plan = crate::collect::history_plan(root, sessions_dir, outbox);
    let n = plan.items.len() as u32;
    let outbox = outbox.to_path_buf();
    std::thread::spawn(move || {
        crate::collect::history_enqueue(plan, &outbox);
    });
    Ok(n)
}

// ------------------------------------------------------------------ sessions

/// The active tuning session: car, facts, latest tune revision. Baseline
/// included so the frontend can summarize "delta vs baseline" without
/// dumping the whole tune.
#[derive(Serialize, Type, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SessionView {
    pub car: Option<i32>,
    pub car_name: Option<String>,
    pub facts: BTreeMap<String, String>,
    pub revisions: u32,
    pub latest: Option<BTreeMap<String, String>>,
    pub baseline: Option<BTreeMap<String, String>>,
    pub campaign_start: Option<String>,
}

pub fn session_view(s: &crate::tuning::TuningSession, journal_base: &str) -> SessionView {
    SessionView {
        car: s.car,
        car_name: s.car.and_then(crate::cars::car_name).map(str::to_string),
        facts: s.facts.clone(),
        revisions: s.revisions.len() as u32,
        latest: s.latest().map(|rev| rev.values.clone()),
        baseline: s
            .revisions
            .first()
            .filter(|_| s.revisions.len() > 1)
            .map(|rev| rev.values.clone()),
        campaign_start: campaign_start(s, journal_base),
    }
}

/// When the active campaign began: the earlier of the first tune revision and
/// the first journaled stint (the seeded baseline stint starts before the
/// first save). Scopes the frontend stint list to the campaign.
fn campaign_start(s: &crate::tuning::TuningSession, journal_base: &str) -> Option<String> {
    let mut start = s.revisions.first().map(|r| r.stamp.clone());
    let jpath = crate::tuning::journal_path_for(s.car, journal_base);
    if let Ok(text) = std::fs::read_to_string(&jpath)
        && let Some(first) = crate::analysis::journal::parse_journal(&text).first()
        && let Some(stamp) = crate::advise::stint_stamp(&first.path)
    {
        start = Some(match start {
            Some(cur) if cur.as_str() <= stamp => cur,
            _ => stamp.to_string(),
        });
    }
    start
}

/// Create/update the active session: car + facts. Posting a DIFFERENT car
/// than the current one is a session switch (legacy scheme): the active
/// campaign archives to its per-car file and the new car's parked campaign
/// (if any) resumes. An empty fact value removes the fact; an unparseable
/// `car` clears the car.
pub struct SessionUpdate {
    pub reset: bool,
    /// None = leave the car untouched; Some(s) sets `s.parse().ok()`.
    pub car: Option<String>,
    pub facts: Vec<(String, String)>,
}

pub fn update_session(
    req: &SessionUpdate,
    path: &Path,
    journal_base: &str,
) -> Result<SessionView, ApiError> {
    let posted_car: Option<i32> = req.car.as_deref().and_then(|v| v.parse().ok());
    let current = crate::tuning::TuningSession::load(path);
    let mut s = if req.reset {
        crate::tuning::TuningSession::default()
    } else if posted_car.is_some() && current.car.is_some() && posted_car != current.car {
        // Switching cars = switching SESSIONS: archive the active session to
        // its per-car file (tune-session-<ordinal>.txt, same scheme as
        // journals) and resume the new car's archived session if one exists.
        // Nothing is lost; switching back restores the whole campaign.
        let base = path.to_string_lossy();
        let archive = crate::tuning::journal_path_for(current.car, &base);
        let _ = current.save(archive.as_ref());
        let resumed = crate::tuning::TuningSession::load(
            crate::tuning::journal_path_for(posted_car, &base).as_ref(),
        );
        if resumed.car == posted_car {
            resumed
        } else {
            crate::tuning::TuningSession::default()
        }
    } else {
        current
    };
    if let Some(v) = &req.car {
        s.car = v.parse().ok();
    }
    for (k, v) in &req.facts {
        if v.trim().is_empty() {
            s.facts.remove(k);
        } else {
            s.facts.insert(k.clone(), v.trim().to_string());
        }
    }
    s.save(path).map_err(ApiError::internal)?;
    Ok(session_view(&s, journal_base))
}

/// A session switch is a hard recording boundary: cut the stint and drop any
/// pending tune-note chain (the chain belonged to the outgoing session).
pub fn split_and_drop_pending(recorder: &crate::record::SharedRecorder) {
    let mut r = recorder.lock().unwrap();
    r.split_requested = true;
    r.pending_note = None;
    r.pending_base_rev = None;
}

/// Manual stint cut (the dashboard's "new session" button), with an optional
/// note to journal against the next stint.
pub fn request_split(recorder: &crate::record::SharedRecorder, note: Option<String>) {
    let mut r = recorder.lock().unwrap();
    r.split_requested = true;
    r.pending_note = note.filter(|n| !n.trim().is_empty());
}

/// Archiving a blank session would save nothing worth resuming: no car, no
/// revisions, and no facts beyond display prefs.
fn session_is_blank(s: &crate::tuning::TuningSession) -> bool {
    s.car.is_none() && s.revisions.is_empty() && s.facts.keys().all(|k| k.starts_with("unit_"))
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Park the active session pair under a stamped id: the session file is copied
/// to its archive name and the car's live journal moves with it. Ids are
/// "<ordinal>-<stamp>"; the legacy car-switch scheme's plain "<ordinal>" ids
/// coexist and resume the same way.
fn archive_active(
    s: &crate::tuning::TuningSession,
    session_file: &str,
    journal_base: &str,
) -> std::io::Result<String> {
    let base_id = format!(
        "{}-{}",
        s.car.map_or_else(|| "none".into(), |c| c.to_string()),
        crate::util::utc_stamp(now_secs()),
    );
    // Same car archived twice within a second (quick new+resume clicks) must
    // not overwrite the first archive: bump a counter until the id is free.
    let mut id = base_id.clone();
    let mut n = 2;
    while Path::new(&crate::tuning::suffixed_path(session_file, &id)).exists()
        || Path::new(&crate::tuning::suffixed_path(journal_base, &id)).exists()
    {
        id = format!("{base_id}-{n}");
        n += 1;
    }
    s.save(crate::tuning::suffixed_path(session_file, &id).as_ref())?;
    let live = crate::tuning::journal_path_for(s.car, journal_base);
    if Path::new(&live).exists() {
        let parked = crate::tuning::suffixed_path(journal_base, &id);
        std::fs::rename(&live, &parked)?;
        // Boundary marker (a comment; the entry parser skips it): a parked
        // campaign accrues no implicit trajectory steps while other campaigns
        // drive the same car (advise::campaign_bound).
        append_line(
            &parked,
            &format!("# parked {}", crate::util::utc_stamp(now_secs())),
        )?;
    }
    Ok(id)
}

fn append_line(path: &str, line: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    let mut f = std::fs::OpenOptions::new().append(true).open(path)?;
    writeln!(f, "{line}")
}

/// Start a fresh session (e.g. after an upgrade rebuild): the active pair is
/// archived and a blank session (carrying only the unit display prefs, plus
/// any posted name/description/car) becomes active. The first tune save
/// seeds the new journal's baseline against the next stint.
pub fn new_session(
    car: Option<String>,
    name: Option<String>,
    description: Option<String>,
    session_file: &str,
    journal_base: &str,
) -> Result<SessionView, ApiError> {
    let current = crate::tuning::TuningSession::load(session_file.as_ref());
    if !session_is_blank(&current) {
        archive_active(&current, session_file, journal_base).map_err(ApiError::internal)?;
    }
    let mut fresh = crate::tuning::TuningSession::default();
    for (k, v) in &current.facts {
        if k.starts_with("unit_") {
            fresh.facts.insert(k.clone(), v.clone());
        }
    }
    fresh.car = car.as_deref().and_then(|v| v.trim().parse().ok());
    for (k, v) in [("name", name), ("description", description)] {
        if let Some(v) = v.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
            fresh.facts.insert(k.to_string(), v.to_string());
        }
    }
    fresh
        .save(session_file.as_ref())
        .map_err(ApiError::internal)?;
    Ok(session_view(&fresh, journal_base))
}

/// Swap an archived session back in: the active pair is archived first, then
/// the chosen pair becomes active (files move, nothing is copied; an
/// archived session has exactly one home).
pub fn resume_session(
    id: &str,
    session_file: &str,
    journal_base: &str,
) -> Result<SessionView, ApiError> {
    let id = id.trim();
    if id.is_empty() {
        return Err(ApiError::bad("missing id"));
    }
    if !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(ApiError::bad("bad id"));
    }
    let archived_session = crate::tuning::suffixed_path(session_file, id);
    if !Path::new(&archived_session).exists() {
        return Err(ApiError::not_found("no such session"));
    }
    let restored = crate::tuning::TuningSession::load(archived_session.as_ref());
    let current = crate::tuning::TuningSession::load(session_file.as_ref());
    // The restored journal must not clobber a live one. The archive step below
    // frees the live journal only when the active session is the same car;
    // otherwise a journal already at the target belongs to a THIRD campaign
    // (parked by the legacy car-switch scheme) and moving over it would lose it.
    let target = crate::tuning::journal_path_for(restored.car, journal_base);
    let src = crate::tuning::suffixed_path(journal_base, id);
    let frees_target = !session_is_blank(&current) && current.car == restored.car;
    if src != target && Path::new(&target).exists() && !frees_target {
        return Err(ApiError::conflict(format!(
            "{target} already exists: another session for this car is parked; resume it first"
        )));
    }
    if !session_is_blank(&current) {
        archive_active(&current, session_file, journal_base).map_err(ApiError::internal)?;
    }
    std::fs::rename(&archived_session, session_file).map_err(ApiError::internal)?;
    if src != target
        && Path::new(&src).exists()
        && let Err(e) = std::fs::rename(&src, &target)
    {
        return Err(ApiError::internal(e));
    }
    // The resume marker floors the implicit-step scan: stints other campaigns
    // drove while this one was parked stay out of its trajectory.
    if Path::new(&target).exists()
        && let Err(e) = append_line(
            &target,
            &format!("# resumed {}", crate::util::utc_stamp(now_secs())),
        )
    {
        return Err(ApiError::internal(e));
    }
    Ok(session_view(&restored, journal_base))
}

/// Count of stints a journal file references (non-comment lines).
fn journal_stints(path: &str) -> usize {
    std::fs::read_to_string(path)
        .map(|t| {
            t.lines()
                .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
                .count()
        })
        .unwrap_or(0)
}

/// One session in the library listing; `id: None` is the active session.
#[derive(Serialize, Type, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SessionRow {
    pub id: Option<String>,
    pub car: Option<i32>,
    pub car_name: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub revisions: u32,
    pub stints: u32,
}

/// The session library: the active session plus every archived sibling
/// (<stem>-<id>.<ext>), newest first.
#[derive(Serialize, Type, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SessionsView {
    pub active: SessionRow,
    pub archived: Vec<SessionRow>,
}

pub fn sessions_view(session_file: &str, journal_base: &str) -> SessionsView {
    let row = |id: Option<&str>, s: &crate::tuning::TuningSession, stints: usize| SessionRow {
        id: id.map(str::to_string),
        car: s.car,
        car_name: s.car.and_then(crate::cars::car_name).map(str::to_string),
        name: s.facts.get("name").cloned(),
        description: s.facts.get("description").cloned(),
        revisions: s.revisions.len() as u32,
        stints: stints as u32,
    };
    let path = Path::new(session_file);
    let dir = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let file_name = path
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (stem, ext) = file_name
        .rsplit_once('.')
        .unwrap_or((file_name.as_str(), ""));
    let (prefix, suffix) = (format!("{stem}-"), format!(".{ext}"));
    let mut archived: Vec<(std::time::SystemTime, SessionRow)> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for entry in rd.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name == file_name {
                continue;
            }
            let Some(id) = name
                .strip_prefix(prefix.as_str())
                .and_then(|r| r.strip_suffix(suffix.as_str()))
                .filter(|id| !id.is_empty())
            else {
                continue;
            };
            let s = crate::tuning::TuningSession::load(&entry.path());
            let stints = journal_stints(&crate::tuning::suffixed_path(journal_base, id));
            let modified = entry
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            archived.push((modified, row(Some(id), &s, stints)));
        }
    }
    archived.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    let active = crate::tuning::TuningSession::load(path);
    let active_stints = journal_stints(&crate::tuning::journal_path_for(active.car, journal_base));
    SessionsView {
        active: row(None, &active, active_stints),
        archived: archived.into_iter().map(|(_, r)| r).collect(),
    }
}

// ----------------------------------------------------------------- tune save

#[derive(Serialize, Type, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TuneSaveView {
    pub ok: bool,
    /// The netted journal note this save will journal against the next stint;
    /// None for the baseline (first revision) or a change that nets to zero.
    pub note: Option<String>,
    pub changed: bool,
}

/// Save a new tune revision. The journal note is derived by diffing against
/// the previous revision; a changed tune also cuts the stint (the note
/// journals against the next stint that opens). The first revision is the
/// baseline: stored, no note, no cut. `partial` (accepting a suggestion)
/// merges the posted keys onto the LATEST revision, and consecutive saves
/// with no stint between them net into ONE journal note (diffed against the
/// last DRIVEN revision, the pending chain's base).
pub fn save_tune(
    values: &[(String, String)],
    partial: bool,
    path: &Path,
    recorder: &crate::record::SharedRecorder,
) -> Result<TuneSaveView, ApiError> {
    let mut s = crate::tuning::TuningSession::load(path);
    let mut rev = crate::tuning::Revision {
        stamp: crate::util::utc_stamp(now_secs()),
        ..Default::default()
    };
    for (k, v) in values {
        let v = v.trim();
        if !v.is_empty() && crate::tuning::FIELDS.iter().any(|(key, _)| key == k) {
            rev.values.insert(k.clone(), v.to_string());
        }
    }
    if rev.values.is_empty() {
        return Err(ApiError::bad("empty tune"));
    }
    if partial {
        let Some(latest) = s.latest() else {
            return Err(ApiError::bad(
                "no tune on file to merge a partial save onto",
            ));
        };
        let mut merged = latest.values.clone();
        merged.append(&mut rev.values);
        rev.values = merged;
    }
    let (has_pending, pending_base) = {
        let r = recorder.lock().unwrap();
        (r.pending_note.is_some(), r.pending_base_rev)
    };
    let base_idx = if has_pending {
        pending_base
    } else {
        s.revisions.len().checked_sub(1)
    };
    let note = base_idx
        .and_then(|i| s.revisions.get(i))
        .map(|base| crate::tuning::diff_note(base, &rev))
        .unwrap_or_default();
    let first = s.revisions.is_empty();
    if !first && note.is_empty() {
        // Nets to no change vs the driven tune (a pending chain was reverted):
        // nothing to journal, drop any pending note.
        {
            let mut r = recorder.lock().unwrap();
            r.pending_note = None;
            r.pending_base_rev = None;
        }
        if s.latest().is_some_and(|l| l.values != rev.values) {
            s.revisions.push(rev);
            s.save(path).map_err(ApiError::internal)?;
        }
        return Ok(TuneSaveView {
            ok: true,
            note: None,
            changed: false,
        });
    }
    s.revisions.push(rev);
    s.save(path).map_err(ApiError::internal)?;
    if !first {
        let mut r = recorder.lock().unwrap();
        r.split_requested = true;
        r.pending_note = Some(note.clone());
        r.pending_base_rev = base_idx;
    }
    Ok(TuneSaveView {
        ok: true,
        note: (!first).then_some(note),
        changed: true,
    })
}

/// One slider in the pending set: differs from the last DRIVEN revision.
#[derive(Serialize, Type, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PendingChange {
    pub key: String,
    pub phrase: String,
    pub from: Option<String>,
    pub to: String,
}

/// The pending basket: tune edits saved since the last driven run, netted
/// (the recorder's pending chain). None = the saved tune has been driven.
#[derive(Serialize, Type, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PendingView {
    /// The netted journal note the next run will be journaled under.
    pub note: String,
    pub changes: Vec<PendingChange>,
}

pub fn pending_view(
    session_path: &Path,
    recorder: &crate::record::SharedRecorder,
) -> Option<PendingView> {
    let (note, base_idx) = {
        let r = recorder.lock().unwrap();
        (r.pending_note.clone()?, r.pending_base_rev)
    };
    let s = crate::tuning::TuningSession::load(session_path);
    let latest = s.latest()?.values.clone();
    let base = base_idx.and_then(|i| s.revisions.get(i)).map(|r| &r.values);
    let phrase = |k: &str| {
        crate::tuning::FIELDS
            .iter()
            .find(|(key, _)| *key == k)
            .map_or_else(|| k.to_string(), |(_, p)| p.to_string())
    };
    let changes = latest
        .iter()
        .filter(|(k, v)| base.and_then(|b| b.get(*k)) != Some(v))
        .map(|(k, v)| PendingChange {
            key: k.clone(),
            phrase: phrase(k),
            from: base.and_then(|b| b.get(k)).cloned(),
            to: v.clone(),
        })
        .collect();
    Some(PendingView { note, changes })
}

// -------------------------------------------------------------------- stints

/// One recorded stint in the sessions directory.
#[derive(Serialize, Type, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StintRow {
    pub file: String,
    pub bytes: u32,
    pub car: i32,
    pub car_name: String,
}

/// Every .ftel in the sessions directory, oldest first (name order).
pub fn stint_rows(dir: &str) -> Vec<StintRow> {
    let mut rows = Vec::new();
    if let Ok(read_dir) = std::fs::read_dir(dir) {
        let mut paths: Vec<_> = read_dir
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "ftel"))
            .collect();
        paths.sort();
        for p in paths {
            let bytes = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0) as u32;
            let car = stint_car(&p).unwrap_or(0);
            let name = crate::cars::car_name(car).unwrap_or("");
            rows.push(StintRow {
                file: p.display().to_string(),
                bytes,
                car,
                car_name: name.to_string(),
            });
        }
    }
    rows
}

/// First car seen driving in the session (bounded scan; the file may open
/// with menu frames, but driving starts within moments in every real capture).
pub fn stint_car(path: &Path) -> Option<i32> {
    let mut reader = crate::stint::StintReader::open(path).ok()?;
    for _ in 0..20_000 {
        let (_, payload) = reader.next_packet().ok()??;
        if let Ok(frame) = crate::packet::decode(&payload)
            && frame.is_race_on
            && frame.car_ordinal != 0
        {
            return Some(frame.car_ordinal);
        }
    }
    None
}

/// Journal files (live and archived, matching the base's naming scheme) that
/// reference a stint by filename.
fn journals_referencing(stint_name: &str, journal_base: &str) -> Vec<String> {
    let base = Path::new(journal_base);
    let dir = match base.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let stem = base
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for entry in rd.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with(&stem) || !name.ends_with(".txt") {
                continue;
            }
            if std::fs::read_to_string(entry.path()).is_ok_and(|text| text.contains(stint_name)) {
                out.push(name);
            }
        }
    }
    out.sort();
    out
}

/// Delete one stint recording. Stricter than the read guard: only a bare
/// filename inside the stints directory (no paths), never the file the
/// recorder is writing right now, and a journaled stint (campaign evidence;
/// deleting it degrades advice) requires an explicit `force` so the frontend
/// can confirm first.
pub fn delete_stint(
    sessions_dir: &str,
    name: &str,
    active: Option<&Path>,
    force: bool,
    journal_base: &str,
) -> Result<(), ApiError> {
    if name.contains('/') || name.contains('\\') || name.contains("..") || !name.ends_with(".ftel")
    {
        return Err(ApiError::bad("bad file parameter"));
    }
    if active
        .and_then(|p| p.file_name())
        .is_some_and(|a| a.to_string_lossy() == name)
    {
        return Err(ApiError::conflict("stint is currently being recorded"));
    }
    if !force && let Some(journal) = journals_referencing(name, journal_base).first() {
        return Err(ApiError::conflict(format!(
            "journaled in {journal}; deleting drops that step's measurement"
        )));
    }
    match std::fs::remove_file(Path::new(sessions_dir).join(name)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(ApiError::not_found("no such stint"))
        }
        Err(e) => Err(ApiError::internal(e)),
    }
}

/// What deleting an archived session would remove alongside its setup
/// history: runs only its journal references (deletable), split from runs
/// another journal also cites (kept) and lines whose recording is already
/// gone.
#[derive(Serialize, Type, Debug, Clone, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SessionDeletePlan {
    pub runs: u32,
    pub mb: f64,
    pub shared: u32,
    pub missing: u32,
}

/// The archived pair for `id` plus the run split: (session file, journal
/// file, deletable run names, plan). Shared/missing runs never make the
/// deletable list, so `delete_session` can remove it blindly.
fn archived_session_parts(
    id: &str,
    session_file: &str,
    journal_base: &str,
    sessions_dir: &str,
) -> Result<(String, String, Vec<String>, SessionDeletePlan), ApiError> {
    let id = id.trim();
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(ApiError::bad("bad id"));
    }
    let session_path = crate::tuning::suffixed_path(session_file, id);
    if !Path::new(&session_path).exists() {
        return Err(ApiError::not_found("no such session"));
    }
    let journal_path = crate::tuning::suffixed_path(journal_base, id);
    let own_journal = Path::new(&journal_path)
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_default();
    let text = std::fs::read_to_string(&journal_path).unwrap_or_default();
    let names: std::collections::BTreeSet<String> = crate::analysis::journal::parse_journal(&text)
        .into_iter()
        .filter_map(|e| {
            Path::new(&e.path)
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
        })
        .collect();
    let mut deletable = Vec::new();
    let mut plan = SessionDeletePlan::default();
    let mut bytes = 0u64;
    for name in names {
        let Ok(md) = Path::new(sessions_dir).join(&name).metadata() else {
            plan.missing += 1;
            continue;
        };
        if journals_referencing(&name, journal_base)
            .iter()
            .any(|j| *j != own_journal)
        {
            plan.shared += 1;
            continue;
        }
        plan.runs += 1;
        bytes += md.len();
        deletable.push(name);
    }
    plan.mb = bytes as f64 / 1e6;
    Ok((session_path, journal_path, deletable, plan))
}

/// Preview what `delete_session` would remove, for the confirm dialog.
pub fn session_delete_plan(
    id: &str,
    session_file: &str,
    journal_base: &str,
    sessions_dir: &str,
) -> Result<SessionDeletePlan, ApiError> {
    archived_session_parts(id, session_file, journal_base, sessions_dir).map(|(.., plan)| plan)
}

/// Delete an archived session: its session file, its journal, and (when
/// `delete_runs`) the recordings only its journal references. Runs cited by
/// any other journal are always kept, as is a recording the recorder is
/// mid-writing (it stays behind as an unjournaled run). Only archived
/// sessions have an id; the active session must be archived first.
pub fn delete_session(
    id: &str,
    delete_runs: bool,
    session_file: &str,
    journal_base: &str,
    sessions_dir: &str,
    active: Option<&Path>,
) -> Result<(), ApiError> {
    let (session_path, journal_path, deletable, _) =
        archived_session_parts(id, session_file, journal_base, sessions_dir)?;
    if delete_runs {
        let active_name = active
            .and_then(|p| p.file_name())
            .map(|f| f.to_string_lossy().into_owned());
        for name in deletable {
            if active_name.as_deref() == Some(name.as_str()) {
                continue;
            }
            match std::fs::remove_file(Path::new(sessions_dir).join(&name)) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(ApiError::internal(e)),
            }
        }
    }
    if Path::new(&journal_path).exists() {
        std::fs::remove_file(&journal_path).map_err(ApiError::internal)?;
    }
    std::fs::remove_file(&session_path).map_err(ApiError::internal)
}

/// Build the stint's telemetry bundle for manual export. The
/// bundler self-verifies, so an Ok is a bundle that already round-tripped.
pub fn export_bundle(
    sessions_dir: &str,
    file: &str,
    session_file: &str,
    journal_base: &str,
) -> Result<(String, Vec<u8>), ApiError> {
    if !file.ends_with(".ftel") || file.contains('/') || file.contains('\\') {
        return Err(ApiError::bad(
            "file must be a bare .ftel name from the sessions directory",
        ));
    }
    let session = crate::tuning::TuningSession::load(Path::new(session_file));
    let journal_path = crate::tuning::journal_path_for(session.car, journal_base);
    let journal = std::fs::read_to_string(&journal_path).unwrap_or_default();
    crate::bundle::build(&Path::new(sessions_dir).join(file), &session, &journal)
        .map_err(ApiError::bad)
}

// ------------------------------------------------------------- laps, compare

/// Only relative .ftel paths with no traversal: the API exposes session
/// recordings, nothing else.
fn is_safe_session_path(file: &str) -> bool {
    file.ends_with(".ftel") && !file.contains("..") && !file.starts_with('/')
}

fn checked_session_path(file: &str) -> Result<&Path, ApiError> {
    if is_safe_session_path(file) {
        Ok(Path::new(file))
    } else {
        Err(ApiError::bad("bad or missing file parameter"))
    }
}

/// Per-lap distance-binned speed traces: the lap chart's data.
#[derive(Serialize, Type, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LapView {
    pub lap: u32,
    pub time: f32,
    pub standing: bool,
    pub speeds: Vec<f32>,
}

#[derive(Serialize, Type, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LapsView {
    pub bin_meters: f32,
    pub best_time: f32,
    /// Per-bin corroboration of the spliced ideal: true = a second lap
    /// reproduces this bin's speed within splice tolerance. Drives the confidence
    /// strip under the speed chart.
    pub corroborated: Vec<bool>,
    pub laps: Vec<LapView>,
}

pub fn laps_view(file: &str) -> Result<LapsView, ApiError> {
    let path = checked_session_path(file)?;
    let session = crate::analysis::Stint::load(path)
        .map_err(|e| ApiError::internal(format!("{}: {e}", path.display())))?;
    let profile =
        crate::analysis::profile::stint_profile(&session.frames).map_err(ApiError::internal)?;
    let laps = profile
        .laps
        .iter()
        .map(|lap| LapView {
            lap: lap.lap_number as u32 + 1,
            time: lap.time_s,
            standing: lap.standing_start,
            speeds: lap.bins[..profile.shared_bins]
                .iter()
                .map(|b| b.speed_avg)
                .collect(),
        })
        .collect();
    Ok(LapsView {
        bin_meters: crate::analysis::profile::BIN_METERS,
        best_time: profile.best_lap_time_s,
        corroborated: profile.corroboration().corroborated,
        laps,
    })
}

/// One side of an A/B comparison.
#[derive(Serialize, Type, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CompareSide {
    pub file: String,
    pub laps: u32,
    pub best: f32,
    pub ideal: f32,
    pub standing_only: bool,
}

/// A/B comparison: both composited ideal-lap speed traces plus the per-bin
/// time delta (B − A), for the overlay + segment-delta view.
#[derive(Serialize, Type, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CompareView {
    pub bin_meters: f32,
    pub a: CompareSide,
    pub b: CompareSide,
    pub speeds_a: Vec<f32>,
    pub speeds_b: Vec<f32>,
    pub times_a: Vec<f32>,
    pub delta: Vec<f32>,
    pub unequal_laps: bool,
    pub car_mismatch: bool,
}

pub fn compare_view(a: &str, b: &str) -> Result<CompareView, ApiError> {
    let a_path = checked_session_path(a)?;
    let b_path = checked_session_path(b)?;
    let profile = |path: &Path| -> Result<_, ApiError> {
        let session = crate::analysis::Stint::load(path)
            .map_err(|e| ApiError::internal(format!("{}: {e}", path.display())))?;
        crate::analysis::profile::stint_profile(&session.frames)
            .map_err(|e| ApiError::internal(format!("{}: {e}", path.display())))
    };
    let pa = profile(a_path)?;
    let pb = profile(b_path)?;
    let cmp = crate::analysis::compare::compare(&pa, &pb).map_err(ApiError::internal)?;
    let shared = cmp.bin_delta_s.len();
    let speeds = |p: &crate::analysis::profile::StintProfile| {
        p.composite.bins[..shared]
            .iter()
            .map(|bin| bin.speed_avg)
            .collect()
    };
    let side = |path: &Path, p: &crate::analysis::profile::StintProfile| CompareSide {
        file: path.display().to_string(),
        laps: p.laps.len() as u32,
        best: p.best_lap_time_s,
        ideal: p.composite.time_s,
        standing_only: p.standing_start_only,
    };
    Ok(CompareView {
        bin_meters: crate::analysis::profile::BIN_METERS,
        a: side(a_path, &pa),
        b: side(b_path, &pb),
        speeds_a: speeds(&pa),
        speeds_b: speeds(&pb),
        times_a: pa.composite.bins[..shared]
            .iter()
            .map(|b| b.time_s)
            .collect(),
        delta: cmp.bin_delta_s.clone(),
        unequal_laps: pa.laps.len() != pb.laps.len(),
        car_mismatch: cmp.car_mismatch,
    })
}

/// Full text report for one stint (the per-run report view).
pub fn report_text(file: &str) -> Result<String, ApiError> {
    let path = checked_session_path(file)?;
    crate::analysis::report::full_session_report(path).map_err(ApiError::internal)
}

// -------------------------------------------------------------------- advise

#[derive(Serialize, Type, Debug, Clone)]
#[serde(rename_all_fields = "camelCase", untagged)]
pub enum OutcomeView {
    Measured {
        word: String,
        delta_s: f32,
        unequal_laps: bool,
    },
    NotComparable {
        error: String,
    },
}

#[derive(Serialize, Type, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct StepFamilyView {
    pub area: String,
    /// Where this family's fingerprint is judged: "straights" | "entry" | "corners".
    pub channel: String,
}

#[derive(Serialize, Type, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RowAnchorView {
    pub vs_step: u32,
    pub areas: String,
    pub delta_s: f32,
    pub word: String,
    pub weak: bool,
}

#[derive(Serialize, Type, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct StepView {
    pub path: String,
    pub laps: u32,
    pub best_s: f32,
    pub ideal_s: f32,
    /// (understeer index, front slip frac, rear slip frac).
    pub balance: Option<(f32, f32, f32)>,
    pub note: Option<String>,
    /// Slider positions relative to baseline, when the note trail supports them.
    pub pos: Option<(f32, f32)>,
    pub outcome: Option<OutcomeView>,
    /// Where the time moved vs the previous step: (entry, exit, straights).
    pub split: Option<(f32, f32, f32)>,
    pub anchor: Option<RowAnchorView>,
    /// Families this step's note changed, each with its judged channel.
    pub families: Vec<StepFamilyView>,
}

#[derive(Serialize, Type, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AnchorView {
    pub vs_step: u32,
    pub areas: String,
    pub changes: String,
    pub delta_s: f32,
    pub word: String,
    pub weak: bool,
    pub reconciled: bool,
    pub split: (f32, f32, f32),
    pub effects: BTreeMap<String, f32>,
}

#[derive(Serialize, Type, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AbaView {
    pub families: String,
    pub effect_s: f32,
    pub drift_s: f32,
    pub effects: BTreeMap<String, f32>,
}

#[derive(Serialize, Type, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MeasurementView {
    pub from_step: u32,
    pub to_step: u32,
    pub desc: String,
    pub delta_s: f32,
    pub split: Option<(f32, f32, f32)>,
    pub weak: bool,
    pub direct: bool,
    pub effects: BTreeMap<String, f32>,
}

#[derive(Serialize, Type, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LandscapeView {
    pub area: String,
    pub phrase: String,
    pub key: Option<String>,
    /// (value, cumulative ideal delta s, samples), ascending by value.
    pub nodes: Vec<(f32, f32, u32)>,
    /// y = ax² + bx + c least-squares fit over the nodes (3+ nodes).
    pub fit: Option<(f32, f32, f32)>,
    pub vertex: Option<f32>,
    pub measurements: Vec<MeasurementView>,
}

#[derive(Serialize, Type, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct RecommendationView {
    pub confidence: String,
    pub area: String,
    pub suggestion: Option<String>,
    /// Machine-readable accept payload: canonical (key, value) pairs.
    pub apply: Vec<(String, String)>,
    pub advice: String,
    pub evidence: Vec<String>,
}

#[derive(Serialize, Type, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TuneFieldView {
    pub phrase: String,
    pub value: String,
    pub unit: Option<String>,
}

#[derive(Serialize, Type, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AdviseView {
    pub journal: Option<String>,
    pub advice_for: String,
    pub steps: Vec<StepView>,
    pub anchor: Option<AnchorView>,
    pub aba: Option<AbaView>,
    pub landscapes: Vec<LandscapeView>,
    pub drift_floor: Option<(u32, f32)>,
    pub effect_floor: BTreeMap<String, f32>,
    pub in_progress: Option<String>,
    pub missing: Vec<String>,
    pub recommendations: Vec<RecommendationView>,
    pub current_tune: Vec<TuneFieldView>,
}

fn measurement_view(m: &crate::advise::MeasurementView) -> MeasurementView {
    MeasurementView {
        from_step: m.from_step as u32,
        to_step: m.to_step as u32,
        desc: m.desc.clone(),
        delta_s: m.delta_s,
        split: m.split,
        weak: m.weak,
        direct: m.direct,
        effects: effects_map(&m.effects),
    }
}

pub fn advise_view(v: &crate::advise::AdviseView) -> AdviseView {
    AdviseView {
        journal: v.journal.clone(),
        advice_for: v.advice_for.clone(),
        steps: v
            .steps
            .iter()
            .map(|s| StepView {
                path: s.path.clone(),
                laps: s.laps as u32,
                best_s: s.best_s,
                ideal_s: s.ideal_s,
                balance: s.balance,
                note: s.note.clone(),
                pos: s.pos,
                outcome: s.outcome.as_ref().map(|o| match o {
                    Ok((word, delta, unequal)) => OutcomeView::Measured {
                        word: word.to_string(),
                        delta_s: *delta,
                        unequal_laps: *unequal,
                    },
                    Err(e) => OutcomeView::NotComparable { error: e.clone() },
                }),
                split: s.split,
                families: s
                    .families
                    .iter()
                    .map(|f| StepFamilyView {
                        area: f.area.to_string(),
                        channel: f.channel.to_string(),
                    })
                    .collect(),
                anchor: s.anchor.as_ref().map(|a| RowAnchorView {
                    vs_step: a.vs_step as u32,
                    areas: a.areas.clone(),
                    delta_s: a.delta_s,
                    word: a.word.to_string(),
                    weak: a.weak,
                }),
            })
            .collect(),
        anchor: v.anchor.as_ref().map(|a| AnchorView {
            vs_step: a.vs_step as u32,
            areas: a.areas.clone(),
            changes: a.changes.clone(),
            delta_s: a.delta_s,
            word: a.word.to_string(),
            weak: a.weak,
            reconciled: a.reconciled,
            split: a.split,
            effects: effects_map(&a.effects),
        }),
        aba: v.aba.as_ref().map(|a| AbaView {
            families: a.families.clone(),
            effect_s: a.effect_s,
            drift_s: a.drift_s,
            effects: effects_map(&a.effects),
        }),
        landscapes: v
            .landscapes
            .iter()
            .map(|l| LandscapeView {
                area: l.area.to_string(),
                phrase: l.phrase.clone(),
                key: l.key.clone(),
                nodes: l
                    .nodes
                    .iter()
                    .map(|(v, c, n)| (*v, *c, *n as u32))
                    .collect(),
                fit: l.fit,
                vertex: l.vertex,
                measurements: l.measurements.iter().map(measurement_view).collect(),
            })
            .collect(),
        drift_floor: v.drift_floor.map(|(n, f)| (n as u32, f)),
        effect_floor: effects_map(&v.effect_floor),
        in_progress: v.in_progress.clone(),
        missing: v.missing.clone(),
        recommendations: v
            .recommendations
            .iter()
            .map(|r| RecommendationView {
                confidence: r.confidence.label().to_string(),
                area: r.area.to_string(),
                suggestion: r.suggestion.clone(),
                apply: r.apply.clone(),
                advice: r.advice.clone(),
                evidence: r.evidence.clone(),
            })
            .collect(),
        current_tune: v
            .current_tune
            .iter()
            .map(|(phrase, value, unit)| TuneFieldView {
                phrase: phrase.clone(),
                value: value.clone(),
                unit: unit.map(str::to_string),
            })
            .collect(),
    }
}

/// Advice for the active session: resolves the session car's journal and runs
/// the shared advise engine.
pub fn advise_active(
    session_file: &str,
    journal_base: &str,
    sessions_dir: &str,
) -> Result<AdviseView, ApiError> {
    let session = crate::tuning::TuningSession::load(Path::new(session_file));
    let journal = crate::tuning::journal_path_for(session.car, journal_base);
    crate::advise::advise(&journal, Path::new(session_file), sessions_dir)
        .map(|v| advise_view(&v))
        .map_err(ApiError::internal)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Switching the session car archives the active campaign to its per-car
    /// file and restores it intact when switching back.
    #[test]
    fn car_switch_archives_and_restores_sessions() {
        let dir =
            std::env::temp_dir().join(format!("tuners-car-switch-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tune-session.txt");
        let jb = dir.join("tune-journal.txt").to_string_lossy().into_owned();

        // McLaren session with a revision on file.
        let mut s = crate::tuning::TuningSession {
            car: Some(1314),
            ..Default::default()
        };
        s.facts.insert("abs".into(), "on".into());
        s.revisions.push(crate::tuning::Revision {
            stamp: "20260721-000000".into(),
            values: [("arb_f".to_string(), "18.5".to_string())]
                .into_iter()
                .collect(),
        });
        s.save(&path).unwrap();

        let switch = |car: &str| SessionUpdate {
            reset: false,
            car: Some(car.to_string()),
            facts: Vec::new(),
        };
        // Switch to an RWD car: fresh session, McLaren archived.
        update_session(&switch("227"), &path, &jb).unwrap();
        let now = crate::tuning::TuningSession::load(&path);
        assert_eq!(now.car, Some(227));
        assert!(now.revisions.is_empty(), "fresh session for the new car");
        let archived = crate::tuning::TuningSession::load(
            crate::tuning::journal_path_for(Some(1314), &path.to_string_lossy()).as_ref(),
        );
        assert_eq!(archived.car, Some(1314));
        assert_eq!(archived.revisions.len(), 1, "campaign archived intact");

        // Switch back: the McLaren campaign is restored, revisions included.
        update_session(&switch("1314"), &path, &jb).unwrap();
        let restored = crate::tuning::TuningSession::load(&path);
        assert_eq!(restored.car, Some(1314));
        assert_eq!(restored.revisions.len(), 1);
        assert_eq!(restored.facts.get("abs").map(String::as_str), Some("on"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Effect vectors serialize as a plain JSON object keyed by field; empty
    /// stays a valid (empty) object. Replaces the old effects_json shape test.
    #[test]
    fn effects_serialize_as_object() {
        assert_eq!(
            serde_json::to_value(effects_map(&Vec::new())).unwrap(),
            serde_json::json!({})
        );
        let fx = vec![("balance", 0.05f32), ("apex_speed", -1.25f32)];
        let v = serde_json::to_value(effects_map(&fx)).unwrap();
        assert_eq!(v["apex_speed"], serde_json::json!(-1.25));
        assert_eq!(v["balance"].as_f64().unwrap(), 0.05f32 as f64);
    }

    /// The live-state payload keeps the dashboard's camelCase wire names.
    /// Replaces the old live_state_json shape test.
    #[test]
    fn live_state_serializes_with_wire_names() {
        let empty = crate::live::LiveState::default();
        let rec = crate::record::new_shared();
        let v = serde_json::to_value(live_state_view(&empty, &rec.lock().unwrap())).unwrap();
        assert_eq!(v["file"], serde_json::Value::Null);
        assert_eq!(v["ageMs"], serde_json::Value::Null);
        assert_eq!(v["frame"], serde_json::Value::Null);
        assert!(v["recorder"]["mode"].is_string());

        let state = crate::live::LiveState {
            file: Some("sessions/session-x.ftel".into()),
            latest: Some(crate::analysis::TimedFrame {
                recv_us: 0,
                frame: crate::simulate::synth_frame(2.5),
            }),
            last_data: Some(std::time::Instant::now()),
            ..Default::default()
        };
        let v = serde_json::to_value(live_state_view(&state, &rec.lock().unwrap())).unwrap();
        assert_eq!(v["file"], serde_json::json!("session-x.ftel"));
        assert_eq!(v["frame"]["raceOn"], serde_json::json!(true));
        for key in ["speedMps", "rpm", "maxRpm", "tireTempF", "currentLapS"] {
            assert!(!v["frame"][key].is_null(), "missing frame key {key}");
        }
        assert!(quality_view(None).is_none());
    }

    /// Deleting an archived session removes its pair and only the runs no
    /// other journal references; the plan previews exactly that split.
    #[test]
    fn archived_session_delete_and_plan() {
        let dir = std::env::temp_dir().join(format!("tuners-sessdel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let sessions = dir.join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        let sfile = dir.join("tune-session.txt").to_string_lossy().into_owned();
        let jbase = dir.join("tune-journal.txt").to_string_lossy().into_owned();
        let sdir = sessions.to_string_lossy().into_owned();

        // Archived pair for car 99: one run only it references, one shared
        // with car 55's live journal, one whose recording is already gone.
        let id = "99-20260727-000000";
        std::fs::write(dir.join(format!("tune-session-{id}.txt")), "car = 99\n").unwrap();
        std::fs::write(
            dir.join(format!("tune-journal-{id}.txt")),
            "# parked\nsessions/stint-a.ftel | baseline\nsessions/stint-b.ftel | front arb +1\nsessions/stint-gone.ftel | note\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("tune-journal-55.txt"),
            "sessions/stint-b.ftel | baseline\n",
        )
        .unwrap();
        std::fs::write(sessions.join("stint-a.ftel"), vec![0u8; 100]).unwrap();
        std::fs::write(sessions.join("stint-b.ftel"), b"x").unwrap();

        let plan = session_delete_plan(id, &sfile, &jbase, &sdir).unwrap();
        assert_eq!(
            plan,
            SessionDeletePlan {
                runs: 1,
                mb: 100.0 / 1e6,
                shared: 1,
                missing: 1
            }
        );
        let err = session_delete_plan("nope", &sfile, &jbase, &sdir).unwrap_err();
        assert_eq!(err.kind, ErrorKind::NotFound, "{err}");
        let err = session_delete_plan("../evil", &sfile, &jbase, &sdir).unwrap_err();
        assert_eq!(err.kind, ErrorKind::BadRequest, "{err}");

        delete_session(id, true, &sfile, &jbase, &sdir, None).unwrap();
        assert!(
            !sessions.join("stint-a.ftel").exists(),
            "exclusive run goes"
        );
        assert!(sessions.join("stint-b.ftel").exists(), "shared run stays");
        assert!(!dir.join(format!("tune-session-{id}.txt")).exists());
        assert!(!dir.join(format!("tune-journal-{id}.txt")).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Deleting a journaled stint needs an explicit force; unjournaled stints
    /// delete freely. Campaign start is the earlier of first revision and
    /// first journaled stint.
    #[test]
    fn journaled_stint_delete_requires_force() {
        let dir = std::env::temp_dir().join(format!("tuners-delguard-{}", std::process::id()));
        let sessions = dir.join("sessions");
        std::fs::create_dir_all(&sessions).unwrap();
        let jbase = dir.join("tune-journal.txt").to_string_lossy().into_owned();
        std::fs::write(
            dir.join("tune-journal-99.txt"),
            "# car\nsessions/stint-20260725-100000.ftel | baseline\n",
        )
        .unwrap();
        for f in ["stint-20260725-100000.ftel", "stint-20260725-110000.ftel"] {
            std::fs::write(sessions.join(f), b"x").unwrap();
        }
        let sdir = sessions.to_string_lossy().into_owned();

        let err = delete_stint(&sdir, "stint-20260725-100000.ftel", None, false, &jbase)
            .expect_err("journaled stint must not delete without force");
        assert_eq!(err.kind, ErrorKind::Conflict, "{err}");
        assert!(err.message.contains("tune-journal-99.txt"), "{err}");
        assert!(sessions.join("stint-20260725-100000.ftel").exists());

        delete_stint(&sdir, "stint-20260725-110000.ftel", None, false, &jbase)
            .expect("unjournaled deletes without force");

        delete_stint(&sdir, "stint-20260725-100000.ftel", None, true, &jbase)
            .expect("force overrides the guard");
        assert!(!sessions.join("stint-20260725-100000.ftel").exists());

        // Campaign start: journal baseline stint (100000) predates the first
        // revision save (100500), so the earlier stamp wins.
        let mut s = crate::tuning::TuningSession {
            car: Some(99),
            ..Default::default()
        };
        s.revisions.push(crate::tuning::Revision {
            stamp: "20260725-100500".into(),
            ..Default::default()
        });
        assert_eq!(
            campaign_start(&s, &jbase).as_deref(),
            Some("20260725-100000")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// New-session archives the active pair whole; resume swaps it back, and
    /// two campaigns for the SAME car keep separate journals throughout.
    #[test]
    fn session_new_and_resume_roundtrip_same_car() {
        let dir = std::env::temp_dir().join(format!("tuners-sessions-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let session_file = dir.join("tune-session.txt");
        let journal_base = dir.join("tune-journal.txt");
        let (sf, jb) = (
            session_file.to_string_lossy().into_owned(),
            journal_base.to_string_lossy().into_owned(),
        );

        // Campaign A: car 2793 with a name, one revision, a journal with 2 stints.
        let mut a = crate::tuning::TuningSession {
            car: Some(2793),
            ..Default::default()
        };
        a.facts.insert("name".into(), "awd aero".into());
        a.facts.insert("unit_pressure".into(), "psi".into());
        a.revisions.push(crate::tuning::Revision {
            stamp: "1".into(),
            ..Default::default()
        });
        a.save(&session_file).unwrap();
        let journal_a = crate::tuning::journal_path_for(Some(2793), &jb);
        std::fs::write(
            &journal_a,
            "# car\nsessions/a.ftel | baseline\nsessions/b.ftel | x\n",
        )
        .unwrap();

        // New session: A is archived (session + journal move together), the
        // fresh session keeps unit prefs and takes the posted name.
        let fresh = new_session(None, Some("rwd build".into()), None, &sf, &jb).unwrap();
        assert!(
            !Path::new(&journal_a).exists(),
            "journal A moved to the archive"
        );
        let parked = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .find(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("tune-journal-2793-")
            })
            .expect("archived journal");
        assert!(
            std::fs::read_to_string(parked.path())
                .unwrap()
                .contains("# parked "),
            "parked marker closes the campaign"
        );
        assert_eq!(fresh.car, None);
        assert_eq!(fresh.facts.get("name").unwrap(), "rwd build");
        assert_eq!(
            fresh.facts.get("unit_pressure").unwrap(),
            "psi",
            "unit prefs carry"
        );
        assert_eq!(fresh.revisions, 0);

        let list = sessions_view(&sf, &jb);
        let row = &list.archived[0];
        assert_eq!(row.name.as_deref(), Some("awd aero"));
        assert_eq!(row.stints, 2);
        let id = row.id.clone().expect("archived id in listing");

        // Make the fresh session campaign B on the SAME car, with its own journal.
        let mut b = crate::tuning::TuningSession::load(&session_file);
        b.car = Some(2793);
        b.save(&session_file).unwrap();
        std::fs::write(&journal_a, "# car\nsessions/c.ftel | baseline\n").unwrap();

        // Resume A: B is archived in turn, A's session AND journal come back.
        let restored = resume_session(&id, &sf, &jb).unwrap();
        assert_eq!(restored.facts.get("name").unwrap(), "awd aero");
        assert_eq!(restored.revisions, 1);
        let journal = std::fs::read_to_string(&journal_a).unwrap();
        assert!(
            journal.contains("sessions/b.ftel"),
            "campaign A journal restored: {journal}"
        );
        assert!(
            journal.contains("# resumed "),
            "resume marker floors the implicit-step scan"
        );
        let list = sessions_view(&sf, &jb);
        assert!(
            list.archived
                .iter()
                .any(|r| r.name.as_deref() == Some("rwd build") && r.stints == 1),
            "campaign B archived with its own journal"
        );

        // Bad ids are rejected, unknown ids are not found.
        assert_eq!(
            resume_session("../evil", &sf, &jb).unwrap_err().kind,
            ErrorKind::BadRequest
        );
        assert_eq!(
            resume_session("none-19700101-000000", &sf, &jb)
                .unwrap_err()
                .kind,
            ErrorKind::NotFound
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Accepting suggestions saves PARTIAL tunes: posted keys merge onto the
    /// latest revision, and multiple accepts before the next stint net into
    /// ONE journal note diffed against the last driven revision.
    #[test]
    fn partial_saves_merge_and_net_into_one_note() {
        let dir = std::env::temp_dir().join(format!("tuners-partial-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tune-session.txt");
        let mut s = crate::tuning::TuningSession {
            car: Some(2793),
            ..Default::default()
        };
        s.revisions.push(crate::tuning::Revision {
            stamp: "20260724-000000".into(),
            values: [
                ("arb_f".to_string(), "18.3".to_string()),
                ("final_drive".to_string(), "3.95".to_string()),
                ("rebound_f".to_string(), "10.6".to_string()),
            ]
            .into_iter()
            .collect(),
        });
        s.save(&path).unwrap();
        let recorder = crate::record::new_shared();
        let post = |pairs: &[(&str, &str)], recorder: &crate::record::SharedRecorder| {
            let values: Vec<(String, String)> = pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            save_tune(&values, true, &path, recorder)
        };

        // Accept #1: front arb only. Unposted keys carry over from the latest.
        let out = post(&[("arb_f", "16.8")], &recorder).unwrap();
        assert!(
            out.note.as_deref().unwrap().contains("front arb -1.5"),
            "{out:?}"
        );
        let latest_vals = |p: &Path| {
            crate::tuning::TuningSession::load(p)
                .latest()
                .unwrap()
                .values
                .clone()
        };
        let vals = latest_vals(&path);
        assert_eq!(vals.get("arb_f").unwrap(), "16.8");
        assert_eq!(
            vals.get("final_drive").unwrap(),
            "3.95",
            "unposted keys carry over"
        );
        assert_eq!(vals.get("rebound_f").unwrap(), "10.6");

        // Accept #2 before any stint: chains onto #1 and the pending note nets
        // BOTH changes against the driven baseline.
        post(&[("final_drive", "4.1")], &recorder).unwrap();
        let note = recorder.lock().unwrap().pending_note.clone().unwrap();
        assert!(
            note.contains("front arb -1.5") && note.contains("final drive +0.15"),
            "{note}"
        );
        let vals = latest_vals(&path);
        assert_eq!(
            vals.get("arb_f").unwrap(),
            "16.8",
            "accept #2 chains onto #1"
        );
        assert_eq!(vals.get("final_drive").unwrap(), "4.1");

        // Accepting the original arb back nets the chain to one remaining change.
        post(&[("arb_f", "18.3")], &recorder).unwrap();
        let note = recorder.lock().unwrap().pending_note.clone().unwrap();
        assert!(
            note.contains("final drive") && !note.contains("front arb"),
            "{note}"
        );

        // A partial save with no tune on file is rejected.
        let empty = dir.join("empty-session.txt");
        let err = post_at(&empty, &recorder).unwrap_err();
        assert_eq!(err.kind, ErrorKind::BadRequest);

        let _ = std::fs::remove_dir_all(&dir);
    }

    fn post_at(
        path: &Path,
        recorder: &crate::record::SharedRecorder,
    ) -> Result<TuneSaveView, ApiError> {
        save_tune(
            &[("arb_f".to_string(), "16.8".to_string())],
            true,
            path,
            recorder,
        )
    }

    #[test]
    fn delete_stint_guards_and_deletes() {
        let dir = std::env::temp_dir().join(format!("tuners-del-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dir_s = dir.to_string_lossy().into_owned();
        std::fs::write(dir.join("stint-x.ftel"), b"data").unwrap();

        let jb = dir.join("tune-journal.txt").to_string_lossy().into_owned();
        for bad in ["../stint-x.ftel", "sub/stint-x.ftel", "stint-x.txt"] {
            let err = delete_stint(&dir_s, bad, None, false, &jb).unwrap_err();
            assert_eq!(err.kind, ErrorKind::BadRequest, "{bad}");
        }
        let err = delete_stint(
            &dir_s,
            "stint-x.ftel",
            Some(dir.join("stint-x.ftel").as_path()),
            false,
            &jb,
        )
        .unwrap_err();
        assert_eq!(
            err.kind,
            ErrorKind::Conflict,
            "active recording is protected"
        );
        assert!(dir.join("stint-x.ftel").exists());

        delete_stint(&dir_s, "stint-x.ftel", None, false, &jb).unwrap();
        assert!(!dir.join("stint-x.ftel").exists());

        let err = delete_stint(&dir_s, "stint-x.ftel", None, false, &jb).unwrap_err();
        assert_eq!(err.kind, ErrorKind::NotFound);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The read guard on stint file arguments: only relative .ftel paths with
    /// no traversal reach the filesystem.
    #[test]
    fn file_args_reject_unsafe_paths() {
        for bad in ["../../etc/passwd", "/etc/passwd", "Cargo.toml", ""] {
            assert_eq!(
                report_text(bad).unwrap_err().kind,
                ErrorKind::BadRequest,
                "report {bad}"
            );
            assert_eq!(
                laps_view(bad).unwrap_err().kind,
                ErrorKind::BadRequest,
                "laps {bad}"
            );
            assert_eq!(
                compare_view(bad, "fixtures/rivals-lap-boundary-01.ftel")
                    .unwrap_err()
                    .kind,
                ErrorKind::BadRequest,
                "compare {bad}"
            );
        }
        // A safe relative fixture path passes the guard (whatever the load
        // outcome under the current cwd, it is never rejected as unsafe).
        if let Err(e) = report_text("fixtures/rivals-lap-boundary-01.ftel") {
            assert_ne!(e.kind, ErrorKind::BadRequest);
        }
    }

    /// Fixture-driven: the committed fixtures are short race-on segments
    /// with no completed lap, so the laps view must fail with the profile
    /// error (Internal, since the decode ran), never a guard rejection. When a
    /// real session library is present (dev machine; gitignored elsewhere),
    /// the full chart geometry is asserted end to end.
    #[test]
    fn laps_view_over_fixture() {
        let err = laps_view("fixtures/real-01.ftel").expect_err("no completed laps");
        assert_eq!(err.kind, ErrorKind::Internal);
        assert!(err.message.contains("laps"), "{err}");

        let newest = std::fs::read_dir("sessions").ok().map(|rd| {
            let mut paths: Vec<_> = rd
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "ftel"))
                .collect();
            paths.sort();
            paths
        });
        let Some(mut paths) = newest else { return };
        // Auto-cut stints can be short (no completed lap): use the newest
        // one that profiles.
        let Some(v) = paths
            .iter()
            .rev()
            .find_map(|p| laps_view(&p.to_string_lossy()).ok())
        else {
            return;
        };
        paths.clear();
        assert!(v.bin_meters > 0.0 && v.best_time > 0.0);
        assert!(!v.laps.is_empty());
        let bins = v.laps[0].speeds.len();
        assert!(bins > 0, "shared bins present");
        assert!(v.laps.iter().all(|l| l.speeds.len() == bins));
        assert_eq!(v.corroborated.len(), bins, "strip aligns with the bins");
        let j = serde_json::to_value(&v).unwrap();
        assert!(j["binMeters"].is_number() && j["bestTime"].is_number());
    }

    #[test]
    fn stint_list_empty_when_dir_missing() {
        assert!(stint_rows("no-such-dir").is_empty());
    }
}
