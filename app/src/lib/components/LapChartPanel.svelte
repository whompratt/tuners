<script lang="ts">
  import { app } from "$lib/app.svelte";
  import { SERIES, SPD, fmtLap } from "$lib/units";

  let canvas: HTMLCanvasElement | undefined = $state();
  let wrap: HTMLDivElement | undefined = $state();
  let tip: HTMLDivElement | undefined = $state();
  let tipHtml = $state("");
  let tipShown = $state(false);
  let tipLeft = $state(0);
  let tipTop = $state(0);

  let best = $derived(
    app.lapData ? Math.min(...app.lapData.laps.filter((l) => !l.standing).map((l) => l.time)) : 0,
  );

  function layout() {
    const lapData = app.lapData!;
    const dpr = window.devicePixelRatio || 1;
    const cssW = canvas!.clientWidth,
      cssH = 320;
    canvas!.width = cssW * dpr;
    canvas!.height = cssH * dpr;
    const ctx = canvas!.getContext("2d")!;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    const pad = { l: 56, r: 12, t: 8, b: 28 };
    const bins = Math.max(...lapData.laps.map((l) => l.speeds.length));
    const maxMph = Math.max(...lapData.laps.flatMap((l) => l.speeds)) * SPD().k;
    return {
      ctx, cssW, cssH, pad,
      plotW: cssW - pad.l - pad.r,
      plotH: cssH - pad.t - pad.b,
      bins,
      kmMax: (bins * lapData.binMeters) / 1000,
      yMax: Math.ceil(maxMph / 20) * 20,
    };
  }

  function drawChart(hoverBin?: number | null) {
    if (!app.lapData || !canvas) return;
    const lapData = app.lapData;
    const { ctx, cssW, cssH, pad, plotW, plotH, bins, kmMax, yMax } = layout();
    const css = (name: string) => getComputedStyle(document.documentElement).getPropertyValue(name).trim();
    const x = (bin: number) => pad.l + (bin / (bins - 1)) * plotW;
    const y = (mph: number) => pad.t + plotH - (mph / yMax) * plotH;
    ctx.clearRect(0, 0, cssW, cssH);

    ctx.font = "13px system-ui, sans-serif";
    ctx.lineWidth = 1;
    for (let g = 0; g <= yMax; g += 40) {
      ctx.strokeStyle = css("--grid");
      ctx.beginPath(); ctx.moveTo(pad.l, y(g)); ctx.lineTo(cssW - pad.r, y(g)); ctx.stroke();
      ctx.fillStyle = css("--muted");
      ctx.textAlign = "right"; ctx.textBaseline = "middle";
      ctx.fillText(`${g}`, pad.l - 8, y(g));
    }
    ctx.textAlign = "center"; ctx.textBaseline = "top";
    for (let km = 0; km <= kmMax; km += 1) {
      ctx.fillText(`${km} km`, x((km * 1000) / lapData.binMeters), pad.t + plotH + 8);
    }
    ctx.save();
    ctx.translate(11, pad.t + plotH / 2); ctx.rotate(-Math.PI / 2);
    ctx.fillText(SPD().l, 0, 0);
    ctx.restore();
    ctx.strokeStyle = css("--baseline");
    ctx.beginPath(); ctx.moveTo(pad.l, y(0)); ctx.lineTo(cssW - pad.r, y(0)); ctx.stroke();

    lapData.laps.forEach((lap, i) => {
      ctx.strokeStyle = SERIES[i % SERIES.length];
      ctx.lineWidth = 2; ctx.lineJoin = "round";
      ctx.beginPath();
      lap.speeds.forEach((v, b) =>
        b ? ctx.lineTo(x(b), y(v * SPD().k)) : ctx.moveTo(x(b), y(v * SPD().k)),
      );
      ctx.stroke();
    });

    // Confidence strip: which bins of the spliced ideal a SECOND lap reproduces
    // (splice tolerance). Green = corroborated, dim = single-lap evidence.
    const corr = lapData.corroborated || [];
    if (corr.length > 1) {
      const yTop = pad.t + plotH + 2;
      const xw = plotW / Math.max(bins - 1, 1);
      for (let b = 0; b < Math.min(bins, corr.length); ) {
        let e = b;
        while (e < corr.length && corr[e] === corr[b]) e++;
        ctx.fillStyle = corr[b] ? "rgba(25,158,112,0.75)" : "rgba(137,135,129,0.25)";
        ctx.fillRect(Math.max(pad.l, x(b) - xw / 2), yTop, (e - b) * xw, 3.5);
        b = e;
      }
    }

    if (hoverBin != null) {
      ctx.strokeStyle = css("--baseline");
      ctx.lineWidth = 1;
      ctx.beginPath(); ctx.moveTo(x(hoverBin), pad.t); ctx.lineTo(x(hoverBin), pad.t + plotH); ctx.stroke();
    }
  }

  function onMove(ev: MouseEvent) {
    if (!app.lapData || !canvas) return;
    const rect = canvas.getBoundingClientRect();
    // layout() resizes the canvas, which CLEARS it — call it once, before
    // drawChart, never after.
    const { pad, plotW, bins } = layout();
    const px = ev.clientX - rect.left;
    if (px < pad.l || px > pad.l + plotW) {
      drawChart();
      tipShown = false;
      return;
    }
    const bin = Math.round(((px - pad.l) / plotW) * (bins - 1));
    drawChart(bin);
    const lapData = app.lapData;
    const km = ((bin * lapData.binMeters) / 1000).toFixed(2);
    const rows = lapData.laps
      .map((lap, i) => ({ lap, i, mph: (lap.speeds[bin] ?? 0) * SPD().k }))
      .sort((a, b) => b.mph - a.mph)
      .map(
        ({ lap, i, mph }) =>
          `<div><span class="chip" style="background:${SERIES[i % SERIES.length]}"></span>` +
          `lap ${lap.lap} <span class="num">${mph.toFixed(0)} ${SPD().l}</span></div>`,
      );
    const uncorr =
      lapData.corroborated && lapData.corroborated.length > bin && !lapData.corroborated[bin]
        ? '<div style="color:var(--muted);font-size:11px">single-lap bin — uncorroborated</div>'
        : "";
    tipHtml = `<div class="t-head">${km} km</div>` + rows.join("") + uncorr;
    tipShown = true;
    const wrapRect = wrap!.getBoundingClientRect();
    const flip = px > wrapRect.width - 170;
    tipLeft = flip ? px - (tip?.offsetWidth ?? 160) - 12 : px + 12;
    tipTop = Math.max(0, ev.clientY - rect.top - 20);
  }

  $effect(() => {
    void app.lapData;
    void app.unitsTick;
    drawChart();
  });
</script>

<svelte:window onresize={() => drawChart()} />

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
      {#if (app.lapData.corroborated || []).length > 1}
        <span style="color:var(--muted)">strip under chart: ideal-lap corroboration (green = a second lap agrees)</span>
      {/if}
    </div>
    <div
      id="canvas-wrap"
      bind:this={wrap}
      onmousemove={onMove}
      onmouseleave={() => { drawChart(); tipShown = false; }}
      role="img"
    >
      <canvas bind:this={canvas} height="320"></canvas>
      <div
        class="tip"
        bind:this={tip}
        style="display:{tipShown ? 'block' : 'none'};left:{tipLeft}px;top:{tipTop}px"
      >
        <!-- eslint-disable-next-line svelte/no-at-html-tags -->
        {@html tipHtml}
      </div>
    </div>
  </div>
{/if}
