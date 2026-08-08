// Telemetry bundle collector: the deployed twin of `tuners
// receive`. A dumb authenticated PUT into R2. Never parses telemetry;
// validation/quarantine happen in `tuners ingest` after the bucket is synced
// down. The protocol (and every status code) is pinned by tests/receive.rs
// in the main repo; keep the two implementations in lockstep.
//
//   PUT /v1/bundle/<name>.tar.zst
//     Authorization: Bearer <token>
//     X-Bundle-SHA256: <64 hex of the body>
//   -> 200 {"ok":true,"stored":"<sender>/<name>-<hash16>.tar.zst","duplicate":bool}
//
//   GET /v1/priors
//   -> 200 crowd-priors.json body, ETag + X-Priors-Signature (ed25519 hex,
//      clients verify against their pinned key), 304 on If-None-Match,
//      404 until an artifact is published. Public and unauthenticated by
//      design: receiving the crowd's priors must not require contributing.
//      The maintainer pipeline uploads the artifact directly to R2
//      (wrangler r2 object put; sender prefixes are 16-hex so `_priors/`
//      can never collide) — the Worker has no write surface for it.
//
// Auth is OPEN by default (strangers-scale opt-in): the app generates its own
// 64-hex token at opt-in and the sender id is sha256(token)[..16]: stable
// pseudonymous identity without issuance. Misbehaving senders go in the
// BLOCKLIST secret (JSON array of sender ids). Setting the TOKENS secret
// (JSON {"<token>":"<sender-id>"}) flips the endpoint back to allowlist-only
// mode, the abuse circuit breaker.
//
// Cost protection (the wallet is the thing to defend; data hygiene is
// ingest's job): per-IP rate limit binding, per-sender rolling-24h byte cap,
// and a GLOBAL bucket-size ceiling so hostile uploads can pin storage at a
// known worst case instead of growing it. Checks run cheap-to-expensive so
// rejected requests cost headers, not R2 operations. R2 itself verifies the
// claimed hash (put option `sha256`); the Worker never buffers a body.
//
// Bindings: BUNDLES (R2), IP_LIMIT (ratelimit), TOKENS / BLOCKLIST (secrets,
// both optional), MAX_BUNDLE_MB / DAILY_CAP_MB / GLOBAL_CAP_GB (vars).

const NAME_RE = /^[A-Za-z0-9_-][A-Za-z0-9._-]{0,99}\.tar\.zst$/;
const TOKEN_RE = /^[0-9a-f]{64}$/;
const RATE_LIMIT = 30;
const RATE_PERIOD_MS = 60_000;
const PRIORS_KEY = "_priors/crowd-priors.json";
const PRIORS_SIG_KEY = "_priors/crowd-priors.json.sig";
const PRIORS_MAX_AGE_S = 3600;

// In-isolate sliding-window throttle: the enforced rate limit. Per-isolate,
// so it's a bound on a single noisy client, not a global guarantee: the
// hard cost bounds are the storage ceilings and the free plan's fail-closed
// request cap, not this.
const rlWindow = new Map();

function isolateThrottled(ip, limit, periodMs) {
  const now = Date.now();
  const recent = (rlWindow.get(ip) ?? []).filter((t) => now - t < periodMs);
  if (recent.length >= limit) {
    rlWindow.set(ip, recent);
    return true;
  }
  recent.push(now);
  // Bound memory against key-spraying; losing counters just un-throttles.
  if (rlWindow.size > 10_000) rlWindow.clear();
  rlWindow.set(ip, recent);
  return false;
}

function json(status, obj) {
  return new Response(JSON.stringify(obj), {
    status,
    headers: { "content-type": "application/json" },
  });
}

function fail(status, error) {
  return json(status, { ok: false, error });
}

async function senderId(token) {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(token));
  return [...new Uint8Array(digest)]
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("")
    .slice(0, 16);
}

/// Bytes this sender stored in the last 24h, from object metadata: same
/// no-database cap derivation as the Rust receiver uses on the filesystem.
async function recentBytes(bucket, sender) {
  const cutoff = Date.now() - 24 * 60 * 60 * 1000;
  let total = 0;
  let cursor;
  do {
    const page = await bucket.list({ prefix: `${sender}/`, cursor });
    for (const obj of page.objects) {
      if (obj.uploaded.getTime() >= cutoff) total += obj.size;
    }
    cursor = page.truncated ? page.cursor : undefined;
  } while (cursor);
  return total;
}

// Whole-bucket size, cached per isolate for 60s (and bumped on accepted
// uploads) so a bucket-full attack costs ~zero list operations per request.
// Best-effort across isolates; set GLOBAL_CAP_GB with margin, it is a
// storage ceiling, not an accounting ledger.
let globalCache = null;

async function globalBytes(bucket) {
  if (globalCache && Date.now() - globalCache.at < 60_000) return globalCache.bytes;
  let total = 0;
  let cursor;
  do {
    const page = await bucket.list({ cursor });
    for (const obj of page.objects) total += obj.size;
    cursor = page.truncated ? page.cursor : undefined;
  } while (cursor);
  globalCache = { bytes: total, at: Date.now() };
  return total;
}

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    if (request.method === "GET" && url.pathname === "/healthz") {
      return json(200, { ok: true });
    }
    if (request.method === "GET" && url.pathname === "/v1/priors") {
      const ip = request.headers.get("cf-connecting-ip") ?? "local";
      if (isolateThrottled(ip, RATE_LIMIT, RATE_PERIOD_MS)) {
        return fail(429, "rate limited, slow down");
      }
      // R2 conditionals take the bare etag; If-None-Match arrives quoted.
      const inm = (request.headers.get("if-none-match") ?? "")
        .replace(/^W\//, "")
        .replace(/"/g, "");
      let obj;
      try {
        obj = await env.BUNDLES.get(
          PRIORS_KEY,
          inm ? { onlyIf: { etagDoesNotMatch: inm } } : undefined,
        );
      } catch {
        obj = null;
      }
      if (!obj) return fail(404, "no priors published");
      const headers = {
        etag: obj.httpEtag,
        "cache-control": `public, max-age=${PRIORS_MAX_AGE_S}`,
      };
      // Precondition failed: R2 answers with metadata but no body.
      if (!obj.body) return new Response(null, { status: 304, headers });
      const sig = await env.BUNDLES.get(PRIORS_SIG_KEY);
      if (sig) headers["x-priors-signature"] = (await sig.text()).trim();
      headers["content-type"] = "application/json";
      return new Response(obj.body, { status: 200, headers });
    }
    if (request.method !== "PUT") {
      return fail(405, "PUT bundles to /v1/bundle/<name>.tar.zst");
    }
    if (!url.pathname.startsWith("/v1/bundle/")) {
      return fail(404, "unknown path");
    }

    const ip = request.headers.get("cf-connecting-ip") ?? "local";
    if (env.IP_LIMIT) {
      // The platform limiter is consulted but NOT relied upon: measured
      // non-enforcing on this account 2026-07-25/26 (fresh namespace, 8x
      // sustained load, success:true throughout). Honored if it ever counts.
      const { success } = await env.IP_LIMIT.limit({ key: ip });
      if (!success) return fail(429, "rate limited, slow down");
    }
    if (isolateThrottled(ip, RATE_LIMIT, RATE_PERIOD_MS)) {
      return fail(429, "rate limited, slow down");
    }

    const auth = request.headers.get("authorization") ?? "";
    const token = auth.match(/^Bearer\s+(\S+)$/i)?.[1]?.toLowerCase();
    if (!token) return fail(401, "missing bearer token");
    let sender;
    if (env.TOKENS) {
      // Lockdown mode: only issued tokens upload.
      sender = JSON.parse(env.TOKENS)[token];
      if (!sender) return fail(401, "unknown token");
    } else {
      if (!TOKEN_RE.test(token)) {
        return fail(401, "token must be 64 hex chars (client-generated at opt-in)");
      }
      sender = await senderId(token);
      if (JSON.parse(env.BLOCKLIST ?? "[]").includes(sender)) {
        return fail(403, "sender blocked");
      }
    }

    const name = url.pathname.slice("/v1/bundle/".length);
    if (!NAME_RE.test(name)) {
      return fail(400, "bundle name must be <stem>.tar.zst, stem of [A-Za-z0-9._-]");
    }
    const stem = name.slice(0, -".tar.zst".length);

    const claimed = (request.headers.get("x-bundle-sha256") ?? "").toLowerCase();
    if (!/^[0-9a-f]{64}$/.test(claimed)) {
      return fail(400, "X-Bundle-SHA256 header (64 hex) required");
    }

    const len = Number(request.headers.get("content-length"));
    if (!Number.isFinite(len) || len === 0) {
      return fail(len === 0 ? 400 : 411, len === 0 ? "empty body" : "Content-Length required");
    }
    const maxBytes = parseFloat(env.MAX_BUNDLE_MB ?? "64") * 1024 * 1024;
    if (len > maxBytes) return fail(413, `bundle exceeds ${Math.floor(maxBytes)} byte cap`);

    const globalCap = parseFloat(env.GLOBAL_CAP_GB ?? "20") * 1024 * 1024 * 1024;
    if ((await globalBytes(env.BUNDLES)) + len > globalCap) {
      return fail(507, "collection storage is full; uploads paused, retry tomorrow");
    }

    const dailyCap = parseFloat(env.DAILY_CAP_MB ?? "512") * 1024 * 1024;
    if ((await recentBytes(env.BUNDLES, sender)) + len > dailyCap) {
      return fail(429, "daily upload cap reached; the outbox can retry tomorrow");
    }

    // Hash-suffixed key: retries of the same content dedupe to one object,
    // a re-cut stint with the same stamp stores alongside instead of
    // overwriting.
    const key = `${sender}/${stem}-${claimed.slice(0, 16)}.tar.zst`;
    if (await env.BUNDLES.head(key)) {
      return json(200, { ok: true, stored: key, duplicate: true });
    }
    try {
      await env.BUNDLES.put(key, request.body, { sha256: claimed });
    } catch (e) {
      // R2 rejects a body whose hash doesn't match the claim.
      return fail(422, "body does not match X-Bundle-SHA256");
    }
    if (globalCache) globalCache.bytes += len;
    return json(200, { ok: true, stored: key, duplicate: false });
  },
};
