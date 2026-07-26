<script lang="ts">
  import { app } from "$lib/app.svelte";
  import { SERIES, SPD, fmtLap } from "$lib/units";
  import { drawLaps, lapsBinAt, lapsLayout } from "$lib/charts/laps";
  import { palette } from "$lib/charts/palette";
  import Chart from "$lib/ui/Chart.svelte";
  import Tooltip from "$lib/ui/Tooltip.svelte";

  let hoverBin: number | null = $state(null);
  let tipX = $state(0);
  let tipY = $state(0);
  let wrapW = $state(0);

  let best = $derived(
    app.lapData ? Math.min(...app.lapData.laps.filter((l) => !l.standing).map((l) => l.time)) : 0,
  );

  // Closure identity = redraw trigger: changes with data, units, or hover.
  let draw = $derived.by(() => {
    const data = app.lapData;
    const spd = (app.unitsTick, SPD());
    const hover = hoverBin;
    return (ctx: CanvasRenderingContext2D, cssW: number) => {
      if (!data) return;
      drawLaps(ctx, lapsLayout(data, spd.k, cssW), data, spd, palette(), hover);
    };
  });

  function onmove(px: number, py: number, cssW: number) {
    if (!app.lapData) return;
    const L = lapsLayout(app.lapData, SPD().k, cssW);
    hoverBin = lapsBinAt(L, px);
    tipX = px;
    tipY = py;
    wrapW = cssW;
  }

  let tipRows = $derived.by(() => {
    if (hoverBin == null || !app.lapData) return null;
    const bin = hoverBin;
    const data = app.lapData;
    return {
      km: ((bin * data.binMeters) / 1000).toFixed(2),
      laps: data.laps
        .map((lap, i) => ({ lap, i, v: (lap.speeds[bin] ?? 0) * SPD().k }))
        .sort((a, b) => b.v - a.v),
      uncorroborated: data.corroborated.length > bin && !data.corroborated[bin],
    };
  });
</script>

{#if app.lapData}
  <div class="panel">
    <h2>Speed by distance — flying laps</h2>
    <div class="legend-row">
      {#each app.lapData.laps as lap, i (lap.lap)}
        <span>
          <span class="chip" style="background:{SERIES[i % SERIES.length]}"></span>
          lap {lap.lap} — <span class="num">{fmtLap(lap.time)}</span>{lap.standing
            ? " (standing start)"
            : lap.time === best
              ? " (best)"
              : ""}
        </span>
      {/each}
      {#if app.lapData.corroborated.length > 1}
        <span style="color:var(--muted)">strip under chart: ideal-lap corroboration (green = a second lap agrees)</span>
      {/if}
    </div>
    <Chart height={320} {draw} {onmove} onleave={() => (hoverBin = null)}>
      <Tooltip shown={tipRows != null} x={tipX} y={tipY} wrapWidth={wrapW} flipAt={170}>
        {#if tipRows}
          <div class="t-head">{tipRows.km} km</div>
          {#each tipRows.laps as r (r.i)}
            <div>
              <span class="chip" style="background:{SERIES[r.i % SERIES.length]}"></span>
              lap {r.lap.lap} <span class="num">{r.v.toFixed(0)} {SPD().l}</span>
            </div>
          {/each}
          {#if tipRows.uncorroborated}
            <div style="color:var(--muted);font-size:11px">single-lap bin — uncorroborated</div>
          {/if}
        {/if}
      </Tooltip>
    </Chart>
  </div>
{/if}
