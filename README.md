# tuners

Desktop tuning assistant for Forza Horizon 6.

Reads telemetry from the game's built-in UDP output ("Data Out") to analyse how
the car actually behaves on track, and gives directional, evidence-cited tuning
advice. Setup changes are A/B-compared via spliced ideal laps that are robust to
driving mistakes, and a history-aware tuning journal means advice learns from
what you've already tried.

## Features

- Automatic recording while you drive: stints are cut per drive, per car, and
  per tune change, with no start/stop buttons.
- Live view with lap charts and a confidence gauge showing how trustworthy the
  data collected so far is.
- Tune A/B comparison built on spliced ideal laps, so a mistake or rewind
  doesn't poison the verdict, with gains and losses broken down into corner
  entry, corner exit, and straights.
- Directional tuning advice with the evidence cited: balance, damping, gearing,
  brake, differential, aero, and tire rules calibrated against real telemetry.
- A tuning journal that remembers every change and its measured outcome:
  advice builds on what you've already tried, flags changes that made the car
  slower, and homes in on the best value once a slider has been probed a few
  times.
- One-click apply: accept a suggestion and it becomes the next tune revision.
- Imperial, metric, and UK display units.
- The full analysis engine is also available as command line tools.

_tuners_ is in active development. Planned:

- Advice informed by the whole community's shared telemetry, so a car you've
  never tuned gets useful suggestions from the very first stint.
- Full starting-setup suggestions for a car, rather than one adjustment at a
  time.
- Detection of more handling problems (snap oversteer, brake dive, sluggish
  direction changes) as calibration data lands.

## Installation

Pre-built Windows and Linux packages can be downloaded from the
[latest release](https://github.com/whompratt/tuners/releases). Every
release is built from source by GitHub Actions, with SHA-256 checksums
in the release notes. Free code signing is provided by
[SignPath.io](https://signpath.io), certificate by the
[SignPath Foundation](https://signpath.org).

Alternatively, build from source. You'll need Rust (stable), Node, and pnpm,
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
first-time setup, port: anything outside 5200-5300 (default 20440), then fully
restart the game. (The game binds its own socket in the 5200-5300 range; see
the [official Data Out documentation](https://support.forza.net/hc/en-us/articles/51744149102611-Forza-Horizon-6-Data-Out-Documentation).)

If running _tuners_ under WSL NAT, use the WSL address from `hostname -I`;
localhost is not forwarded for UDP.

## Usage

The desktop app is the main way in: it records automatically while you drive
(race mode only; menus and free roam are skipped), shows live charts and a
confidence gauge, and journals every tune change for A/B comparison and advice.

**What to drive**: Rivals is the recommended loop. Conditions are identical
every run, restarts are free, and lap times come through in telemetry, which
is exactly what tune A/B comparison needs. Regular races, custom races, and
route events work too, including point-to-point sprints. Free roam and
open-world time attack aren't recorded: time attack sends no lap timing
(the game keeps the on-screen timer to itself), so its runs could never be
timed or compared. Weather and time of day also vary in the open world,
which would muddy verdicts even with times.

**The loop**: enter your car's current tune once as a baseline, drive a
handful of laps, read the verdict, apply the suggested change, drive again.
The confidence gauge shows how well your laps corroborate each other: more
clean laps, more trust in the comparison. Advice always cites the evidence
it's based on.

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
```

See [docs/guide.md](docs/guide.md) for capture practice (rewinds, lap counts,
A-B-A protocol).

## Data Collection

_tuners_ can optionally share your telemetry to improve the advice engine for
everyone. It is **off by default** and nothing leaves your machine until you
turn it on (Settings → telemetry sharing).

**Why share?** The advice engine learns from real setup changes and their
measured outcomes. The more telemetry it has seen across more cars, surfaces,
and drivers, the better its suggestions get, especially early in a
tuning session before you've tried much yourself. More data means better
advice for everyone, including you.

**What is sent**: recorded telemetry (car physics data only; the game's
Data Out packet contains no personal information) plus the car and tune
settings needed to interpret it. Anything you typed as free text is stripped
before export.

**What is never sent**: names, notes, comments, or file paths. There are no
accounts either: bundles carry only a random, pseudonymous sender id, so
nothing links them to you as a person.

**When it's sent**: uploads happen quietly in the background and never while
you're driving.

**Staying in control**: turn sharing off at any time. Older recordings are
only shared if you explicitly ask, and `tuners export` lets you inspect
exactly what a bundle contains before anything is uploaded.

**What comes back**: everyone's shared data is distilled into a small
anonymous summary of which adjustments helped on which kinds of builds, and
every install downloads it to inform advice about adjustments you haven't
tried yet. You receive it whether or not you share.

The full plain-language policy, including retention and how to have your
data deleted, is in [docs/privacy.md](docs/privacy.md).

## Docs

- [docs/design.md](docs/design.md): objectives, constraints, architecture, design principles
- [docs/telemetry.md](docs/telemetry.md): the packet, verified quirks, what's (not) available
- [docs/guide.md](docs/guide.md): user-facing capture & interpretation guidance

## License

GPL-3.0-or-later; see [LICENSE](LICENSE).
