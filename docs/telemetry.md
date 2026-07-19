# Telemetry: FH6 "Data Out"

How we get data out of the game, what's in it, and what still needs verifying.
This file is the single source of truth for data-collection facts. The packet layout
below is transcribed from the official Forza Support documentation (link at bottom).

## Transport

- **One-way UDP** to a configurable IP/port, sent at the game's frame rate (variable —
  resample using `TimestampMS`, not receive time). Nothing is ever received by the game.
- Enabled under **Settings → HUD and Gameplay → Data Out** (toggle + IP + port).
  `127.0.0.1` works same-PC; a LAN IP works for console → PC.
- **Avoid listening on ports 5200–5300** — the game binds its outgoing socket in that
  range.
- Data is sent **only while actively driving** — nothing during menus, pauses, replays,
  rewinds, or after finishing a race. `IsRaceOn` additionally distinguishes race-on
  from race-stopped packets.
- Single fixed packet format; no format selector (unlike Forza Motorsport).

## Packet layout

Total packet size: **324 bytes**, little-endian (byte order assumed from prior Forza
titles — verify). Types: `S`/`U`/`F` = signed int / unsigned int / float, number = bits.
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
| 116 | S32 ×4 | WheelOnRumbleStrip | 0/1 |
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
| 268 | F32 ×4 | TireTemp | units unspecified in doc (°F in prior titles — verify) |
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
  is always **0** — treat as padding.
- **Little-endian** confirmed; **tire temps are °F** (~200 peak under load).
- **The official "data is only sent while driving" note is wrong in practice**: the
  game keeps streaming packets in menus/pauses with `IsRaceOn = 0` and everything
  except `TimestampMS` zeroed. Filter on `IsRaceOn`, not on packet presence.
- **`Gear` = 0 is reverse, and 11 appears transiently mid-drive** (a handful of
  frames, likely a neutral/mid-shift sentinel) — analysis must not treat 11 as a
  real gear.
- Send rate matches frame rate as documented (~168 Hz observed on a 165 Hz setup).
- Setup gotcha: the game only starts honouring a newly configured Data Out target
  after a **full game restart**.
- **`DistanceTraveled` is always 0.0 in free roam** (likely race-only). Distance must
  be integrated from `Speed` over `TimestampMS`.

## Still to verify

- Whether `NumCylinders == 0` reliably identifies EVs (needs a capture in an EV).

## What the packet gives us for free (no user input needed)

- Car identity (`CarOrdinal`), class/PI, **drivetrain type**, cylinder count.
- Redline (`EngineMaxRpm`), observed peak power/torque (watts/Nm, live).
- Gear count and **effective gear ratios** — derivable from `CurrentEngineRpm` vs
  `WheelRotationSpeed` per `Gear`.
- Full behaviour set for tuning analysis: per-corner suspension travel (normalized
  *and* meters), slip ratio/angle/combined, tire temps, G-forces, inputs.

## What is NOT in the packet

- Weight, weight distribution, fitted upgrades, tune settings, tire compound,
  suspension type — must come from user input or a community car dataset.
- Tire wear, track/route identifier.

## Practical notes

- Record **raw packets + receive timestamps** to disk before any decoding. Raw logs are
  replayable test fixtures and survive decoder bugs.
- One socket, no auth, LAN-only: no privacy/security surface beyond the local network.

## Sources

- [Official FH6 Data Out documentation (Forza Support)](https://support.forza.net/hc/en-us/articles/51744149102611-Forza-Horizon-6-Data-Out-Documentation)
  — full text pasted by the user 2026-07-19; layout above transcribed from it.
- [fh6-tel — Rust/Tauri FH6 telemetry dashboard](https://github.com/TheBanHammer/fh6-tel)
- [MOZA FH6 telemetry setup guide](https://support.mozaracing.com/en/support/solutions/articles/70000683812-forza-horizon-6-telemetry-settings-control-mapping-setup-guide)
