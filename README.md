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

## Data Collection

_tuners_ can optionally share your telemetry to improve the advice engine for
everyone. It is **off by default** and nothing leaves your machine until you
turn it on (Settings → telemetry sharing).

**Why share?** The advice engine learns which setup changes actually move
which behaviours - an "effect map" built from real A/B measurements. From a
single player's sessions it can only learn about the cars and changes that
player happened to try; pooled across many players, cars, surfaces, and
drivetrains it can tell a new user "other people who changed this saw that"
before they've tried anything themselves. More data directly means better,
earlier suggestions - including for you.

**What is sent** - one bundle per recorded stint, containing:

- the raw telemetry recording, exactly as the game emitted it (car physics
  channels only: speed, slip, suspension, tire temps, and so on - the Data Out
  packet contains no personal information),
- the car and tune-revision context needed to interpret it, with **all free
  text structurally stripped**: session facts are allowlisted, and journal
  notes are rebuilt from parsed setup deltas ("front arb -2"). Anything that
  doesn't match the machine grammar simply isn't exported - text is never
  redacted in place, it's absent by construction.

**What is never sent** - names, notes, comments, file paths, or anything you
typed as prose. There are no accounts: at opt-in the app generates a random
token locally, and the server sees only a hash of it as a pseudonymous sender
id. Nothing links a bundle to you as a person.

**When it's sent** - bundles queue in a local outbox and upload only while
telemetry is idle (no uploads compete with your driving), oldest first. A
bundle is removed from the queue only once the server confirms receipt. The
receiving side re-validates every bundle on arrival - recordings are fully
re-decoded and the free-text strip is verified - and quarantines anything that
doesn't check out.

**Staying in control** - turn sharing off at any time; disabling asks whether
to discard anything still queued. Previously recorded stints are only shared
if you explicitly choose "share existing recordings", which shows a
count-and-size preview first. You can inspect exactly what a bundle contains
with `tuners export`, which writes the same archive to a file instead of
uploading it.



## Docs

- [docs/design.md](docs/design.md) - objectives, constraints, architecture, design principles
- [docs/telemetry.md](docs/telemetry.md) - the packet, verified quirks, what's (not) available
- [docs/guide.md](docs/guide.md) - user-facing capture & interpretation guidance

## License

GPL-3.0-or-later - see [LICENSE](LICENSE).
