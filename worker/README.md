# tuners-collect — the deployed collection endpoint (plan 009)

A Cloudflare Worker in front of a private R2 bucket. It is deliberately dumb:
token check, name check, caps, then the body streams straight into R2 with
R2 verifying the claimed SHA-256. All real validation happens later, in
`tuners ingest`, after the bucket is synced down. The protocol and status
codes are pinned by `tests/receive.rs` in the main repo (the `tuners receive`
subcommand is the reference implementation and local test harness — the same
curl commands work against both).

## Protocol

```
PUT https://<worker-url>/v1/bundle/<name>.tar.zst
  Authorization: Bearer <token>
  X-Bundle-SHA256: <64 hex of the body>
-> 200 {"ok":true,"stored":"<sender>/<name>-<hash16>.tar.zst","duplicate":false}
   401 unknown/missing token          413 over MAX_BUNDLE_MB
   400 bad name / bad or missing hash 429 over DAILY_CAP_MB (rolling 24h)
   422 body hash != claimed           GET /healthz -> 200
```

Retries are idempotent: the stored key carries the first 16 hash chars, so a
replay of the same content answers `"duplicate":true` and rewrites nothing,
while different content under the same stint stamp stores alongside.

## One-time setup (your Cloudflare account)

```sh
cd worker
npm install
npx wrangler login                              # opens the browser, free plan is fine
npx wrangler r2 bucket create tuners-bundles
```

Mint tokens (one per sender) and store the map as the TOKENS secret:

```sh
openssl rand -hex 32                            # repeat per sender
npx wrangler secret put TOKENS
# paste one line, e.g.: {"<hex-a>":"jake","<hex-b>":"friend-1"}
```

Deploy:

```sh
npx wrangler deploy                             # prints https://tuners-collect.<you>.workers.dev
```

Smoke test against the real deployment:

```sh
printf 'hello' > /tmp/b.bin
curl -i -X PUT --data-binary @/tmp/b.bin \
  -H "Authorization: Bearer <hex-a>" \
  -H "X-Bundle-SHA256: $(sha256sum /tmp/b.bin | cut -d' ' -f1)" \
  https://tuners-collect.<you>.workers.dev/v1/bundle/smoke.tar.zst
# then delete it: npx wrangler r2 object delete tuners-bundles/jake/smoke-<hash16>.tar.zst
```

## Operations

- **Revoke a sender**: edit the map, `npx wrangler secret put TOKENS` again
  (takes effect in seconds; no redeploy of code needed).
- **Delete a sender's data**: their prefix in the bucket —
  `npx wrangler r2 object delete` per object, or the dashboard's bucket view.
- **Pull data down for ingest** (zero egress cost): create an R2 API token in
  the dashboard (R2 → Manage API Tokens → read-only is enough), then
  `rclone sync r2:tuners-bundles ./inbox` with an rclone remote of type s3 /
  provider Cloudflare, endpoint `https://<account-id>.r2.cloudflarestorage.com`.
  `tuners ingest ./inbox` (plan phase 3) runs on the synced copy.
- **Caps**: `MAX_BUNDLE_MB` / `DAILY_CAP_MB` vars in wrangler.jsonc; the daily
  cap is derived from object listings (no state), same as the Rust receiver.
- **Privacy**: observability is off in wrangler.jsonc — no per-request logs
  retained, matching the plan's "nothing about the sender beyond the token".

## Local development (no account needed)

```sh
npx wrangler dev        # emulated R2, TOKENS from .dev.vars (gitignored)
```

`.dev.vars` holds a throwaway token map for dev only; the round-trip curl
suite from tests/receive.rs was run against the emulator (all status codes,
dedupe, hash rejection, both caps) on 2026-07-25.
