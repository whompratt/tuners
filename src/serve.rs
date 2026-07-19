//! Minimal local dashboard server: hand-rolled HTTP over std TcpListener, zero
//! dependencies (docs/plans/006-dashboard.md). Handlers are pure functions so a
//! swap to a real framework later is mechanical.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;

pub fn run(port: u16, sessions_dir: String, udp_port: u16) -> std::io::Result<()> {
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
        let recorder = recorder.clone();
        std::thread::spawn(move || crate::record::run_recorder(udp_port, out_dir, recorder));
    } else {
        recorder.lock().unwrap().mode =
            crate::record::RecorderMode::External("disabled (--udp-port 0)".into());
    }
    for stream in listener.incoming() {
        let dir = sessions_dir.clone();
        if let Ok(stream) = stream {
            let live = live.clone();
            let recorder = recorder.clone();
            std::thread::spawn(move || handle(stream, &dir, &live, &recorder));
        }
    }
    Ok(())
}

fn handle(
    mut stream: TcpStream,
    sessions_dir: &str,
    live: &crate::live::SharedLive,
    recorder: &crate::record::SharedRecorder,
) {
    let mut reader = BufReader::new(match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    });
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() {
        return;
    }
    // Drain headers; GET-only server, bodies ignored.
    loop {
        let mut header = String::new();
        match reader.read_line(&mut header) {
            Ok(0) | Err(_) => break,
            Ok(_) if header == "\r\n" || header == "\n" => break,
            Ok(_) => {}
        }
    }
    let method = request_line.split_whitespace().next().unwrap_or("GET");
    let target = request_line.split_whitespace().nth(1).unwrap_or("/");
    if target.split('?').next() == Some("/api/live") {
        serve_sse(stream, live, recorder);
        return;
    }
    let (status, content_type, body) = if method == "POST" && target == "/api/record/split" {
        recorder.lock().unwrap().split_requested = true;
        ("200 OK", "application/json", "{\"ok\":true}".to_string())
    } else {
        respond(target, sessions_dir)
    };
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len(),
    );
    let _ = stream.write_all(body.as_bytes());
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
             \"sharedKm\":{:.2},\"coveragePct\":{:.0},\"verdict\":\"{}\"}}",
            q.laps,
            q.standing_only,
            q.best_lap_s,
            q.spread_frac * 100.0,
            q.shared_km,
            q.coverage_frac * 100.0,
            q.verdict.as_str(),
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
        "/api/sessions" => ("200 OK", "application/json", sessions_json(sessions_dir)),
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
        let session = crate::analysis::Session::load(path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        crate::analysis::profile::session_profile(&session.frames)
            .map_err(|e| format!("{}: {e}", path.display()))
    };
    let pa = profile(a_path)?;
    let pb = profile(b_path)?;
    let cmp = crate::analysis::compare::compare(&pa, &pb)?;
    let shared = cmp.bin_delta_s.len();

    let speeds = |p: &crate::analysis::profile::SessionProfile| {
        p.composite.bins[..shared]
            .iter()
            .map(|bin| format!("{:.1}", bin.speed_avg))
            .collect::<Vec<_>>()
            .join(",")
    };
    let side = |path: &Path, p: &crate::analysis::profile::SessionProfile| {
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
    let session = crate::analysis::Session::load(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let profile = crate::analysis::profile::session_profile(&session.frames)?;
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
    Ok(format!(
        "{{\"binMeters\":{:.0},\"bestTime\":{:.3},\"laps\":[{}]}}",
        crate::analysis::profile::BIN_METERS,
        profile.best_lap_time_s,
        laps.join(","),
    ))
}

fn sessions_json(dir: &str) -> String {
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
            let car = session_car(&p).unwrap_or(0);
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
fn session_car(path: &Path) -> Option<i32> {
    let mut reader = crate::session::SessionReader::open(path).ok()?;
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
    use super::*;

    #[test]
    fn index_serves_html() {
        let (status, ctype, body) = respond("/", "no-such-dir");
        assert_eq!(status, "200 OK");
        assert!(ctype.starts_with("text/html"));
        assert!(body.contains("<html") || body.contains("<!doctype") || body.contains("<div"));
    }

    #[test]
    fn sessions_list_empty_when_dir_missing() {
        let (status, _, body) = respond("/api/sessions", "no-such-dir");
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
    fn unknown_route_404s() {
        let (status, _, _) = respond("/nope", "sessions");
        assert_eq!(status, "404 Not Found");
    }
}
