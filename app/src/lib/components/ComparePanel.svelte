<script lang="ts">
  import { app, baseName, errMsg } from "$lib/app.svelte";
  import { commands, type CompareView } from "$lib/bindings";
  import { SPD, fmtLap } from "$lib/units";
  import {
    A_COLOR, B_COLOR, SEG_BINS, type Cmp,
    compareBinAt, compareLayout, drawCompare,
  } from "$lib/charts/compare";
  import { palette } from "$lib/charts/palette";
  import Chart from "$lib/ui/Chart.svelte";
  import Tooltip from "$lib/ui/Tooltip.svelte";

  // f32 exports as `number | null` (NaN honesty); sanitize once at the
  // boundary so the chart math stays clean.
  const n = (v: number | null) => v ?? 0;
  const sanitize = (v: CompareView): Cmp => ({
    binMeters: n(v.binMeters),
    a: { ...v.a, best: n(v.a.best), median: n(v.a.median), ideal: n(v.a.ideal) },
    b: { ...v.b, best: n(v.b.best), median: n(v.b.median), ideal: n(v.b.ideal) },
    speedsA: v.speedsA.map(n),
    speedsB: v.speedsB.map(n),
    timesA: v.timesA.map(n),
    delta: v.delta.map(n),
    verdictDeltaS: n(v.verdictDeltaS),
    currencies: [n(v.currencies[0]), n(v.currencies[1]), n(v.currencies[2])],
    unequalLaps: v.unequalLaps,
    carMismatch: v.carMismatch,
  });

  let cmpData: Cmp | null = $state(null);
  let error: string | null = $state(null);
  let hoverBin: number | null = $state(null);
  let tipX = $state(0);
  let tipY = $state(0);
  let wrapW = $state(0);

  const N_SECTORS = 3;

  $effect(() => {
    const a = app.selA,
      b = app.selB;
    if (!a || !b) {
      cmpData = null;
      error = null;
      return;
    }
    commands.compare(a, b).then((r) => {
      if (r.status === "ok") {
        cmpData = sanitize(r.data);
        error = null;
      } else {
        cmpData = null;
        error = errMsg(r.error);
      }
    });
  });

  const word = (d: number) =>
    Math.abs(d) < 0.01 ? "even" : d < 0 ? `B faster by ${(-d).toFixed(2)}s` : `A faster by ${d.toFixed(2)}s`;

  // Garage61-style sector gaps: thirds of the route, A's ideal sector time plus
  // B's delta, tinted by the gainer's identity color with intensity ~ magnitude.
  let sectors = $derived.by(() => {
    if (!cmpData) return [];
    const bins = cmpData.delta.length;
    const per = Math.ceil(bins / N_SECTORS);
    const out: { aTime: number; delta: number }[] = [];
    for (let s = 0; s < N_SECTORS; s++) {
      const lo = s * per,
        hi = Math.min((s + 1) * per, bins);
      out.push({
        aTime: cmpData.timesA.slice(lo, hi).reduce((x, y) => x + y, 0),
        delta: cmpData.delta.slice(lo, hi).reduce((x, y) => x + y, 0),
      });
    }
    return out;
  });
  let sectorMax = $derived(Math.max(0.01, ...sectors.map((s) => Math.abs(s.delta))));

  let draw = $derived.by(() => {
    const data = cmpData;
    const spd = (app.unitsTick, SPD());
    const hover = hoverBin;
    return (ctx: CanvasRenderingContext2D, cssW: number) => {
      if (!data) return;
      drawCompare(ctx, compareLayout(data, spd.k, cssW), data, spd, palette(), hover);
    };
  });

  function onmove(px: number, py: number, cssW: number) {
    if (!cmpData) return;
    const L = compareLayout(cmpData, SPD().k, cssW);
    hoverBin = compareBinAt(L, px);
    tipX = px;
    tipY = py;
    wrapW = cssW;
  }

  let tip = $derived.by(() => {
    if (hoverBin == null || !cmpData) return null;
    const bin = hoverBin;
    const seg =
      cmpData.delta
        .slice(Math.floor(bin / SEG_BINS) * SEG_BINS, (Math.floor(bin / SEG_BINS) + 1) * SEG_BINS)
        .reduce((x, y) => x + y, 0) ?? 0;
    const gap = cmpData.delta.slice(0, bin + 1).reduce((x, y) => x + y, 0); // negative = B ahead
    return {
      km: ((bin * cmpData.binMeters) / 1000).toFixed(2),
      a: (cmpData.speedsA[bin] ?? 0) * SPD().k,
      b: (cmpData.speedsB[bin] ?? 0) * SPD().k,
      seg,
      gap,
    };
  });
</script>

{#if cmpData || error}
  <div class="panel">
    <h2>A/B comparison</h2>
    {#if error}
      <div style="font-size:13px;color:var(--ink-2);margin-bottom:8px">cannot compare: {error}</div>
    {:else if cmpData}
      <div style="font-size:13px;color:var(--ink-2);margin-bottom:8px">
        best lap: <span class="num">{word(cmpData.b.best - cmpData.a.best)}</span> · median lap:
        <span class="num">{word(cmpData.b.median - cmpData.a.median)}</span> · optimal lap (spliced):
        <span class="num">{word(cmpData.b.ideal - cmpData.a.ideal)}</span> · <b>verdict (2-of-3 vote):
        <span class="num">{word(cmpData.verdictDeltaS)}</span></b>
        {#if Math.sign(cmpData.verdictDeltaS) !== Math.sign(cmpData.currencies[0]) && (Math.abs(cmpData.verdictDeltaS) >= 0.05 || Math.abs(cmpData.currencies[0]) >= 0.05)}
          <div style="color:var(--muted)">
            best and median lap agree against the optimal-lap comparison (it rewards laps fast in different places);
            the verdict follows the majority
          </div>
        {/if}
        {#if cmpData.unequalLaps}
          · <span style="color:var(--muted)">note: unequal lap counts bias the optimal lap toward the run with more laps</span>
        {/if}
        {#if cmpData.carMismatch}
          · <span style="color:var(--muted)">note: different cars, so this compares cars, not tunes</span>
        {/if}
      </div>
      <div class="legend-row">
        {#each sectors as s, i (i)}
          <span
            class="sector"
            style="background:rgba({s.delta < 0 ? '25,158,112' : '57,135,229'},{(
              0.12 + 0.45 * (Math.abs(s.delta) / sectorMax)
            ).toFixed(2)})"
          >
            S{i + 1} · A <span class="num">{s.aTime.toFixed(3)}s</span> · {s.delta < 0 ? "B" : "A"}
            <span class="num">−{Math.abs(s.delta).toFixed(3)}s</span>
          </span>
        {/each}
      </div>
      <div class="legend-row">
        {#each [[A_COLOR, "A", cmpData.a], [B_COLOR, "B", cmpData.b]] as [c, tag, s] (tag)}
          <span>
            <span class="chip" style="background:{c}"></span>{tag}: {baseName((s as typeof cmpData.a).file)} ·
            best <span class="num">{fmtLap((s as typeof cmpData.a).best)}</span>, optimal
            <span class="num">{fmtLap((s as typeof cmpData.a).ideal)}</span> ({(s as typeof cmpData.a).laps} laps)
          </span>
        {/each}
      </div>
      <Chart height={530} {draw} {onmove} onleave={() => (hoverBin = null)}>
        <Tooltip shown={tip != null} x={tipX} y={tipY} wrapWidth={wrapW} flipAt={200}>
          {#if tip}
            <div class="t-head">{tip.km} km</div>
            <div><span class="chip" style="background:{A_COLOR}"></span>A <span class="num">{tip.a.toFixed(0)} {SPD().l}</span></div>
            <div><span class="chip" style="background:{B_COLOR}"></span>B <span class="num">{tip.b.toFixed(0)} {SPD().l}</span></div>
            <div>this 250 m: <span class="num">{tip.seg < 0 ? "B" : "A"} quicker by {Math.abs(tip.seg).toFixed(2)}s</span></div>
            <div>gap so far: <span class="num">{tip.gap < 0 ? "B" : "A"} ahead by {Math.abs(tip.gap).toFixed(2)}s</span></div>
          {/if}
        </Tooltip>
      </Chart>
    {/if}
  </div>
{/if}
