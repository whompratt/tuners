# tuners

Tuning assistance for Forza Horizon 6. Captures the game's UDP telemetry ("Data Out"),
analyses driving behaviour, and gives directional, evidence-cited tune advice —
including A/B comparison of setups via spliced "ideal laps" that are robust to
driving mistakes, and a history-aware tuning journal.

## Commands

```
tuners capture    record a session (game's Data Out -> .ftel file)
tuners analyze    per-stint observations: tires, grip, suspension, damping, gearing, laps
tuners compare    tune A/B: lap-time delta, where it comes from, mistakes excluded
tuners recommend  directional tune advice with evidence (blind mode)
tuners advise     history-aware advice from a tuning journal (tune-journal.txt)
tuners serve      local web dashboard: charts, A/B comparison, reports
tuners replay     integrity-check a recorded session
tuners simulate   synthetic telemetry (stand-in for the game, for development)
```

Game setup: Settings → HUD and Gameplay → Data Out On, IP = the machine running
`tuners capture` (under WSL NAT, the WSL address from `hostname -I` — localhost is
not forwarded for UDP), any port outside 5200-5300 (default 20440), then restart
the game once. See [docs/guide.md](docs/guide.md) for capture practice (rewinds,
lap counts, A-B-A protocol).

## Docs

- [docs/design.md](docs/design.md) — objectives, constraints, architecture, design principles
- [docs/telemetry.md](docs/telemetry.md) — the packet, verified quirks, what's (not) available
- [docs/guide.md](docs/guide.md) — user-facing capture & interpretation guidance
- [docs/plans/](docs/plans/) — numbered plans with status (001 capture … 008 damping/events)
- [CLAUDE.md](CLAUDE.md) — current work state
