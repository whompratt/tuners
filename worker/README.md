# tuners-collect: the deployed collection endpoint

A Cloudflare Worker in front of a private R2 bucket. It is deliberately dumb:
rate limit, token shape, name check, caps, then the body streams straight
into R2 with R2 verifying the claimed SHA-256. All real validation happens
later, in `tuners ingest`, after the bucket is synced down. The protocol and
status codes are pinned by `tests/receive.rs` in the main repo (`tuners
receive` is the reference twin and local test harness; the same curl
commands work against both).

## Protocol

```
PUT https://<worker-url>/v1/bundle/<name>.tar.zst
  Authorization: Bearer <token>
  X-Bundle-SHA256: <64 hex of the body>
-> 200 {"ok":true,"stored":"<sender>/<name>-<hash16>.tar.zst","duplicate":false}
   401 malformed/unknown token        413 over MAX_BUNDLE_MB
   400 bad name / bad or missing hash 422 body hash != claimed
   403 sender blocklisted             429 rate limited, or over DAILY_CAP_MB
   507 global storage ceiling reached GET /healthz -> 200
```

**Auth is open**: no tokens are issued. The app generates a random 64-hex
token once at opt-in and its sender id is `sha256(token)[..16]`: a stable
pseudonymous identity (needed for per-driver grouping, caps, and
delete-on-request) without any registration step. Setting the `TOKENS`
secret flips the endpoint to allowlist-only lockdown mode, the abuse
circuit breaker.

Retries are idempotent: the stored key carries the first 16 content-hash
chars, so a replay of the same content answers `"duplicate":true` and
rewrites nothing, while different content under the same stint stamp stores
alongside.

## Cost protection

Cloudflare's model can't produce a bandwidth bill: egress from R2 is free,
DDoS absorption is free and unmetered, and on the Workers **free** plan the
100k requests/day cap **fails closed** (requests are refused, never billed).
The only billable product here is R2, bounded in layers:

- `GLOBAL_CAP_GB` (20): hard ceiling on total bucket size, enforced per PUT
  from a cached listing → worst case is storage *pinned* ~10 GB over the free
  tier ≈ **$0.15/month**, never growing.
- Per-IP throttle, 30 requests/min: enforced by an in-isolate sliding window
  in index.js. The platform `IP_LIMIT` ratelimit binding is also consulted
  but was measured NON-ENFORCING on this account (2026-07-25/26: fresh
  namespace, 8x sustained load, `success:true` throughout; emulator
  enforces, production doesn't); it stays wired in case it starts counting.
  The in-isolate window bounds a single noisy client per isolate. The hard
  cost bounds are the storage ceilings and the fail-closed request cap, not
  this throttle.
- Per-sender rolling-24h byte cap (`DAILY_CAP_MB`) and 64 MB max bundle.
- Rejected requests cost zero R2 operations (checks run cheap-to-expensive).
- Absolute worst case (sustained distributed attack saturating the free
  plan's 100k req/day for a month, all passing the rate limit): R2 operations
  land in the low tens of dollars, and the response is one command:
  `npx wrangler secret put TOKENS` to flip to lockdown mode.

Account-side backstops (dashboard, one-time):
1. **Stay on the Workers free plan**: do not subscribe to Workers Paid;
   the daily cap is the request circuit breaker.
2. **Billing notification**: Notifications → Add → *Usage Based Billing* →
   product R2, low threshold, email. Fires before a bill grows.
3. R2 was the product that required a payment card; the two caps above are
   what bound that card's exposure.

## One-time setup (your Cloudflare account)

```sh
cd worker
npm install
npx wrangler login                              # opens the browser, free plan is fine
npx wrangler r2 bucket create tuners-bundles
npx wrangler deploy                             # prints https://tuners-collect.<you>.workers.dev
```

No token step; senders mint their own. Smoke test against the deployment:

```sh
printf 'hello' > /tmp/b.bin
curl -i -X PUT --data-binary @/tmp/b.bin \
  -H "Authorization: Bearer $(openssl rand -hex 32)" \
  -H "X-Bundle-SHA256: $(sha256sum /tmp/b.bin | cut -d' ' -f1)" \
  https://tuners-collect.<you>.workers.dev/v1/bundle/smoke.tar.zst
# then delete it: npx wrangler r2 object delete tuners-bundles/<sender>/smoke-<hash16>.tar.zst
```

## Operations

- **Block a sender**: `npx wrangler secret put BLOCKLIST` with a JSON array
  of sender ids, e.g. `["a3f9c2e811d04b57"]` (seconds, no redeploy).
- **Emergency lockdown**: `npx wrangler secret put TOKENS` with
  `{"<64-hex>":"<sender-id>"}`; only issued tokens upload until the secret
  is deleted (`npx wrangler secret delete TOKENS`).
- **Delete a sender's data**: their prefix in the bucket, via
  `npx wrangler r2 object delete` per object, or the dashboard's bucket view.
  Users quote the sender id shown in their dashboard (delete-on-request).
- **Pull data down for ingest** (zero egress cost): create a read-only R2 API
  token (dashboard → R2 → Manage API Tokens), then
  `rclone sync r2:tuners-bundles ./inbox` with an rclone remote of type s3 /
  provider Cloudflare, endpoint `https://<account-id>.r2.cloudflarestorage.com`.
  `tuners ingest ./inbox` (plan phase 3) runs on the synced copy.
- **Privacy**: observability is off in wrangler.jsonc, so no per-request logs
  retained, matching the plan's "nothing about the sender beyond the token".

## Local development (no account needed)

```sh
npx wrangler dev        # emulated R2; BLOCKLIST dev fixture in .dev.vars
```

The round-trip curl suite from tests/receive.rs was last run green against
the emulator 2026-07-25: open-mode auth (sender-id derivation cross-checked
with the Rust twin), dedupe, hash rejection, blocklist 403, rate limit 429,
per-sender 429, global 507, and lockdown mode via `--var TOKENS:...`.
Live smoke test against the deployed endpoint 2026-07-25/26: all of the
above verified except the platform ratelimit binding (see Cost protection);
uploads round-tripped byte-identical through real R2.
