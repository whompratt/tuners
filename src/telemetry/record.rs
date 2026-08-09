//! In-process recorder: the app shell binds the UDP port and cuts the stream
//! into session files automatically, so the user runs one binary, opens the
//! dashboard, and drives. Session boundaries come from the Cutter state machine
//! plus the dashboard's "new session" button (tune edits are invisible in
//! telemetry, so the button is the tune separator; the rest are safety nets).
//!
//! Recording is gated on race mode: `IsRaceOn` with a live `DistanceTraveled`
//! (exactly 0.0 in free roam; nonzero, even negative at spawn, in race modes,
//! see telemetry.md). Open-world time attack also drives with both set but
//! never moves a lap channel, so its stints could never be timed or compared:
//! the cutter excludes it like free roam. Menu/pause frames inside a session
//! are buffered and flushed when racing resumes, preserving the race-off gaps
//! that gap classification (pause/rewind/restart) depends on.
//!
//! A finish certificate (results-screen flicker carrying the official run
//! time, see `analysis::finish_certificate`) closes the session eagerly: the
//! run is over, so the stint finalizes seconds after the finish instead of
//! waiting out the 5-minute menu gap. Gaps without a certificate (mid-run
//! pauses, abandons) keep the long window so a paused run is never severed.

use crate::telemetry::packet;
use crate::telemetry::stint::StintWriter;
use crate::util;
use std::collections::VecDeque;
use std::net::UdpSocket;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Pre-session history kept so a new file starts with the countdown, not mid-launch.
const PRELUDE_KEEP_US: u64 = 30_000_000;
/// A race-off gap (menus, tune edits, results screens) longer than this ends the session.
const GAP_CLOSE_US: u64 = 300_000_000;
/// Sustained free-roam driving (moving with a dead odometer) ends the session this fast.
const FREEROAM_CLOSE_US: u64 = 5_000_000;
/// No packets at all for this long (game closed) ends the session.
const SILENCE_CLOSE_US: u64 = 30_000_000;
/// Free-roam detection needs the car actually moving (~11 mph, same as analysis).
const FREEROAM_MIN_SPEED_MPS: f32 = 5.0;
/// This many consecutive undecodable packets open a raw failsafe recording:
/// if a game patch breaks the format, we still capture raw data to fix later.
const UNDECODABLE_FAILSAFE_RUN: u32 = 100;
/// After an eager finish close, results screens can flash the certificate
/// again; within this window such a flash is menu noise, not a new session.
const FINISH_GUARD_US: u64 = 60_000_000;
/// Race modes tick `CurrentLap` from the countdown's GO, and the longest
/// measured lap-0 rollout is 5.74s of race clock: past this, a zero lap clock
/// with positive distance cannot be a race (telemetry.md, route-kind clocks).
const LAPLESS_MIN_RACE_T_S: f32 = 6.0;

/// Open-world time attack: race-on with a live route distance but every lap
/// channel pinned at zero for the whole event (telemetry.md "Open-world time
/// attack"). Countdowns read both clocks 0.0 with distance still negative, so
/// launches stay recorded; finish certificates and mid-race frames carry a
/// nonzero lap channel, so they never match.
fn time_attack(f: &packet::TelemetryFrame) -> bool {
    f.is_race_on
        && f.distance_traveled > 0.0
        && f.current_race_time >= LAPLESS_MIN_RACE_T_S
        && f.current_lap == 0.0
        && f.lap_number == 0
        && f.last_lap == 0.0
        && f.best_lap == 0.0
}

pub enum Action {
    Open { car: i32 },
    Write { recv_us: u64, payload: Vec<u8> },
    Close,
}

/// Decides where session files begin and end. Pure over (recv_us, payload),
/// no I/O, no wall clock, so every boundary rule is unit-testable.
#[derive(Default)]
pub struct Cutter {
    active: bool,
    /// Frames not yet written: pre-session prelude when idle, gap frames when active.
    buf: VecDeque<(u64, Vec<u8>)>,
    /// Car ordinal the active session was opened for.
    car: i32,
    last_packet_us: u64,
    freeroam_since: Option<u64>,
    undecodable_run: u32,
    /// Format broke mid-stream: record everything raw, close only on silence.
    raw_failsafe: bool,
    /// Last driving frame of the active session (race mode, car moving):
    /// the anchor a finish certificate is matched against.
    last_drive: Option<packet::TelemetryFrame>,
    /// A finish certificate closed the session: (when, the run's last driving
    /// frame). Survives the close so trailing certificate re-flashes are
    /// recognized as noise instead of opening a junk session.
    finished: Option<(u64, packet::TelemetryFrame)>,
}

impl Cutter {
    pub fn is_active(&self) -> bool {
        self.active
    }

    fn flush(&mut self, out: &mut Vec<Action>) {
        for (recv_us, payload) in self.buf.drain(..) {
            out.push(Action::Write { recv_us, payload });
        }
    }

    fn trim_prelude(&mut self, now_us: u64) {
        while self
            .buf
            .front()
            .is_some_and(|(us, _)| now_us.saturating_sub(*us) > PRELUDE_KEEP_US)
        {
            self.buf.pop_front();
        }
    }

    fn close(&mut self, out: &mut Vec<Action>) {
        out.push(Action::Close);
        self.active = false;
        self.raw_failsafe = false;
        self.freeroam_since = None;
        self.buf.clear();
        self.car = 0;
        self.last_drive = None;
    }

    /// One received datagram → the file actions it triggers.
    pub fn feed(&mut self, recv_us: u64, payload: &[u8]) -> Vec<Action> {
        self.last_packet_us = recv_us;
        let mut out = Vec::new();

        if self.raw_failsafe {
            out.push(Action::Write {
                recv_us,
                payload: payload.to_vec(),
            });
            return out;
        }

        let Ok(frame) = packet::decode(payload) else {
            self.undecodable_run += 1;
            self.buf.push_back((recv_us, payload.to_vec()));
            if self.undecodable_run >= UNDECODABLE_FAILSAFE_RUN {
                self.raw_failsafe = true;
                if !self.active {
                    self.active = true;
                    out.push(Action::Open { car: 0 });
                }
                self.flush(&mut out);
            } else if !self.active {
                self.trim_prelude(recv_us);
            } else if self.gap_expired(recv_us) {
                self.close(&mut out);
            }
            return out;
        };
        self.undecodable_run = 0;

        let race_mode = frame.is_race_on && frame.distance_traveled != 0.0 && !time_attack(&frame);
        if race_mode {
            self.freeroam_since = None;
            if !self.active {
                // A certificate re-flash shortly after an eager finish close
                // is results-screen noise, not a new session.
                if let Some((t, last)) = &self.finished
                    && recv_us.saturating_sub(*t) < FINISH_GUARD_US
                    && crate::analysis::finish_certificate(last, &frame)
                {
                    self.buf.push_back((recv_us, payload.to_vec()));
                    self.trim_prelude(recv_us);
                    return out;
                }
                self.finished = None;
                self.active = true;
                self.car = frame.car_ordinal;
                out.push(Action::Open { car: self.car });
                self.flush(&mut out);
            } else if frame.car_ordinal != 0 && self.car != 0 && frame.car_ordinal != self.car {
                // Different car: unambiguously a different comparison unit.
                self.close(&mut out);
                self.active = true;
                self.car = frame.car_ordinal;
                out.push(Action::Open { car: self.car });
            } else {
                if self.car == 0 {
                    self.car = frame.car_ordinal;
                }
                // Race-off gap resolved by a finish certificate: the run
                // demonstrably ended (a mid-run pause can never match: its
                // race clock resumes within hundredths, under the predicate's
                // +0.5s advance guard). Close now instead of waiting out the
                // menu gap: the session is final and advice recomputes on a
                // complete stint before any setup decision. The flicker is
                // written first so analysis adopts the official time in-file.
                if !self.buf.is_empty()
                    && let Some(last) = self.last_drive
                    && crate::analysis::finish_certificate(&last, &frame)
                {
                    self.flush(&mut out); // gap frames belong before the flicker
                    out.push(Action::Write {
                        recv_us,
                        payload: payload.to_vec(),
                    });
                    self.finished = Some((recv_us, last));
                    self.close(&mut out);
                    return out;
                }
                self.flush(&mut out); // gap frames belong before this one
            }
            out.push(Action::Write {
                recv_us,
                payload: payload.to_vec(),
            });
            if frame.speed > FREEROAM_MIN_SPEED_MPS {
                self.last_drive = Some(frame);
            }
            return out;
        }

        // Not race mode: menus/pause/free roam. Buffer; decide whether the session is over.
        self.buf.push_back((recv_us, payload.to_vec()));
        if !self.active {
            self.trim_prelude(recv_us);
            return out;
        }
        // Free roam (dead odometer) or time attack: sustained unrecordable
        // driving closes an open session fast instead of polluting it.
        let freeroam_driving = frame.is_race_on
            && frame.speed > FREEROAM_MIN_SPEED_MPS
            && (frame.distance_traveled == 0.0 || time_attack(&frame));
        if freeroam_driving {
            let since = *self.freeroam_since.get_or_insert(recv_us);
            if recv_us.saturating_sub(since) >= FREEROAM_CLOSE_US {
                self.close(&mut out);
                return out;
            }
        } else {
            self.freeroam_since = None;
        }
        if self.gap_expired(recv_us) {
            self.close(&mut out);
        }
        out
    }

    fn gap_expired(&self, now_us: u64) -> bool {
        self.buf
            .front()
            .is_some_and(|(us, _)| now_us.saturating_sub(*us) >= GAP_CLOSE_US)
    }

    /// The dashboard's "new session" button, i.e. the tune-change separator.
    pub fn split(&mut self) -> Vec<Action> {
        let mut out = Vec::new();
        if self.active {
            self.close(&mut out);
        }
        out
    }

    /// Periodic check while no packets arrive (game closed mid-session).
    pub fn tick(&mut self, now_us: u64) -> Vec<Action> {
        let mut out = Vec::new();
        if self.active && now_us.saturating_sub(self.last_packet_us) >= SILENCE_CLOSE_US {
            self.close(&mut out);
        }
        out
    }
}

/// What the recorder is doing, for the dashboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecorderMode {
    /// UDP port taken (external `tuners capture` or another instance): view-only.
    External(String),
    /// Listening; no race telemetry yet (menus / free roam are not recorded).
    Waiting,
    Recording,
}

pub struct RecorderStatus {
    pub mode: RecorderMode,
    pub file: Option<PathBuf>,
    pub packets: u64,
    /// When the last raw datagram hit the socket, menus included, which are
    /// never recorded. Lets onboarding confirm the Data Out wiring before any
    /// race telemetry exists.
    pub last_udp: Option<std::time::Instant>,
    /// Car ordinal seen in raw packets. Free roam carries it while never
    /// being recorded (DistanceTraveled stays 0 there, so the cutter never
    /// opens), so onboarding detects the car without requiring an event.
    pub udp_car: Option<i32>,
    pub split_requested: bool,
    /// Tune-change note from the dashboard, journaled against the NEXT stint
    /// that opens (journal lines describe the change since the previous stint).
    pub pending_note: Option<String>,
    /// Revision index the pending note diffs from (the last DRIVEN revision);
    /// lets consecutive saves with no stint between them net into one note.
    pub pending_base_rev: Option<usize>,
    /// Most recently closed session: the baseline seed for an empty journal.
    pub last_closed: Option<PathBuf>,
}

pub type SharedRecorder = Arc<Mutex<RecorderStatus>>;

pub fn new_shared() -> SharedRecorder {
    Arc::new(Mutex::new(RecorderStatus {
        mode: RecorderMode::Waiting,
        file: None,
        packets: 0,
        last_udp: None,
        udp_car: None,
        split_requested: false,
        pending_note: None,
        pending_base_rev: None,
        last_closed: None,
    }))
}

/// Append a dashboard tune-change note to the journal against the stint that
/// just opened, seeding the previous stint as baseline when the journal is new
/// (with a car-name header so per-car journal files are self-describing).
fn journal_note(
    journal: &std::path::Path,
    prev: Option<&std::path::Path>,
    new: &std::path::Path,
    note: &str,
    car: Option<i32>,
) {
    let text = std::fs::read_to_string(journal).unwrap_or_default();
    let mut lines = crate::advice::journal::append_lines(
        &text,
        prev.map(|p| p.to_string_lossy()).as_deref(),
        &new.to_string_lossy(),
        note,
    );
    if text.trim().is_empty()
        && let Some(car) = car
    {
        let name = crate::cars::car_name(car).unwrap_or("unknown car");
        lines = format!(
            "# {name} (ordinal {car})
{lines}"
        );
    }
    let write = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(journal)
        .and_then(|mut f| std::io::Write::write_all(&mut f, lines.as_bytes()));
    match write {
        Ok(()) => println!("journal: {}", lines.trim_end().replace('\n', "; ")),
        Err(e) => eprintln!("cannot write journal {}: {e}", journal.display()),
    }
}

fn unix_micros() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before 1970")
        .as_micros() as u64
}

/// Bind the UDP port and record forever. On bind failure, marks the status
/// External and returns; the caller's tailer still serves the live view of
/// whatever an external capture writes.
pub fn run_recorder(
    port: u16,
    out_dir: PathBuf,
    journal_base: PathBuf,
    session_file: PathBuf,
    shared: SharedRecorder,
) {
    let socket = match UdpSocket::bind(("0.0.0.0", port)) {
        Ok(s) => s,
        Err(e) => {
            shared.lock().unwrap().mode = RecorderMode::External(e.to_string());
            println!("udp port {port} busy ({e}): view-only mode, run `tuners capture` yourself");
            return;
        }
    };
    socket
        .set_read_timeout(Some(Duration::from_millis(250)))
        .expect("set_read_timeout");
    println!(
        "recording automatically from udp {port} into {}",
        out_dir.display()
    );

    let mut cutter = Cutter::default();
    let mut writer: Option<StintWriter> = None;
    let mut open_car: i32 = 0;
    let mut last_closed_car: Option<i32> = None;
    let mut buf = [0u8; 2048];
    loop {
        let split = {
            let mut s = shared.lock().unwrap();
            std::mem::take(&mut s.split_requested)
        };
        let mut actions = if split { cutter.split() } else { Vec::new() };
        match socket.recv_from(&mut buf) {
            Ok((len, _)) => {
                {
                    let mut s = shared.lock().unwrap();
                    s.last_udp = Some(std::time::Instant::now());
                    // Menu packets zero everything but the timestamp, so a
                    // non-zero ordinal is a real car (free roam included).
                    if let Ok(f) = crate::telemetry::packet::decode(&buf[..len])
                        && f.car_ordinal != 0
                    {
                        s.udp_car = Some(f.car_ordinal);
                    }
                }
                actions.extend(cutter.feed(unix_micros(), &buf[..len]));
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                actions.extend(cutter.tick(unix_micros()))
            }
            Err(_) => std::thread::sleep(Duration::from_millis(250)),
        }

        for action in actions {
            match action {
                Action::Open { car } => {
                    let _ = std::fs::create_dir_all(&out_dir);
                    // Suffix on collision: a split + reopen inside one second must
                    // not truncate the file just closed.
                    let stamp = util::utc_stamp(unix_micros() / 1_000_000);
                    let mut path = out_dir.join(format!("stint-{stamp}.ftel"));
                    let mut n = 2;
                    while path.exists() {
                        path = out_dir.join(format!("stint-{stamp}-{n}.ftel"));
                        n += 1;
                    }
                    match StintWriter::create(&path) {
                        Ok(w) => {
                            writer = Some(w);
                            open_car = car;
                            let last_closed = {
                                let mut s = shared.lock().unwrap();
                                s.mode = RecorderMode::Recording;
                                s.file = Some(path.clone());
                                s.packets = 0;
                                s.last_closed.clone()
                            };
                            // The journal belongs to the session: notes only attach
                            // to stints of the session's car. A pending note for a
                            // different car stays pending until that car returns.
                            let session = crate::advice::tuning::TuningSession::load(&session_file);
                            let car_matches = session.car.is_none_or(|c| c == car);
                            let has_note = shared.lock().unwrap().pending_note.is_some();
                            if has_note && car_matches {
                                let note = {
                                    let mut r = shared.lock().unwrap();
                                    r.pending_base_rev = None;
                                    r.pending_note.take().unwrap()
                                };
                                let journal = crate::advice::tuning::journal_path_for(
                                    session.car,
                                    &journal_base.to_string_lossy(),
                                );
                                let baseline_ok =
                                    session.car.is_none() || last_closed_car == session.car;
                                // Baseline seed must survive app restarts: with no
                                // (matching) stint closed in this process, fall back
                                // to the newest on-disk stint of the session car.
                                let baseline = last_closed.filter(|_| baseline_ok).or_else(|| {
                                    crate::advice::advise::latest_stint_for_car(
                                        &out_dir.to_string_lossy(),
                                        session.car,
                                    )
                                    .map(PathBuf::from)
                                    .filter(|p| p != &path)
                                });
                                journal_note(
                                    Path::new(&journal),
                                    baseline.as_deref(),
                                    &path,
                                    &note,
                                    session.car,
                                );
                            }
                        }
                        Err(e) => eprintln!("cannot create stint file {}: {e}", path.display()),
                    }
                }
                Action::Write { recv_us, payload } => {
                    if let Some(w) = &mut writer {
                        if let Err(e) = w.write_packet(recv_us, &payload) {
                            eprintln!("session write failed: {e}");
                        } else {
                            shared.lock().unwrap().packets = w.packets();
                        }
                    }
                }
                Action::Close => {
                    writer = None;
                    last_closed_car = Some(open_car);
                    let closed = {
                        let mut s = shared.lock().unwrap();
                        s.mode = RecorderMode::Waiting;
                        s.last_closed = s.file.take();
                        s.last_closed.clone()
                    };
                    // Finalization is the collection moment: bundle
                    // into the outbox on a worker thread, never inline.
                    if let Some(path) = closed {
                        crate::sharing::collect::maybe_enqueue(
                            path,
                            session_file.clone(),
                            open_car,
                        );
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::packet::TelemetryFrame;

    const S: u64 = 1_000_000; // one second in micros

    fn race_frame(distance: f32, car: i32) -> Vec<u8> {
        packet::encode(&TelemetryFrame {
            is_race_on: true,
            distance_traveled: distance,
            car_ordinal: car,
            speed: 40.0,
            ..Default::default()
        })
        .to_vec()
    }

    fn menu_frame() -> Vec<u8> {
        packet::encode(&TelemetryFrame::default()).to_vec()
    }

    fn freeroam_frame() -> Vec<u8> {
        packet::encode(&TelemetryFrame {
            is_race_on: true,
            distance_traveled: 0.0,
            speed: 30.0,
            car_ordinal: 42,
            ..Default::default()
        })
        .to_vec()
    }

    fn kinds(actions: &[Action]) -> Vec<&'static str> {
        actions
            .iter()
            .map(|a| match a {
                Action::Open { .. } => "open",
                Action::Write { .. } => "write",
                Action::Close => "close",
            })
            .collect()
    }

    #[test]
    fn race_mode_opens_with_prelude_menu_does_not() {
        let mut c = Cutter::default();
        // Menus alone never open a file, and the prelude stays capped at 30s.
        for i in 0..100 {
            assert!(c.feed(i * S, &menu_frame()).is_empty());
        }
        // Race spawn (distance negative counts as race mode) opens and flushes
        // only the last 30s of prelude before the live frame.
        let actions = c.feed(100 * S, &race_frame(-27.0, 42));
        let k = kinds(&actions);
        assert_eq!(k[0], "open");
        assert_eq!(*k.last().unwrap(), "write");
        let writes = k.iter().filter(|s| **s == "write").count();
        assert!(
            (30..=32).contains(&writes),
            "prelude ~30s of frames, got {writes}"
        );
    }

    #[test]
    fn free_roam_driving_never_opens() {
        let mut c = Cutter::default();
        for i in 0..600 {
            assert!(c.feed(i * S / 10, &freeroam_frame()).is_empty());
        }
        assert!(!c.is_active());
    }

    #[test]
    fn menu_gap_is_flushed_on_resume_and_closes_after_5min() {
        let mut c = Cutter::default();
        c.feed(0, &race_frame(10.0, 42));
        // 60s of menus (tune edit): buffered, session stays open.
        for i in 1..=60 {
            assert!(c.feed(i * S, &menu_frame()).is_empty());
            assert!(c.is_active());
        }
        // Resume: the whole gap is written before the live frame.
        let k = kinds(&c.feed(61 * S, &race_frame(500.0, 42)));
        assert_eq!(k.iter().filter(|s| **s == "write").count(), 61);
        assert!(!k.contains(&"open"), "same session continues");

        // A gap that reaches 5 minutes closes the session (buffer dropped).
        let mut last = Vec::new();
        for i in 0..=301 {
            last = c.feed((62 + i) * S, &menu_frame());
            if !c.is_active() {
                break;
            }
        }
        assert_eq!(kinds(&last), vec!["close"]);
    }

    #[test]
    fn freeroam_exit_closes_within_seconds() {
        let mut c = Cutter::default();
        c.feed(0, &race_frame(10.0, 42));
        let mut last = Vec::new();
        for i in 0..100 {
            last = c.feed(S + i * S / 10, &freeroam_frame());
            if !c.is_active() {
                break;
            }
        }
        assert_eq!(kinds(&last), vec!["close"]);
        // and the free-roam frames were dropped, not written
    }

    /// Open-world time attack: race-on, live route distance, free-roam race
    /// clock, every lap channel zero (the measured signature).
    fn ta_frame(race_t: f32, dist: f32) -> Vec<u8> {
        packet::encode(&TelemetryFrame {
            is_race_on: true,
            distance_traveled: dist,
            car_ordinal: 42,
            speed: 40.0,
            current_race_time: race_t,
            ..Default::default()
        })
        .to_vec()
    }

    #[test]
    fn time_attack_driving_never_opens() {
        let mut c = Cutter::default();
        for i in 0..600u64 {
            let t = 180.0 + i as f32 / 10.0;
            assert!(c.feed(i * S / 10, &ta_frame(t, 5.0 + i as f32)).is_empty());
        }
        assert!(!c.is_active());
    }

    #[test]
    fn time_attack_after_race_closes_within_seconds() {
        let mut c = Cutter::default();
        c.feed(0, &drive_frame(100.0, 2, 45.0));
        let mut last = Vec::new();
        for i in 0..100u64 {
            last = c.feed(
                S + i * S / 10,
                &ta_frame(200.0 + i as f32 / 10.0, 50.0 + i as f32),
            );
            if !c.is_active() {
                break;
            }
        }
        assert_eq!(kinds(&last), vec!["close"]);
    }

    #[test]
    fn lap_line_reset_frame_does_not_sever() {
        let mut c = Cutter::default();
        c.feed(0, &drive_frame(100.0, 2, 45.0));
        // A lap-boundary frame could read CurrentLap exactly 0.0 with the race
        // clock large, but LapNumber is nonzero mid-race: still race mode.
        let boundary = packet::encode(&TelemetryFrame {
            is_race_on: true,
            distance_traveled: 400.0,
            car_ordinal: 42,
            speed: 40.0,
            current_race_time: 100.1,
            lap_number: 3,
            last_lap: 45.1,
            best_lap: 45.1,
            ..Default::default()
        })
        .to_vec();
        let k = kinds(&c.feed(S, &boundary));
        assert_eq!(k, vec!["write"]);
        assert!(c.is_active());
    }

    #[test]
    fn car_change_splits_sessions() {
        let mut c = Cutter::default();
        c.feed(0, &race_frame(10.0, 42));
        let k = kinds(&c.feed(S, &race_frame(20.0, 99)));
        assert_eq!(k, vec!["close", "open", "write"]);
    }

    #[test]
    fn split_button_closes_and_next_race_frame_reopens() {
        let mut c = Cutter::default();
        c.feed(0, &race_frame(10.0, 42));
        assert_eq!(kinds(&c.split()), vec!["close"]);
        assert!(kinds(&c.split()).is_empty(), "idempotent when idle");
        let k = kinds(&c.feed(S, &race_frame(20.0, 42)));
        assert_eq!(k, vec!["open", "write"]);
    }

    /// Driving frame with the clock/lap state the certificate predicate reads.
    fn drive_frame(race_t: f32, lap: u16, lap_t: f32) -> Vec<u8> {
        packet::encode(&TelemetryFrame {
            is_race_on: true,
            distance_traveled: 400.0,
            car_ordinal: 42,
            speed: 40.0,
            current_race_time: race_t,
            lap_number: lap,
            current_lap: lap_t,
            ..Default::default()
        })
        .to_vec()
    }

    /// Results-screen flicker: race-on, lap incremented, LastLap = official
    /// time, race clock run forward, car stationary, position garbage.
    fn cert_frame(race_t: f32, lap: u16, official: f32) -> Vec<u8> {
        packet::encode(&TelemetryFrame {
            is_race_on: true,
            distance_traveled: 123.0,
            car_ordinal: 42,
            speed: 0.0,
            current_race_time: race_t,
            lap_number: lap,
            last_lap: official,
            ..Default::default()
        })
        .to_vec()
    }

    #[test]
    fn finish_certificate_closes_eagerly() {
        let mut c = Cutter::default();
        c.feed(0, &drive_frame(100.0, 2, 45.0));
        // Race-off at the line, then the certificate flicker ~4s later.
        for i in 1..=4 {
            assert!(c.feed(i * S, &menu_frame()).is_empty());
        }
        let k = kinds(&c.feed(5 * S, &cert_frame(104.0, 3, 45.2)));
        // Gap frames, then the flicker (analysis adopts the time in-file),
        // then the eager close.
        assert_eq!(
            k,
            vec!["write", "write", "write", "write", "write", "close"]
        );
        assert!(!c.is_active());
    }

    #[test]
    fn pause_at_lap_line_stays_stitched() {
        let mut c = Cutter::default();
        c.feed(0, &drive_frame(100.0, 2, 45.0));
        for i in 1..=20 {
            assert!(c.feed(i * S, &menu_frame()).is_empty());
        }
        // Resume on lap+1 with a matching LastLap but the race clock
        // continuous: a pause taken right at a lap line, not a finish.
        let resume = packet::encode(&TelemetryFrame {
            is_race_on: true,
            distance_traveled: 401.0,
            car_ordinal: 42,
            speed: 40.0,
            current_race_time: 100.1,
            lap_number: 3,
            current_lap: 0.1,
            last_lap: 45.1,
            ..Default::default()
        })
        .to_vec();
        let k = kinds(&c.feed(21 * S, &resume));
        assert!(!k.contains(&"close"), "paused run must not be severed");
        assert!(c.is_active());
    }

    #[test]
    fn trailing_flicker_after_finish_does_not_reopen() {
        let mut c = Cutter::default();
        c.feed(0, &drive_frame(100.0, 2, 45.0));
        c.feed(S, &menu_frame());
        c.feed(2 * S, &cert_frame(104.0, 3, 45.2));
        assert!(!c.is_active());
        // The results screen flashes the certificate again: noise, no session.
        assert!(c.feed(10 * S, &cert_frame(110.0, 3, 45.2)).is_empty());
        assert!(!c.is_active());
        // A real new run still opens normally.
        let k = kinds(&c.feed(30 * S, &drive_frame(0.5, 0, 0.5)));
        assert_eq!(k[0], "open");
        assert!(c.is_active());
    }

    #[test]
    fn pause_past_gap_window_severs_and_resume_reopens() {
        // Pin of the severed-run shape: a mid-run pause outliving the gap
        // window splits the run over two files; the resumed file starts
        // mid-run and the two pool at the campaign level as same-setup stints.
        let mut c = Cutter::default();
        c.feed(0, &drive_frame(100.0, 2, 45.0));
        let mut closed = false;
        for i in 1..=301 {
            let k = kinds(&c.feed(i * S, &menu_frame()));
            if k.contains(&"close") {
                closed = true;
                break;
            }
        }
        assert!(closed, "no certificate: the long window still cuts");
        // Resume mid-run: a new file opens for the remainder.
        let k = kinds(&c.feed(320 * S, &drive_frame(105.0, 2, 50.0)));
        assert_eq!(k[0], "open");
        assert!(c.is_active());
    }

    #[test]
    fn silence_tick_closes_session() {
        let mut c = Cutter::default();
        c.feed(0, &race_frame(10.0, 42));
        assert!(c.tick(10 * S).is_empty(), "10s of silence is not enough");
        assert_eq!(kinds(&c.tick(31 * S)), vec!["close"]);
    }

    #[test]
    fn undecodable_stream_triggers_raw_failsafe() {
        let mut c = Cutter::default();
        let junk = vec![0u8; 17];
        let mut opened = false;
        for i in 0..UNDECODABLE_FAILSAFE_RUN as u64 + 10 {
            let k = kinds(&c.feed(i * S / 60, &junk));
            if k.contains(&"open") {
                opened = true;
                // everything buffered so far is flushed raw
                assert!(
                    k.iter().filter(|s| **s == "write").count() as u32 >= UNDECODABLE_FAILSAFE_RUN
                );
            }
        }
        assert!(opened, "failsafe must open a raw recording");
        assert!(c.is_active());
        // Silence still closes it.
        assert_eq!(kinds(&c.tick(10_000 * S)), vec!["close"]);
    }
}
