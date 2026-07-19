//! In-process recorder: `tuners serve` binds the UDP port and cuts the stream
//! into session files automatically, so the user runs one binary, opens the
//! dashboard, and drives. Session boundaries come from the Cutter state machine
//! plus the dashboard's "new session" button (tune edits are invisible in
//! telemetry — the button is the tune separator; the rest are safety nets).
//!
//! Recording is gated on race mode: `IsRaceOn` with a live `DistanceTraveled`
//! (exactly 0.0 in free roam, nonzero — even negative at spawn — in race modes,
//! see telemetry.md). Menu/pause frames inside a session are buffered and
//! flushed when racing resumes, preserving the race-off gaps that gap
//! classification (pause/rewind/restart) depends on.

use crate::packet;
use crate::session::SessionWriter;
use crate::util;
use std::collections::VecDeque;
use std::net::UdpSocket;
use std::path::PathBuf;
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
/// This many consecutive undecodable packets open a raw failsafe recording —
/// if a game patch breaks the format, we still capture raw data to fix later.
const UNDECODABLE_FAILSAFE_RUN: u32 = 100;

pub enum Action {
    Open,
    Write { recv_us: u64, payload: Vec<u8> },
    Close,
}

/// Decides where session files begin and end. Pure over (recv_us, payload) —
/// no I/O, no wall clock — so every boundary rule is unit-testable.
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
    }

    /// One received datagram → the file actions it triggers.
    pub fn feed(&mut self, recv_us: u64, payload: &[u8]) -> Vec<Action> {
        self.last_packet_us = recv_us;
        let mut out = Vec::new();

        if self.raw_failsafe {
            out.push(Action::Write { recv_us, payload: payload.to_vec() });
            return out;
        }

        let Ok(frame) = packet::decode(payload) else {
            self.undecodable_run += 1;
            self.buf.push_back((recv_us, payload.to_vec()));
            if self.undecodable_run >= UNDECODABLE_FAILSAFE_RUN {
                self.raw_failsafe = true;
                if !self.active {
                    self.active = true;
                    out.push(Action::Open);
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

        let race_mode = frame.is_race_on && frame.distance_traveled != 0.0;
        if race_mode {
            self.freeroam_since = None;
            if !self.active {
                self.active = true;
                self.car = frame.car_ordinal;
                out.push(Action::Open);
                self.flush(&mut out);
            } else if frame.car_ordinal != 0 && self.car != 0 && frame.car_ordinal != self.car {
                // Different car: unambiguously a different comparison unit.
                self.close(&mut out);
                self.active = true;
                self.car = frame.car_ordinal;
                out.push(Action::Open);
            } else {
                if self.car == 0 {
                    self.car = frame.car_ordinal;
                }
                self.flush(&mut out); // gap frames belong before this one
            }
            out.push(Action::Write { recv_us, payload: payload.to_vec() });
            return out;
        }

        // Not race mode: menus/pause/free roam. Buffer; decide whether the session is over.
        self.buf.push_back((recv_us, payload.to_vec()));
        if !self.active {
            self.trim_prelude(recv_us);
            return out;
        }
        let freeroam_driving = frame.is_race_on
            && frame.distance_traveled == 0.0
            && frame.speed > FREEROAM_MIN_SPEED_MPS;
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

    /// The dashboard's "new session" button — the tune-change separator.
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
    /// UDP port taken (external `tuners capture` or another serve) — view-only.
    External(String),
    /// Listening; no race telemetry yet (menus / free roam are not recorded).
    Waiting,
    Recording,
}

pub struct RecorderStatus {
    pub mode: RecorderMode,
    pub file: Option<PathBuf>,
    pub packets: u64,
    pub split_requested: bool,
    /// Tune-change note from the dashboard, journaled against the NEXT session
    /// that opens (journal lines describe the change since the previous session).
    pub pending_note: Option<String>,
    /// Most recently closed session — the baseline seed for an empty journal.
    pub last_closed: Option<PathBuf>,
}

pub type SharedRecorder = Arc<Mutex<RecorderStatus>>;

pub fn new_shared() -> SharedRecorder {
    Arc::new(Mutex::new(RecorderStatus {
        mode: RecorderMode::Waiting,
        file: None,
        packets: 0,
        split_requested: false,
        pending_note: None,
        last_closed: None,
    }))
}

/// Append a dashboard tune-change note to the journal against the session that
/// just opened, seeding the previous session as baseline when the journal is new.
fn journal_note(journal: &std::path::Path, prev: Option<&std::path::Path>, new: &std::path::Path, note: &str) {
    let text = std::fs::read_to_string(journal).unwrap_or_default();
    let lines = crate::analysis::journal::append_lines(
        &text,
        prev.map(|p| p.to_string_lossy()).as_deref(),
        &new.to_string_lossy(),
        note,
    );
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
/// External and returns — the caller's tailer still serves the live view of
/// whatever an external capture writes.
pub fn run_recorder(port: u16, out_dir: PathBuf, journal: PathBuf, shared: SharedRecorder) {
    let socket = match UdpSocket::bind(("0.0.0.0", port)) {
        Ok(s) => s,
        Err(e) => {
            shared.lock().unwrap().mode = RecorderMode::External(e.to_string());
            println!("udp port {port} busy ({e}) — view-only mode, run `tuners capture` yourself");
            return;
        }
    };
    socket
        .set_read_timeout(Some(Duration::from_millis(250)))
        .expect("set_read_timeout");
    println!("recording automatically from udp {port} into {}", out_dir.display());

    let mut cutter = Cutter::default();
    let mut writer: Option<SessionWriter> = None;
    let mut buf = [0u8; 2048];
    loop {
        let split = {
            let mut s = shared.lock().unwrap();
            std::mem::take(&mut s.split_requested)
        };
        let mut actions = if split { cutter.split() } else { Vec::new() };
        match socket.recv_from(&mut buf) {
            Ok((len, _)) => actions.extend(cutter.feed(unix_micros(), &buf[..len])),
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
                Action::Open => {
                    let _ = std::fs::create_dir_all(&out_dir);
                    // Suffix on collision: a split + reopen inside one second must
                    // not truncate the file just closed.
                    let stamp = util::utc_stamp(unix_micros() / 1_000_000);
                    let mut path = out_dir.join(format!("session-{stamp}.ftel"));
                    let mut n = 2;
                    while path.exists() {
                        path = out_dir.join(format!("session-{stamp}-{n}.ftel"));
                        n += 1;
                    }
                    match SessionWriter::create(&path) {
                        Ok(w) => {
                            writer = Some(w);
                            let (note, last_closed) = {
                                let mut s = shared.lock().unwrap();
                                s.mode = RecorderMode::Recording;
                                s.file = Some(path.clone());
                                s.packets = 0;
                                (s.pending_note.take(), s.last_closed.clone())
                            };
                            if let Some(note) = note {
                                journal_note(&journal, last_closed.as_deref(), &path, &note);
                            }
                        }
                        Err(e) => eprintln!("cannot create session file {}: {e}", path.display()),
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
                    let mut s = shared.lock().unwrap();
                    s.mode = RecorderMode::Waiting;
                    s.last_closed = s.file.take();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::TelemetryFrame;

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
                Action::Open => "open",
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
        assert!((30..=32).contains(&writes), "prelude ~30s of frames, got {writes}");
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
                assert!(k.iter().filter(|s| **s == "write").count() as u32 >= UNDECODABLE_FAILSAFE_RUN);
            }
        }
        assert!(opened, "failsafe must open a raw recording");
        assert!(c.is_active());
        // Silence still closes it.
        assert_eq!(kinds(&c.tick(10_000 * S)), vec!["close"]);
    }
}
