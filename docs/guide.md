# User guide

Practical notes on capturing good data and reading the tool's output. (Seed of the
eventual frontend help — keep this written for users, not developers.)

## Rewinds: how the tool treats them

**Short version: drive however you normally drive. Rewinding is fine; so is not
rewinding.**

When you rewind, FH6 restores the car's exact state and lets you re-drive. The tool
reconstructs what you kept: driving you rewound over is erased, and the retry
counts as part of the lap. A rewound lap therefore shows up as a normal lap with a
real time — the tool doesn't care that the leaderboard marks it invalid, because it
evaluates your *setup*, not your leaderboard eligibility. Each detected rewind is
noted in the `analyze` output.

Two things worth knowing:

- **Rewinds can slightly reduce comparison confidence.** A lap finished via
  rewind-retries reflects polished execution — mildly faster than your natural
  pace. That optimism cancels out when both sessions of an A/B are driven with a
  similar rewind habit, but comparing a heavily-rewound session against a
  no-rewind session tilts the verdict. Keep the habit roughly consistent across
  sessions you intend to compare.
- **You don't need to rewind for the tool's sake.** When you make a mistake, it's
  fine to just keep driving: comparisons are built from spliced "ideal" laps that
  take each stretch of road from whichever lap drove it best, so a botched corner
  in one lap is simply outvoted by your clean laps. Rewind when *you* want a clean
  run — not to protect the data.

## Capturing sessions worth comparing

- Rivals (or any restartable race mode) is the intended loop: identical conditions
  every session, lap times, and live distance data.
- 3+ flying laps per session give the ideal-lap splicer real material; the out lap
  is detected and excluded automatically on circuits.
- **Drive a similar number of laps in sessions you'll compare**: the session with
  more laps gives its ideal more material, biasing the ideal-vs-ideal verdict in
  its favor (`compare` warns when counts differ). Adjacent segments all moving the
  same way is a stronger setup signal than the ideal total.
- When A/B-testing a tune change, remember the driver-learning confounder: you get
  faster at a track regardless of the tune. For a verdict you trust, re-run tune A
  afterwards (A-B-A) — if it matches the first A run, the delta was the tune.
