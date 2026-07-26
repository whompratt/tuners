//! UDP capture: receive Data Out packets, record them raw, show a live status line.

use crate::packet::{self, PACKET_LEN, TelemetryFrame};
use crate::stint::StintWriter;
use crate::util;
use std::collections::HashSet;
use std::io::{self, Write};
use std::net::UdpSocket;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub struct CaptureOpts {
    pub port: u16,
    pub out_dir: PathBuf,
    /// Stop after this many packets (mainly for headless testing).
    pub max_packets: Option<u64>,
    /// Stop this long after startup (mainly for headless testing).
    pub max_duration: Option<Duration>,
}

pub fn run(opts: &CaptureOpts) -> io::Result<PathBuf> {
    let socket = UdpSocket::bind(("0.0.0.0", opts.port)).map_err(|e| {
        io::Error::new(
            e.kind(),
            format!("cannot listen on UDP port {}: {e}", opts.port),
        )
    })?;
    socket.set_read_timeout(Some(Duration::from_millis(500)))?;

    std::fs::create_dir_all(&opts.out_dir)?;
    let path = opts.out_dir.join(format!(
        "stint-{}.ftel",
        util::utc_stamp(unix_now().as_secs())
    ));
    let mut writer = StintWriter::create(&path)?;

    println!("listening on 0.0.0.0:{} (udp)", opts.port);
    println!("recording to {}", path.display());
    println!("stop with Ctrl+C");

    let start = Instant::now();
    let mut buf = [0u8; 2048];
    let mut decode_errors: u64 = 0;
    let mut odd_sizes: HashSet<usize> = HashSet::new();
    let mut last_status = Instant::now() - Duration::from_secs(1);

    loop {
        if opts.max_duration.is_some_and(|d| start.elapsed() >= d) {
            break;
        }
        match socket.recv_from(&mut buf) {
            Ok((len, _src)) => {
                writer.write_packet(unix_now().as_micros() as u64, &buf[..len])?;
                if len != PACKET_LEN && odd_sizes.insert(len) {
                    eprintln!(
                        "\nwarning: unexpected packet size {len} (expected {PACKET_LEN}); recording anyway"
                    );
                }
                match packet::decode(&buf[..len]) {
                    Ok(frame) => {
                        if last_status.elapsed() >= Duration::from_millis(100) {
                            print_status(writer.packets(), &frame, start.elapsed());
                            last_status = Instant::now();
                        }
                    }
                    Err(_) => decode_errors += 1,
                }
                if opts.max_packets.is_some_and(|max| writer.packets() >= max) {
                    break;
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                if writer.packets() == 0 {
                    print!(
                        "\r[waiting] no packets yet on port {} ({}s)  ",
                        opts.port,
                        start.elapsed().as_secs()
                    );
                    io::stdout().flush().ok();
                }
            }
            Err(e) => return Err(e),
        }
    }

    println!(
        "\ncaptured {} packets ({decode_errors} decode errors) in {:.1}s",
        writer.packets(),
        start.elapsed().as_secs_f32()
    );
    Ok(path)
}

fn print_status(packets: u64, f: &TelemetryFrame, elapsed: Duration) {
    let total_secs = elapsed.as_secs();
    print!(
        "\r[rec {:02}:{:02}] {:>6} pkts | {:6.1} mph | rpm {:>5.0}/{:<5.0} | gear {} | race {}  ",
        total_secs / 60,
        total_secs % 60,
        packets,
        f.speed * util::MPS_TO_MPH,
        f.current_engine_rpm,
        f.engine_max_rpm,
        f.gear,
        if f.is_race_on { "on " } else { "off" },
    );
    io::stdout().flush().ok();
}

fn unix_now() -> Duration {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before 1970")
}
