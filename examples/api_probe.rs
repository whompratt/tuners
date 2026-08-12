//! Exercise the app command layer the way the Analysis tab does: stint rows,
//! laps view, compare, report, and the advise view, printing each as JSON so
//! frontend-killing shapes (duplicate keyed-each keys, nulls) can be checked
//! offline against a repro data root.

fn main() {
    let root = std::env::args()
        .nth(1)
        .expect("usage: api_probe <data-root>");
    std::env::set_current_dir(&root).unwrap();
    unsafe { std::env::set_var("TUNERS_DATA", &root) };

    let rows = tuners::api::stint_rows("sessions");
    eprintln!("stint_rows: {} rows", rows.len());
    let newest = rows.first().cloned();

    match tuners::api::advise_active("tune-session.txt", "tune-journal.txt", "sessions") {
        Ok(v) => {
            let j = serde_json::to_string(&v).unwrap();
            println!("{j}");
        }
        Err(e) => eprintln!("advise_active ERROR: {e:?}"),
    }

    if let Some(r) = newest {
        match tuners::api::laps_view(&r.file) {
            Ok(v) => eprintln!("laps_view ok: {} laps", v.laps.len()),
            Err(e) => eprintln!("laps_view ERROR: {e:?}"),
        }
        match tuners::api::report_text(&r.file) {
            Ok(t) => eprintln!("report ok: {} chars", t.len()),
            Err(e) => eprintln!("report ERROR: {e:?}"),
        }
    }
}
