# tuners

Desktop tuning assistant for Forza Horizon 6.

Reads telemetry from the game's built-in UDP output ("Data Out") to analyse how
the car actually behaves on track, and gives directional, evidence-cited tuning
advice. Setup changes are A/B-compared via spliced ideal laps that are robust to
driving mistakes, and a history-aware tuning journal means advice learns from
what you've already tried.

## Installation

Pre-built Windows and Linux packages can be downloaded from the
[latest release](https://github.com/whompratt/tuners/releases).

Alternatively, build from source. You'll need Rust (stable), Node, and pnpm -
plus the [Tauri prerequisites](https://tauri.app/start/prerequisites/) on Linux
(WebKitGTK etc.):

```
git clone https://github.com/whompratt/tuners.git
cd tuners/app
pnpm install
pnpm tauri build    # or `pnpm tauri dev` to run unpackaged
```

## Game setup

However you installed it, the app needs Forza Horizon 6 to send it telemetry:
Settings → HUD and Gameplay → Data Out: On, IP: as shown in the app's
first-time setup, port: anything outside 5200-5300 (default 20440) - then fully
restart the game.

If running _tuners_ under WSL NAT, use the WSL address from `hostname -I` -
localhost is not forwarded for UDP.

## Usage

The desktop app is the main way in: it records automatically while you drive
(race mode only - menus and free roam are skipped), shows live charts and a
confidence gauge, and journals every tune change for A/B comparison and advice.

The same engine is also available as command line tools:

```
tuners capture    record a stint from Data Out, with live status
tuners analyze    per-stint observations: tires, grip, suspension, damping, gearing, laps
tuners compare    tune A/B: lap-time delta, where it comes from, mistakes excluded
tuners recommend  directional tune advice with evidence (blind mode, no tune input)
tuners advise     history-aware advice from a tuning journal
tuners replay     integrity-check a recorded stint
tuners simulate   synthetic telemetry (stand-in for the game, for development)
tuners export     bundle a stint for manual sharing or upload
tuners ingest     validate received telemetry bundles and file them per sender
tuners receive    local telemetry-collection endpoint
```

See [docs/guide.md](docs/guide.md) for capture practice (rewinds, lap counts,
A-B-A protocol).

## Docs

- [docs/design.md](docs/design.md) - objectives, constraints, architecture, design principles
- [docs/telemetry.md](docs/telemetry.md) - the packet, verified quirks, what's (not) available
- [docs/guide.md](docs/guide.md) - user-facing capture & interpretation guidance

## License

GPL-3.0-or-later - see [LICENSE](LICENSE).
