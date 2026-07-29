use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;
use tuners::{
    advice, analysis,
    telemetry::{capture, replay, simulate},
};

const USAGE: &str = "\
tuners: FH6 tuning assistant (telemetry capture spike)

USAGE:
  tuners capture  [--port 20440] [--out sessions] [--packets N] [--duration SECS]
                    listen for Data Out packets, record a stint, show live status
  tuners replay   <stint-file>
                    decode a recorded stint and print a summary (exits non-zero on errors)
  tuners analyze  <stint-file> [--units imperial|metric|uk]
                    per-stint tuning observations: tires, grip, suspension, gearing
                    (units default to the active session's display prefs)
  tuners compare  <stint-A> <stint-B>
                    tune A/B: lap-time delta, where it comes from, mistakes excluded
  tuners recommend <stint-file>
                    directional tune advice with evidence (blind mode: no tune input)
  tuners advise   [journal-file]
                    history-aware advice from a tuning journal. Default: the
                    active session car's journal (tune-journal-<car>.txt),
                    falling back to tune-journal.txt with no session;
                    journal lines: <stint-file> | <change since previous stint>
  tuners map      [--out effect-map.tsv] [--rebuild]
                    refresh the cross-campaign effect map: harvest campaigns
                    whose journals/sessions/library changed (all of them with
                    --rebuild) into per family x direction behaviour
                    distributions, write the sample file, and print the
                    aggregated summary. The app refreshes this automatically
                    while telemetry is idle
  tuners simulate [--addr 127.0.0.1] [--port 20440] [--packets 600] [--rate 60] [--timescale 1]
                    send synthetic telemetry (stand-in for the game); timescale
                    compresses in-game time for headless lap testing
  tuners export   <stint-file> [--out .]
                    write the stint's telemetry-collection bundle
                    (bundle-<car>-<stamp>.tar.zst: raw recording + free-text-
                    stripped session/journal) for manual sharing or upload
  tuners ingest   <dir> [--library library] [--quarantine quarantine]
                    strict-validate received bundles (rclone-synced inbox or
                    hand-delivered exports) and file survivors per sender;
                    failures are quarantined with a written reason
  tuners receive  [--port 8090] [--bind 127.0.0.1] [--root inbox]
                  [--tokens receive-tokens.txt] [--blocklist receive-blocklist.txt]
                  [--max-mb 64] [--daily-mb 512] [--global-mb 20480]
                    telemetry-collection endpoint, local twin of
                    worker/: bundle PUTs stored per sender under --root. Open
                    mode by default (client-generated 64-hex tokens, sender =
                    sha256 prefix); a --tokens file that exists = lockdown
                    mode. --issue <sender-id> mints a lockdown token
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
        "map" => cmd_map(&args[1..]),
        "simulate" => cmd_simulate(&args[1..]),
        "receive" => cmd_receive(&args[1..]),
        "export" => cmd_export(&args[1..]),
        "ingest" => cmd_ingest(&args[1..]),
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
        out_dir: tuners::util::data_path("sessions"),
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
        _ => Err("usage: tuners replay <stint-file>".into()),
    }
}

/// Strip a `--units imperial|metric|uk` flag from args and set the report
/// display units. Default: the active session's unit prefs (the UK user's
/// psi/°C/mph carry into the CLI), imperial with no session.
fn apply_units(args: &[String]) -> Result<Vec<String>, String> {
    let mut rest = Vec::new();
    let mut choice: Option<String> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "--units" {
            choice = Some(value(a, it.next())?.clone());
        } else {
            rest.push(a.clone());
        }
    }
    let units = match choice.as_deref() {
        Some("imperial") => tuners::util::DisplayUnits {
            temp_c: false,
            speed_kmh: false,
        },
        Some("metric") => tuners::util::DisplayUnits {
            temp_c: true,
            speed_kmh: true,
        },
        Some("uk") => tuners::util::DisplayUnits {
            temp_c: true,
            speed_kmh: false,
        },
        Some(other) => return Err(format!("--units {other}: use imperial, metric, or uk")),
        None => {
            let s = tuners::advice::tuning::TuningSession::load(&tuners::util::data_path(
                "tune-session.txt",
            ));
            tuners::util::DisplayUnits {
                temp_c: s.facts.get("unit_temp").map(String::as_str) == Some("c"),
                speed_kmh: s.facts.get("unit_speed").map(String::as_str) == Some("kmh"),
            }
        }
    };
    tuners::util::set_display_units(units);
    Ok(rest)
}

fn cmd_analyze(args: &[String]) -> Result<(), String> {
    let args = apply_units(args)?;
    let [path] = &args[..] else {
        return Err("usage: tuners analyze <stint-file> [--units imperial|metric|uk]".into());
    };
    print!("{}", analysis::report::full_session_report(path.as_ref())?);
    Ok(())
}

fn cmd_recommend(args: &[String]) -> Result<(), String> {
    let args = apply_units(args)?;
    let [path] = &args[..] else {
        return Err("usage: tuners recommend <stint-file>".into());
    };
    let session = analysis::Stint::load(path.as_ref()).map_err(|e| format!("{path}: {e}"))?;
    let segments = analysis::driving_segments(&session.frames, 5.0);
    // Advise from the longest stint: the most driving under one set of conditions.
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
        analysis::report::render_recommendations(&advice::recommend::recommend(
            &overall,
            &per_lap,
            &Default::default(),
        ))
    );
    Ok(())
}

fn cmd_advise(args: &[String]) -> Result<(), String> {
    let journal_path = match args {
        [] => {
            // The journal belongs to the session: with an active session car,
            // the default resolves to that car's journal file.
            let session = tuners::advice::tuning::TuningSession::load(&tuners::util::data_path(
                "tune-session.txt",
            ));
            let path = tuners::advice::tuning::journal_path_for(
                session.car,
                &tuners::util::data_path("tune-journal.txt").to_string_lossy(),
            );
            if let Some(car) = session.car {
                let name = tuners::cars::car_name(car).unwrap_or("unknown car");
                println!("journal: {path} (session car: {name})");
            }
            path
        }
        [p] => p.clone(),
        _ => return Err("usage: tuners advise [journal-file]".into()),
    };
    let view = tuners::advice::advise::advise(
        &journal_path,
        &tuners::util::data_path("tune-session.txt"),
        &tuners::util::data_path("sessions").to_string_lossy(),
    )?;

    if view.journal.is_none() {
        println!(
            "no journal yet: blind advice on the latest stint; the journal starts \
             with your first tune change\n"
        );
    } else {
        println!("tuning trajectory ({} stints):", view.steps.len());
        for (i, step) in view.steps.iter().enumerate() {
            let mut line = format!(
                "  {}. {}  {} lap(s)  best {}  ideal {}",
                i + 1,
                step.path,
                step.laps,
                tuners::util::format_lap_time(step.best_s),
                tuners::util::format_lap_time(step.ideal_s),
            );
            if let Some((idx, front, rear)) = step.balance {
                line.push_str(&format!(
                    "  balance {idx:+.2} (F {:.0}%/R {:.0}% of limit)",
                    front * 100.0,
                    rear * 100.0,
                ));
            }
            if let Some(note) = &step.note {
                line.push_str(&format!("  - {note}"));
            }
            if let Some((f, r)) = step.pos {
                line.push_str(&format!("  [pos F {f:+.1} / R {r:+.1}]"));
            }
            match &step.outcome {
                Some(Ok((word, delta, unequal))) => {
                    line.push_str(&format!("  → {word} (ideal {delta:+.2}s)"));
                    if let Some((e, x, st)) = step.split {
                        line.push_str(&format!(
                            "  [entry {e:+.2}s / exit {x:+.2}s / straights {st:+.2}s]"
                        ));
                    }
                    if *unequal {
                        line.push_str("  [unequal lap counts]");
                    }
                }
                Some(Err(e)) => line.push_str(&format!("  → not comparable ({e})")),
                None => {}
            }
            if let Some(a) = &step.anchor {
                if a.areas.is_empty() {
                    line.push_str(&format!(
                        "  [same setup as step {}: {:+.2}s = drift]",
                        a.vs_step, a.delta_s
                    ));
                } else {
                    line.push_str(&format!(
                        "  [vs step {} ({}): {} {:+.2}s{}]",
                        a.vs_step,
                        a.areas,
                        a.word,
                        a.delta_s,
                        if a.weak { ", single-lap" } else { "" },
                    ));
                }
            }
            println!("{line}");
        }
        let movers = |fx: &tuners::analysis::effects::Effects| {
            let m = tuners::analysis::effects::movers(fx, Some(&view.effect_floor));
            (!m.is_empty()).then(|| tuners::analysis::effects::describe(&m))
        };
        if let Some(a) = &view.anchor {
            if a.areas.is_empty() {
                println!(
                    "  cleanest comparison for the last stint: step {} has the SAME \
                     setup, so the {:+.2}s ideal delta is pure driver/track drift",
                    a.vs_step, a.delta_s,
                );
            } else {
                println!(
                    "  cleanest comparison for the last stint: vs step {} (setups \
                     differ only in {}: {}) → {} {:+.2}s  [entry {:+.2}s / exit \
                     {:+.2}s / straights {:+.2}s]{}{}",
                    a.vs_step,
                    a.areas,
                    a.changes,
                    a.word,
                    a.delta_s,
                    a.split.0,
                    a.split.1,
                    a.split.2,
                    if a.weak {
                        "  [single-lap side, corroborate]"
                    } else {
                        ""
                    },
                    if a.reconciled {
                        ""
                    } else {
                        "  [multi-area, informational]"
                    },
                );
                if let Some(m) = movers(&a.effects) {
                    println!("    behaviour that moved with it (above noise): {m}");
                }
            }
        }
        if let Some(aba) = &view.aba {
            println!(
                "  A-B-A on {}: drift-corrected cost of the excursion {:+.2}s ideal; \
                 driver/track drift {:+.2}s per stint; outcome margins near that \
                 drift are noise",
                aba.families, aba.effect_s, aba.drift_s,
            );
            if let Some(m) = movers(&aba.effects) {
                println!("    drift-corrected behavioural movement: {m}");
            }
        }
    }

    if !view.current_tune.is_empty() {
        let vals: Vec<String> = view
            .current_tune
            .iter()
            .map(|(phrase, v, unit)| match unit {
                Some(unit) => format!("{phrase} {v} {unit}"),
                None => format!("{phrase} {v}"),
            })
            .collect();
        println!("\ncurrent tune (tune-session.txt): {}", vals.join(", "));
    }
    if let Some((pairs, floor)) = view.drift_floor {
        println!(
            "  measured drift floor: ±{floor:.2}s across {pairs} same-setup pair{}; \
             single-comparison margins below this are noise",
            if pairs == 1 { "" } else { "s" },
        );
    }
    let mapped: Vec<_> = view
        .landscapes
        .iter()
        .filter(|l| l.nodes.len() >= 2)
        .collect();
    if !mapped.is_empty() {
        println!("\nmeasured landscapes (cumulative ideal delta vs first tried; lower = faster):");
        for l in mapped {
            let nodes: Vec<String> = l
                .nodes
                .iter()
                .map(|(v, cum, _)| format!("{v} → {cum:+.2}s"))
                .collect();
            let vertex = l
                .vertex
                .map(|v| format!("  | est. optimum ≈ {v}"))
                .unwrap_or_default();
            println!("  {}: {}{vertex}", l.phrase, nodes.join(", "));
        }
    }
    if let Some(p) = &view.in_progress {
        println!(
            "\nnote: {p} is journaled but has no completed laps yet (still \
             recording?); its step joins the trajectory once a lap completes"
        );
    }
    for p in &view.missing {
        println!(
            "\nnote: {p} is journaled but the recording no longer exists \
             (deleted?), so it was skipped; its tune change was merged into the next step"
        );
    }
    for p in &view.no_laps {
        println!(
            "\nnote: {p} has no completed laps (an event entered and abandoned?), \
             so it was skipped; any tune change was merged into the next step"
        );
    }
    let asks = view
        .recommendations
        .iter()
        .filter(|r| r.suggestion.as_ref().is_some_and(|s| !s.contains("hold")))
        .count();
    println!("\nadvice for {}:", view.advice_for);
    if asks > 1 {
        println!(
            "(suggestions are ALTERNATIVES: apply one per stint, drive it, \
             then re-advise)"
        );
    }
    println!();
    print!(
        "{}",
        analysis::report::render_recommendations(&view.recommendations)
    );
    Ok(())
}

fn cmd_compare(args: &[String]) -> Result<(), String> {
    let [path_a, path_b] = args else {
        return Err("usage: tuners compare <stint-A> <stint-B>".into());
    };
    let load = |path: &String| -> Result<_, String> {
        let session = analysis::Stint::load(path.as_ref()).map_err(|e| format!("{path}: {e}"))?;
        analysis::profile::stint_profile(&session.frames).map_err(|e| format!("{path}: {e}"))
    };
    let a = load(path_a)?;
    let b = load(path_b)?;
    let cmp = analysis::compare::compare(&a, &b)?;
    println!("A = {path_a}\nB = {path_b}\n");
    print!("{}", analysis::compare::render(&a, &b, &cmp));
    Ok(())
}

fn cmd_map(args: &[String]) -> Result<(), String> {
    let mut out = tuners::util::data_path("effect-map.tsv");
    let mut force = false;
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--out" => out = PathBuf::from(it.next().ok_or("--out needs a value")?),
            "--rebuild" => force = true,
            other => return Err(format!("unknown flag '{other}' for map")),
        }
    }
    let root = tuners::util::data_root().to_path_buf();
    let scratch = std::env::temp_dir().join(format!("tuners-map-{}", std::process::id()));
    let result = tuners::advice::effectmap::refresh(
        &root,
        &tuners::util::data_path("sessions").to_string_lossy(),
        &out,
        &scratch,
        force,
    );
    let _ = std::fs::remove_dir_all(&scratch);
    let (map, report) = result?;
    for line in &report {
        println!("{line}");
    }
    if map.samples.is_empty() {
        return Err("no measurements harvested; the map needs journaled campaigns".into());
    }
    println!(
        "\n{} ({} samples, {} campaigns)\n",
        out.display(),
        map.samples.len(),
        map.floors.len(),
    );
    print!(
        "{}",
        tuners::advice::effectmap::summary(&tuners::advice::effectmap::aggregate(&map))
    );
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

fn cmd_export(args: &[String]) -> Result<(), String> {
    let mut out_dir = ".".to_string();
    let mut stint: Option<String> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--out" => out_dir = value(a, it.next())?.clone(),
            _ if stint.is_none() => stint = Some(a.clone()),
            other => return Err(format!("unexpected argument '{other}' for export")),
        }
    }
    let stint = stint.ok_or("usage: tuners export <stint-file> [--out dir]")?;
    let session =
        tuners::advice::tuning::TuningSession::load(&tuners::util::data_path("tune-session.txt"));
    let journal_path = tuners::advice::tuning::journal_path_for(
        session.car,
        &tuners::util::data_path("tune-journal.txt").to_string_lossy(),
    );
    let journal = std::fs::read_to_string(&journal_path).unwrap_or_default();
    let raw_len = std::fs::metadata(&stint).map(|m| m.len()).unwrap_or(0);

    let (name, bytes) = tuners::sharing::bundle::build(stint.as_ref(), &session, &journal)?;
    let path = std::path::Path::new(&out_dir).join(&name);
    std::fs::write(&path, &bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    println!(
        "{} ({:.1} MB from {:.1} MB raw, {:.1}x, self-verified; journal: {journal_path})",
        path.display(),
        bytes.len() as f64 / 1e6,
        raw_len as f64 / 1e6,
        raw_len as f64 / bytes.len() as f64,
    );
    Ok(())
}

fn cmd_ingest(args: &[String]) -> Result<(), String> {
    let mut library = "library".to_string();
    let mut quarantine = "quarantine".to_string();
    let mut inbox: Option<String> = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--library" => library = value(a, it.next())?.clone(),
            "--quarantine" => quarantine = value(a, it.next())?.clone(),
            _ if inbox.is_none() => inbox = Some(a.clone()),
            other => return Err(format!("unexpected argument '{other}' for ingest")),
        }
    }
    let inbox = inbox.ok_or("usage: tuners ingest <dir> [--library dir] [--quarantine dir]")?;
    let report =
        tuners::sharing::ingest::ingest_dir(inbox.as_ref(), library.as_ref(), quarantine.as_ref())
            .map_err(|e| e.to_string())?;
    for name in &report.ingested {
        println!("ingested  {name}");
    }
    for (name, reason) in &report.quarantined {
        println!("QUARANTINED {name}: {reason}");
    }
    println!(
        "{} ingested, {} already known, {} quarantined",
        report.ingested.len(),
        report.skipped,
        report.quarantined.len(),
    );
    Ok(())
}

fn cmd_receive(args: &[String]) -> Result<(), String> {
    let mut port: u16 = 8090;
    let mut bind = "127.0.0.1".to_string();
    let mut root = "inbox".to_string();
    let mut tokens = "receive-tokens.txt".to_string();
    let mut blocklist = "receive-blocklist.txt".to_string();
    let mut max_mb: u64 = 64;
    let mut daily_mb: u64 = 512;
    let mut global_mb: u64 = 20 * 1024;
    let mut issue: Option<String> = None;
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        match flag.as_str() {
            "--port" => port = parse(flag, it.next())?,
            "--bind" => bind = value(flag, it.next())?.clone(),
            "--root" => root = value(flag, it.next())?.clone(),
            "--tokens" => tokens = value(flag, it.next())?.clone(),
            "--blocklist" => blocklist = value(flag, it.next())?.clone(),
            "--max-mb" => max_mb = parse(flag, it.next())?,
            "--daily-mb" => daily_mb = parse(flag, it.next())?,
            "--global-mb" => global_mb = parse(flag, it.next())?,
            "--issue" => issue = Some(value(flag, it.next())?.clone()),
            other => return Err(format!("unknown flag '{other}' for receive")),
        }
    }
    if let Some(sender) = issue {
        let token = tuners::sharing::receive::issue_token(tokens.as_ref(), &sender)?;
        println!("token for {sender}: {token}");
        println!("(appended to {tokens}; hand the token to the sender, keep the file private)");
        return Ok(());
    }
    let cfg = tuners::sharing::receive::ReceiveConfig {
        root: PathBuf::from(root),
        tokens_path: PathBuf::from(tokens),
        blocklist_path: PathBuf::from(blocklist),
        max_bundle_bytes: max_mb * 1024 * 1024,
        daily_cap_bytes: daily_mb * 1024 * 1024,
        global_cap_bytes: global_mb * 1024 * 1024,
    };
    tuners::sharing::receive::run(&bind, port, cfg).map_err(|e| e.to_string())
}

fn value<'a>(flag: &str, v: Option<&'a String>) -> Result<&'a String, String> {
    v.ok_or_else(|| format!("{flag} requires a value"))
}

fn parse<T: std::str::FromStr>(flag: &str, v: Option<&String>) -> Result<T, String> {
    value(flag, v)?
        .parse()
        .map_err(|_| format!("invalid value for {flag}"))
}
