//! Telemetry-sharing consent, the outbox, and historic backfill.

use super::*;

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
    let cfg = crate::sharing::collect::CollectConfig::load(config);
    let rejected = std::fs::read_dir(outbox.join("rejected"))
        .map(|rd| rd.flatten().count())
        .unwrap_or(0);
    SharingView {
        enabled: cfg.enabled,
        endpoint: cfg.endpoint.clone(),
        sender: (!cfg.token.is_empty()).then(|| crate::sharing::collect::sender_id(&cfg.token)),
        queued: crate::sharing::collect::queued(outbox).len() as u32,
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
    let mut cfg = crate::sharing::collect::CollectConfig::load(config);
    if enabled {
        cfg.enabled = true;
        if cfg.token.len() != 64 {
            cfg.token = crate::sharing::collect::generate_token();
        }
        if let Some(e) = endpoint.as_deref().map(str::trim).filter(|e| !e.is_empty()) {
            cfg.endpoint = e.to_string();
        }
        if cfg.endpoint.is_empty() {
            cfg.endpoint = crate::sharing::collect::DEFAULT_ENDPOINT.to_string();
        }
    } else {
        cfg.enabled = false;
        if discard {
            for p in crate::sharing::collect::queued(outbox) {
                let _ = std::fs::remove_file(p);
            }
        }
    }
    cfg.save(config).map_err(ApiError::internal)?;
    Ok(sharing_view(config, outbox))
}

/// Crowd-prior fetch preference plus stored-artifact status. Deliberately
/// independent of the sharing consent above: receiving the crowd's priors
/// must not require contributing.
#[derive(Serialize, Type, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PriorsView {
    pub enabled: bool,
    pub endpoint: String,
    /// Generation stamp of the stored artifact, when one is on disk.
    pub generated: Option<String>,
    /// Unix ms of the stored artifact's last write.
    pub updated_ms: Option<f64>,
}

pub fn priors_view() -> PriorsView {
    use crate::advice::priors;
    let cfg = priors::FetchConfig::load(&crate::util::data_path(priors::CONFIG_FILE));
    let updated_ms = std::fs::metadata(crate::util::data_path(priors::ARTIFACT_FILE))
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as f64);
    PriorsView {
        enabled: cfg.enabled,
        endpoint: cfg.endpoint,
        generated: priors::load().map(|p| p.generated),
        updated_ms,
    }
}

/// Toggle the crowd-prior fetch. Disabling also removes the stored
/// artifact: advice stops drawing on crowd data immediately, not when the
/// file happens to age out.
pub fn set_priors(enabled: bool) -> Result<PriorsView, ApiError> {
    use crate::advice::priors;
    let path = crate::util::data_path(priors::CONFIG_FILE);
    let mut cfg = priors::FetchConfig::load(&path);
    cfg.enabled = enabled;
    cfg.save(&path).map_err(ApiError::internal)?;
    if !enabled {
        let _ = std::fs::remove_file(crate::util::data_path(priors::ARTIFACT_FILE));
        let _ = std::fs::remove_file(crate::util::data_path(priors::ETAG_FILE));
    }
    Ok(priors_view())
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
    let p = crate::sharing::collect::history_plan(root, sessions_dir, outbox);
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
    let cfg = crate::sharing::collect::CollectConfig::load(config);
    if !cfg.ready() {
        return Err(ApiError {
            kind: ErrorKind::Forbidden,
            message: "turn on telemetry sharing first".into(),
        });
    }
    let plan = crate::sharing::collect::history_plan(root, sessions_dir, outbox);
    let n = plan.items.len() as u32;
    let outbox = outbox.to_path_buf();
    std::thread::spawn(move || {
        crate::sharing::collect::history_enqueue(plan, &outbox);
    });
    Ok(n)
}

// ------------------------------------------------------------------ sessions
