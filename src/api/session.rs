//! Session lifecycle and tune entry: setup facts, revisions, the
//! pending-note chain, archive/resume, and the sessions list.

use super::*;

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
    /// Baseline form seed copied from a source project at duplication;
    /// present only until the first tune save.
    pub prefill: Option<BTreeMap<String, String>>,
}

pub fn session_view(s: &crate::advice::tuning::TuningSession, journal_base: &str) -> SessionView {
    SessionView {
        car: s.car,
        car_name: s.car.and_then(crate::cars::car_name).map(str::to_string),
        facts: s.facts.clone(),
        revisions: s.revisions.len() as u32,
        latest: s.latest().map(|rev| rev.values.clone()),
        prefill: s.prefill.clone().filter(|p| !p.is_empty()),
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
pub(super) fn campaign_start(
    s: &crate::advice::tuning::TuningSession,
    journal_base: &str,
) -> Option<String> {
    let mut start = s.revisions.first().map(|r| r.stamp.clone());
    let jpath = crate::advice::tuning::journal_path_for(s.car, journal_base);
    if let Ok(text) = std::fs::read_to_string(&jpath)
        && let Some(first) = crate::advice::journal::parse_journal(&text).first()
        && let Some(stamp) = crate::advice::advise::stint_stamp(&first.path)
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
    let current = crate::advice::tuning::TuningSession::load(path);
    let mut s = if req.reset {
        crate::advice::tuning::TuningSession::default()
    } else if posted_car.is_some() && current.car.is_some() && posted_car != current.car {
        // Switching cars = switching SESSIONS: archive the active session to
        // its per-car file (tune-session-<ordinal>.txt, same scheme as
        // journals) and resume the new car's archived session if one exists.
        // Nothing is lost; switching back restores the whole campaign.
        let base = path.to_string_lossy();
        let archive = crate::advice::tuning::journal_path_for(current.car, &base);
        let _ = current.save(archive.as_ref());
        let resumed = crate::advice::tuning::TuningSession::load(
            crate::advice::tuning::journal_path_for(posted_car, &base).as_ref(),
        );
        if resumed.car == posted_car {
            resumed
        } else {
            crate::advice::tuning::TuningSession::default()
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
pub fn split_and_drop_pending(recorder: &crate::telemetry::record::SharedRecorder) {
    let mut r = recorder.lock().unwrap();
    r.split_requested = true;
    r.pending_note = None;
    r.pending_base_rev = None;
}

/// Manual stint cut (the dashboard's "new session" button), with an optional
/// note to journal against the next stint.
pub fn request_split(recorder: &crate::telemetry::record::SharedRecorder, note: Option<String>) {
    let mut r = recorder.lock().unwrap();
    r.split_requested = true;
    r.pending_note = note.filter(|n| !n.trim().is_empty());
}

/// Archiving a blank session would save nothing worth resuming: no car, no
/// revisions, and no facts beyond display prefs.
fn session_is_blank(s: &crate::advice::tuning::TuningSession) -> bool {
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
    s: &crate::advice::tuning::TuningSession,
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
    while Path::new(&crate::advice::tuning::suffixed_path(session_file, &id)).exists()
        || Path::new(&crate::advice::tuning::suffixed_path(journal_base, &id)).exists()
    {
        id = format!("{base_id}-{n}");
        n += 1;
    }
    s.save(crate::advice::tuning::suffixed_path(session_file, &id).as_ref())?;
    let live = crate::advice::tuning::journal_path_for(s.car, journal_base);
    if Path::new(&live).exists() {
        let parked = crate::advice::tuning::suffixed_path(journal_base, &id);
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

pub(super) fn append_line(path: &str, line: &str) -> std::io::Result<()> {
    use std::io::{Read as _, Seek as _, SeekFrom, Write as _};
    let mut f = std::fs::OpenOptions::new()
        .read(true)
        .append(true)
        .open(path)?;
    // A file whose last write lacked a trailing newline (e.g. a hand-edited
    // journal) must not have the line glued onto its final one — a parked
    // boundary marker swallowed into a note is silently lost to the parser.
    if f.seek(SeekFrom::End(0))? > 0 {
        f.seek(SeekFrom::End(-1))?;
        let mut last = [0u8; 1];
        f.read_exact(&mut last)?;
        if last[0] != b'\n' {
            writeln!(f)?;
        }
    }
    writeln!(f, "{line}")
}

/// A new campaign for `car` appends its first note to the car's UNSUFFIXED
/// journal. If one already exists that the archive step won't free (the
/// active session is a different car, or blank), it belongs to a campaign
/// parked by the legacy car-switch scheme — appending would leak stints
/// across campaigns. Same guard resume_session applies to its rename.
fn parked_journal_conflict(
    car: Option<i32>,
    current: &crate::advice::tuning::TuningSession,
    journal_base: &str,
) -> Result<(), ApiError> {
    let Some(car) = car else { return Ok(()) };
    let target = crate::advice::tuning::journal_path_for(Some(car), journal_base);
    let frees_target = !session_is_blank(current) && current.car == Some(car);
    if Path::new(&target).exists() && !frees_target {
        return Err(ApiError::conflict(format!(
            "{target} already exists: another session for this car is parked; resume it first"
        )));
    }
    Ok(())
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
    let current = crate::advice::tuning::TuningSession::load(session_file.as_ref());
    let posted_car: Option<i32> = car.as_deref().and_then(|v| v.trim().parse().ok());
    parked_journal_conflict(posted_car, &current, journal_base)?;
    if !session_is_blank(&current) {
        archive_active(&current, session_file, journal_base).map_err(ApiError::internal)?;
    }
    let mut fresh = crate::advice::tuning::TuningSession::default();
    for (k, v) in &current.facts {
        if k.starts_with("unit_") {
            fresh.facts.insert(k.clone(), v.clone());
        }
    }
    fresh.car = posted_car;
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

/// New project seeded from an existing one (same-car new-circuit or exemplar
/// work): copies SETUP STATE — car, facts, limits, and the latest tune as a
/// `[prefill]` form seed — never campaign history (no journal, no revisions,
/// no measurements). The first save after duplication is still the explicit
/// baseline, so pre-drive tweaks never journal as steps. `source_id: None`
/// duplicates the active session; `Some(id)` a stamped archive.
pub fn duplicate_session(
    source_id: Option<String>,
    name: Option<String>,
    description: Option<String>,
    session_file: &str,
    journal_base: &str,
) -> Result<SessionView, ApiError> {
    let current = crate::advice::tuning::TuningSession::load(session_file.as_ref());
    let source = match source_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        None => current.clone(),
        Some(id) => {
            if !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                return Err(ApiError::bad("bad id"));
            }
            let path = crate::advice::tuning::suffixed_path(session_file, id);
            if !Path::new(&path).exists() {
                return Err(ApiError::not_found("no such session"));
            }
            crate::advice::tuning::TuningSession::load(path.as_ref())
        }
    };
    // Car is NOT changeable on a duplicate: limits and weight facts are
    // car-specific, so cross-car duplication would copy lies.
    if source.car.is_none() {
        return Err(ApiError::bad("source session has no car"));
    }
    parked_journal_conflict(source.car, &current, journal_base)?;
    if !session_is_blank(&current) {
        archive_active(&current, session_file, journal_base).map_err(ApiError::internal)?;
    }
    let mut fresh = crate::advice::tuning::TuningSession {
        car: source.car,
        facts: source.facts.clone(),
        prefill: source
            .latest()
            .map(|rev| rev.values.clone())
            .filter(|v| !v.is_empty()),
        revisions: Vec::new(),
    };
    for (k, v) in [("name", name), ("description", description)] {
        match v.as_deref().map(str::trim).filter(|v| !v.is_empty()) {
            Some(v) => {
                fresh.facts.insert(k.to_string(), v.to_string());
            }
            None => {
                fresh.facts.remove(k);
            }
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
    let archived_session = crate::advice::tuning::suffixed_path(session_file, id);
    if !Path::new(&archived_session).exists() {
        return Err(ApiError::not_found("no such session"));
    }
    let restored = crate::advice::tuning::TuningSession::load(archived_session.as_ref());
    let current = crate::advice::tuning::TuningSession::load(session_file.as_ref());
    // The restored journal must not clobber a live one. The archive step below
    // frees the live journal only when the active session is the same car;
    // otherwise a journal already at the target belongs to a THIRD campaign
    // (parked by the legacy car-switch scheme) and moving over it would lose it.
    let target = crate::advice::tuning::journal_path_for(restored.car, journal_base);
    let src = crate::advice::tuning::suffixed_path(journal_base, id);
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
    let row =
        |id: Option<&str>, s: &crate::advice::tuning::TuningSession, stints: usize| SessionRow {
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
            let s = crate::advice::tuning::TuningSession::load(&entry.path());
            let stints = journal_stints(&crate::advice::tuning::suffixed_path(journal_base, id));
            let modified = entry
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            archived.push((modified, row(Some(id), &s, stints)));
        }
    }
    archived.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    let active = crate::advice::tuning::TuningSession::load(path);
    let active_stints = journal_stints(&crate::advice::tuning::journal_path_for(
        active.car,
        journal_base,
    ));
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
    recorder: &crate::telemetry::record::SharedRecorder,
) -> Result<TuneSaveView, ApiError> {
    let mut s = crate::advice::tuning::TuningSession::load(path);
    // The duplication prefill has done its job once any tune is saved: the
    // first save IS the baseline the prefill existed to seed.
    s.prefill = None;
    let mut rev = crate::advice::tuning::Revision {
        stamp: crate::util::utc_stamp(now_secs()),
        ..Default::default()
    };
    for (k, v) in values {
        let v = v.trim();
        if !v.is_empty()
            && crate::advice::tuning::FIELDS
                .iter()
                .any(|(key, _)| key == k)
        {
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
        .map(|base| crate::advice::tuning::diff_note(base, &rev))
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
    recorder: &crate::telemetry::record::SharedRecorder,
) -> Option<PendingView> {
    let (note, base_idx) = {
        let r = recorder.lock().unwrap();
        (r.pending_note.clone()?, r.pending_base_rev)
    };
    let s = crate::advice::tuning::TuningSession::load(session_path);
    let latest = s.latest()?.values.clone();
    let base = base_idx.and_then(|i| s.revisions.get(i)).map(|r| &r.values);
    let phrase = |k: &str| {
        crate::advice::tuning::FIELDS
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
