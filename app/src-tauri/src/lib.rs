//! Tauri shell for the tuners desktop app: typed commands over the
//! engine's api module, TS bindings via tauri-specta, and the recorder/live/
//! drainer lifecycle that `tuners serve` used to own.

use std::path::{Path, PathBuf};
use std::time::Duration;

use tauri::Manager as _;
use tauri_specta::Event as _;
use tuners::api;

/// Resolved data-root paths plus the shared live/recorder state every
/// command reads.
pub struct AppState {
    root: PathBuf,
    sessions_dir: String,
    session_file: String,
    journal_base: String,
    config: PathBuf,
    outbox: PathBuf,
    recorder: tuners::telemetry::record::SharedRecorder,
}

type S<'a> = tauri::State<'a, AppState>;

// ------------------------------------------------------------------- events

/// ~4x/s snapshot of live telemetry + recorder status (was the SSE `state`
/// event).
#[derive(Clone, serde::Serialize, specta::Type, tauri_specta::Event)]
pub struct LiveStateEvent(pub api::LiveStateView);

/// Data-quality summary, emitted only when it changes; None until a
/// comparable lap exists.
#[derive(Clone, serde::Serialize, specta::Type, tauri_specta::Event)]
pub struct QualityEvent(pub Option<api::QualityView>);

/// A recording closed (the run auto-cut or the recorder stopped): refresh
/// verdicts. In external mode (another capture owns the socket) the close is
/// inferred from the tailed newest file: rotation, or the tail going stale.
#[derive(Clone, serde::Serialize, specta::Type, tauri_specta::Event)]
pub struct RunFinishedEvent {
    pub file: String,
}

/// The newest recording rotated (covers external capture too): refresh run
/// lists.
#[derive(Clone, serde::Serialize, specta::Type, tauri_specta::Event)]
pub struct RunsChangedEvent {
    pub file: Option<String>,
}

// ----------------------------------------------------------------- commands
//
// Plain #[tauri::command] runs on the MAIN thread: a slow body freezes the
// whole window (Windows "Not Responding"). Every command that touches the
// filesystem, the recorder, or the network is marked (async) so it executes
// on the runtime pool instead — the window never waits; the wire signature
// is identical either way. Only pure in-memory lookups stay sync.

#[tauri::command(async)]
#[specta::specta]
fn stints(state: S) -> Vec<api::StintRow> {
    api::stint_rows(&state.sessions_dir)
}

#[tauri::command(async)]
#[specta::specta]
fn laps(file: String) -> Result<api::LapsView, api::ApiError> {
    api::laps_view(&file)
}

#[tauri::command(async)]
#[specta::specta]
fn compare(a: String, b: String) -> Result<api::CompareView, api::ApiError> {
    api::compare_view(&a, &b)
}

#[tauri::command(async)]
#[specta::specta]
fn report(file: String) -> Result<String, api::ApiError> {
    api::report_text(&file)
}

#[tauri::command(async)]
#[specta::specta]
fn session(state: S) -> api::SessionView {
    let s = tuners::advice::tuning::TuningSession::load(state.session_file.as_ref());
    api::session_view(&s, &state.journal_base)
}

#[tauri::command(async)]
#[specta::specta]
fn update_session(
    state: S,
    reset: bool,
    car: Option<String>,
    facts: Vec<(String, String)>,
) -> Result<api::SessionView, api::ApiError> {
    let req = api::SessionUpdate { reset, car, facts };
    api::update_session(&req, state.session_file.as_ref(), &state.journal_base)
}

#[tauri::command(async)]
#[specta::specta]
fn save_tune(
    state: S,
    values: Vec<(String, String)>,
    partial: bool,
) -> Result<api::TuneSaveView, api::ApiError> {
    api::save_tune(
        &values,
        partial,
        state.session_file.as_ref(),
        &state.recorder,
    )
}

#[tauri::command(async)]
#[specta::specta]
fn sessions(state: S) -> api::SessionsView {
    api::sessions_view(&state.session_file, &state.journal_base)
}

#[tauri::command(async)]
#[specta::specta]
fn new_session(
    state: S,
    car: Option<String>,
    name: Option<String>,
    description: Option<String>,
) -> Result<api::SessionView, api::ApiError> {
    let out = api::new_session(
        car,
        name,
        description,
        &state.session_file,
        &state.journal_base,
    )?;
    api::split_and_drop_pending(&state.recorder);
    Ok(out)
}

#[tauri::command(async)]
#[specta::specta]
fn resume_session(state: S, id: String) -> Result<api::SessionView, api::ApiError> {
    let out = api::resume_session(&id, &state.session_file, &state.journal_base)?;
    api::split_and_drop_pending(&state.recorder);
    Ok(out)
}

#[tauri::command(async)]
#[specta::specta]
fn duplicate_session(
    state: S,
    source_id: Option<String>,
    name: Option<String>,
    description: Option<String>,
) -> Result<api::SessionView, api::ApiError> {
    let out = api::duplicate_session(
        source_id,
        name,
        description,
        &state.session_file,
        &state.journal_base,
    )?;
    api::split_and_drop_pending(&state.recorder);
    Ok(out)
}

#[tauri::command(async)]
#[specta::specta]
fn advise(state: S) -> Result<api::AdviseView, api::ApiError> {
    api::advise_active(
        &state.session_file,
        &state.journal_base,
        &state.sessions_dir,
    )
}

#[tauri::command(async)]
#[specta::specta]
fn pending(state: S) -> Option<api::PendingView> {
    api::pending_view(state.session_file.as_ref(), &state.recorder)
}

#[tauri::command]
#[specta::specta]
fn effect_fields() -> Vec<api::EffectFieldView> {
    api::effect_fields()
}

#[tauri::command]
#[specta::specta]
fn car_list() -> Vec<api::CarView> {
    api::car_list()
}

#[tauri::command(async)]
#[specta::specta]
fn effect_map_status() -> Option<api::EffectMapStatus> {
    api::effect_map_status()
}

#[tauri::command(async)]
#[specta::specta]
fn sharing(state: S) -> api::SharingView {
    api::sharing_view(&state.config, &state.outbox)
}

#[tauri::command(async)]
#[specta::specta]
fn set_sharing(
    state: S,
    enabled: bool,
    endpoint: Option<String>,
    discard: bool,
) -> Result<api::SharingView, api::ApiError> {
    api::set_sharing(&state.config, &state.outbox, enabled, endpoint, discard)
}

#[tauri::command(async)]
#[specta::specta]
fn sharing_history_plan(state: S) -> api::HistoryPlanView {
    api::history_plan_view(&state.root, &state.sessions_dir, &state.outbox)
}

#[tauri::command(async)]
#[specta::specta]
fn share_history(state: S) -> Result<u32, api::ApiError> {
    api::share_history(
        &state.root,
        &state.sessions_dir,
        &state.outbox,
        &state.config,
    )
}

#[tauri::command(async)]
#[specta::specta]
fn session_delete_plan(state: S, id: String) -> Result<api::SessionDeletePlan, api::ApiError> {
    api::session_delete_plan(
        &id,
        &state.session_file,
        &state.journal_base,
        &state.sessions_dir,
    )
}

#[tauri::command(async)]
#[specta::specta]
fn delete_session(state: S, id: String, delete_runs: bool) -> Result<(), api::ApiError> {
    let active = state.recorder.lock().unwrap().file.clone();
    api::delete_session(
        &id,
        delete_runs,
        &state.session_file,
        &state.journal_base,
        &state.sessions_dir,
        active.as_deref(),
    )
}

#[tauri::command(async)]
#[specta::specta]
fn delete_stint(state: S, file: String, force: bool) -> Result<(), api::ApiError> {
    let active = state.recorder.lock().unwrap().file.clone();
    api::delete_stint(
        &state.sessions_dir,
        &file,
        active.as_deref(),
        force,
        &state.journal_base,
    )
}

#[tauri::command(async)]
#[specta::specta]
fn record_split(state: S, note: Option<String>) {
    api::request_split(&state.recorder, note);
}

/// Build the stint's bundle and write it to `dest` (picked via the native
/// save dialog frontend-side). Returns the bundle's canonical name.
#[tauri::command(async)]
#[specta::specta]
fn export_stint(state: S, file: String, dest: String) -> Result<String, api::ApiError> {
    let (name, bytes) = api::export_bundle(
        &state.sessions_dir,
        &file,
        &state.session_file,
        &state.journal_base,
    )?;
    std::fs::write(&dest, &bytes).map_err(|e| api::ApiError {
        kind: api::ErrorKind::Internal,
        message: e.to_string(),
    })?;
    Ok(name)
}

/// Best-guess LAN IP for the Data Out hookup screen: a connected UDP socket
/// reveals which local address routes outward, without sending a packet.
/// None when offline or loopback-only; the wizard then shows 127.0.0.1 alone.
#[tauri::command(async)]
#[specta::specta]
fn lan_ip() -> Option<String> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("8.8.8.8:53").ok()?;
    let ip = sock.local_addr().ok()?.ip();
    (!ip.is_loopback() && !ip.is_unspecified()).then(|| ip.to_string())
}

/// True when running inside a flatpak sandbox, where the tauri updater
/// cannot replace the app (/app is immutable; updates come via flatpak).
/// The frontend skips the startup update check when this is set.
#[tauri::command(async)]
#[specta::specta]
fn in_flatpak() -> bool {
    std::env::var_os("FLATPAK_ID").is_some() || std::path::Path::new("/.flatpak-info").exists()
}

// -------------------------------------------------------------------- shell

pub fn specta_builder() -> tauri_specta::Builder<tauri::Wry> {
    tauri_specta::Builder::<tauri::Wry>::new()
        .commands(tauri_specta::collect_commands![
            stints,
            laps,
            compare,
            report,
            session,
            update_session,
            save_tune,
            sessions,
            new_session,
            resume_session,
            duplicate_session,
            advise,
            sharing,
            set_sharing,
            sharing_history_plan,
            share_history,
            delete_stint,
            session_delete_plan,
            delete_session,
            record_split,
            export_stint,
            effect_fields,
            effect_map_status,
            car_list,
            pending,
            lan_ip,
            in_flatpak,
        ])
        .events(tauri_specta::collect_events![
            LiveStateEvent,
            QualityEvent,
            RunFinishedEvent,
            RunsChangedEvent,
        ])
}

/// Tail age at which an externally captured run is assumed cut: the cutter
/// closes 5s after race-off. A race-on gap (pause) can fire this early with
/// the run still open, which advise already tolerates as in-progress; the
/// latch re-arms when data resumes, so the true close notifies again.
const EXTERNAL_STALE_MS: u32 = 10_000;

/// Edge detection for run boundaries, one `observe` per emitter poll.
///
/// When the in-process recorder is the writer, its recording-file edge is the
/// exact close signal. In external mode nothing is ever "recording"
/// in-process, so a close is inferred from the tailed newest file instead:
/// rotation (a new newest means the previous one was cut) or the tail going
/// stale (run over, next not started). The latch keeps the two inferences
/// from notifying the same file twice.
#[derive(Default)]
struct RunEdges {
    prev_recording: Option<String>,
    prev_newest: Option<String>,
    stale_notified: bool,
}

impl RunEdges {
    /// Returns (run that finished, whether the newest file rotated).
    fn observe(
        &mut self,
        mode: &str,
        recorder_file: Option<&str>,
        newest: Option<&str>,
        age_ms: Option<u32>,
    ) -> (Option<String>, bool) {
        let mut finished = None;
        let recording = (mode == "recording")
            .then(|| recorder_file.map(str::to_string))
            .flatten();
        if let Some(prev) = self.prev_recording.take()
            && recording.as_deref() != Some(prev.as_str())
        {
            finished = Some(prev);
        }
        self.prev_recording = recording;
        let mut rotated = false;
        if newest != self.prev_newest.as_deref() {
            rotated = true;
            if mode == "external"
                && !self.stale_notified
                && let Some(prev) = self.prev_newest.clone()
            {
                finished = Some(prev);
            }
            self.stale_notified = false;
            self.prev_newest = newest.map(str::to_string);
        } else if mode == "external"
            && let (Some(age), Some(f)) = (age_ms, newest)
        {
            if age < EXTERNAL_STALE_MS {
                self.stale_notified = false;
            } else if !self.stale_notified {
                self.stale_notified = true;
                finished = Some(f.to_string());
            }
        }
        (finished, rotated)
    }
}

/// Emits live-state/quality/run-finished/runs-changed by polling the shared
/// state the tailer and recorder threads maintain: the typed replacement
/// for the SSE loop, plus edge detection for run boundaries (RunEdges).
fn run_emitter(
    app: tauri::AppHandle,
    live: tuners::telemetry::live::SharedLive,
    recorder: tuners::telemetry::record::SharedRecorder,
) {
    let mut sent_quality_seq = u64::MAX;
    let mut edges = RunEdges::default();
    loop {
        let (state_view, quality) = {
            let s = live.lock().unwrap();
            let r = recorder.lock().unwrap();
            let quality = (s.quality_seq != sent_quality_seq)
                .then(|| (s.quality_seq, api::quality_view(s.quality.as_ref())));
            (api::live_state_view(&s, &r), quality)
        };
        let (finished, rotated) = edges.observe(
            &state_view.recorder.mode,
            state_view.recorder.file.as_deref(),
            state_view.file.as_deref(),
            state_view.age_ms,
        );
        if let Some(file) = finished {
            let _ = RunFinishedEvent { file }.emit(&app);
        }
        if rotated {
            let _ = RunsChangedEvent {
                file: state_view.file.clone(),
            }
            .emit(&app);
        }
        let _ = LiveStateEvent(state_view).emit(&app);
        if let Some((seq, q)) = quality {
            sent_quality_seq = seq;
            let _ = QualityEvent(q).emit(&app);
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Keeps the cross-campaign effect map fresh: a cheap mtime-based staleness
/// check every pass, the harvest only when campaigns actually changed and
/// telemetry is idle (decoding a campaign's stints while the recorder is
/// busy would fight the drive for CPU). First pass runs shortly after boot
/// so a stale map catches up before the first advise.
fn run_map_refresher(live: tuners::telemetry::live::SharedLive) {
    let mut delay = Duration::from_secs(5);
    loop {
        std::thread::sleep(delay);
        delay = Duration::from_secs(60);
        let busy = live
            .lock()
            .unwrap()
            .last_data
            .is_some_and(|t| t.elapsed() < Duration::from_secs(30));
        if busy {
            continue;
        }
        let scratch = std::env::temp_dir().join(format!("tuners-map-{}", std::process::id()));
        match tuners::advice::effectmap::refresh(
            Path::new("."),
            "sessions",
            &tuners::util::data_path("effect-map.tsv"),
            &scratch,
            false,
        ) {
            Ok((_, report)) => {
                for line in report.iter().filter(|l| l.as_str() != "up to date") {
                    println!("effect map: {line}");
                }
            }
            Err(e) => println!("effect map: {e}"),
        }
        let _ = std::fs::remove_dir_all(&scratch);
    }
}

/// Uploads queued bundles only while telemetry is idle.
/// Config is re-read every pass, so the sharing toggle takes effect without
/// a restart.
fn run_drainer(live: tuners::telemetry::live::SharedLive, config: PathBuf, outbox: PathBuf) {
    loop {
        std::thread::sleep(Duration::from_secs(60));
        let cfg = tuners::sharing::collect::CollectConfig::load(&config);
        if !cfg.ready() {
            continue;
        }
        let fresh = || {
            live.lock()
                .unwrap()
                .last_data
                .is_some_and(|t| t.elapsed() < tuners::sharing::collect::IDLE_BEFORE_DRAIN)
        };
        if fresh() {
            continue;
        }
        for line in tuners::sharing::collect::drain(&outbox, &cfg, &fresh) {
            println!("collect: {line}");
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // NOTE: bindings.ts is generated by the `bindings_export` test, not here.
    // A run()-time export would touch the file on every launch and put
    // `tauri dev`'s file watcher into a rebuild loop.
    let builder = specta_builder();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .invoke_handler(builder.invoke_handler())
        .setup(move |app| {
            builder.mount_events(app);

            // Single data root: TUNERS_DATA (dev) wins over app_data_dir.
            // The process ANCHORS ITS CWD there and uses the same relative
            // names as the CLI: journal entries store stint paths relative to
            // the root ("sessions/x.ftel"), and the read guards reject
            // absolute paths; both only hold when the engine runs from the
            // root, exactly like `tuners serve` used to.
            let fallback = app.path().app_data_dir()?;
            tuners::util::set_data_root(fallback);
            // The resolved root (TUNERS_DATA or app_data_dir) may not exist
            // yet (first boot, or a fresh scratch dir). It IS the data root,
            // so create it rather than dying on set_current_dir.
            let root = tuners::util::data_root();
            std::fs::create_dir_all(root)
                .and_then(|()| std::env::set_current_dir(root))
                .map_err(|e| format!("data root {}: {e}", root.display()))?;
            let root = PathBuf::from(".");
            let sessions_dir = "sessions".to_string();
            let session_file = "tune-session.txt".to_string();
            let journal_base = "tune-journal.txt".to_string();
            let config = PathBuf::from(tuners::sharing::collect::CONFIG_PATH);
            let outbox = PathBuf::from(tuners::sharing::collect::OUTBOX_DIR);
            std::fs::create_dir_all(&sessions_dir).ok();

            let live: tuners::telemetry::live::SharedLive = Default::default();
            let recorder = tuners::telemetry::record::new_shared();
            {
                let dir = sessions_dir.clone();
                let session = session_file.clone();
                let journal = journal_base.clone();
                let live = live.clone();
                std::thread::spawn(move || {
                    tuners::telemetry::live::run_tailer(dir, session, journal, live)
                });
            }
            {
                let out_dir = PathBuf::from(&sessions_dir);
                let journal = PathBuf::from(&journal_base);
                let session = PathBuf::from(&session_file);
                let recorder = recorder.clone();
                std::thread::spawn(move || {
                    tuners::telemetry::record::run_recorder(
                        20440, out_dir, journal, session, recorder,
                    )
                });
            }
            {
                let live = live.clone();
                let (config, outbox) = (config.clone(), outbox.clone());
                std::thread::spawn(move || run_drainer(live, config, outbox));
            }
            {
                let live = live.clone();
                std::thread::spawn(move || run_map_refresher(live));
            }
            {
                let handle = app.handle().clone();
                let (live, recorder) = (live.clone(), recorder.clone());
                std::thread::spawn(move || run_emitter(handle, live, recorder));
            }

            app.manage(AppState {
                root,
                sessions_dir,
                session_file,
                journal_base,
                config,
                outbox,
                recorder,
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::{EXTERNAL_STALE_MS, RunEdges};

    #[test]
    fn recorder_close_finishes_run() {
        let mut e = RunEdges::default();
        assert_eq!(
            e.observe("recording", Some("a.ftel"), Some("a.ftel"), Some(0)),
            (None, true)
        );
        // run closed: recorder idles, newest unchanged
        let (fin, _) = e.observe("waiting", None, Some("a.ftel"), Some(500));
        assert_eq!(fin.as_deref(), Some("a.ftel"));
        // idle polls stay quiet; recorder-owned modes never use the stale path
        assert_eq!(
            e.observe("waiting", None, Some("a.ftel"), Some(EXTERNAL_STALE_MS * 2)),
            (None, false)
        );
    }

    #[test]
    fn external_rotation_finishes_previous() {
        let mut e = RunEdges::default();
        // boot: first newest is a rotation but nothing closed
        assert_eq!(
            e.observe("external", None, Some("a.ftel"), Some(100)),
            (None, true)
        );
        let (fin, rot) = e.observe("external", None, Some("b.ftel"), Some(100));
        assert_eq!(fin.as_deref(), Some("a.ftel"));
        assert!(rot);
    }

    #[test]
    fn external_stale_tail_finishes_once_and_rearms() {
        let mut e = RunEdges::default();
        e.observe("external", None, Some("a.ftel"), Some(100));
        let (fin, _) = e.observe("external", None, Some("a.ftel"), Some(EXTERNAL_STALE_MS));
        assert_eq!(fin.as_deref(), Some("a.ftel"), "stale tail = run over");
        // latched: staying stale must not re-notify
        assert_eq!(
            e.observe(
                "external",
                None,
                Some("a.ftel"),
                Some(EXTERNAL_STALE_MS * 3)
            ),
            (None, false)
        );
        // data resumes (paused race came back), then goes stale again
        e.observe("external", None, Some("a.ftel"), Some(200));
        let (fin, _) = e.observe("external", None, Some("a.ftel"), Some(EXTERNAL_STALE_MS));
        assert_eq!(fin.as_deref(), Some("a.ftel"), "re-armed after fresh data");
    }

    #[test]
    fn external_rotation_after_stale_does_not_double_notify() {
        let mut e = RunEdges::default();
        e.observe("external", None, Some("a.ftel"), Some(100));
        e.observe("external", None, Some("a.ftel"), Some(EXTERNAL_STALE_MS));
        // next run starts: a.ftel already notified, only the rotation fires
        assert_eq!(
            e.observe("external", None, Some("b.ftel"), Some(100)),
            (None, true)
        );
    }

    #[test]
    fn recording_rotation_defers_to_recorder_edge() {
        let mut e = RunEdges::default();
        e.observe("recording", Some("a.ftel"), Some("a.ftel"), Some(0));
        // recorder cut a and immediately opened b: exactly one finish, from
        // the recorder edge, alongside the rotation
        let (fin, rot) = e.observe("recording", Some("b.ftel"), Some("b.ftel"), Some(0));
        assert_eq!(fin.as_deref(), Some("a.ftel"));
        assert!(rot);
    }

    /// The full command/event surface exports TS bindings: the compile-time
    /// contract check that replaces the old JSON-shape tests' role at the
    /// transport layer.
    #[test]
    fn bindings_export() {
        let dir = std::env::temp_dir().join(format!("tuners-bindings-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("bindings.ts");
        super::specta_builder()
            .export(specta_typescript::Typescript::default(), &out)
            .expect("export");
        // Also refresh the committed bindings so the frontend never drifts
        // from the command surface between dev runs.
        super::specta_builder()
            .export(
                specta_typescript::Typescript::default(),
                "../src/lib/bindings.ts",
            )
            .expect("export to src/lib");
        let text = std::fs::read_to_string(&out).unwrap();
        for name in [
            "stints",
            "advise",
            "saveTune",
            "LiveStateEvent",
            "runsChanged",
        ] {
            assert!(
                text.to_lowercase().contains(&name.to_lowercase()),
                "bindings missing {name}"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
