//! End-to-end tests for the bundle receiver: a real listener on an
//! ephemeral port, raw HTTP over TcpStream. The same requests must behave
//! identically against the Cloudflare Worker (worker/); this is the protocol's
//! reference implementation.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use tuners::sharing::receive::{ReceiveConfig, run_listener};
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
        priors_path: None,
    };
    std::thread::spawn(move || run_listener(listener, cfg));
    Server { addr, root }
}

/// Raw GET returning (status, joined header lines, body).
fn get(addr: SocketAddr, path: &str, extra_headers: &str) -> (u16, String, String) {
    let mut s = TcpStream::connect(addr).unwrap();
    s.write_all(format!("GET {path} HTTP/1.1\r\nHost: t\r\n{extra_headers}\r\n").as_bytes())
        .unwrap();
    let mut resp = String::new();
    s.read_to_string(&mut resp).unwrap();
    let status = resp.split_whitespace().nth(1).unwrap().parse().unwrap();
    let (head, body) = resp.split_once("\r\n\r\n").unwrap();
    (status, head.to_string(), body.to_string())
}

fn header_value<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines()
        .find_map(|l| {
            l.split_once(": ")
                .filter(|(k, _)| k.eq_ignore_ascii_case(name))
        })
        .map(|(_, v)| v)
}

/// Lockdown-mode server (issued-token allowlist), generous global cap.
fn start(max_bundle_bytes: u64, daily_cap_bytes: u64, tag: &str) -> Server {
    start_mode(max_bundle_bytes, daily_cap_bytes, u64::MAX, true, "", tag)
}

/// PUT with explicit content-length so tests can lie about it (413 path).
/// Early rejects race the body write against the server's close (a TCP RST
/// can eat the response), so an empty read retries on a fresh connection.
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
    // 32 of 40 daily bytes used; the next 16 must be refused.
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
    // a third from a fresh sender is refused; per-sender caps alone can't
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

/// GET /v1/priors pins the crowd-prior distribution protocol (plan 025):
/// public and unauthenticated, content ETag with 304 on If-None-Match, the
/// detached ed25519 signature riding X-Priors-Signature so a client verifies
/// against its pinned key in one round trip, 404 while unpublished. The
/// Worker serves the same shape from R2 (etag values differ; both opaque).
#[test]
fn priors_route() {
    use tuners::advice::priors;

    // Unconfigured server: 404, same as an empty bucket.
    let srv = start(64 << 20, 512 << 20, "priors-off");
    let (status, _, body) = get(srv.addr, "/v1/priors", "");
    assert_eq!(status, 404, "{body}");
    assert!(body.contains("no priors published"), "{body}");

    // Published artifact + signature: the full maintainer-to-client loop.
    let dir = std::env::temp_dir().join(format!("tuners-receive-priors-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let artifact = dir.join("crowd-priors.json");
    let content = "{\"schema\":1,\"minAppSchema\":1,\"cells\":[]}\n";
    std::fs::write(&artifact, content).unwrap();
    let key = dir.join("priors.key");
    let pubkey = priors::keygen(&key).unwrap();
    let sig = priors::sign(&key, content.as_bytes()).unwrap();
    std::fs::write(dir.join("crowd-priors.json.sig"), format!("{sig}\n")).unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let cfg = ReceiveConfig {
        root: dir.join("inbox"),
        tokens_path: dir.join("no-tokens.txt"),
        blocklist_path: dir.join("no-blocklist.txt"),
        max_bundle_bytes: 64 << 20,
        daily_cap_bytes: 512 << 20,
        global_cap_bytes: u64::MAX,
        priors_path: Some(artifact.clone()),
    };
    std::thread::spawn(move || run_listener(listener, cfg));

    let (status, head, got) = get(addr, "/v1/priors", "");
    assert_eq!(status, 200, "{got}");
    assert_eq!(got, content);
    let etag = header_value(&head, "etag")
        .expect("etag header")
        .to_string();
    assert!(etag.starts_with('"') && etag.ends_with('"'), "{etag}");
    assert!(header_value(&head, "cache-control").is_some(), "{head}");
    let served_sig = header_value(&head, "x-priors-signature").expect("sig header");
    assert!(priors::verify(&pubkey, got.as_bytes(), served_sig));
    assert!(!priors::verify(&pubkey, b"tampered", served_sig));

    // Matching If-None-Match: 304, no body, etag still present.
    let (status, head, got) = get(addr, "/v1/priors", &format!("If-None-Match: {etag}\r\n"));
    assert_eq!(status, 304, "{got}");
    assert!(got.is_empty(), "{got}");
    assert_eq!(header_value(&head, "etag"), Some(etag.as_str()));

    // Stale etag: full 200 again.
    let (status, ..) = get(addr, "/v1/priors", "If-None-Match: \"stale\"\r\n");
    assert_eq!(status, 200);

    // Updated artifact: the etag moves, the old one stops matching.
    std::fs::write(
        &artifact,
        "{\"schema\":1,\"minAppSchema\":1,\"cells\":[{}]}\n",
    )
    .unwrap();
    let (status, head, _) = get(addr, "/v1/priors", &format!("If-None-Match: {etag}\r\n"));
    assert_eq!(status, 200);
    assert_ne!(header_value(&head, "etag"), Some(etag.as_str()));
}

/// The client fetch loop against the twin: verified store, ETag reuse,
/// tamper rejection, and the unpublished endpoint.
#[test]
fn priors_fetch_e2e() {
    use tuners::advice::priors;

    let dir = std::env::temp_dir().join(format!("tuners-fetch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let served = dir.join("served.json");
    // A real (empty-corpus) artifact: fetch parses before storing, so a
    // structurally invalid body would be rejected even correctly signed.
    let content = priors::render(&priors::derive(
        &Default::default(),
        "20260808-000000".into(),
    ));
    let content = content.as_str();
    std::fs::write(served.as_path(), content).unwrap();
    let key = dir.join("priors.key");
    let pubkey = priors::keygen(&key).unwrap();
    let sig = priors::sign(&key, content.as_bytes()).unwrap();
    std::fs::write(dir.join("served.json.sig"), format!("{sig}\n")).unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let cfg = ReceiveConfig {
        root: dir.join("inbox"),
        tokens_path: dir.join("no-tokens.txt"),
        blocklist_path: dir.join("no-blocklist.txt"),
        max_bundle_bytes: 64 << 20,
        daily_cap_bytes: 512 << 20,
        global_cap_bytes: u64::MAX,
        priors_path: Some(served.clone()),
    };
    std::thread::spawn(move || run_listener(listener, cfg));

    let artifact = dir.join("crowd-priors.json");
    let etag = dir.join("crowd-priors.etag");

    // First fetch: stored after verification, etag cached.
    let out = priors::fetch_to(&endpoint, &pubkey, &artifact, &etag).unwrap();
    assert!(matches!(out, priors::FetchOutcome::Updated));
    assert_eq!(std::fs::read_to_string(&artifact).unwrap(), content);
    assert!(!std::fs::read_to_string(&etag).unwrap().trim().is_empty());

    // Second fetch: 304 via the cached etag, nothing rewritten.
    let before = std::fs::metadata(&artifact).unwrap().modified().unwrap();
    let out = priors::fetch_to(&endpoint, &pubkey, &artifact, &etag).unwrap();
    assert!(matches!(out, priors::FetchOutcome::Unchanged));
    assert_eq!(
        std::fs::metadata(&artifact).unwrap().modified().unwrap(),
        before
    );

    // Wrong pinned key: the response is discarded, the store untouched.
    let err = priors::fetch_to(
        &endpoint,
        &"0".repeat(64),
        &artifact,
        &dir.join("other.etag"),
    )
    .unwrap_err();
    assert!(err.contains("signature"), "{err}");
    assert_eq!(std::fs::read_to_string(&artifact).unwrap(), content);

    // Tampered artifact on the server: same rejection.
    std::fs::write(
        &served,
        "{\"schema\":1,\"minAppSchema\":1,\"cells\":[{}]}\n",
    )
    .unwrap();
    let err = priors::fetch_to(&endpoint, &pubkey, &artifact, &dir.join("t.etag")).unwrap_err();
    assert!(err.contains("signature"), "{err}");
    assert_eq!(std::fs::read_to_string(&artifact).unwrap(), content);

    // Unpublished endpoint: Missing, local copy kept.
    let srv = start(64 << 20, 512 << 20, "fetch-off");
    let endpoint = format!("http://{}", srv.addr);
    let out = priors::fetch_to(&endpoint, &pubkey, &artifact, &etag).unwrap();
    assert!(matches!(out, priors::FetchOutcome::Missing));
    assert_eq!(std::fs::read_to_string(&artifact).unwrap(), content);
}
