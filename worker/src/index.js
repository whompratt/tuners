// Telemetry bundle collector (plan 009): the deployed twin of `tuners
// receive`. A dumb authenticated PUT into R2 — never parses telemetry;
// validation/quarantine happen in `tuners ingest` after the bucket is synced
// down. The protocol (and every status code) is pinned by tests/receive.rs
// in the main repo; keep the two implementations in lockstep.
//
//   PUT /v1/bundle/<name>.tar.zst
//     Authorization: Bearer <token>
//     X-Bundle-SHA256: <64 hex of the body>
//   -> 200 {"ok":true,"stored":"<sender>/<name>-<hash16>.tar.zst","duplicate":bool}
//
// R2 itself verifies the claimed hash (put option `sha256`) — the Worker
// never buffers or hashes the body, so a corrupted upload is rejected by
// storage, not by code that could drift.
//
// Bindings: BUNDLES (R2), TOKENS (secret: JSON {"<token>":"<sender-id>"}),
// MAX_BUNDLE_MB / DAILY_CAP_MB (vars).

const NAME_RE = /^[A-Za-z0-9_-][A-Za-z0-9._-]{0,99}\.tar\.zst$/;

function json(status, obj) {
  return new Response(JSON.stringify(obj), {
    status,
    headers: { "content-type": "application/json" },
  });
}

function fail(status, error) {
  return json(status, { ok: false, error });
}

/// Bytes this sender stored in the last 24h, from object metadata — same
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

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    if (request.method === "GET" && url.pathname === "/healthz") {
      return json(200, { ok: true });
    }
    if (request.method !== "PUT") {
      return fail(405, "PUT bundles to /v1/bundle/<name>.tar.zst");
    }
    if (!url.pathname.startsWith("/v1/bundle/")) {
      return fail(404, "unknown path");
    }

    // Everything is checked before the body is consumed: a rejected upload
    // costs the sender headers, not megabytes.
    const auth = request.headers.get("authorization") ?? "";
    const token = auth.match(/^Bearer\s+(\S+)$/i)?.[1];
    if (!token) return fail(401, "missing bearer token");
    const sender = JSON.parse(env.TOKENS ?? "{}")[token];
    if (!sender) return fail(401, "unknown token");

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

    const dailyCap = parseFloat(env.DAILY_CAP_MB ?? "512") * 1024 * 1024;
    if ((await recentBytes(env.BUNDLES, sender)) + len > dailyCap) {
      return fail(429, "daily upload cap reached — the outbox can retry tomorrow");
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
    return json(200, { ok: true, stored: key, duplicate: false });
  },
};
