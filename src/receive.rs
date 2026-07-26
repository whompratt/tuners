//! Telemetry bundle receiver (plan 009): the collection endpoint. A dumb
//! authenticated PUT into filesystem storage — the server never parses
//! telemetry; validation and quarantine happen later in `tuners ingest`.
//! Hand-rolled HTTP over std like serve.rs; TLS belongs to a reverse proxy
//! (Caddy/nginx) in front, so this binds loopback by default.
//!
//! Protocol (client = the outbox drainer, or curl when debugging):
//!
//!   PUT /v1/bundle/<name>.tar.zst
//!     Authorization: Bearer <token>        (issued via `receive --issue`)
//!     X-Bundle-SHA256: <64 hex>            (SHA-256 of the request body)
//!     Content-Length: <bytes>
//!   -> 200 {"ok":true,"stored":"<sender>/<name>-<hash16>.tar.zst",...}
//!
//!   GET /healthz -> 200 (unauthenticated, for proxy health checks)
//!
//! The body is hashed while streaming to a temp file and only renamed into
//! place when the hash matches the header — a truncated or corrupted upload
//! never lands. The stored name carries the first 16 hash hex chars, so a
//! retried upload of the same content answers 200 "duplicate" and rewrites
//! nothing, while a re-cut stint with the same stamp stores alongside instead
//! of silently overwriting.
//!
//! Storage is one directory per sender under `root` — the per-sender library
//! namespace the plan asks for.
//!
//! Auth is OPEN by default (strangers-scale opt-in): the client generates its
//! own 64-hex token and the sender id is sha256(token)[..16] — stable
//! pseudonymous identity without issuance. Misbehaving sender ids go in the
//! blocklist file. If the tokens file exists (`<token> <sender-id>` lines,
//! re-read per request), the endpoint is in allowlist-only lockdown mode
//! instead. Delete a sender's data on request by removing their directory;
//! nothing else about the sender is recorded.
//!
//! Cost/abuse bounds mirror the Worker: per-sender rolling-24h cap plus a
//! global storage ceiling (507 when full). The Worker adds a per-IP rate
//! limit; this local twin doesn't bother (it sits on loopback).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

pub struct ReceiveConfig {
    pub root: PathBuf,
    pub tokens_path: PathBuf,
    pub blocklist_path: PathBuf,
    pub max_bundle_bytes: u64,
    /// Per-sender cap on bytes accepted in any rolling 24h window.
    pub daily_cap_bytes: u64,
    /// Ceiling on total stored bytes across all senders: hostile uploads can
    /// pin storage at this worst case but never grow it.
    pub global_cap_bytes: u64,
}

pub fn run(bind: &str, port: u16, cfg: ReceiveConfig) -> std::io::Result<()> {
    let listener = TcpListener::bind((bind, port))?;
    let mode = if cfg.tokens_path.exists() {
        let senders = std::fs::read_to_string(&cfg.tokens_path)
            .map(|text| text.lines().filter(|l| token_line(l).is_some()).count())
            .unwrap_or(0);
        format!(
            "lockdown ({} issued token(s) in {})",
            senders,
            cfg.tokens_path.display()
        )
    } else {
        "open (client-generated tokens, sender = sha256(token)[..16])".to_string()
    };
    println!(
        "tuners receiver: http://{bind}:{port}/v1/bundle/  root {}  auth {mode}",
        cfg.root.display(),
    );
    run_listener(listener, cfg)
}

/// Accept loop, separated so tests can bind an ephemeral port themselves.
pub fn run_listener(listener: TcpListener, cfg: ReceiveConfig) -> std::io::Result<()> {
    let cfg = std::sync::Arc::new(cfg);
    for stream in listener.incoming().flatten() {
        let cfg = cfg.clone();
        std::thread::spawn(move || handle(stream, &cfg));
    }
    Ok(())
}

fn handle(mut stream: TcpStream, cfg: &ReceiveConfig) {
    // An internet-facing endpoint gets slow/stuck clients; don't hold a
    // thread open for them.
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));
    let mut reader = BufReader::new(match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    });
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).is_err() || request_line.len() > 2048 {
        return;
    }
    let method = request_line.split_whitespace().next().unwrap_or("");
    let target = request_line.split_whitespace().nth(1).unwrap_or("/");

    let mut content_length: Option<u64> = None;
    let mut auth: Option<String> = None;
    let mut claimed_sha: Option<String> = None;
    let mut expect_continue = false;
    for _ in 0..64 {
        let mut header = String::new();
        match reader.read_line(&mut header) {
            Ok(0) | Err(_) => return,
            Ok(_) if header == "\r\n" || header == "\n" => break,
            Ok(n) if n > 8192 => return,
            Ok(_) => {
                let lower = header.to_ascii_lowercase();
                if let Some(v) = lower.strip_prefix("content-length:") {
                    content_length = v.trim().parse().ok();
                } else if let Some(v) = lower.strip_prefix("x-bundle-sha256:") {
                    claimed_sha = Some(v.trim().to_string());
                } else if lower.starts_with("expect:") && lower.contains("100-continue") {
                    expect_continue = true;
                } else if let Some(v) = header.to_ascii_lowercase().strip_prefix("authorization:") {
                    // Token case matters; take it from the original header.
                    let raw = header[header.len() - v.len()..].trim();
                    auth = raw
                        .strip_prefix("Bearer ")
                        .or_else(|| raw.strip_prefix("bearer "))
                        .map(|t| t.trim().to_string());
                }
            }
        }
    }

    let mut body_consumed = 0u64;
    let (status, body) = match (method, target) {
        ("GET", "/healthz") => ("200 OK", "{\"ok\":true}".to_string()),
        ("PUT", t) if t.starts_with("/v1/bundle/") => {
            let name = &t["/v1/bundle/".len()..];
            match put_bundle(
                cfg,
                name,
                auth.as_deref(),
                claimed_sha.as_deref(),
                content_length,
                expect_continue,
                &mut stream,
                &mut reader,
                &mut body_consumed,
            ) {
                Ok(pair) | Err(pair) => pair,
            }
        }
        ("PUT", _) => ("404 Not Found", err_json("unknown path")),
        _ => (
            "405 Method Not Allowed",
            err_json("PUT bundles to /v1/bundle/<name>.tar.zst"),
        ),
    };
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len(),
    );
    let _ = stream.write_all(body.as_bytes());
    // A rejected upload leaves body bytes unread, and closing with unread
    // data can RST away the response we just wrote (curl then reports "no
    // response", so a permanent 4xx looks transient to the drainer). Drain
    // exactly the unread remainder — zero on clean paths, and bounded by the
    // read timeout if the client stalls mid-body.
    let mut drain_left = content_length
        .unwrap_or(0)
        .saturating_sub(body_consumed)
        .min(cfg.max_bundle_bytes);
    let mut buf = [0u8; 64 * 1024];
    while drain_left > 0 {
        match reader.read(&mut buf[..(drain_left.min(64 * 1024) as usize)]) {
            Ok(0) | Err(_) => break,
            Ok(n) => drain_left -= n as u64,
        }
    }
}

type Reply = (&'static str, String);

#[allow(clippy::too_many_arguments)]
fn put_bundle(
    cfg: &ReceiveConfig,
    name: &str,
    auth: Option<&str>,
    claimed_sha: Option<&str>,
    content_length: Option<u64>,
    expect_continue: bool,
    stream: &mut TcpStream,
    reader: &mut BufReader<TcpStream>,
    body_consumed: &mut u64,
) -> Result<Reply, Reply> {
    // Everything is checked BEFORE the body is read: a rejected upload costs
    // the sender headers, not megabytes.
    let token = auth.ok_or(("401 Unauthorized", err_json("missing bearer token")))?;
    let sender = authenticate(cfg, token)?;
    let stem = validate_bundle_name(name).ok_or((
        "400 Bad Request",
        err_json("bundle name must be <stem>.tar.zst, stem of [A-Za-z0-9._-]"),
    ))?;
    let claimed = claimed_sha
        .map(str::to_ascii_lowercase)
        .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or((
            "400 Bad Request",
            err_json("X-Bundle-SHA256 header (64 hex) required"),
        ))?;
    let len = content_length.ok_or(("411 Length Required", err_json("Content-Length required")))?;
    if len == 0 {
        return Err(("400 Bad Request", err_json("empty body")));
    }
    if len > cfg.max_bundle_bytes {
        return Err((
            "413 Content Too Large",
            err_json(&format!("bundle exceeds {} byte cap", cfg.max_bundle_bytes)),
        ));
    }
    if total_bytes(&cfg.root).saturating_add(len) > cfg.global_cap_bytes {
        return Err((
            "507 Insufficient Storage",
            err_json("collection storage is full — uploads paused, retry tomorrow"),
        ));
    }
    let sender_dir = cfg.root.join(&sender);
    if recent_bytes(&sender_dir).saturating_add(len) > cfg.daily_cap_bytes {
        return Err((
            "429 Too Many Requests",
            err_json("daily upload cap reached — the outbox can retry tomorrow"),
        ));
    }

    std::fs::create_dir_all(&sender_dir)
        .map_err(|e| ("500 Internal Server Error", err_json(&e.to_string())))?;
    if expect_continue {
        let _ = stream.write_all(b"HTTP/1.1 100 Continue\r\n\r\n");
    }

    // Stream to a unique temp file while hashing; only a verified body is
    // renamed into place.
    static TMP_SEQ: AtomicU64 = AtomicU64::new(0);
    let tmp = sender_dir.join(format!(
        ".part-{}-{}",
        std::process::id(),
        TMP_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let outcome = (|| -> Result<Reply, Reply> {
        let mut out = std::fs::File::create(&tmp)
            .map_err(|e| ("500 Internal Server Error", err_json(&e.to_string())))?;
        let mut hasher = crate::util::Sha256::new();
        let mut buf = [0u8; 64 * 1024];
        let mut remaining = len;
        while remaining > 0 {
            let want = remaining.min(buf.len() as u64) as usize;
            let n = reader
                .read(&mut buf[..want])
                .map_err(|_| ("400 Bad Request", err_json("body read failed")))?;
            if n == 0 {
                return Err(("400 Bad Request", err_json("body truncated")));
            }
            *body_consumed += n as u64;
            hasher.update(&buf[..n]);
            out.write_all(&buf[..n])
                .map_err(|e| ("500 Internal Server Error", err_json(&e.to_string())))?;
            remaining -= n as u64;
        }
        let hex = hasher.finish_hex();
        if hex != claimed {
            return Err((
                "422 Unprocessable Content",
                err_json("body does not match X-Bundle-SHA256"),
            ));
        }
        let final_name = format!("{stem}-{}.tar.zst", &hex[..16]);
        let dest = sender_dir.join(&final_name);
        let duplicate = dest.exists();
        if !duplicate {
            std::fs::rename(&tmp, &dest)
                .map_err(|e| ("500 Internal Server Error", err_json(&e.to_string())))?;
        }
        println!(
            "{} {sender}/{final_name} ({len} bytes)",
            if duplicate { "duplicate" } else { "stored" },
        );
        Ok((
            "200 OK",
            format!(
                "{{\"ok\":true,\"stored\":\"{sender}/{final_name}\",\"duplicate\":{duplicate}}}"
            ),
        ))
    })();
    if outcome.is_err() {
        let _ = std::fs::remove_file(&tmp);
    } else if tmp.exists() {
        // Duplicate path: the verified temp copy is redundant.
        let _ = std::fs::remove_file(&tmp);
    }
    outcome
}

fn err_json(msg: &str) -> String {
    format!("{{\"ok\":false,\"error\":\"{}\"}}", msg.replace('"', "'"))
}

/// Tokens file present = lockdown (issued tokens only); absent = open mode,
/// where any well-formed 64-hex client-generated token maps to the sender id
/// sha256(token)[..16] — the same derivation as the Worker, pinned by tests.
fn authenticate(cfg: &ReceiveConfig, token: &str) -> Result<String, Reply> {
    if cfg.tokens_path.exists() {
        return sender_for_token(&cfg.tokens_path, token)
            .ok_or(("401 Unauthorized", err_json("unknown token")));
    }
    let token = token.to_ascii_lowercase();
    if token.len() != 64
        || !token
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err((
            "401 Unauthorized",
            err_json("token must be 64 hex chars (client-generated at opt-in)"),
        ));
    }
    let sender = crate::util::sha256_hex(token.as_bytes())[..16].to_string();
    if blocklisted(&cfg.blocklist_path, &sender) {
        return Err(("403 Forbidden", err_json("sender blocked")));
    }
    Ok(sender)
}

/// Blocklist file: one sender id per line, comments/blanks ignored.
fn blocklisted(path: &Path, sender: &str) -> bool {
    std::fs::read_to_string(path).is_ok_and(|text| {
        text.lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .any(|l| l == sender)
    })
}

/// Total stored bytes across every sender directory (the global ceiling).
fn total_bytes(root: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(root) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| {
            let Ok(files) = std::fs::read_dir(e.path()) else {
                return 0;
            };
            files
                .flatten()
                .filter_map(|f| {
                    let md = f.metadata().ok()?;
                    (md.is_file() && f.file_name().to_string_lossy().ends_with(".tar.zst"))
                        .then_some(md.len())
                })
                .sum()
        })
        .sum()
}

/// `<token> <sender-id>` from a tokens-file line, skipping comments/blanks.
fn token_line(line: &str) -> Option<(&str, &str)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    let (tok, sender) = line.split_once(char::is_whitespace)?;
    let sender = sender.trim();
    (!tok.is_empty() && !sender.is_empty()).then_some((tok, sender))
}

/// Re-read per request so revocation (deleting a line) needs no restart.
fn sender_for_token(tokens_path: &Path, presented: &str) -> Option<String> {
    let text = std::fs::read_to_string(tokens_path).ok()?;
    for line in text.lines() {
        if let Some((tok, sender)) = token_line(line)
            && constant_time_eq(tok, presented)
        {
            return Some(sender.to_string());
        }
    }
    None
}

fn constant_time_eq(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

/// The stem of a safe `<stem>.tar.zst` client name: no separators, so a
/// hostile name can never traverse out of the sender's directory.
fn validate_bundle_name(name: &str) -> Option<&str> {
    let stem = name.strip_suffix(".tar.zst")?;
    (!stem.is_empty()
        && stem.len() <= 100
        && !stem.starts_with('.')
        && stem
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.'))
    .then_some(stem)
}

/// Bytes this sender stored in the last 24h (mtime = receipt time). Derived
/// from the filesystem so the cap needs no database and survives restarts.
fn recent_bytes(sender_dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(sender_dir) else {
        return 0;
    };
    let now = SystemTime::now();
    entries
        .flatten()
        .filter_map(|e| {
            let md = e.metadata().ok()?;
            if !md.is_file() || !e.file_name().to_string_lossy().ends_with(".tar.zst") {
                return None;
            }
            let age = now.duration_since(md.modified().ok()?).unwrap_or_default();
            (age.as_secs() < 86_400).then_some(md.len())
        })
        .sum()
}

/// Mint a sender token: 32 random bytes (hex) appended to the tokens file,
/// which is kept owner-only since it is the whole credential store.
pub fn issue_token(tokens_path: &Path, sender: &str) -> Result<String, String> {
    let ok_id = !sender.is_empty()
        && sender.len() <= 32
        && sender
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !sender.starts_with('-');
    if !ok_id {
        return Err("sender id must be 1-32 chars of [a-z0-9-], not starting with '-'".into());
    }
    let mut bytes = [0u8; 32];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .map_err(|e| format!("/dev/urandom: {e} (token issuance runs on the server)"))?;
    let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(tokens_path)
        .map_err(|e| format!("{}: {e}", tokens_path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = file.set_permissions(std::fs::Permissions::from_mode(0o600));
    }
    writeln!(file, "{token} {sender}").map_err(|e| e.to_string())?;
    Ok(token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_names_are_fenced() {
        assert_eq!(
            validate_bundle_name("bundle-2793-20260725-191508.tar.zst"),
            Some("bundle-2793-20260725-191508")
        );
        assert_eq!(validate_bundle_name("a.tar.zst"), Some("a"));
        for bad in [
            "no-suffix",
            ".tar.zst",
            "..-evil.tar.zst",
            "a/b.tar.zst",
            "a\\b.tar.zst",
            "sp ace.tar.zst",
            "évil.tar.zst",
        ] {
            assert_eq!(validate_bundle_name(bad), None, "{bad}");
        }
        let long = format!("{}.tar.zst", "x".repeat(101));
        assert_eq!(validate_bundle_name(&long), None);
    }

    #[test]
    fn tokens_file_lines() {
        assert_eq!(token_line("abc123 jake"), Some(("abc123", "jake")));
        assert_eq!(token_line("  abc123\tjake  "), Some(("abc123", "jake")));
        assert_eq!(token_line("# comment"), None);
        assert_eq!(token_line(""), None);
        assert_eq!(token_line("token-without-sender"), None);
    }

    #[test]
    fn sender_ids_are_fenced() {
        let dir = std::env::temp_dir().join(format!("tuners-issue-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let tokens = dir.join("tokens.txt");
        for bad in ["", "UPPER", "with space", "-lead", &"x".repeat(33)] {
            assert!(issue_token(&tokens, bad).is_err(), "{bad}");
        }
        let tok = issue_token(&tokens, "friend-1").unwrap();
        assert_eq!(tok.len(), 64);
        assert_eq!(sender_for_token(&tokens, &tok).as_deref(), Some("friend-1"));
        assert_eq!(sender_for_token(&tokens, "wrong"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
