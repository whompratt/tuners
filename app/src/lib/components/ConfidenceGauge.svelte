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
  // Out lap: the recorder is live on lap 0 with no comparable lap yet —
  // the first lap only sets the reference, so nothing can accrue
  // confidence until it completes. Say that instead of a vague nudge.
  let f = $derived(app.live?.frame ?? null);
  let outLap = $derived(
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
    {#if outLap}
      out lap: skipping
    {:else}
      confidence: {{ good: "enough for A/B", ok: "nearly there", low: "keep driving" }[qBand] ?? "keep driving"}
    {/if}
  </div>
  {#if outLap}
    <div class="q-note" style="max-width:{width}px">
      the first lap sets the reference and is not timed as a flying lap; confidence starts on the next crossing
    </div>
  {:else if q}
    <div class="q-sub">
      {q.laps} {q.standingOnly ? "standing run(s)" : "flying lap(s)"} · best {fmtLap(q.bestLapS ?? 0)}
    </div>
    {#if q.note}
      <div class="q-note" style="max-width:{width}px">{q.note}</div>
    {/if}
  {/if}
</div>
