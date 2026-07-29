//! Stint recordings on disk: listing, car probe, delete/export, and the
//! session-file path guards.

use super::*;

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
    let mut reader = crate::telemetry::stint::StintReader::open(path).ok()?;
    for _ in 0..20_000 {
        let (_, payload) = reader.next_packet().ok()??;
        if let Ok(frame) = crate::telemetry::packet::decode(&payload)
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
    let session_path = crate::advice::tuning::suffixed_path(session_file, id);
    if !Path::new(&session_path).exists() {
        return Err(ApiError::not_found("no such session"));
    }
    let journal_path = crate::advice::tuning::suffixed_path(journal_base, id);
    let own_journal = Path::new(&journal_path)
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_default();
    let text = std::fs::read_to_string(&journal_path).unwrap_or_default();
    let names: std::collections::BTreeSet<String> = crate::advice::journal::parse_journal(&text)
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
    let session = crate::advice::tuning::TuningSession::load(Path::new(session_file));
    let journal_path = crate::advice::tuning::journal_path_for(session.car, journal_base);
    let journal = std::fs::read_to_string(&journal_path).unwrap_or_default();
    crate::sharing::bundle::build(&Path::new(sessions_dir).join(file), &session, &journal)
        .map_err(ApiError::bad)
}

// ------------------------------------------------------------- laps, compare

/// Only relative .ftel paths with no traversal: the API exposes session
/// recordings, nothing else.
pub(super) fn is_safe_session_path(file: &str) -> bool {
    file.ends_with(".ftel") && !file.contains("..") && !file.starts_with('/')
}

pub(super) fn checked_session_path(file: &str) -> Result<&Path, ApiError> {
    if is_safe_session_path(file) {
        Ok(Path::new(file))
    } else {
        Err(ApiError::bad("bad or missing file parameter"))
    }
}
