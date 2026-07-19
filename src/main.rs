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
  tuners simulate [--addr 127.0.0.1] [--port 20440] [--packets 600] [--rate 60]
                    send synthetic telemetry (stand-in for the game)
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
    let session = analysis::Session::load(path.as_ref()).map_err(|e| format!("{path}: {e}"))?;
    if session.decode_errors > 0 {
        eprintln!("warning: {} packets failed to decode", session.decode_errors);
    }
    let stints = analysis::split_stints(&session.frames, 5.0);
    if stints.is_empty() {
        return Err(format!(
            "no driving stints of 5s or longer found ({} frames total)",
            session.frames.len()
        ));
    }
    println!("{path}: {} stint(s)\n", stints.len());
    for (i, stint) in stints.iter().enumerate() {
        let metrics = analysis::metrics::stint_metrics(stint);
        println!("{}", analysis::report::render_stint(i + 1, &metrics));
        let laps = analysis::split_laps(stint);
        if laps.len() > 1 {
            println!("{}", analysis::report::render_laps(&laps));
        }
    }
    Ok(())
}

fn cmd_recommend(args: &[String]) -> Result<(), String> {
    let [path] = args else {
        return Err("usage: tuners recommend <session-file>".into());
    };
    let session = analysis::Session::load(path.as_ref()).map_err(|e| format!("{path}: {e}"))?;
    let stints = analysis::split_stints(&session.frames, 5.0);
    // Advise from the longest stint — the most driving under one set of conditions.
    let Some(stint) = stints.iter().max_by_key(|s| s.len()) else {
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
    };
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--addr" => opts.addr = value(flag, it.next())?.clone(),
            "--port" => opts.port = parse(flag, it.next())?,
            "--packets" => opts.packets = parse(flag, it.next())?,
            "--rate" => opts.rate = parse(flag, it.next())?,
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
