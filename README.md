# tuners

Tuning assistance for Forza Horizon 6. Captures the game's UDP telemetry ("Data Out"),
analyses driving behaviour, and suggests tune adjustments (tire pressures, alignment,
springs/ARBs, gearing) grounded in what the car is actually doing on the road.

Status: early exploration. See [docs/design.md](docs/design.md) for objectives and
high-level design, and [docs/telemetry.md](docs/telemetry.md) for how data is collected
and what is/isn't available.

## Quick start

Nothing runnable yet beyond `cargo run`. First milestone is a telemetry capture spike —
see [docs/plans/001-telemetry-capture.md](docs/plans/001-telemetry-capture.md).
