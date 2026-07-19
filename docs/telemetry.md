# Telemetry: FH6 "Data Out"

How we get data out of the game, what's in it, and what still needs verifying.
This file is the single source of truth for data-collection facts; when the packet
layout is confirmed against real captures, it gets recorded here.

## Confirmed

- FH6 has an official **Data Out** feature: **one-way UDP**, sent to a configurable
  remote IP and port. Nothing is ever received by the game.
- Enabled in-game under **Settings → HUD and Gameplay → Data Out**; sending starts as
  soon as the player drives. `127.0.0.1` works when the game and app run on the same PC;
  a LAN IP works for console → PC.
- Send rate equals the game's frame rate (i.e. variable — commonly 60+ Hz on PC).
- **Single fixed packet format.** Unlike Forza Motorsport, there is no format selector
  in the settings.
- Packet contents (per official docs and community tools): vehicle dynamics, tire data
  (temperatures, slip), race/lap status, driver inputs (throttle, brake, clutch,
  handbrake, steering), G-forces, orientation.

## To verify (blocking for the decoder)

- **Exact byte layout**: field names, types, offsets, total packet size. The official
  spec is at the Forza Support article below (fetch it in a browser — it 403s
  automated fetches). Cross-check against real captured packets, and against
  open-source decoders (e.g. `TheBanHammer/fh6-tel`, in Rust) if licensing permits.
- Whether **suspension travel / ride height** fields are present (they were in the
  FH5-era format; important for spring/damper recommendations).
- Whether **car ordinal / class / PI** fields identify the car reliably.
- Default port (community setups commonly use `20440`, but the port is user-chosen).
- Behaviour in menus/photo mode (FH5 kept sending zeroed/frozen packets — a
  "race is on" flag or equivalent is needed to filter these).

## Practical notes

- Record **raw packets + receive timestamps** to disk before any decoding. Raw logs are
  replayable test fixtures and survive decoder bugs.
- Irregular sample spacing (frame-rate-tied) → analysis resamples using the packet's
  own timestamp field, not receive time.
- One socket, no auth, LAN-only: no privacy/security surface beyond the local network.

## Sources

- [Official FH6 Data Out documentation (Forza Support)](https://support.forza.net/hc/en-us/articles/51744149102611-Forza-Horizon-6-Data-Out-Documentation)
- [fh6-tel — Rust/Tauri FH6 telemetry dashboard](https://github.com/TheBanHammer/fh6-tel)
- [FH6 ESP32 telemetry dashboard (another independent decoder)](https://github.com/ToTo-40417/Forza-Horizon-6_Telemetry-Live-Dashboard)
- [MOZA FH6 telemetry setup guide](https://support.mozaracing.com/en/support/solutions/articles/70000683812-forza-horizon-6-telemetry-settings-control-mapping-setup-guide)
