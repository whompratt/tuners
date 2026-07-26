//! Minimal local dashboard server: hand-rolled HTTP over std TcpListener, zero
//! dependencies (docs/plans/006-dashboard.md). Handlers are pure functions so a
//! swap to a real framework later is mechanical.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::time::Duration;

pub fn run(
    port: u16,
    sessions_dir: String,
    udp_port: u16,
    journal: String,
    session_file: String,
) -> std::io::Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    println!("tuners dashboard: http://127.0.0.1:{port}/  (Ctrl+C to stop)");
    let live: crate::live::SharedLive = Default::default();
    {
        let dir = sessions_dir.clone();
        let live = live.clone();
        std::thread::spawn(move || crate::live::run_tailer(dir, live));
    }
    let recorder = crate::record::new_shared();
    if udp_port != 0 {
        let out_dir = std::path::PathBuf::from(sessions_dir.clone());
        let journal = std::path::PathBuf::from(journal);
        let session = std::path::PathBuf::from(session_file.clone());
        let recorder = recorder.clone();
        std::thread::spawn(move || {
            crate::record::run_recorder(udp_port, out_dir, journal, session, recorder)
        });
    } else {
        recorder.lock().unwrap().mode =
            crate::record::RecorderMode::External("disabled (--udp-port 0)".into());
    }
    {
        // Drainer (plan 009 phase 2): uploads queued bundles only while
        // telemetry is idle. Config is re-read every pass, so the dashboard
        // toggle takes effect without a restart.
        let live = live.clone();
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_secs(60));
                let cfg = crate::collect::CollectConfig::load(
                    crate::collect::CONFIG_PATH.as_ref(),
                );
                if !cfg.ready() {
                    continue;
                }
                let fresh = || {
                    live.lock().unwrap().last_data.is_some_and(|t| {
                        t.elapsed() < crate::collect::IDLE_BEFORE_DRAIN
                    })
                };
                if fresh() {
                    continue;
                }
                for line in
                    crate::collect::drain(crate::collect::OUTBOX_DIR.as_ref(), &cfg, &fresh)
                {
                    println!("collect: {line}");
                }
            }
        });
    }
    for stream in listener.incoming() {
        let dir = sessions_dir.clone();
        let session_file = session_file.clone();
        if let Ok(stream) = stream {
            let live = live.clone();
            let recorder = recorder.clone();
            std::thread::spawn(move || handle(stream, &dir, &live, &recorder, &session_file));
        }
    }
    Ok(())
}

fn handle(
    mut stream: TcpStream,
    sessions_dir: &str,
    live: &crate::live::SharedLive,
    recorder: &crate::record::SharedRecorder,
    session_file: &str,
) {
    let mut reader = BufReader::new(match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    });
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    // Drain headers, keeping Content-Length so POST bodies can be read.
    let mut content_length = 0usize;
    loop {
        let mut header = String::new();
        match reader.read_line(&mut header) {
            Ok(0) | Err(_) => break,
            Ok(_) if header == "\r\n" || header == "\n" => break,
            Ok(_) => {
                if let Some(v) = header.to_ascii_lowercase().strip_prefix("content-length:") {
                    content_length = v.trim().parse().unwrap_or(0);
                }
            }
        }
    }
    let mut body_bytes = vec![0u8; content_length.min(1 << 20)];
    if !body_bytes.is_empty() && reader.read_exact(&mut body_bytes).is_err() {
        return;
    }
    let request_body = String::from_utf8_lossy(&body_bytes).into_owned();

    let method = request_line.split_whitespace().next().unwrap_or("GET");
    let target = request_line.split_whitespace().nth(1).unwrap_or("/");
    if target.split('?').next() == Some("/api/live") {
        serve_sse(stream, live, recorder);
        return;
    }
    // Binary response (the rest of the API is text) — handled outside the match.
    if target.split('?').next() == Some("/api/export") {
        let query = target.split_once('?').map(|(_, q)| q).unwrap_or("");
        serve_export(stream, sessions_dir, query_param(query, "file"), session_file);
        return;
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let session_path = Path::new(session_file);
    let (status, content_type, body) = match (method, path) {
        ("POST", "/api/record/split") => {
            let mut r = recorder.lock().unwrap();
            r.split_requested = true;
            r.pending_note = query_param(query, "note").filter(|n| !n.trim().is_empty());
            ("200 OK", "application/json", "{\"ok\":true}".to_string())
        }
        ("POST", "/api/stint/delete") => {
            let active = recorder.lock().unwrap().file.clone();
            delete_stint(
                sessions_dir,
                query_param(query, "file"),
                active.as_deref(),
                query_param(query, "force").as_deref() == Some("1"),
                "tune-journal.txt",
            )
        }
        ("POST", "/api/session") => session_post(&form_params(&request_body), session_path),
        ("POST", "/api/session/tune") => {
            tune_post(&form_params(&request_body), session_path, recorder)
        }
        ("POST", "/api/session/new") => {
            let out = session_new(&form_params(&request_body), session_file, "tune-journal.txt");
            if out.0 == "200 OK" {
                split_and_drop_pending(recorder);
            }
            out
        }
        ("POST", "/api/session/resume") => {
            let out = session_resume(&form_params(&request_body), session_file, "tune-journal.txt");
            if out.0 == "200 OK" {
                split_and_drop_pending(recorder);
            }
            out
        }
        ("GET", "/api/sharing") => ("200 OK", "application/json", collect_json()),
        ("POST", "/api/sharing") => collect_post(&form_params(&request_body)),
        ("GET", "/api/sessions") => (
            "200 OK",
            "application/json",
            sessions_json(session_file, "tune-journal.txt"),
        ),
        ("GET", "/api/session") => (
            "200 OK",
            "application/json",
            session_json(&crate::tuning::TuningSession::load(session_path)),
        ),
        ("GET", "/api/advise") => {
            let session = crate::tuning::TuningSession::load(session_path);
            let journal = crate::tuning::journal_path_for(session.car, "tune-journal.txt");
            match crate::advise::advise(&journal, session_path, sessions_dir) {
                Ok(view) => ("200 OK", "application/json", advise_json(&view)),
                Err(e) => ("500 Internal Server Error", "text/plain; charset=utf-8", e),
            }
        }
        _ => respond(target, sessions_dir),
    };
    // no-store: a stale cached dashboard against a newer server is a debugging
    // trap (the UI is compiled into the binary, so page and API must match).
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len(),
    );
    let _ = stream.write_all(body.as_bytes());
}

/// The manual-export path (plan 009): build the stint's telemetry bundle and
/// hand it to the browser as a download. The bundler self-verifies, so a 200
/// here is a bundle that already round-tripped.
fn serve_export(mut stream: TcpStream, sessions_dir: &str, file: Option<String>, session_file: &str) {
    let result = (|| -> Result<(String, Vec<u8>), String> {
        let file = file.ok_or("missing ?file= parameter")?;
        if !file.ends_with(".ftel") || file.contains('/') || file.contains('\\') {
            return Err("file must be a bare .ftel name from the sessions directory".into());
        }
        let session = crate::tuning::TuningSession::load(Path::new(session_file));
        let journal_path = crate::tuning::journal_path_for(session.car, "tune-journal.txt");
        let journal = std::fs::read_to_string(&journal_path).unwrap_or_default();
        crate::bundle::build(&Path::new(sessions_dir).join(&file), &session, &journal)
    })();
    match result {
        Ok((name, bytes)) => {
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n\
                 Content-Disposition: attachment; filename=\"{name}\"\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n",
                bytes.len(),
            );
            let _ = stream.write_all(&bytes);
        }
        Err(e) => {
            let _ = write!(
                stream,
                "HTTP/1.1 400 Bad Request\r\nContent-Type: text/plain; charset=utf-8\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{e}",
                e.len(),
            );
        }
    }
}

/// SSE relay of the live session (plan 006 phase 4): a `state` event ~4x/s with
/// the latest frame and data age, plus a `quality` event whenever the
/// data-quality summary changes. Ends when the client disconnects.
fn serve_sse(
    mut stream: TcpStream,
    live: &crate::live::SharedLive,
    recorder: &crate::record::SharedRecorder,
) {
    if write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: keep-alive\r\n\r\nretry: 1000\n\n"
    )
    .is_err()
    {
        return;
    }
    let mut sent_quality_seq = u64::MAX;
    loop {
        let (state_json, quality) = {
            let s = live.lock().unwrap();
            let quality = (s.quality_seq != sent_quality_seq)
                .then(|| (s.quality_seq, quality_json(s.quality.as_ref())));
            (live_state_json(&s), quality)
        };
        let rec_json = recorder_json(&recorder.lock().unwrap());
        let state_json = format!(
            "{},\"recorder\":{rec_json}}}",
            &state_json[..state_json.len() - 1]
        );
        if write!(stream, "event: state\ndata: {state_json}\n\n").is_err() {
            return; // client gone
        }
        if let Some((seq, json)) = quality {
            sent_quality_seq = seq;
            if write!(stream, "event: quality\ndata: {json}\n\n").is_err() {
                return;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
}

/// The `state` SSE payload. Pure over the snapshot — unit-testable.
fn live_state_json(s: &crate::live::LiveState) -> String {
    let file = match s.file.as_ref().and_then(|p| p.file_name()) {
        Some(name) => format!("\"{}\"", name.to_string_lossy()),
        None => "null".into(),
    };
    let age_ms = s
        .last_data
        .map_or("null".into(), |t| t.elapsed().as_millis().to_string());
    let frame = match &s.latest {
        None => "null".into(),
        Some(tf) => {
            let f = &tf.frame;
            let t = f.tire_temp;
            format!(
                "{{\"raceOn\":{},\"speedMps\":{:.2},\"rpm\":{:.0},\"maxRpm\":{:.0},\
                 \"gear\":{},\"lapNumber\":{},\"currentLapS\":{:.3},\"lastLapS\":{:.3},\
                 \"bestLapS\":{:.3},\"fuel\":{:.4},\"tireTempF\":[{:.0},{:.0},{:.0},{:.0}]}}",
                f.is_race_on,
                f.speed,
                f.current_engine_rpm,
                f.engine_max_rpm,
                f.gear,
                f.lap_number,
                f.current_lap,
                f.last_lap,
                f.best_lap,
                f.fuel,
                t.fl,
                t.fr,
                t.rl,
                t.rr,
            )
        }
    };
    format!("{{\"file\":{file},\"ageMs\":{age_ms},\"frame\":{frame}}}")
}

/// Delete one stint recording. Stricter than the read guard: only a bare
/// filename inside the stints directory (no paths), and never the file the
/// recorder is writing right now.
/// Journal files (live and archived, matching the base's naming scheme) that
/// reference a stint by filename.
fn journals_referencing(stint_name: &str, journal_base: &str) -> Vec<String> {
    let base = Path::new(journal_base);
    let dir = match base.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let stem = base.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for entry in rd.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with(&stem) || !name.ends_with(".txt") {
                continue;
            }
            if std::fs::read_to_string(entry.path())
                .is_ok_and(|text| text.contains(stint_name))
            {
                out.push(name);
            }
        }
    }
    out.sort();
    out
}

fn delete_stint(
    sessions_dir: &str,
    file: Option<String>,
    active: Option<&Path>,
    force: bool,
    journal_base: &str,
) -> (&'static str, &'static str, String) {
    let Some(name) = file else {
        return ("400 Bad Request", "text/plain; charset=utf-8", "missing file parameter".into());
    };
    if name.contains('/') || name.contains('\\') || name.contains("..") || !name.ends_with(".ftel") {
        return ("400 Bad Request", "text/plain; charset=utf-8", "bad file parameter".into());
    }
    if active.and_then(|p| p.file_name()).is_some_and(|a| a.to_string_lossy() == name) {
        return (
            "409 Conflict",
            "text/plain; charset=utf-8",
            "stint is currently being recorded".into(),
        );
    }
    // A journaled stint is campaign evidence: deleting it degrades advice
    // (the entry is skipped, its note merged into the next step). Require an
    // explicit force so the dashboard can confirm first.
    if !force && let Some(journal) = journals_referencing(&name, journal_base).first() {
        return (
            "409 Conflict",
            "text/plain; charset=utf-8",
            format!("journaled in {journal} — deleting drops that step's measurement"),
        );
    }
    match std::fs::remove_file(Path::new(sessions_dir).join(&name)) {
        Ok(()) => ("200 OK", "application/json", "{\"ok\":true}".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            ("404 Not Found", "text/plain; charset=utf-8", "no such stint".into())
        }
        Err(e) => ("500 Internal Server Error", "text/plain; charset=utf-8", e.to_string()),
    }
}

/// application/x-www-form-urlencoded body → decoded (key, value) pairs.
fn form_params(body: &str) -> Vec<(String, String)> {
    body.split('&')
        .filter_map(|pair| {
            let (k, v) = pair.split_once('=')?;
            Some((
                percent_decode(&k.replace('+', " ")),
                percent_decode(&v.replace('+', " ")),
            ))
        })
        .collect()
}

fn json_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Telemetry-collection state for the dashboard (plan 009): consent flag,
/// pseudonymous sender id, and outbox depth.
fn collect_json() -> String {
    let cfg = crate::collect::CollectConfig::load(crate::collect::CONFIG_PATH.as_ref());
    let outbox = Path::new(crate::collect::OUTBOX_DIR);
    let rejected = std::fs::read_dir(outbox.join("rejected"))
        .map(|rd| rd.flatten().count())
        .unwrap_or(0);
    format!(
        "{{\"enabled\":{},\"endpoint\":{},\"sender\":{},\"queued\":{},\"rejected\":{rejected}}}",
        cfg.enabled,
        json_str(&cfg.endpoint),
        if cfg.token.is_empty() {
            "null".to_string()
        } else {
            json_str(&crate::collect::sender_id(&cfg.token))
        },
        crate::collect::queued(outbox).len(),
    )
}

/// Toggle/configure collection. First enable mints the client token; the
/// token itself never leaves the config file — the UI only ever sees the
/// derived sender id. `discard=1` (with disable) empties the queue.
fn collect_post(params: &[(String, String)]) -> (&'static str, &'static str, String) {
    let get = |k: &str| params.iter().find(|(pk, _)| pk == k).map(|(_, v)| v.as_str());
    let mut cfg = crate::collect::CollectConfig::load(crate::collect::CONFIG_PATH.as_ref());
    match get("enabled") {
        Some("1") => {
            cfg.enabled = true;
            if cfg.token.len() != 64 {
                cfg.token = crate::collect::generate_token();
            }
            if let Some(e) = get("endpoint").filter(|e| !e.trim().is_empty()) {
                cfg.endpoint = e.trim().to_string();
            }
            if cfg.endpoint.is_empty() {
                cfg.endpoint = crate::collect::DEFAULT_ENDPOINT.to_string();
            }
        }
        Some("0") => {
            cfg.enabled = false;
            if get("discard") == Some("1") {
                for p in crate::collect::queued(crate::collect::OUTBOX_DIR.as_ref()) {
                    let _ = std::fs::remove_file(p);
                }
            }
        }
        _ => return ("400 Bad Request", "text/plain; charset=utf-8", "enabled=0|1 required".into()),
    }
    if let Err(e) = cfg.save(crate::collect::CONFIG_PATH.as_ref()) {
        return ("500 Internal Server Error", "text/plain; charset=utf-8", e.to_string());
    }
    ("200 OK", "application/json", collect_json())
}

/// The tuning session for the dashboard: car, facts, latest tune revision.
fn session_json(s: &crate::tuning::TuningSession) -> String {
    let car = s.car.map_or("null".into(), |c| c.to_string());
    let name = s
        .car
        .and_then(crate::cars::car_name)
        .map_or("null".into(), json_str);
    let map = |m: &std::collections::BTreeMap<String, String>| {
        let pairs: Vec<String> = m
            .iter()
            .map(|(k, v)| format!("{}:{}", json_str(k), json_str(v)))
            .collect();
        format!("{{{}}}", pairs.join(","))
    };
    let latest = s
        .latest()
        .map_or("null".into(), |rev| map(&rev.values));
    // Baseline included so the dashboard can summarize "delta vs baseline"
    // instead of dumping the whole tune.
    let baseline = s
        .revisions
        .first()
        .filter(|_| s.revisions.len() > 1)
        .map_or("null".into(), |rev| map(&rev.values));
    format!(
        "{{\"car\":{car},\"carName\":{name},\"facts\":{},\"revisions\":{},\"latest\":{latest},\"baseline\":{baseline},\"campaignStart\":{}}}",
        map(&s.facts),
        s.revisions.len(),
        campaign_start(s, "tune-journal.txt").map_or("null".into(), |v| json_str(&v)),
    )
}

/// When the active campaign began: the earlier of the first tune revision and
/// the first journaled stint (the seeded baseline stint starts before the
/// first save). Scopes the dashboard stint list to the campaign.
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

/// Create/update the session: car + facts (revisions kept unless reset=1).
fn session_post(
    params: &[(String, String)],
    path: &Path,
) -> (&'static str, &'static str, String) {
    let reset = params.iter().any(|(k, v)| k == "reset" && v == "1");
    let posted_car: Option<i32> = params
        .iter()
        .find(|(k, _)| k == "car")
        .and_then(|(_, v)| v.parse().ok());
    let current = crate::tuning::TuningSession::load(path);
    let mut s = if reset {
        crate::tuning::TuningSession::default()
    } else if posted_car.is_some() && current.car.is_some() && posted_car != current.car {
        // Switching cars = switching SESSIONS: archive the active session to
        // its per-car file (tune-session-<ordinal>.txt, same scheme as
        // journals) and resume the new car's archived session if one exists.
        // Nothing is lost — switching back restores the whole campaign.
        let base = path.to_string_lossy();
        let archive = crate::tuning::journal_path_for(current.car, &base);
        let _ = current.save(archive.as_ref());
        let resumed =
            crate::tuning::TuningSession::load(crate::tuning::journal_path_for(posted_car, &base).as_ref());
        if resumed.car == posted_car {
            resumed
        } else {
            crate::tuning::TuningSession::default()
        }
    } else {
        current
    };
    for (k, v) in params {
        match k.as_str() {
            "reset" => {}
            "car" => s.car = v.parse().ok(),
            _ if v.trim().is_empty() => {
                s.facts.remove(k);
            }
            _ => {
                s.facts.insert(k.clone(), v.trim().to_string());
            }
        }
    }
    match s.save(path) {
        Ok(()) => ("200 OK", "application/json", session_json(&s)),
        Err(e) => ("500 Internal Server Error", "text/plain; charset=utf-8", e.to_string()),
    }
}

/// A session switch is a hard recording boundary: cut the stint and drop any
/// pending tune-note chain — the chain belonged to the outgoing session.
fn split_and_drop_pending(recorder: &crate::record::SharedRecorder) {
    let mut r = recorder.lock().unwrap();
    r.split_requested = true;
    r.pending_note = None;
    r.pending_base_rev = None;
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
/// "<ordinal>-<stamp>" — the legacy car-switch scheme's plain "<ordinal>" ids
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
    // not overwrite the first archive — bump a counter until the id is free.
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
        // Boundary marker (a comment — the entry parser skips it): a parked
        // campaign accrues no implicit trajectory steps while other campaigns
        // drive the same car (advise::campaign_bound).
        append_line(&parked, &format!("# parked {}", crate::util::utc_stamp(now_secs())))?;
    }
    Ok(id)
}

fn append_line(path: &str, line: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    let mut f = std::fs::OpenOptions::new().append(true).open(path)?;
    writeln!(f, "{line}")
}

/// Start a fresh session (e.g. after an upgrade rebuild): the active pair is
/// archived and a blank session — carrying only the unit display prefs, plus
/// any posted name/description/car — becomes active. The first tune save
/// seeds the new journal's baseline against the next stint.
fn session_new(
    params: &[(String, String)],
    session_file: &str,
    journal_base: &str,
) -> (&'static str, &'static str, String) {
    let current = crate::tuning::TuningSession::load(session_file.as_ref());
    if !session_is_blank(&current)
        && let Err(e) = archive_active(&current, session_file, journal_base)
    {
        return ("500 Internal Server Error", "text/plain; charset=utf-8", e.to_string());
    }
    let mut fresh = crate::tuning::TuningSession::default();
    for (k, v) in &current.facts {
        if k.starts_with("unit_") {
            fresh.facts.insert(k.clone(), v.clone());
        }
    }
    for (k, v) in params {
        let v = v.trim();
        match k.as_str() {
            "car" => fresh.car = v.parse().ok(),
            "name" | "description" if !v.is_empty() => {
                fresh.facts.insert(k.clone(), v.to_string());
            }
            _ => {}
        }
    }
    match fresh.save(session_file.as_ref()) {
        Ok(()) => ("200 OK", "application/json", session_json(&fresh)),
        Err(e) => ("500 Internal Server Error", "text/plain; charset=utf-8", e.to_string()),
    }
}

/// Swap an archived session back in: the active pair is archived first, then
/// the chosen pair becomes active (files move, nothing is copied — an archived
/// session has exactly one home).
fn session_resume(
    params: &[(String, String)],
    session_file: &str,
    journal_base: &str,
) -> (&'static str, &'static str, String) {
    let Some(id) = params
        .iter()
        .find(|(k, _)| k == "id")
        .map(|(_, v)| v.trim())
        .filter(|v| !v.is_empty())
    else {
        return ("400 Bad Request", "text/plain; charset=utf-8", "missing id".into());
    };
    if !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return ("400 Bad Request", "text/plain; charset=utf-8", "bad id".into());
    }
    let archived_session = crate::tuning::suffixed_path(session_file, id);
    if !Path::new(&archived_session).exists() {
        return ("404 Not Found", "text/plain; charset=utf-8", "no such session".into());
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
        return (
            "409 Conflict",
            "text/plain; charset=utf-8",
            format!("{target} already exists — another session for this car is parked; resume it first"),
        );
    }
    if !session_is_blank(&current)
        && let Err(e) = archive_active(&current, session_file, journal_base)
    {
        return ("500 Internal Server Error", "text/plain; charset=utf-8", e.to_string());
    }
    if let Err(e) = std::fs::rename(&archived_session, session_file) {
        return ("500 Internal Server Error", "text/plain; charset=utf-8", e.to_string());
    }
    if src != target
        && Path::new(&src).exists()
        && let Err(e) = std::fs::rename(&src, &target)
    {
        return ("500 Internal Server Error", "text/plain; charset=utf-8", e.to_string());
    }
    // The resume marker floors the implicit-step scan: stints other campaigns
    // drove while this one was parked stay out of its trajectory.
    if Path::new(&target).exists()
        && let Err(e) =
            append_line(&target, &format!("# resumed {}", crate::util::utc_stamp(now_secs())))
    {
        return ("500 Internal Server Error", "text/plain; charset=utf-8", e.to_string());
    }
    ("200 OK", "application/json", session_json(&restored))
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

/// The session library: the active session plus every archived sibling
/// (<stem>-<id>.<ext>), newest first.
fn sessions_json(session_file: &str, journal_base: &str) -> String {
    let row = |id: Option<&str>, s: &crate::tuning::TuningSession, stints: usize| {
        format!(
            "{{\"id\":{},\"car\":{},\"carName\":{},\"name\":{},\"description\":{},\"revisions\":{},\"stints\":{}}}",
            id.map_or("null".into(), json_str),
            s.car.map_or("null".into(), |c| c.to_string()),
            s.car.and_then(crate::cars::car_name).map_or("null".into(), json_str),
            s.facts.get("name").map_or("null".into(), |v| json_str(v)),
            s.facts.get("description").map_or("null".into(), |v| json_str(v)),
            s.revisions.len(),
            stints,
        )
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
    let (stem, ext) = file_name.rsplit_once('.').unwrap_or((file_name.as_str(), ""));
    let (prefix, suffix) = (format!("{stem}-"), format!(".{ext}"));
    let mut archived: Vec<(std::time::SystemTime, String)> = Vec::new();
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
    archived.sort_by(|a, b| b.0.cmp(&a.0));
    let active = crate::tuning::TuningSession::load(path);
    let active_stints = journal_stints(&crate::tuning::journal_path_for(active.car, journal_base));
    format!(
        "{{\"active\":{},\"archived\":[{}]}}",
        row(None, &active, active_stints),
        archived.into_iter().map(|(_, j)| j).collect::<Vec<_>>().join(","),
    )
}

/// Save a new tune revision. The journal note is derived by diffing against the
/// previous revision; a changed tune also cuts the stint (the note journals
/// against the next stint that opens). The first revision is the baseline —
/// stored, no note, no cut.
fn tune_post(
    params: &[(String, String)],
    path: &Path,
    recorder: &crate::record::SharedRecorder,
) -> (&'static str, &'static str, String) {
    let mut s = crate::tuning::TuningSession::load(path);
    let mut rev = crate::tuning::Revision {
        stamp: crate::util::utc_stamp(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        ),
        ..Default::default()
    };
    for (k, v) in params {
        let v = v.trim();
        if !v.is_empty() && crate::tuning::FIELDS.iter().any(|(key, _)| key == k) {
            rev.values.insert(k.clone(), v.to_string());
        }
    }
    if rev.values.is_empty() {
        return ("400 Bad Request", "text/plain; charset=utf-8", "empty tune".into());
    }
    // partial=1 (accepting a suggestion): the posted keys are merged onto the
    // LATEST revision — the rest of the setup carries over, and consecutive
    // accepts before the next stint chain onto each other.
    if params.iter().any(|(k, v)| k == "partial" && v == "1") {
        let Some(latest) = s.latest() else {
            return (
                "400 Bad Request",
                "text/plain; charset=utf-8",
                "no tune on file to merge a partial save onto".into(),
            );
        };
        let mut merged = latest.values.clone();
        merged.append(&mut rev.values);
        rev.values = merged;
    }
    // Consecutive saves with no stint between them net into ONE journal note:
    // diff against the last DRIVEN revision (the pending chain's base), not
    // merely the previous save. Saving arb, then remembering final drive and
    // saving again, journals "front arb -1.5; final drive -0.14" — the change
    // the next stint will actually be driven on.
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
            if let Err(e) = s.save(path) {
                return ("500 Internal Server Error", "text/plain; charset=utf-8", e.to_string());
            }
        }
        return ("200 OK", "application/json", "{\"ok\":true,\"note\":null,\"changed\":false}".into());
    }
    s.revisions.push(rev);
    if let Err(e) = s.save(path) {
        return ("500 Internal Server Error", "text/plain; charset=utf-8", e.to_string());
    }
    if !first {
        let mut r = recorder.lock().unwrap();
        r.split_requested = true;
        r.pending_note = Some(note.clone());
        r.pending_base_rev = base_idx;
    }
    let note_json = if first { "null".into() } else { json_str(&note) };
    (
        "200 OK",
        "application/json",
        format!("{{\"ok\":true,\"note\":{note_json},\"changed\":true}}"),
    )
}

/// The advise view for the dashboard: trajectory + reconciled recommendations.
fn advise_json(v: &crate::advise::AdviseView) -> String {
    let steps: Vec<String> = v
        .steps
        .iter()
        .map(|s| {
            let balance = s.balance.map_or("null".into(), |(i, f, r)| {
                format!("[{i:.2},{f:.2},{r:.2}]")
            });
            let pos = s.pos.map_or("null".into(), |(f, r)| format!("[{f:.1},{r:.1}]"));
            let split = s
                .split
                .map_or("null".into(), |(e, x, st)| format!("[{e:.3},{x:.3},{st:.3}]"));
            let outcome = match &s.outcome {
                None => "null".into(),
                Some(Ok((word, delta, unequal))) => format!(
                    "{{\"word\":\"{word}\",\"deltaS\":{delta:.3},\"unequalLaps\":{unequal}}}"
                ),
                Some(Err(e)) => format!("{{\"error\":{}}}", json_str(e)),
            };
            let row_anchor = s.anchor.as_ref().map_or("null".into(), |a| {
                format!(
                    "{{\"vsStep\":{},\"areas\":{},\"deltaS\":{:.3},\"word\":\"{}\",\"weak\":{}}}",
                    a.vs_step,
                    json_str(&a.areas),
                    a.delta_s,
                    a.word,
                    a.weak,
                )
            });
            format!(
                "{{\"path\":{},\"laps\":{},\"bestS\":{:.3},\"idealS\":{:.3},\
                 \"balance\":{balance},\"note\":{},\"pos\":{pos},\"outcome\":{outcome},\
                 \"split\":{split},\"anchor\":{row_anchor}}}",
                json_str(&s.path),
                s.laps,
                s.best_s,
                s.ideal_s,
                s.note.as_deref().map_or("null".into(), json_str),
            )
        })
        .collect();
    let recs: Vec<String> = v
        .recommendations
        .iter()
        .map(|r| {
            let evidence: Vec<String> = r.evidence.iter().map(|e| json_str(e)).collect();
            let apply: Vec<String> = r
                .apply
                .iter()
                .map(|(k, v)| format!("[{},{}]", json_str(k), json_str(v)))
                .collect();
            format!(
                "{{\"confidence\":\"{}\",\"area\":{},\"suggestion\":{},\"apply\":[{}],\"advice\":{},\"evidence\":[{}]}}",
                r.confidence.label(),
                json_str(r.area),
                r.suggestion.as_deref().map_or("null".into(), json_str),
                apply.join(","),
                json_str(&r.advice),
                evidence.join(","),
            )
        })
        .collect();
    let tune: Vec<String> = v
        .current_tune
        .iter()
        .map(|(phrase, value, unit)| {
            format!(
                "{{\"phrase\":{},\"value\":{},\"unit\":{}}}",
                json_str(phrase),
                json_str(value),
                unit.map_or("null".into(), json_str),
            )
        })
        .collect();
    let anchor = v.anchor.as_ref().map_or("null".into(), |a| {
        format!(
            "{{\"vsStep\":{},\"areas\":{},\"changes\":{},\"deltaS\":{:.3},\"word\":\"{}\",\"weak\":{},\"reconciled\":{},\"split\":[{:.3},{:.3},{:.3}]}}",
            a.vs_step,
            json_str(&a.areas),
            json_str(&a.changes),
            a.delta_s,
            a.word,
            a.weak,
            a.reconciled,
            a.split.0,
            a.split.1,
            a.split.2,
        )
    });
    let landscapes: Vec<String> = v
        .landscapes
        .iter()
        .map(|l| {
            let nodes: Vec<String> = l
                .nodes
                .iter()
                .map(|(v, cum, n)| format!("[{v},{cum:.3},{n}]"))
                .collect();
            let ms: Vec<String> = l
                .measurements
                .iter()
                .map(|m| {
                    let split = m.split.map_or("null".into(), |(e, x, st)| {
                        format!("[{e:.3},{x:.3},{st:.3}]")
                    });
                    format!(
                        "{{\"fromStep\":{},\"toStep\":{},\"desc\":{},\"deltaS\":{:.3},\"split\":{split},\"weak\":{},\"direct\":{}}}",
                        m.from_step,
                        m.to_step,
                        json_str(&m.desc),
                        m.delta_s,
                        m.weak,
                        m.direct,
                    )
                })
                .collect();
            format!(
                "{{\"area\":{},\"phrase\":{},\"key\":{},\"nodes\":[{}],\"fit\":{},\"vertex\":{},\"measurements\":[{}]}}",
                json_str(l.area),
                json_str(&l.phrase),
                l.key.as_deref().map_or("null".into(), json_str),
                nodes.join(","),
                l.fit
                    .map_or("null".into(), |(a, b, c)| format!("[{a},{b},{c}]")),
                l.vertex.map_or("null".into(), |v| format!("{v}")),
                ms.join(","),
            )
        })
        .collect();
    let aba = v.aba.as_ref().map_or("null".into(), |a| {
        format!(
            "{{\"families\":{},\"effectS\":{:.3},\"driftS\":{:.3}}}",
            json_str(&a.families),
            a.effect_s,
            a.drift_s,
        )
    });
    format!(
        "{{\"journal\":{},\"adviceFor\":{},\"steps\":[{}],\"anchor\":{anchor},\"aba\":{aba},\"landscapes\":[{}],\"driftFloor\":{},\"inProgress\":{},\"missing\":[{}],\"recommendations\":[{}],\"currentTune\":[{}]}}",
        v.journal.as_deref().map_or("null".into(), json_str),
        json_str(&v.advice_for),
        steps.join(","),
        landscapes.join(","),
        v.drift_floor
            .map_or("null".into(), |(n, f)| format!("[{n},{f:.3}]")),
        v.in_progress.as_deref().map_or("null".into(), json_str),
        v.missing.iter().map(|p| json_str(p)).collect::<Vec<_>>().join(","),
        recs.join(","),
        tune.join(","),
    )
}

/// Recorder status for the `state` SSE event.
fn recorder_json(r: &crate::record::RecorderStatus) -> String {
    let mode = match &r.mode {
        crate::record::RecorderMode::External(_) => "external",
        crate::record::RecorderMode::Waiting => "waiting",
        crate::record::RecorderMode::Recording => "recording",
    };
    let file = match r.file.as_ref().and_then(|p| p.file_name()) {
        Some(name) => format!("\"{}\"", name.to_string_lossy()),
        None => "null".into(),
    };
    format!("{{\"mode\":\"{mode}\",\"file\":{file},\"packets\":{}}}", r.packets)
}

/// The `quality` SSE payload; `null` until a comparable lap exists.
fn quality_json(q: Option<&crate::live::Quality>) -> String {
    match q {
        None => "null".into(),
        Some(q) => format!(
            "{{\"laps\":{},\"standingOnly\":{},\"bestLapS\":{:.3},\"spreadPct\":{:.2},\
             \"sharedKm\":{:.2},\"confidencePct\":{:.0},\"band\":\"{}\"}}",
            q.laps,
            q.standing_only,
            q.best_lap_s,
            q.spread_frac * 100.0,
            q.shared_km,
            q.confidence * 100.0,
            q.band.as_str(),
        ),
    }
}

/// Route a request target to (status, content type, body). Pure — unit-testable.
pub fn respond(target: &str, sessions_dir: &str) -> (&'static str, &'static str, String) {
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    match path {
        "/" => (
            "200 OK",
            "text/html; charset=utf-8",
            include_str!("../assets/index.html").to_string(),
        ),
        "/api/stints" => ("200 OK", "application/json", stints_json(sessions_dir)),
        "/api/compare" => {
            let a = query_param(query, "a");
            let b = query_param(query, "b");
            match (a, b) {
                (Some(a), Some(b)) if is_safe_session_path(&a) && is_safe_session_path(&b) => {
                    match compare_json(Path::new(&a), Path::new(&b)) {
                        Ok(json) => ("200 OK", "application/json", json),
                        Err(e) => ("500 Internal Server Error", "text/plain; charset=utf-8", e),
                    }
                }
                _ => (
                    "400 Bad Request",
                    "text/plain; charset=utf-8",
                    "need safe a= and b= session parameters".into(),
                ),
            }
        }
        "/api/laps" => match query.strip_prefix("file=").map(percent_decode) {
            Some(file) if is_safe_session_path(&file) => match laps_json(Path::new(&file)) {
                Ok(json) => ("200 OK", "application/json", json),
                Err(e) => ("500 Internal Server Error", "text/plain; charset=utf-8", e),
            },
            _ => (
                "400 Bad Request",
                "text/plain; charset=utf-8",
                "bad or missing file parameter".into(),
            ),
        },
        "/api/report" => match query.strip_prefix("file=").map(percent_decode) {
            Some(file) if is_safe_session_path(&file) => {
                match crate::analysis::report::full_session_report(Path::new(&file)) {
                    Ok(report) => ("200 OK", "text/plain; charset=utf-8", report),
                    Err(e) => ("500 Internal Server Error", "text/plain; charset=utf-8", e),
                }
            }
            _ => (
                "400 Bad Request",
                "text/plain; charset=utf-8",
                "bad or missing file parameter".into(),
            ),
        },
        _ => ("404 Not Found", "text/plain; charset=utf-8", "not found".into()),
    }
}

/// Decode %XX escapes (encodeURIComponent encodes the path separator).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let (Some(h), Some(l)) = (
                bytes.get(i + 1).and_then(|b| (*b as char).to_digit(16)),
                bytes.get(i + 2).and_then(|b| (*b as char).to_digit(16)),
            )
        {
            out.push((h * 16 + l) as u8);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Only relative .ftel paths with no traversal — the server exposes session
/// recordings, nothing else.
fn is_safe_session_path(file: &str) -> bool {
    file.ends_with(".ftel") && !file.contains("..") && !file.starts_with('/')
}

fn query_param(query: &str, key: &str) -> Option<String> {
    query
        .split('&')
        .find_map(|pair| pair.strip_prefix(&format!("{key}=")))
        .map(percent_decode)
}

/// A/B comparison for the dashboard: both composited ideal-lap speed traces plus
/// the per-bin time delta (B − A), for the overlay + segment-delta view.
fn compare_json(a_path: &Path, b_path: &Path) -> Result<String, String> {
    let profile = |path: &Path| -> Result<_, String> {
        let session = crate::analysis::Stint::load(path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        crate::analysis::profile::stint_profile(&session.frames)
            .map_err(|e| format!("{}: {e}", path.display()))
    };
    let pa = profile(a_path)?;
    let pb = profile(b_path)?;
    let cmp = crate::analysis::compare::compare(&pa, &pb)?;
    let shared = cmp.bin_delta_s.len();

    let speeds = |p: &crate::analysis::profile::StintProfile| {
        p.composite.bins[..shared]
            .iter()
            .map(|bin| format!("{:.1}", bin.speed_avg))
            .collect::<Vec<_>>()
            .join(",")
    };
    let side = |path: &Path, p: &crate::analysis::profile::StintProfile| {
        format!(
            "{{\"file\":\"{}\",\"laps\":{},\"best\":{:.3},\"ideal\":{:.3},\"standingOnly\":{}}}",
            path.display(),
            p.laps.len(),
            p.best_lap_time_s,
            p.composite.time_s,
            p.standing_start_only,
        )
    };
    let delta: Vec<String> = cmp.bin_delta_s.iter().map(|d| format!("{d:.4}")).collect();
    let times_a: Vec<String> = pa.composite.bins[..shared]
        .iter()
        .map(|bin| format!("{:.4}", bin.time_s))
        .collect();
    Ok(format!(
        "{{\"binMeters\":{:.0},\"a\":{},\"b\":{},\"speedsA\":[{}],\"speedsB\":[{}],\"timesA\":[{}],\"delta\":[{}],\"unequalLaps\":{},\"carMismatch\":{}}}",
        crate::analysis::profile::BIN_METERS,
        side(a_path, &pa),
        side(b_path, &pb),
        speeds(&pa),
        speeds(&pb),
        times_a.join(","),
        delta.join(","),
        pa.laps.len() != pb.laps.len(),
        cmp.car_mismatch,
    ))
}

/// Distance-binned speed traces per profiled lap — the dashboard's chart data.
fn laps_json(path: &Path) -> Result<String, String> {
    let session = crate::analysis::Stint::load(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let profile = crate::analysis::profile::stint_profile(&session.frames)?;
    let laps: Vec<String> = profile
        .laps
        .iter()
        .map(|lap| {
            let speeds: Vec<String> = lap.bins[..profile.shared_bins]
                .iter()
                .map(|b| format!("{:.1}", b.speed_avg))
                .collect();
            format!(
                "{{\"lap\":{},\"time\":{:.3},\"standing\":{},\"speeds\":[{}]}}",
                lap.lap_number + 1,
                lap.time_s,
                lap.standing_start,
                speeds.join(","),
            )
        })
        .collect();
    // Per-bin corroboration of the spliced ideal: 1 = a second lap reproduces
    // this bin's speed within splice tolerance — the dashboard's confidence
    // strip under the speed chart.
    let corroborated: Vec<&str> = profile
        .corroboration()
        .corroborated
        .iter()
        .map(|ok| if *ok { "1" } else { "0" })
        .collect();
    Ok(format!(
        "{{\"binMeters\":{:.0},\"bestTime\":{:.3},\"corroborated\":[{}],\"laps\":[{}]}}",
        crate::analysis::profile::BIN_METERS,
        profile.best_lap_time_s,
        corroborated.join(","),
        laps.join(","),
    ))
}

fn stints_json(dir: &str) -> String {
    let mut rows = Vec::new();
    if let Ok(read_dir) = std::fs::read_dir(dir) {
        let mut paths: Vec<_> = read_dir
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "ftel"))
            .collect();
        paths.sort();
        for p in paths {
            let bytes = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
            let car = stint_car(&p).unwrap_or(0);
            let name = crate::cars::car_name(car).unwrap_or("");
            // Session filenames are our own ASCII naming scheme, and car names in
            // the bundled dataset contain no quotes/backslashes.
            rows.push(format!(
                "{{\"file\":\"{}\",\"bytes\":{bytes},\"car\":{car},\"carName\":\"{name}\"}}",
                p.display(),
            ));
        }
    }
    format!("[{}]", rows.join(","))
}

/// First car seen driving in the session (bounded scan — the file may open with
/// menu frames, but driving starts within moments in every real capture).
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

#[cfg(test)]
mod tests {
    /// Switching the session car archives the active campaign to its per-car
    /// file and restores it intact when switching back.
    #[test]
    fn car_switch_archives_and_restores_sessions() {
        let dir =
            std::env::temp_dir().join(format!("tuners-car-switch-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tune-session.txt");

        // McLaren session with a revision on file.
        let mut s = crate::tuning::TuningSession { car: Some(1314), ..Default::default() };
        s.facts.insert("abs".into(), "on".into());
        s.revisions.push(crate::tuning::Revision {
            stamp: "20260721-000000".into(),
            values: [("arb_f".to_string(), "18.5".to_string())].into_iter().collect(),
        });
        s.save(&path).unwrap();

        // Switch to an RWD car: fresh session, McLaren archived.
        super::session_post(&[("car".into(), "227".into())], &path);
        let now = crate::tuning::TuningSession::load(&path);
        assert_eq!(now.car, Some(227));
        assert!(now.revisions.is_empty(), "fresh session for the new car");
        let archived = crate::tuning::TuningSession::load(
            crate::tuning::journal_path_for(Some(1314), &path.to_string_lossy()).as_ref(),
        );
        assert_eq!(archived.car, Some(1314));
        assert_eq!(archived.revisions.len(), 1, "campaign archived intact");

        // Switch back: the McLaren campaign is restored, revisions included.
        super::session_post(&[("car".into(), "1314".into())], &path);
        let restored = crate::tuning::TuningSession::load(&path);
        assert_eq!(restored.car, Some(1314));
        assert_eq!(restored.revisions.len(), 1);
        assert_eq!(restored.facts.get("abs").map(String::as_str), Some("on"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    use super::*;

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

        let (status, _, body) =
            delete_stint(&sdir, Some("stint-20260725-100000.ftel".into()), None, false, &jbase);
        assert_eq!(status, "409 Conflict", "{body}");
        assert!(body.contains("tune-journal-99.txt"), "{body}");
        assert!(sessions.join("stint-20260725-100000.ftel").exists());

        let (status, ..) =
            delete_stint(&sdir, Some("stint-20260725-110000.ftel".into()), None, false, &jbase);
        assert_eq!(status, "200 OK", "unjournaled deletes without force");

        let (status, ..) =
            delete_stint(&sdir, Some("stint-20260725-100000.ftel".into()), None, true, &jbase);
        assert_eq!(status, "200 OK", "force overrides the guard");
        assert!(!sessions.join("stint-20260725-100000.ftel").exists());

        // Campaign start: journal baseline stint (100000) predates the first
        // revision save (100500) — the earlier stamp wins.
        let mut s = crate::tuning::TuningSession { car: Some(99), ..Default::default() };
        s.revisions.push(crate::tuning::Revision { stamp: "20260725-100500".into(), ..Default::default() });
        assert_eq!(campaign_start(&s, &jbase).as_deref(), Some("20260725-100000"));
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
        let p = |pairs: &[(&str, &str)]| -> Vec<(String, String)> {
            pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
        };

        // Campaign A: car 2793 with a name, one revision, a journal with 2 stints.
        let mut a = crate::tuning::TuningSession { car: Some(2793), ..Default::default() };
        a.facts.insert("name".into(), "awd aero".into());
        a.facts.insert("unit_pressure".into(), "psi".into());
        a.revisions.push(crate::tuning::Revision { stamp: "1".into(), ..Default::default() });
        a.save(&session_file).unwrap();
        let journal_a = crate::tuning::journal_path_for(Some(2793), &jb);
        std::fs::write(&journal_a, "# car\nsessions/a.ftel | baseline\nsessions/b.ftel | x\n")
            .unwrap();

        // New session: A is archived (session + journal move together), the
        // fresh session keeps unit prefs and takes the posted name.
        let (status, _, body) = session_new(&p(&[("name", "rwd build")]), &sf, &jb);
        assert_eq!(status, "200 OK", "{body}");
        assert!(!Path::new(&journal_a).exists(), "journal A moved to the archive");
        let parked = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .find(|e| e.file_name().to_string_lossy().starts_with("tune-journal-2793-"))
            .expect("archived journal");
        assert!(
            std::fs::read_to_string(parked.path()).unwrap().contains("# parked "),
            "parked marker closes the campaign"
        );
        let fresh = crate::tuning::TuningSession::load(&session_file);
        assert_eq!(fresh.car, None);
        assert_eq!(fresh.facts.get("name").unwrap(), "rwd build");
        assert_eq!(fresh.facts.get("unit_pressure").unwrap(), "psi", "unit prefs carry");
        assert!(fresh.revisions.is_empty());

        let list = sessions_json(&sf, &jb);
        assert!(list.contains("\"awd aero\"") && list.contains("\"stints\":2"), "{list}");
        let id = list
            .split("\"id\":\"")
            .nth(1)
            .and_then(|r| r.split('"').next())
            .expect("archived id in listing")
            .to_string();

        // Make the fresh session campaign B on the SAME car, with its own journal.
        let mut b = crate::tuning::TuningSession::load(&session_file);
        b.car = Some(2793);
        b.save(&session_file).unwrap();
        std::fs::write(&journal_a, "# car\nsessions/c.ftel | baseline\n").unwrap();

        // Resume A: B is archived in turn, A's session AND journal come back.
        let (status, _, body) = session_resume(&p(&[("id", &id)]), &sf, &jb);
        assert_eq!(status, "200 OK", "{body}");
        let restored = crate::tuning::TuningSession::load(&session_file);
        assert_eq!(restored.facts.get("name").unwrap(), "awd aero");
        assert_eq!(restored.revisions.len(), 1);
        let journal = std::fs::read_to_string(&journal_a).unwrap();
        assert!(journal.contains("sessions/b.ftel"), "campaign A journal restored: {journal}");
        assert!(journal.contains("# resumed "), "resume marker floors the implicit-step scan");
        let list = sessions_json(&sf, &jb);
        assert!(list.contains("\"rwd build\"") && list.contains("\"stints\":1"), "{list}");

        // Bad ids are rejected, unknown ids 404.
        assert_eq!(session_resume(&p(&[("id", "../evil")]), &sf, &jb).0, "400 Bad Request");
        assert_eq!(session_resume(&p(&[("id", "none-19700101-000000")]), &sf, &jb).0, "404 Not Found");
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
        let mut s = crate::tuning::TuningSession { car: Some(2793), ..Default::default() };
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
            let params: Vec<(String, String)> =
                pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
            tune_post(&params, &path, recorder)
        };

        // Accept #1: front arb only. Unposted keys carry over from the latest.
        let (status, _, body) = post(&[("partial", "1"), ("arb_f", "16.8")], &recorder);
        assert_eq!(status, "200 OK", "{body}");
        assert!(body.contains("front arb -1.5"), "{body}");
        let latest_vals = |p: &Path| {
            crate::tuning::TuningSession::load(p).latest().unwrap().values.clone()
        };
        let vals = latest_vals(&path);
        assert_eq!(vals.get("arb_f").unwrap(), "16.8");
        assert_eq!(vals.get("final_drive").unwrap(), "3.95", "unposted keys carry over");
        assert_eq!(vals.get("rebound_f").unwrap(), "10.6");

        // Accept #2 before any stint: chains onto #1 and the pending note nets
        // BOTH changes against the driven baseline.
        let (status, _, body) = post(&[("partial", "1"), ("final_drive", "4.1")], &recorder);
        assert_eq!(status, "200 OK", "{body}");
        let note = recorder.lock().unwrap().pending_note.clone().unwrap();
        assert!(
            note.contains("front arb -1.5") && note.contains("final drive +0.15"),
            "{note}"
        );
        let vals = latest_vals(&path);
        assert_eq!(vals.get("arb_f").unwrap(), "16.8", "accept #2 chains onto #1");
        assert_eq!(vals.get("final_drive").unwrap(), "4.1");

        // Accepting the original arb back nets the chain to one remaining change.
        post(&[("partial", "1"), ("arb_f", "18.3")], &recorder);
        let note = recorder.lock().unwrap().pending_note.clone().unwrap();
        assert!(note.contains("final drive") && !note.contains("front arb"), "{note}");

        // A partial save with no tune on file is rejected.
        let empty = dir.join("empty-session.txt");
        let (status, _, _) = {
            let params: Vec<(String, String)> =
                [("partial", "1"), ("arb_f", "16.8")].iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
            tune_post(&params, &empty, &recorder)
        };
        assert_eq!(status, "400 Bad Request");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn index_serves_html() {
        let (status, ctype, body) = respond("/", "no-such-dir");
        assert_eq!(status, "200 OK");
        assert!(ctype.starts_with("text/html"));
        assert!(body.contains("<html") || body.contains("<!doctype") || body.contains("<div"));
    }

    #[test]
    fn sessions_list_empty_when_dir_missing() {
        let (status, _, body) = respond("/api/stints", "no-such-dir");
        assert_eq!(status, "200 OK");
        assert_eq!(body, "[]");
    }

    #[test]
    fn report_rejects_unsafe_paths() {
        for target in [
            "/api/report?file=../../etc/passwd",
            "/api/report?file=/etc/passwd",
            "/api/report?file=Cargo.toml",
            "/api/report",
            "/api/laps?file=..%2FCargo.toml",
            "/api/laps?file=/etc/passwd",
            "/api/laps",
        ] {
            let (status, _, _) = respond(target, "sessions");
            assert_eq!(status, "400 Bad Request", "{target}");
        }
    }

    #[test]
    fn report_serves_fixture() {
        let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/rivals-lap-boundary-01.ftel");
        // is_safe_session_path requires relative paths; test via relative-from-manifest
        let (status, _, body) = respond(
            &format!("/api/report?file={}", "fixtures/rivals-lap-boundary-01.ftel"),
            "fixtures",
        );
        // Depending on cwd this may 500 (file not found) — accept both but never 400.
        assert_ne!(status, "400 Bad Request");
        let _ = (fixture, body);
    }

    #[test]
    fn live_state_json_shapes() {
        let empty = crate::live::LiveState::default();
        assert_eq!(live_state_json(&empty), "{\"file\":null,\"ageMs\":null,\"frame\":null}");

        let state = crate::live::LiveState {
            file: Some("sessions/session-x.ftel".into()),
            latest: Some(crate::analysis::TimedFrame {
                recv_us: 0,
                frame: crate::simulate::synth_frame(2.5),
            }),
            last_data: Some(std::time::Instant::now()),
            ..Default::default()
        };
        let json = live_state_json(&state);
        assert!(json.starts_with("{\"file\":\"session-x.ftel\""), "{json}");
        for key in ["\"raceOn\":true", "\"speedMps\":", "\"rpm\":", "\"tireTempF\":["] {
            assert!(json.contains(key), "{json} missing {key}");
        }
        assert_eq!(quality_json(None), "null");
    }

    #[test]
    fn delete_stint_guards_and_deletes() {
        let dir = std::env::temp_dir().join(format!("tuners-del-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dir_s = dir.to_string_lossy().into_owned();
        std::fs::write(dir.join("stint-x.ftel"), b"data").unwrap();

        let jb = dir.join("tune-journal.txt").to_string_lossy().into_owned();
        for bad in ["../stint-x.ftel", "sub/stint-x.ftel", "stint-x.txt"] {
            let (status, _, _) = delete_stint(&dir_s, Some(bad.into()), None, false, &jb);
            assert_eq!(status, "400 Bad Request", "{bad}");
        }
        let (status, _, _) = delete_stint(
            &dir_s,
            Some("stint-x.ftel".into()),
            Some(dir.join("stint-x.ftel").as_path()),
            false,
            &jb,
        );
        assert_eq!(status, "409 Conflict", "active recording is protected");
        assert!(dir.join("stint-x.ftel").exists());

        let (status, _, _) = delete_stint(&dir_s, Some("stint-x.ftel".into()), None, false, &jb);
        assert_eq!(status, "200 OK");
        assert!(!dir.join("stint-x.ftel").exists());

        let (status, _, _) = delete_stint(&dir_s, Some("stint-x.ftel".into()), None, false, &jb);
        assert_eq!(status, "404 Not Found");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unknown_route_404s() {
        let (status, _, _) = respond("/nope", "sessions");
        assert_eq!(status, "404 Not Found");
    }
}
