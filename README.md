# tuners

Tuning assistance for Forza Horizon 6. Captures the game's UDP telemetry ("Data Out"),
analyses driving behaviour, and gives directional, evidence-cited tune advice —
including A/B comparison of setups via spliced "ideal laps" that are robust to
driving mistakes, and a history-aware tuning journal.

## Use

Run `tuners serve`, open http://127.0.0.1:8080/, drive. The server records
telemetry automatically (race mode only — menus and free roam are skipped) and
cuts stint files on its own (car change, long idle). In the dashboard, set up a
**tuning session** for the car you're working on (facts like front weight %,
plus the current tune), then drive; a live view shows a confidence gauge that
tells you when you've driven enough. After editing the tune in-game, enter the
new values — the change is journaled automatically and the next stint starts
fresh, ready for A/B comparison and history-aware advice (`tuners advise`).

## Commands

```
tuners serve      the app: auto-recording + web dashboard (charts, A/B compare,
                  reports, live view + data-quality meter)
tuners capture    record a stint manually (serve then falls back to view-only)
tuners analyze    per-stint observations: tires, grip, suspension, damping, gearing, laps
tuners compare    tune A/B: lap-time delta, where it comes from, mistakes excluded
tuners recommend  directional tune advice with evidence (blind mode)
tuners advise     history-aware advice from a tuning journal (tune-journal.txt)
tuners replay     integrity-check a recorded session
tuners simulate   synthetic telemetry (stand-in for the game, for development)
```

Game setup: Settings → HUD and Gameplay → Data Out On, IP = the machine running
`tuners serve` (under WSL NAT, the WSL address from `hostname -I` — localhost is
not forwarded for UDP), any port outside 5200-5300 (default 20440), then restart
the game once. See [docs/guide.md](docs/guide.md) for capture practice (rewinds,
lap counts, A-B-A protocol).

## Docs

- [docs/design.md](docs/design.md) — objectives, constraints, architecture, design principles
- [docs/telemetry.md](docs/telemetry.md) — the packet, verified quirks, what's (not) available
- [docs/guide.md](docs/guide.md) — user-facing capture & interpretation guidance
