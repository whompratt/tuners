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
    confidence — {{ good: "enough for A/B", ok: "nearly there", low: "keep driving" }[qBand] ?? "keep driving"}
  </div>
  {#if q}
    <div style="font-size:13px;color:var(--muted)">
      {q.laps} {q.standingOnly ? "standing run(s)" : "flying lap(s)"} · best {fmtLap(q.bestLapS ?? 0)}
    </div>
  {/if}
</div>
