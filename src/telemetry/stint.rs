//! Raw session logs: every received datagram plus its receive timestamp, written
//! before any decoding so decoder bugs can be fixed retroactively.
//!
//! Format: 8-byte magic `FH6TEL01`, then per record:
//! u64 LE receive time (microseconds since Unix epoch), u32 LE payload length, payload.

use std::fs::File;
use std::io::{self, BufReader, Read, Write};
use std::path::Path;

pub const MAGIC: &[u8; 8] = b"FH6TEL01";

/// Guard against reading a corrupt length field; real packets are 324 bytes.
const MAX_PAYLOAD: u32 = 64 * 1024;

pub struct StintWriter {
    file: File,
    packets: u64,
}

impl StintWriter {
    pub fn create(path: &Path) -> io::Result<Self> {
        let mut file = File::create(path)?;
        file.write_all(MAGIC)?;
        Ok(Self { file, packets: 0 })
    }

    pub fn write_packet(&mut self, recv_micros: u64, payload: &[u8]) -> io::Result<()> {
        // One write_all per record keeps partially-written records impossible short of
        // a crash mid-syscall, so an interrupted capture stays readable.
        let mut record = Vec::with_capacity(12 + payload.len());
        record.extend_from_slice(&recv_micros.to_le_bytes());
        record.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        record.extend_from_slice(payload);
        self.file.write_all(&record)?;
        self.packets += 1;
        Ok(())
    }

    pub fn packets(&self) -> u64 {
        self.packets
    }
}

pub struct StintReader<R: Read = BufReader<File>> {
    reader: R,
}

impl StintReader {
    pub fn open(path: &Path) -> io::Result<Self> {
        let mut reader = BufReader::new(File::open(path)?);
        let mut magic = [0u8; 8];
        reader.read_exact(&mut magic)?;
        if &magic != MAGIC {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "not a tuners session file (bad magic)",
            ));
        }
        Ok(Self { reader })
    }
}

impl<'a> StintReader<&'a [u8]> {
    /// Read an in-memory recording (bundle ingest validates without touching disk).
    pub fn open_bytes(bytes: &'a [u8]) -> io::Result<Self> {
        match bytes.strip_prefix(MAGIC.as_slice()) {
            Some(rest) => Ok(Self { reader: rest }),
            None => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "not a tuners session file (bad magic)",
            )),
        }
    }
}

impl<R: Read> StintReader<R> {
    /// Returns (receive micros, payload), or None at clean end-of-file.
    pub fn next_packet(&mut self) -> io::Result<Option<(u64, Vec<u8>)>> {
        let mut header = [0u8; 12];
        let mut filled = 0;
        while filled < header.len() {
            let n = self.reader.read(&mut header[filled..])?;
            if n == 0 {
                break;
            }
            filled += n;
        }
        if filled == 0 {
            return Ok(None);
        }
        if filled < header.len() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "truncated record header",
            ));
        }
        let recv_micros = u64::from_le_bytes(header[0..8].try_into().unwrap());
        let len = u32::from_le_bytes(header[8..12].try_into().unwrap());
        if len > MAX_PAYLOAD {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("record length {len} exceeds sanity limit"),
            ));
        }
        let mut payload = vec![0u8; len as usize];
        self.reader.read_exact(&mut payload).map_err(|_| {
            io::Error::new(io::ErrorKind::UnexpectedEof, "truncated record payload")
        })?;
        Ok(Some((recv_micros, payload)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let path = std::env::temp_dir().join(format!("tuners-session-test-{}", std::process::id()));
        let payloads: Vec<Vec<u8>> = vec![vec![1, 2, 3], vec![0; 324], vec![9]];

        let mut w = StintWriter::create(&path).unwrap();
        for (i, p) in payloads.iter().enumerate() {
            w.write_packet(1000 + i as u64, p).unwrap();
        }
        assert_eq!(w.packets(), 3);

        let mut r = StintReader::open(&path).unwrap();
        for (i, p) in payloads.iter().enumerate() {
            let (ts, payload) = r.next_packet().unwrap().unwrap();
            assert_eq!(ts, 1000 + i as u64);
            assert_eq!(&payload, p);
        }
        assert!(r.next_packet().unwrap().is_none());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn rejects_bad_magic() {
        let path = std::env::temp_dir().join(format!("tuners-magic-test-{}", std::process::id()));
        std::fs::write(&path, b"NOTMAGIC").unwrap();
        assert!(StintReader::open(&path).is_err());
        std::fs::remove_file(&path).ok();
    }
}
