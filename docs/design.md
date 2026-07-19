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
- **Telemetry is one-way and send-only.** The packet does identify the car
  (`CarOrdinal`, class, PI) and its drivetrain type, but **weight, weight
  distribution, fitted upgrades, tire compound, and suspension type are not exposed**
  — those need user input or a community car dataset.
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
- **UI**: local web dashboard. The Rust binary serves a browser UI over
  HTTP/WebSocket on localhost; charts render in the browser. Chosen over a TUI
  (charts/graphs are central to presenting telemetry) and over a desktop shell
  (a Tauri wrap can be added later without changing the architecture). UI design is
  Claude-led with user feedback; capture/analysis stay UI-agnostic.

## Non-goals (for now)

- Generating complete tunes from scratch for arbitrary cars.
- Live in-race coaching / overlay (analysis is post-drive to start with).
- Anything requiring memory reading or game modification — Data Out only.

## Primary workflow: rivals iteration

The core use case is a **rivals session**: restartable with guaranteed-identical
conditions, so it is the natural tune-test loop — drive laps, restart, adjust the
tune, drive again, compare. Analysis must respect two rivals facts: lap 1 is always
a **standing start** (out lap, much slower on circuits — excluded from lap
comparisons), and tracks may be **lap-based or point-to-point** (point-to-point has
no flying laps at all, so every run is a standing start and runs compare only to
other runs of the same route). Cross-session comparison — same car/track, tune A vs
tune B — is where recommendations get their evidence.

**Leaderboard validity is irrelevant — the kept drive is the data.** Rewinds
invalidate laps for the leaderboard, but a rewind-and-retry is the driver choosing
better data: the game restores exact car state, so the kept lap is physically
consistent and its time is real. Analysis reconstructs this kept timeline (erasing
superseded driving) rather than discarding rewound laps.

**Balance is the real coordinate system.** Setup changes act on lap time *through*
behaviour (setting → balance → pace), and the first real trajectory showed the
measured balance tracks the *driver's* preferred operating point more than the
hardware — drivers adapt inputs to the feel they like. Long-term direction
([plan 007](plans/007-learning.md)): per-driver balance priors and per-setting
effect maps learned across cars, used as informed starting points and validated
per car. Preferred balance differs by driver and surface; there is no universal
"neutral is best".

**The driver-learning confounder.** Across an iterative tuning session the driver
learns the car and track and gets faster regardless of the tune, so raw
best-lap-vs-best-lap deltas systematically flatter whichever tune was driven later.
The spliced ideal partially controls for this (it extracts the best *executed*
version of each section, discounting one-off mistakes and inconsistency), but it
cannot remove it: learning also raises the ceiling the ideal converges to. When a
verdict matters, the honest protocol is **A-B-A**: re-run the original tune after
the change; if A' ≈ A the delta was the tune, if A' ≈ B it was the driver.
Recommendations built on A/B evidence should state this caveat when sessions were
driven back-to-back on a fresh track/car combo.

## Tune input model

The app iterates on the *user's existing tune* rather than generating one from scratch
(primary use case: "the tune is 90% there but something is wrong I can't pin down").
There are not two hard modes; there is **one incremental input model** where every
field is optional and recommendation quality degrades gracefully:

- **Free from telemetry** (never ask): car identity, class/PI, drivetrain type,
  redline, observed peak power/torque, gear count and effective ratios, EV heuristic
  via cylinder count. Pre-fill these; let the user correct.
- **High-value manual inputs**: tuning goal, weight, front weight %, tire compound,
  suspension type, current tune values, slider limits (springs, ride height, aero),
  assists in use (ABS/TCS/stability — invisible in telemetry but they reshape what
  slip observations mean; e.g. with ABS, sustained braking slip is normal, not lockup).
- **Nice-to-have**: engine position, body type.
- **Manual IMO tire temps (future, for camber)**: telemetry only carries one temp
  per tire, but the in-game HUD shows inner/middle/outer — the signal camber
  advice needs, so the UI should accept them as manual input. Big caveat when
  interpreting: **corner-direction asymmetry**. On a tight track with mostly
  right-handers, the mostly-loaded side (front-left) develops a meaningful IMO
  spread while the rarely-loaded side (front-right) reads "strange" — near-ideal
  camber on the loaded side can coexist with odd numbers on the other, and
  symmetric-looking targets would misadvise. IMO interpretation must weight by
  which side actually got worked (turn-direction balance is derivable from
  telemetry lateral-G history).

With no manual input at all ("blind" = the empty form), the app still produces
**directional deltas** with explanations ("drop front pressure ~1 psi"; "soften front
ARB"), phrased to survive unknown limits ("if already at minimum, do X instead").
As inputs are added, recommendations upgrade to **absolute targets** and gain
limit-awareness (detecting a maxed slider and redirecting to the next-best lever).
Every analysis records which inputs it had, so a session can be re-analysed after
the user fills in more.

## Open questions

- **Car metadata**: car *identity* is solved — a community, telemetry-verified
  `CarOrdinal` → name dataset is bundled (`assets/car-ordinals.tsv`, source and
  date in its header; factual ID mapping, attribution kept). Still open: specs
  (weight, weight distribution, stock gearing) keyed by ordinal.
- Packet-layout residuals (trailing byte, byte order, tire temp units) — tracked in
  [telemetry.md](telemetry.md).
