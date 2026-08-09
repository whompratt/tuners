# Telemetry: FH6 "Data Out"

How we get data out of the game, what's in it, and what still needs verifying.
This file is the single source of truth for data-collection facts. The packet layout
below is transcribed from the official Forza Support documentation (link at bottom).

## Transport

- **One-way UDP** to a configurable IP/port, sent at the game's frame rate (variable:
  resample using `TimestampMS`, not receive time). Nothing is ever received by the game.
- Enabled under **Settings → HUD and Gameplay → Data Out** (toggle + IP + port).
  `127.0.0.1` works same-PC; a LAN IP works for console → PC.
- **Avoid listening on ports 5200–5300**: the game binds its outgoing socket in that
  range.
- Data is sent **only while actively driving**: nothing during menus, pauses, replays,
  rewinds, or after finishing a race. `IsRaceOn` additionally distinguishes race-on
  from race-stopped packets.
- Single fixed packet format; no format selector (unlike Forza Motorsport).

## Packet layout

Total packet size: **324 bytes**, little-endian (byte order assumed from prior Forza
titles; verify). Types: `S`/`U`/`F` = signed int / unsigned int / float, number = bits.
Wheel quads are ordered FL, FR, RL, RR.

| Offset | Type | Field | Notes |
|---|---|---|---|
| 0 | S32 | IsRaceOn | 1 = race on, 0 = menus/race stopped |
| 4 | U32 | TimestampMS | can overflow to 0 |
| 8 | F32 | EngineMaxRpm | |
| 12 | F32 | EngineIdleRpm | |
| 16 | F32 | CurrentEngineRpm | |
| 20 | F32 ×3 | AccelerationX/Y/Z | car local space; X=right, Y=up, Z=forward |
| 32 | F32 ×3 | VelocityX/Y/Z | car local space |
| 44 | F32 ×3 | AngularVelocityX/Y/Z | rad/s; X=pitch, Y=yaw, Z=roll |
| 56 | F32 ×3 | Yaw, Pitch, Roll | radians |
| 68 | F32 ×4 | NormalizedSuspensionTravel | 0.0 = max stretch, 1.0 = max compression |
| 84 | F32 ×4 | TireSlipRatio | normalized; 0 = full grip, \|x\| > 1 = grip loss |
| 100 | F32 ×4 | WheelRotationSpeed | rad/s |
| 116 | S32 ×4 | WheelOnRumbleStrip | 0/1 — MEASURED DEAD in FH6: zero across all 69 library recordings (2026-07-29), including kerb-heavy tarmac circuits; do not build detection on it |
| 132 | S32 ×4 | WheelInPuddle | 0/1 |
| 148 | F32 ×4 | SurfaceRumble | non-dimensional, feeds FFB |
| 164 | F32 ×4 | TireSlipAngle | normalized; 0 = full grip, \|x\| > 1 = grip loss |
| 180 | F32 ×4 | TireCombinedSlip | normalized; 0 = full grip, \|x\| > 1 = grip loss |
| 196 | F32 ×4 | SuspensionTravelMeters | actual travel |
| 212 | S32 | CarOrdinal | unique make/model ID |
| 216 | S32 | CarClass | 0 (D) – 7 (X) |
| 220 | S32 | CarPerformanceIndex | 100–999 |
| 224 | S32 | DrivetrainType | 0 = FWD, 1 = RWD, 2 = AWD |
| 228 | S32 | NumCylinders | |
| 232 | U32 | CarGroup | FH6-only field |
| 236 | F32 | SmashableVelDiff | FH6-only; m/s lost to smashable collision |
| 240 | F32 | SmashableMass | FH6-only; kg of hit smashable |
| 244 | F32 ×3 | PositionX/Y/Z | world space, meters |
| 256 | F32 | Speed | m/s |
| 260 | F32 | Power | watts |
| 264 | F32 | Torque | newton-meters |
| 268 | F32 ×4 | TireTemp | units unspecified in doc (°F in prior titles; verify) |
| 284 | F32 | Boost | PSI above atmospheric |
| 288 | F32 | Fuel | 0.0–1.0 |
| 292 | F32 | DistanceTraveled | meters |
| 296 | F32 | BestLap, LastLap, CurrentLap | seconds; 0.0 if N/A (three fields, 296/300/304) |
| 308 | F32 | CurrentRaceTime | seconds since driving started |
| 312 | U16 | LapNumber | |
| 314 | U8 | RacePosition | |
| 315 | U8 ×4 | Accel, Brake, Clutch, HandBrake | 0–255 |
| 319 | U8 | Gear | |
| 320 | S8 | Steer | -127 left … 127 right |
| 321 | S8 | NormalizedDrivingLine | -127…127 |
| 322 | S8 | NormalizedAIBrakeDifference | -127…127 |

FH6 vs Forza Motorsport: adds `CarGroup`, `SmashableVelDiff`, `SmashableMass` (after
`NumCylinders`); lacks `TireWear` and `TrackOrdinal`.

## Verified against a real capture (2026-07-19, `fixtures/real-01.ftel`)

- Every datagram is exactly **324 bytes**; the undocumented trailing byte (offset 323)
  is always **0**; treat as padding.
- **Little-endian** confirmed; **tire temps are °F** (~200 peak under load).
- **The official "data is only sent while driving" note is wrong in practice**: the
  game keeps streaming packets in menus/pauses with `IsRaceOn = 0` and everything
  except `TimestampMS` zeroed. Filter on `IsRaceOn`, not on packet presence.
- **`Gear` = 0 is reverse, and 11 appears transiently mid-drive** (a handful of
  frames, likely a neutral/mid-shift sentinel). Analysis must not treat 11 as a
  real gear.
- Send rate matches frame rate as documented (~168 Hz observed on a 165 Hz setup).
- Setup gotcha: the game only starts honouring a newly configured Data Out target
  after a **full game restart**.
- **`DistanceTraveled` is always 0.0 in free roam** but live and monotonic in race
  modes (rivals: ~5950 m/lap observed). Integrating `Speed` over `TimestampMS` works
  everywhere.
- **`DistanceTraveled` is route-spline progress, not an odometer**: it can snap
  forward 10-21 m in a single frame with zero world-position change (observed on a
  dirt route, at the same route sections every lap, likely where the driven line
  diverges from the route spline). Snap points are lap-consistent, so distance
  binning stays aligned, but bins crossed by a snap get no frames and must be
  back-filled (profile binning spreads the hop across the crossed bins).
- **`DistanceTraveled` units are NOT real meters**: on the McLaren rivals circuit
  it advances ~2.4x faster than `Speed` integrated over time (5.86 "km" of spline
  covered in 44.8 s at 45-75 m/s true speed). The scale varies by track, so no
  absolute time = distance/speed check is valid on binned data; cross-lap
  comparison at the same bin is the only sound consistency test.
- **Spline snaps can be lap-specific and large**: individual laps have shown
  hundreds of "meters" snapped in one frame (one real lap hid 2.5 s of its time
  this way — its distance-binned time sum read 42.9 s against an authoritative
  45.4 s lap time). Any consumer summing binned time must treat bins whose time
  is far below the cross-lap median for that bin as data holes (profile splicing
  charges such bins the median time and bars them from corroborating).
- **Lap semantics (verified in a rivals session)**: `LapNumber` is 0-based and lap 0
  is the standing-start out lap (~6.5s slower than flying laps in the observed
  session; never compare it to them). `CurrentLap` resets to 0 at each boundary;
  `LastLap`/`BestLap` update exactly at the boundary, so a finished lap's
  authoritative time is read from the first frames of the *next* lap.
- **Route kind is detectable from the clocks (measured 2026-08-03, library-wide)**:
  both clocks tick together from the countdown's GO, and `DistanceTraveled` starts
  negative (the grid sits behind the start line, distance crosses 0 at the line).
  On a **circuit**, `CurrentLap` **resets to ~0 at the start-line crossing** while
  `CurrentRaceTime` keeps running, so for the rest of lap 0 the race clock leads
  the lap clock by the rollout time (measured 1.86-5.74s over 66 circuit race
  starts). On a **point-to-point route the lap clock never resets**: the two clocks
  stay locked for the whole run (|offset| < 0.01s over 20+ runs, three drivers),
  and the run's official time therefore includes the launch rollout. The
  race-minus-lap offset at the end of lap 0 separates the kinds with total margin,
  but the production detector is the RESET EVENT itself (point-to-point assumed
  until a reset is seen): the lap clock stepping down to < 0.5s while the race
  clock advances and distance does not retreat. The guards matter: a rewind steps
  the race clock (and distance) back together with the lap clock, so even a rewind
  to the GO moment cannot fake a line crossing, and restart-menu transitions can
  leak frames mixing the OLD race clock with a NEW near-zero lap clock while
  distance teleports backwards (measured: race 15.07 / lap 0.45 / dist 408→196) —
  excluded by the distance guard. The event also makes the kind knowable live,
  seconds into lap 0.

## Rewinds, restarts, collisions (verified 2026-07-19, deliberate-rewind capture)

- **A rewind appears as a race-off gap** (zeroed `IsRaceOn=0` packets, like a menu),
  and on resume the state is rewound: race clock, lap clock, distance, and
  `LapNumber` all step back. Rewinding over the finish line replays the lap
  transition identically (same race_t, same distance).
- Gap classification by race clock on resume: **unchanged = pause, stepped back =
  rewind, near zero = restart, jumped forward = the clock ran through the block**.
  The post-race results screen is race-off but **keeps the race clock running**
  (measured 2026-07-31, CRX rivals event: a 275.8s wall-clock block resumed with
  the race clock +275.01s), so a forward jump means the frames on either side are
  not continuous driving; real pauses resume within hundredths.
- Pre-race menus can **flicker race-on frames carrying the previous event's race
  clock** (stationary, `DistanceTraveled` 0): a race-on run in which the car never
  moves is menu noise, not driving, and must not anchor gap classification (the
  flicker's clock dropping to the new race's zero otherwise fakes a restart).
- **The finish line goes race-off BEFORE any frame carries the run time**
  (measured 2026-08-05, Celica point-to-point sprints): the last race-on driving
  frame sits meters short of the line with `LastLap` still 0, and the official
  time arrives only in brief race-on flicker from the results screen, seconds to
  tens of seconds later: `LapNumber` incremented, `LastLap` = the official run
  time (within ~0.5s of the last seen lap clock), race clock run forward
  (+2.97..+11.36s measured), position channels garbage (`DistanceTraveled`
  teleports, e.g. 5952 → 196). This "finish certificate" can be a SINGLE frame,
  and when the recorder's idle cut lands between the line and the results
  flicker it opens the NEXT recording instead. Analysis adopts the certificate's
  time for the run it completes (in-file and across the cut); the race clock
  must have advanced across the gap, since a pause taken exactly at a lap line
  also resumes on lap+1 with a matching `LastLap` but within hundredths.
  Without adoption, a run's time survives only when the driver restarts fast
  enough (< 5s of results screen) that the flicker stitches into the segment.
- **The game can flip race-off for a few seconds JUST AFTER GO with the car
  state frozen** (measured 2026-08-06, point-to-point rivals, two of four
  runs on one session: race-off at race_t 3.6-3.8s for 2.7-7.3s of wall
  time, clock/distance/speed identical on resume, +0.01s). Likely a
  loading hitch; also present in two testers' shared recordings. The
  resume clock sits under the 5s restart threshold, so classification
  must test clock+distance continuity BEFORE the near-zero-clock restart
  rule, or the launch is severed and the standing run (not captured from
  its start) becomes unprofilable.
- **`CurrentRaceTime` is the canonical time axis**: it runs in free roam too, freezes
  during pauses, and steps back coherently at rewinds. All durations, distance
  integration, and profile bin times use it (not `TimestampMS`, which keeps running
  through pauses and rewinds).
- **Analysis reconstructs the kept timeline**: frames superseded by a rewind (race
  clock ≥ the resume point) are erased and the retry splices on. A rewind restores
  exact car state, so the result is one continuous, physically consistent lap: the
  game itself performs equal-state splicing. Rewound laps are therefore kept as
  real laps (leaderboard validity is irrelevant to tune evaluation).
- Race-start artifacts: `DistanceTraveled` goes briefly **negative** (spawn is ~27 m
  behind the start line) and `CurrentLap` resets when the line is actually crossed
  (~3 s after launch).
- **Smashable collisions** (breakables: trees, cones) are directly telemetered via
  `SmashableVelDiff`/`SmashableMass`, but observed values are tiny (0.0–0.2 m/s,
  mass often 0), a weak, informational signal.
- **Wall/barrier hits are not telemetered** (they invalidate laps in-game). Not
  currently inferred; candidate heuristic is single-frame accel spikes beyond
  braking capability. Splicing self-protects on time (collisions cost speed), but a
  route where wall-bouncing is net-faster would make a spliced ideal
  leaderboard-illegal; revisit if observed in practice.

## Open-world time attack (measured 2026-08-09, Ram TRX capture)

Time attack runs inside free roam: drive over the start line and the event
begins; there is no lobby and no results screen, and the event ends when the
driver leaves the route or misses a checkpoint. Telemetry-wise it is a third
regime between free roam and race modes:

- `IsRaceOn` is 1 while driving in free roam anyway, so it distinguishes
  nothing here.
- Crossing the start line **activates `DistanceTraveled`** (route-spline
  progress; always 0.0 in plain free roam): the only packet-level marker that
  an event is running. Leaving the event zeroes it, and crossing into another
  route reassigns it (observed teleporting 24182 -> 366 in one frame pair).
- **No lap telemetry at all**: `CurrentLap` stays exactly 0.00, `LapNumber`
  stays 0, and `LastLap`/`BestLap` never update for the entire event
  (verified over all 53,547 frames of the capture). The on-screen event timer
  is not exported, and no finish certificate ever arrives.
- Consequence: time-attack driving **passes the naive race-mode gate**
  (`IsRaceOn && DistanceTraveled != 0`; 44,111 of 53,547 frames on the
  capture), yet its stints could never have laps, times, or verdicts. The
  recorder therefore excludes it via the frozen lap channels (race clock past
  any measured rollout with `CurrentLap`, `LapNumber`, `LastLap`, and
  `BestLap` all still exactly 0; circuits and point-to-point routes tick
  `CurrentLap` from GO, and finish certificates carry nonzero lap fields).
  Two pre-exclusion library recordings contain time-attack driving
  (2026-07-25 20:04 entirely; 2026-08-01 23:03 has a ~3-minute time-attack
  head before its races, which gap classification already separates into its
  own lap-less segment).
- Weather and time of day are live in free roam and have no packet channel,
  so even self-derived timing (e.g. from distance-bin crossings) would be
  condition-confounded across runs.

## Still to verify

- Whether `NumCylinders == 0` reliably identifies EVs (needs a capture in an EV).

## What the packet gives us for free (no user input needed)

- Car identity (`CarOrdinal`), class/PI, **drivetrain type**, cylinder count.
- Redline (`EngineMaxRpm`), observed peak power/torque (watts/Nm, live).
- **`EngineMaxRpm` overstates the real rev cut on most cars** (library norm:
  the cut sits at 91-97% of reported; measured 2026-08-04 across 15 cars).
  The cut is directly observable: **`Torque` collapses to <= 0 at full
  throttle in a held gear** when the limiter fires, and the rpm at each
  collapse onset clusters within ~10 rpm. Upshift/downshift torque dips look
  identical but straddle a `Gear` change; mid-air free-revs reach the
  REPORTED max (unloaded wheels), not the cut. Exception: some limiters
  clamp with torque still positive (Skyline pins 8100 of a reported 10000
  with zero torque collapses) — only a sustained multi-gear ceiling shows
  those.
- Gear count and **effective gear ratios**, derivable from `CurrentEngineRpm` vs
  `WheelRotationSpeed` per `Gear`.
- Full behaviour set for tuning analysis: per-corner suspension travel (normalized
  *and* meters), slip ratio/angle/combined, tire temps, G-forces, inputs.
- **Slip normalization is load-dependent**: normalized slip angle is per tire
  against that tire's CURRENT grip limit. While cornering, the unloaded inside
  wheel reads consistently HIGHER than the loaded outside (measured across five
  cars, 2026-08-01: inside ~0.05-0.09 above the axle mean, outside below it),
  because load loss shrinks the inside tire's limit. Axle means therefore lean
  slightly toward the inside wheel's reading; they do not mask outside-wheel
  saturation.

## What is NOT in the packet

- Weight, weight distribution, fitted upgrades, tune settings, tire compound,
  suspension type: must come from user input or a community car dataset.
- Tire wear, track/route identifier.
- **Per-wheel effective camber**: displayed on the in-game telemetry HUD but not in
  the packet (the 324 bytes are fully accounted for; no camber field). Not derivable
  from suspension travel without per-car geometry.
- **Inner/middle/outer tire temps**: the packet carries a single temp per tire,
  although the in-game telemetry HUD displays IMO temps. Camber advice therefore
  needs manual IMO input (planned for the web UI); see design.md for the
  asymmetric-track caveat that comes with it.

## Practical notes

- Record **raw packets + receive timestamps** to disk before any decoding. Raw logs are
  replayable test fixtures and survive decoder bugs.
- One socket, no auth, LAN-only: no privacy/security surface beyond the local network.

## Sources

- [Official FH6 Data Out documentation (Forza Support)](https://support.forza.net/hc/en-us/articles/51744149102611-Forza-Horizon-6-Data-Out-Documentation)
  (layout above transcribed from the full text as of 2026-07-19).
- [fh6-tel, a Rust/Tauri FH6 telemetry dashboard](https://github.com/TheBanHammer/fh6-tel)
- [MOZA FH6 telemetry setup guide](https://support.mozaracing.com/en/support/solutions/articles/70000683812-forza-horizon-6-telemetry-settings-control-mapping-setup-guide)
