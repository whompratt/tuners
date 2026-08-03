<script lang="ts">
  // The corroboration-confidence arc, shared by the Drive screen and the
  // Home dashboard's Drive card. Reads quality straight from app state.
  import { app } from "$lib/app.svelte";
  import { fmtLap } from "$lib/units";

  let { dim = false, width = 230 }: { dim?: boolean; width?: number } = $props();

  const ARC = Math.PI * 40; // semicircle path length (r=40)
  let q = $derived(app.quality);
  let qBand = $derived(q ? q.band : "low");
  let qPct = $derived(q ? (q.confidencePct ?? 0) : 0);
  // First run: the recorder is live on lap 0 with no comparable lap yet.
  // Nothing can accrue confidence until the same road is driven twice, and
  // at this point the route kind is unknowable (a circuit's out lap only
  // sets the reference; a point-to-point run counts in full once a second
  // run corroborates it), so the copy must not claim the lap is skipped.
  let f = $derived(app.live?.frame ?? null);
  let firstRun = $derived(
    !q && !!f?.raceOn && (f?.lapNumber ?? 1) === 0 && (app.live?.ageMs ?? Infinity) < 5000,
  );
</script>

<div class="live-quality {qBand === 'good' ? 'q-good' : qBand === 'ok' ? 'q-ok' : ''}" class:dim>
  <svg viewBox="0 0 100 62" {width} aria-label="data confidence">
    <path class="q-track" d="M 10 52 A 40 40 0 0 1 90 52" />
    <path
      class="q-fill"
      d="M 10 52 A 40 40 0 0 1 90 52"
      stroke-dasharray="{(ARC * qPct) / 100} 999"
      style="visibility:{qPct > 0 ? 'visible' : 'hidden'}"
    />
    <text class="q-pct" x="50" y="50">{q ? `${Math.round(qPct)}%` : "–"}</text>
  </svg>
  <div class="q-label">
    {#if firstRun}
      first run: setting the reference
    {:else}
      confidence: {{ good: "enough for A/B", ok: "nearly there", low: "keep driving" }[qBand] ?? "keep driving"}
    {/if}
  </div>
  {#if firstRun}
    <div class="q-note" style="max-width:{width}px">
      confidence needs the same road driven twice; it starts with the next lap, or the next run on a point-to-point
    </div>
  {:else if q}
    <div class="q-sub">
      {q.laps}
      {q.pointToPoint ? "point-to-point run(s)" : q.standingOnly ? "standing run(s)" : "flying lap(s)"} · best {fmtLap(
        q.bestLapS ?? 0,
      )}
    </div>
    {#if q.note}
      <div class="q-note" style="max-width:{width}px">{q.note}</div>
    {/if}
  {/if}
</div>
