//! Tauri shell for the tuners desktop app: typed commands over the
//! engine's api module, TS bindings via tauri-specta, and the recorder/live/
//! drainer lifecycle that `tuners serve` used to own.

use std::path::PathBuf;
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
    recorder: tuners::record::SharedRecorder,
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

/// A recording closed (the run auto-cut or the recorder stopped) — refresh
/// verdicts.
#[derive(Clone, serde::Serialize, specta::Type, tauri_specta::Event)]
pub struct RunFinishedEvent {
    pub file: String,
}

/// The newest recording rotated (covers external capture too) — refresh run
/// lists.
#[derive(Clone, serde::Serialize, specta::Type, tauri_specta::Event)]
pub struct RunsChangedEvent {
    pub file: Option<String>,
}

// ----------------------------------------------------------------- commands

#[tauri::command]
#[specta::specta]
fn stints(state: S) -> Vec<api::StintRow> {
    api::stint_rows(&state.sessions_dir)
}

#[tauri::command]
#[specta::specta]
fn laps(file: String) -> Result<api::LapsView, api::ApiError> {
    api::laps_view(&file)
}

#[tauri::command]
#[specta::specta]
fn compare(a: String, b: String) -> Result<api::CompareView, api::ApiError> {
    api::compare_view(&a, &b)
}

#[tauri::command]
#[specta::specta]
fn report(file: String) -> Result<String, api::ApiError> {
    api::report_text(&file)
}

#[tauri::command]
#[specta::specta]
fn session(state: S) -> api::SessionView {
    let s = tuners::tuning::TuningSession::load(state.session_file.as_ref());
    api::session_view(&s, &state.journal_base)
}

#[tauri::command]
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

#[tauri::command]
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

#[tauri::command]
#[specta::specta]
fn sessions(state: S) -> api::SessionsView {
    api::sessions_view(&state.session_file, &state.journal_base)
}

#[tauri::command]
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

#[tauri::command]
#[specta::specta]
fn resume_session(state: S, id: String) -> Result<api::SessionView, api::ApiError> {
    let out = api::resume_session(&id, &state.session_file, &state.journal_base)?;
    api::split_and_drop_pending(&state.recorder);
    Ok(out)
}

#[tauri::command]
#[specta::specta]
fn advise(state: S) -> Result<api::AdviseView, api::ApiError> {
    api::advise_active(
        &state.session_file,
        &state.journal_base,
        &state.sessions_dir,
    )
}

#[tauri::command]
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
fn sharing(state: S) -> api::SharingView {
    api::sharing_view(&state.config, &state.outbox)
}

#[tauri::command]
#[specta::specta]
fn set_sharing(
    state: S,
    enabled: bool,
    endpoint: Option<String>,
    discard: bool,
) -> Result<api::SharingView, api::ApiError> {
    api::set_sharing(&state.config, &state.outbox, enabled, endpoint, discard)
}

#[tauri::command]
#[specta::specta]
fn sharing_history_plan(state: S) -> api::HistoryPlanView {
    api::history_plan_view(&state.root, &state.sessions_dir, &state.outbox)
}

#[tauri::command]
#[specta::specta]
fn share_history(state: S) -> Result<u32, api::ApiError> {
    api::share_history(
        &state.root,
        &state.sessions_dir,
        &state.outbox,
        &state.config,
    )
}

#[tauri::command]
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

#[tauri::command]
#[specta::specta]
fn record_split(state: S, note: Option<String>) {
    api::request_split(&state.recorder, note);
}

/// Build the stint's bundle and write it to `dest` (picked via the native
/// save dialog frontend-side). Returns the bundle's canonical name.
#[tauri::command]
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
/// None when offline or loopback-only — the wizard then shows 127.0.0.1 alone.
#[tauri::command]
#[specta::specta]
fn lan_ip() -> Option<String> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("8.8.8.8:53").ok()?;
    let ip = sock.local_addr().ok()?.ip();
    (!ip.is_loopback() && !ip.is_unspecified()).then(|| ip.to_string())
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
            advise,
            sharing,
            set_sharing,
            sharing_history_plan,
            share_history,
            delete_stint,
            record_split,
            export_stint,
            effect_fields,
            pending,
            lan_ip,
        ])
        .events(tauri_specta::collect_events![
            LiveStateEvent,
            QualityEvent,
            RunFinishedEvent,
            RunsChangedEvent,
        ])
}

/// Emits live-state/quality/run-finished/runs-changed by polling the shared
/// state the tailer and recorder threads maintain — the typed replacement
/// for the SSE loop, plus edge detection for run boundaries. Polling covers
/// external capture (view-only mode) for free: rotation shows up in the
/// tailer's newest-file either way.
fn run_emitter(
    app: tauri::AppHandle,
    live: tuners::live::SharedLive,
    recorder: tuners::record::SharedRecorder,
) {
    let mut sent_quality_seq = u64::MAX;
    let mut prev_recording: Option<String> = None;
    let mut prev_newest: Option<String> = None;
    loop {
        let (state_view, quality) = {
            let s = live.lock().unwrap();
            let r = recorder.lock().unwrap();
            let quality = (s.quality_seq != sent_quality_seq)
                .then(|| (s.quality_seq, api::quality_view(s.quality.as_ref())));
            (api::live_state_view(&s, &r), quality)
        };
        let recording = (state_view.recorder.mode == "recording")
            .then(|| state_view.recorder.file.clone())
            .flatten();
        if let Some(prev) = prev_recording.take()
            && recording.as_deref() != Some(prev.as_str())
        {
            let _ = RunFinishedEvent { file: prev }.emit(&app);
        }
        prev_recording = recording;
        if state_view.file != prev_newest {
            prev_newest = state_view.file.clone();
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

/// Uploads queued bundles only while telemetry is idle.
/// Config is re-read every pass, so the sharing toggle takes effect without
/// a restart.
fn run_drainer(live: tuners::live::SharedLive, config: PathBuf, outbox: PathBuf) {
    loop {
        std::thread::sleep(Duration::from_secs(60));
        let cfg = tuners::collect::CollectConfig::load(&config);
        if !cfg.ready() {
            continue;
        }
        let fresh = || {
            live.lock()
                .unwrap()
                .last_data
                .is_some_and(|t| t.elapsed() < tuners::collect::IDLE_BEFORE_DRAIN)
        };
        if fresh() {
            continue;
        }
        for line in tuners::collect::drain(&outbox, &cfg, &fresh) {
            println!("collect: {line}");
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // NOTE: bindings.ts is generated by the `bindings_export` test, not here —
    // a run()-time export would touch the file on every launch and put
    // `tauri dev`'s file watcher into a rebuild loop.
    let builder = specta_builder();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(builder.invoke_handler())
        .setup(move |app| {
            builder.mount_events(app);

            // Single data root: TUNERS_DATA (dev) wins over app_data_dir.
            // The process ANCHORS ITS CWD there and uses the same relative
            // names as the CLI: journal entries store stint paths relative to
            // the root ("sessions/x.ftel"), and the read guards reject
            // absolute paths — both only hold when the engine runs from the
            // root, exactly like `tuners serve` used to.
            let fallback = app.path().app_data_dir()?;
            tuners::util::set_data_root(fallback);
            // The resolved root (TUNERS_DATA or app_data_dir) may not exist
            // yet — first boot, or a fresh scratch dir. It IS the data root,
            // so create it rather than dying on set_current_dir.
            let root = tuners::util::data_root();
            std::fs::create_dir_all(root)
                .and_then(|()| std::env::set_current_dir(root))
                .map_err(|e| format!("data root {}: {e}", root.display()))?;
            let root = PathBuf::from(".");
            let sessions_dir = "sessions".to_string();
            let session_file = "tune-session.txt".to_string();
            let journal_base = "tune-journal.txt".to_string();
            let config = PathBuf::from(tuners::collect::CONFIG_PATH);
            let outbox = PathBuf::from(tuners::collect::OUTBOX_DIR);
            std::fs::create_dir_all(&sessions_dir).ok();

            let live: tuners::live::SharedLive = Default::default();
            let recorder = tuners::record::new_shared();
            {
                let dir = sessions_dir.clone();
                let live = live.clone();
                std::thread::spawn(move || tuners::live::run_tailer(dir, live));
            }
            {
                let out_dir = PathBuf::from(&sessions_dir);
                let journal = PathBuf::from(&journal_base);
                let session = PathBuf::from(&session_file);
                let recorder = recorder.clone();
                std::thread::spawn(move || {
                    tuners::record::run_recorder(20440, out_dir, journal, session, recorder)
                });
            }
            {
                let live = live.clone();
                let (config, outbox) = (config.clone(), outbox.clone());
                std::thread::spawn(move || run_drainer(live, config, outbox));
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
    /// The full command/event surface exports TS bindings — the compile-time
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
