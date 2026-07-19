# Design

## Objective

Help a Forza Horizon 6 player improve a car's tune without needing to be a suspension
engineer. The app observes how the car behaves via the game's telemetry stream and turns
that into concrete, explained suggestions: "front tire pressures are running 4 psi over
optimal temp range — drop cold pressure", "car understeers on corner entry at high speed —
soften front ARB or add front downforce", etc.

## What makes this viable

FH6 has an official **Data Out** feature: one-way UDP telemetry sent to a configurable
IP/port at the game's frame rate, including speed/RPM/gear, tire data, driver inputs,
G-forces, and lap timing. Details in [telemetry.md](telemetry.md). This works from any
platform (PC or console) since the game just needs a LAN IP to send to.

## What is fundamentally NOT possible (constraints)

These shape the whole design — the app is an *advisor*, not an *autotuner*:

- **Telemetry does not include the current tune settings.** We can see the car's
  behaviour but not the spring rates / pressures / gearing that produced it. Either the
  user enters their tune manually, or recommendations are expressed as *directional
  deltas* ("soften rear springs") rather than absolute values.
- **There is no API to write a tune back into the game.** The user applies every change
  by hand in the tuning menu. The feedback loop is: drive → analyse → suggest → user
  edits tune → drive again.
- **Telemetry is one-way and send-only.** We can't query the game for car metadata
  (weight, drivetrain, upgrades); anything beyond what's in the packet needs a static
  data source or user input. The packet's car ordinal/class fields may help identify
  the car (to be verified against the FH6 packet spec).
- **Send rate = frame rate**, so sample spacing is irregular. Analysis must use the
  packet's timestamp field and resample, not assume fixed dt.

## High-level architecture

Pipeline of small stages, each independently testable:

```
UDP listener → packet decoder → session recorder (raw + decoded)
                                      │
                                      ▼
                          analysis (lap/segment metrics)
                                      │
                                      ▼
                          recommendation engine → UI
```

- **Capture**: bind UDP socket, receive packets, no interpretation. Must never drop the
  raw bytes — record them so decoding bugs can be fixed retroactively.
- **Decode**: map the fixed FH6 packet layout to typed fields. Isolated so a layout
  correction touches one module.
- **Record**: sessions on disk (format TBD — likely raw packet log + derived parquet/CSV).
  Recorded sessions also serve as test fixtures, so analysis can be developed headlessly
  without the game running.
- **Analyse**: segment into laps/corners, compute tuning-relevant metrics (tire temp
  spread across a lap, slip ratios under braking/accel, understeer/oversteer balance,
  time spent at rev limiter per gear).
- **Recommend**: rules mapping metrics to tune adjustments, each with an explanation.
  Start with well-established heuristics (tire temps → pressures, gearing from RPM
  traces) before anything fancy.
- **UI**: undecided — see open questions.

## Non-goals (for now)

- Generating complete tunes from scratch for arbitrary cars.
- Live in-race coaching / overlay (analysis is post-drive to start with).
- Anything requiring memory reading or game modification — Data Out only.

## Open questions

- **UI form factor**: CLI report? TUI? Desktop app? Web dashboard? Affects crate choices;
  capture/analysis pipeline is UI-agnostic either way.
- **Tune input**: do we ask the user to type in their current tune (enables absolute
  recommendations) or stay delta-only?
- **Car metadata**: is there a usable community dataset of FH6 cars (weight, drivetrain,
  stock gearing), and is bundling it legally fine?
- **Exact FH6 packet layout**: needs verification — see [telemetry.md](telemetry.md).
