//! End-to-end tests for the bundle receiver (plan 009): a real listener on an
//! ephemeral port, raw HTTP over TcpStream. The same requests must behave
//! identically against the Cloudflare Worker (worker/) — this is the protocol's
//! reference implementation.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use tuners::receive::{ReceiveConfig, run_listener};
use tuners::util::sha256_hex;

struct Server {
    addr: SocketAddr,
    root: PathBuf,
}

fn start_mode(
    max_bundle_bytes: u64,
    daily_cap_bytes: u64,
    global_cap_bytes: u64,
    lockdown: bool,
    blocklist: &str,
    tag: &str,
) -> Server {
    let dir = std::env::temp_dir().join(format!("tuners-receive-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let tokens = dir.join("tokens.txt");
    if lockdown {
        std::fs::write(&tokens, "# comment line\nsecrettoken friend-1\n").unwrap();
    }
    let blocklist_path = dir.join("blocklist.txt");
    if !blocklist.is_empty() {
        std::fs::write(&blocklist_path, blocklist).unwrap();
    }
    let root = dir.join("inbox");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let cfg = ReceiveConfig {
        root: root.clone(),
        tokens_path: tokens,
        blocklist_path,
        max_bundle_bytes,
        daily_cap_bytes,
        global_cap_bytes,
    };
    std::thread::spawn(move || run_listener(listener, cfg));
    Server { addr, root }
}

/// Lockdown-mode server (issued-token allowlist), generous global cap.
fn start(max_bundle_bytes: u64, daily_cap_bytes: u64, tag: &str) -> Server {
    start_mode(max_bundle_bytes, daily_cap_bytes, u64::MAX, true, "", tag)
}

/// PUT with explicit content-length so tests can lie about it (413 path).
/// Early rejects race the body write against the server's close — a TCP RST
/// can eat the response — so an empty read retries on a fresh connection.
fn put_raw(
    addr: SocketAddr,
    name: &str,
    token: Option<&str>,
    sha: Option<&str>,
    content_length: u64,
    body: &[u8],
) -> (u16, String) {
    for _ in 0..3 {
        let mut s = TcpStream::connect(addr).unwrap();
        let mut req = format!(
            "PUT /v1/bundle/{name} HTTP/1.1\r\nHost: t\r\nContent-Length: {content_length}\r\n"
        );
        if let Some(t) = token {
            req += &format!("Authorization: Bearer {t}\r\n");
        }
        if let Some(h) = sha {
            req += &format!("X-Bundle-SHA256: {h}\r\n");
        }
        req += "\r\n";
        s.write_all(req.as_bytes()).unwrap();
        let _ = s.write_all(body); // may hit a closed socket on early rejects
        let _ = s.shutdown(std::net::Shutdown::Write); // half-close: we sent everything
        let mut resp = String::new();
        let _ = s.read_to_string(&mut resp); // RST after a partial read is fine too
        if let Some(status) = resp.split_whitespace().nth(1).and_then(|c| c.parse().ok()) {
            let body = resp.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
            return (status, body);
        }
    }
    panic!("no HTTP response after 3 attempts");
}

fn put(addr: SocketAddr, name: &str, token: Option<&str>, body: &[u8]) -> (u16, String) {
    let sha = sha256_hex(body);
    put_raw(addr, name, token, Some(&sha), body.len() as u64, body)
}

fn stored_files(root: &Path) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(root.join("friend-1")) else {
        return Vec::new();
    };
    let mut names: Vec<String> = rd
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn upload_store_and_dedupe() {
    let srv = start(64 << 20, 512 << 20, "happy");
    let body = b"pretend this is a tar.zst bundle";
    let hash16 = &sha256_hex(body)[..16];

    let (status, resp) = put(
        srv.addr,
        "bundle-2793-20260725-191508.tar.zst",
        Some("secrettoken"),
        body,
    );
    assert_eq!(status, 200, "{resp}");
    let expect_name = format!("bundle-2793-20260725-191508-{hash16}.tar.zst");
    assert!(
        resp.contains(&format!("\"stored\":\"friend-1/{expect_name}\"")),
        "{resp}"
    );
    assert!(resp.contains("\"duplicate\":false"), "{resp}");
    assert_eq!(stored_files(&srv.root), vec![expect_name.clone()]);
    let on_disk = std::fs::read(srv.root.join("friend-1").join(&expect_name)).unwrap();
    assert_eq!(on_disk, body);

    // Retrying the same upload is idempotent: acknowledged, not re-stored.
    let (status, resp) = put(
        srv.addr,
        "bundle-2793-20260725-191508.tar.zst",
        Some("secrettoken"),
        body,
    );
    assert_eq!(status, 200);
    assert!(resp.contains("\"duplicate\":true"), "{resp}");
    assert_eq!(stored_files(&srv.root).len(), 1);

    // Same stamp, different content (a re-cut stint) stores alongside.
    let (status, _) = put(
        srv.addr,
        "bundle-2793-20260725-191508.tar.zst",
        Some("secrettoken"),
        b"different content",
    );
    assert_eq!(status, 200);
    assert_eq!(stored_files(&srv.root).len(), 2);
}

#[test]
fn auth_is_required() {
    let srv = start(64 << 20, 512 << 20, "auth");
    let (status, _) = put(srv.addr, "a.tar.zst", None, b"x");
    assert_eq!(status, 401);
    let (status, _) = put(srv.addr, "a.tar.zst", Some("wrongtoken"), b"x");
    assert_eq!(status, 401);
    assert!(stored_files(&srv.root).is_empty());
}

#[test]
fn hash_mismatch_stores_nothing() {
    let srv = start(64 << 20, 512 << 20, "hash");
    let bad = "0".repeat(64);
    let (status, _) = put_raw(
        srv.addr,
        "a.tar.zst",
        Some("secrettoken"),
        Some(&bad),
        4,
        b"body",
    );
    assert_eq!(status, 422);
    let (status, _) = put_raw(
        srv.addr,
        "a.tar.zst",
        Some("secrettoken"),
        Some("nothex"),
        4,
        b"body",
    );
    assert_eq!(status, 400);
    let (status, _) = put_raw(srv.addr, "a.tar.zst", Some("secrettoken"), None, 4, b"body");
    assert_eq!(status, 400);
    assert!(stored_files(&srv.root).is_empty());
    // No stray .part temp files either.
    assert!(
        std::fs::read_dir(srv.root.join("friend-1"))
            .map(|rd| rd.count() == 0)
            .unwrap_or(true)
    );
}

#[test]
fn size_and_daily_caps() {
    let srv = start(16, 40, "caps"); // 16-byte bundles, 40 bytes/day
    let sha = sha256_hex(b"");
    let (status, _) = put_raw(
        srv.addr,
        "big.tar.zst",
        Some("secrettoken"),
        Some(&sha),
        17,
        b"",
    );
    assert_eq!(status, 413);

    let (status, _) = put(
        srv.addr,
        "one.tar.zst",
        Some("secrettoken"),
        b"0123456789abcdef",
    );
    assert_eq!(status, 200);
    let (status, _) = put(
        srv.addr,
        "two.tar.zst",
        Some("secrettoken"),
        b"0123456789ABCDEF",
    );
    assert_eq!(status, 200);
    // 32 of 40 daily bytes used — the next 16 must be refused.
    let (status, resp) = put(
        srv.addr,
        "three.tar.zst",
        Some("secrettoken"),
        b"0123456789!@#$%^",
    );
    assert_eq!(status, 429, "{resp}");
    assert_eq!(stored_files(&srv.root).len(), 2);
}

#[test]
fn hostile_names_rejected() {
    let srv = start(64 << 20, 512 << 20, "names");
    for name in [
        "../../etc/passwd.tar.zst",
        "..%2Fescape.tar.zst",
        "plain.txt",
        ".hidden.tar.zst",
    ] {
        let (status, _) = put(srv.addr, name, Some("secrettoken"), b"x");
        assert_eq!(status, 400, "{name}");
    }
    assert!(stored_files(&srv.root).is_empty());
}

/// The open-mode sender derivation is shared with the Worker: token 'a'*64
/// MUST map to sender ffe054fe7ae0cb6d in both implementations (this exact
/// pair was verified against worker/src/index.js on emulated R2, 2026-07-25).
#[test]
fn open_mode_client_tokens() {
    let srv = start_mode(64 << 20, 512 << 20, u64::MAX, false, "", "open");
    let token = "a".repeat(64);
    let body = b"open mode bundle";
    let (status, resp) = put(srv.addr, "x.tar.zst", Some(&token), body);
    assert_eq!(status, 200, "{resp}");
    assert!(resp.contains("\"stored\":\"ffe054fe7ae0cb6d/x-"), "{resp}");
    assert_eq!(
        tuners::util::sha256_hex(token.as_bytes())[..16].to_string(),
        "ffe054fe7ae0cb6d"
    );
    // Uppercase hex normalizes to the same sender.
    let (status, resp) = put(srv.addr, "y.tar.zst", Some(&"A".repeat(64)), body);
    assert_eq!(status, 200);
    assert!(resp.contains("\"stored\":\"ffe054fe7ae0cb6d/y-"), "{resp}");

    for bad in [
        "tooshort",
        &"z".repeat(64),
        &"a".repeat(63),
        &"a".repeat(65),
    ] {
        let (status, _) = put(srv.addr, "z.tar.zst", Some(bad), body);
        assert_eq!(status, 401, "{bad}");
    }
}

#[test]
fn open_mode_blocklist() {
    // Block the sender id of token 'b'*64 (= a0fab1377f49a759, matching the
    // Worker's dev blocklist fixture).
    let blocked = &tuners::util::sha256_hex("b".repeat(64).as_bytes())[..16];
    let srv = start_mode(
        64 << 20,
        512 << 20,
        u64::MAX,
        false,
        &format!("# banned\n{blocked}\n"),
        "blocklist",
    );
    let (status, resp) = put(srv.addr, "x.tar.zst", Some(&"b".repeat(64)), b"nope");
    assert_eq!(status, 403, "{resp}");
    let (status, _) = put(srv.addr, "x.tar.zst", Some(&"a".repeat(64)), b"fine");
    assert_eq!(status, 200);
}

#[test]
fn global_storage_ceiling() {
    // 40-byte global cap: two 16-byte bundles from DIFFERENT senders fit,
    // a third from a fresh sender is refused — per-sender caps alone can't
    // bound cost when tokens are free to mint.
    let srv = start_mode(16, 512 << 20, 40, false, "", "global");
    let (status, _) = put(
        srv.addr,
        "one.tar.zst",
        Some(&"a".repeat(64)),
        b"0123456789abcdef",
    );
    assert_eq!(status, 200);
    let (status, _) = put(
        srv.addr,
        "two.tar.zst",
        Some(&"c".repeat(64)),
        b"0123456789ABCDEF",
    );
    assert_eq!(status, 200);
    let (status, resp) = put(
        srv.addr,
        "three.tar.zst",
        Some(&"d".repeat(64)),
        b"0123456789!@#$%^",
    );
    assert_eq!(status, 507, "{resp}");
    assert!(resp.contains("storage is full"), "{resp}");
}

#[test]
fn health_and_unknown_routes() {
    let srv = start(64 << 20, 512 << 20, "routes");
    let mut s = TcpStream::connect(srv.addr).unwrap();
    s.write_all(b"GET /healthz HTTP/1.1\r\nHost: t\r\n\r\n")
        .unwrap();
    let _ = s.shutdown(std::net::Shutdown::Write);
    let mut resp = String::new();
    s.read_to_string(&mut resp).unwrap();
    assert!(resp.starts_with("HTTP/1.1 200"), "{resp}");

    let mut s = TcpStream::connect(srv.addr).unwrap();
    s.write_all(b"GET /v1/bundle/a.tar.zst HTTP/1.1\r\nHost: t\r\n\r\n")
        .unwrap();
    let _ = s.shutdown(std::net::Shutdown::Write);
    let mut resp = String::new();
    s.read_to_string(&mut resp).unwrap();
    assert!(resp.starts_with("HTTP/1.1 405"), "{resp}");
}
