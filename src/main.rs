use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;
use tuners::{analysis, capture, replay, simulate};

const USAGE: &str = "\
tuners — FH6 tuning assistant (telemetry capture spike)

USAGE:
  tuners capture  [--port 20440] [--out sessions] [--packets N] [--duration SECS]
                    listen for Data Out packets, record a session, show live status
  tuners replay   <session-file>
                    decode a recorded session and print a summary (exits non-zero on errors)
  tuners analyze  <session-file>
                    per-stint tuning observations: tires, grip, suspension, gearing
  tuners compare  <session-A> <session-B>
                    tune A/B: lap-time delta, where it comes from, mistakes excluded
  tuners recommend <session-file>
                    directional tune advice with evidence (blind mode: no tune input)
  tuners advise   [journal-file]
                    history-aware advice from a tuning journal (default tune-journal.txt);
                    journal lines: <session-file> | <change since previous session>
  tuners serve    [--port 8080] [--sessions sessions]
                    local web dashboard: session list, reports, and a live view
                    of the session `tuners capture` is currently recording
  tuners simulate [--addr 127.0.0.1] [--port 20440] [--packets 600] [--rate 60] [--timescale 1]
                    send synthetic telemetry (stand-in for the game); timescale
                    compresses in-game time for headless lap testing
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match dispatch(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn dispatch(args: &[String]) -> Result<(), String> {
    let Some(cmd) = args.first() else {
        print!("{USAGE}");
        return Ok(());
    };
    match cmd.as_str() {
        "capture" => cmd_capture(&args[1..]),
        "replay" => cmd_replay(&args[1..]),
        "analyze" => cmd_analyze(&args[1..]),
        "compare" => cmd_compare(&args[1..]),
        "recommend" => cmd_recommend(&args[1..]),
        "advise" => cmd_advise(&args[1..]),
        "serve" => cmd_serve(&args[1..]),
        "simulate" => cmd_simulate(&args[1..]),
        "help" | "-h" | "--help" => {
            print!("{USAGE}");
            Ok(())
        }
        other => Err(format!("unknown command '{other}' (try 'tuners help')")),
    }
}

fn cmd_capture(args: &[String]) -> Result<(), String> {
    let mut opts = capture::CaptureOpts {
        port: 20440,
        out_dir: PathBuf::from("sessions"),
        max_packets: None,
        max_duration: None,
    };
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--port" => opts.port = parse(flag, it.next())?,
            "--out" => opts.out_dir = PathBuf::from(value(flag, it.next())?),
            "--packets" => opts.max_packets = Some(parse(flag, it.next())?),
            "--duration" => {
                opts.max_duration = Some(Duration::from_secs_f64(parse(flag, it.next())?))
            }
            other => return Err(format!("unknown flag '{other}' for capture")),
        }
    }
    capture::run(&opts).map(|_| ()).map_err(|e| e.to_string())
}

fn cmd_replay(args: &[String]) -> Result<(), String> {
    match args {
        [path] => replay::run(path.as_ref()),
        _ => Err("usage: tuners replay <session-file>".into()),
    }
}

fn cmd_analyze(args: &[String]) -> Result<(), String> {
    let [path] = args else {
        return Err("usage: tuners analyze <session-file>".into());
    };
    print!("{}", analysis::report::full_session_report(path.as_ref())?);
    Ok(())
}

fn cmd_recommend(args: &[String]) -> Result<(), String> {
    let [path] = args else {
        return Err("usage: tuners recommend <session-file>".into());
    };
    let session = analysis::Session::load(path.as_ref()).map_err(|e| format!("{path}: {e}"))?;
    let segments = analysis::driving_segments(&session.frames, 5.0);
    // Advise from the longest stint — the most driving under one set of conditions.
    let Some(stint) = segments.iter().max_by_key(|s| s.len()) else {
        return Err("no driving stints of 5s or longer found".into());
    };

    let overall = analysis::metrics::stint_metrics(stint);
    let laps = analysis::split_laps(stint);
    let per_lap: Vec<_> = laps
        .iter()
        .filter(|l| l.time_s.is_some() && !l.standing_start)
        .map(|l| analysis::metrics::stint_metrics(l.frames))
        .collect();

    println!(
        "{path}: advice from a {:.0}s stint ({} flying lap(s))\n",
        overall.duration_s,
        per_lap.len(),
    );
    print!(
        "{}",
        analysis::report::render_recommendations(&analysis::recommend::recommend(
            &overall, &per_lap
        ))
    );
    Ok(())
}

/// Balance of a session's longest stint: (index, front, rear mean |slip angle| as
/// fraction of grip limit) — shown per trajectory row so the setting -> balance ->
/// lap-time mapping is visible, absolute operating point included.
fn session_balance(session: &analysis::Session) -> Option<(f32, f32, f32)> {
    let segments = analysis::driving_segments(&session.frames, 5.0);
    let stint = segments.iter().max_by_key(|s| s.len())?;
    let m = analysis::metrics::stint_metrics(stint);
    Some((m.understeer_index?, m.cornering_front_slip?, m.cornering_rear_slip?))
}

fn cmd_serve(args: &[String]) -> Result<(), String> {
    let mut port: u16 = 8080;
    let mut sessions_dir = "sessions".to_string();
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--port" => port = parse(flag, it.next())?,
            "--sessions" => sessions_dir = value(flag, it.next())?.clone(),
            other => return Err(format!("unknown flag '{other}' for serve")),
        }
    }
    tuners::serve::run(port, sessions_dir).map_err(|e| e.to_string())
}

fn cmd_advise(args: &[String]) -> Result<(), String> {
    let journal_path = match args {
        [] => "tune-journal.txt",
        [p] => p.as_str(),
        _ => return Err("usage: tuners advise [journal-file]".into()),
    };
    let text = std::fs::read_to_string(journal_path).map_err(|e| format!("{journal_path}: {e}"))?;
    let entries = analysis::journal::parse_journal(&text);
    if entries.is_empty() {
        return Err(format!("{journal_path}: no sessions listed"));
    }

    // Load and profile every session, in journal (chronological) order.
    let mut sessions = Vec::new();
    for entry in &entries {
        let session = analysis::Session::load(entry.path.as_ref())
            .map_err(|e| format!("{}: {e}", entry.path))?;
        let profile = analysis::profile::session_profile(&session.frames)
            .map_err(|e| format!("{}: {e}", entry.path))?;
        sessions.push((entry, session, profile));
    }

    println!("tuning trajectory ({} sessions):", sessions.len());
    let mut last_step: Option<(analysis::journal::Change, analysis::journal::Outcome, &str)> = None;
    for i in 0..sessions.len() {
        let (entry, _, profile) = &sessions[i];
        let mut line = format!(
            "  {}. {}  {} lap(s)  best {}  ideal {}",
            i + 1,
            entry.path,
            profile.laps.len(),
            tuners::util::format_lap_time(profile.best_lap_time_s),
            tuners::util::format_lap_time(profile.composite.time_s),
        );
        if let Some((idx, front, rear)) = session_balance(&sessions[i].1) {
            line.push_str(&format!(
                "  balance {idx:+.2} (F {:.0}%/R {:.0}% of limit)",
                front * 100.0,
                rear * 100.0,
            ));
        }
        if let Some(note) = &entry.note {
            line.push_str(&format!("  — {note}"));
        }
        if i > 0 {
            let prev = &sessions[i - 1].2;
            match analysis::compare::compare(prev, &sessions[i].2) {
                Ok(cmp) => {
                    let outcome = analysis::journal::judge(cmp.ideal_delta_s);
                    let word = match outcome {
                        analysis::journal::Outcome::Improved(_) => "improved",
                        analysis::journal::Outcome::Worsened(_) => "WORSE",
                        _ => "inconclusive",
                    };
                    line.push_str(&format!("  → {word} (ideal {:+.2}s)", cmp.ideal_delta_s));
                    if prev.laps.len() != sessions[i].2.laps.len() {
                        line.push_str("  [unequal lap counts]");
                    }
                    if let Some(note) = &entry.note
                        && let Some(change) = analysis::journal::parse_change(note)
                    {
                        last_step = Some((change, outcome, note));
                    }
                }
                Err(e) => line.push_str(&format!("  → not comparable ({e})")),
            }
        }
        println!("{line}");
    }

    // Blind recommendations for the latest session, reconciled with the last step.
    let (last_entry, last_session, _) = sessions.last().unwrap();
    let segments = analysis::driving_segments(&last_session.frames, 5.0);
    let stint = segments
        .iter()
        .max_by_key(|s| s.len())
        .ok_or("latest session has no driving stints")?;
    let overall = analysis::metrics::stint_metrics(stint);
    let per_lap: Vec<_> = analysis::split_laps(stint)
        .iter()
        .filter(|l| l.time_s.is_some() && !l.standing_start)
        .map(|l| analysis::metrics::stint_metrics(l.frames))
        .collect();
    let mut recs = analysis::recommend::recommend(&overall, &per_lap);
    if let Some((change, outcome, note)) = last_step {
        analysis::journal::reconcile(&mut recs, change, outcome, note);
    }
    println!("\nadvice for {}:\n", last_entry.path);
    print!("{}", analysis::report::render_recommendations(&recs));
    Ok(())
}

fn cmd_compare(args: &[String]) -> Result<(), String> {
    let [path_a, path_b] = args else {
        return Err("usage: tuners compare <session-A> <session-B>".into());
    };
    let load = |path: &String| -> Result<_, String> {
        let session = analysis::Session::load(path.as_ref()).map_err(|e| format!("{path}: {e}"))?;
        analysis::profile::session_profile(&session.frames).map_err(|e| format!("{path}: {e}"))
    };
    let a = load(path_a)?;
    let b = load(path_b)?;
    let cmp = analysis::compare::compare(&a, &b)?;
    println!("A = {path_a}\nB = {path_b}\n");
    print!("{}", analysis::compare::render(&a, &b, &cmp));
    Ok(())
}

fn cmd_simulate(args: &[String]) -> Result<(), String> {
    let mut opts = simulate::SimOpts {
        addr: "127.0.0.1".into(),
        port: 20440,
        packets: 600,
        rate: 60.0,
        timescale: 1.0,
    };
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--addr" => opts.addr = value(flag, it.next())?.clone(),
            "--port" => opts.port = parse(flag, it.next())?,
            "--packets" => opts.packets = parse(flag, it.next())?,
            "--rate" => opts.rate = parse(flag, it.next())?,
            "--timescale" => opts.timescale = parse(flag, it.next())?,
            other => return Err(format!("unknown flag '{other}' for simulate")),
        }
    }
    simulate::run(&opts).map_err(|e| e.to_string())
}

fn value<'a>(flag: &str, v: Option<&'a String>) -> Result<&'a String, String> {
    v.ok_or_else(|| format!("{flag} requires a value"))
}

fn parse<T: std::str::FromStr>(flag: &str, v: Option<&String>) -> Result<T, String> {
    value(flag, v)?
        .parse()
        .map_err(|_| format!("invalid value for {flag}"))
}
