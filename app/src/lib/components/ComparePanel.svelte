<script lang="ts">
  import { app } from "$lib/app.svelte";
  import { commands, type CompareView } from "$lib/bindings";
  import { SPD, fmtLap } from "$lib/units";

  // f32 exports as `number | null` (NaN honesty); sanitize once at the
  // boundary so the chart math stays clean.
  type Cmp = {
    binMeters: number;
    a: { file: string; laps: number; best: number; ideal: number; standingOnly: boolean };
    b: { file: string; laps: number; best: number; ideal: number; standingOnly: boolean };
    speedsA: number[]; speedsB: number[]; timesA: number[]; delta: number[];
    unequalLaps: boolean; carMismatch: boolean;
  };
  const n = (v: number | null) => v ?? 0;
  const sanitize = (v: CompareView): Cmp => ({
    binMeters: n(v.binMeters),
    a: { ...v.a, best: n(v.a.best), ideal: n(v.a.ideal) },
    b: { ...v.b, best: n(v.b.best), ideal: n(v.b.ideal) },
    speedsA: v.speedsA.map(n), speedsB: v.speedsB.map(n),
    timesA: v.timesA.map(n), delta: v.delta.map(n),
    unequalLaps: v.unequalLaps, carMismatch: v.carMismatch,
  });
  let cmpData: Cmp | null = $state(null);
  let error: string | null = $state(null);
  let canvas: HTMLCanvasElement | undefined = $state();
  let wrap: HTMLDivElement | undefined = $state();
  let tip: HTMLDivElement | undefined = $state();
  let tipHtml = $state("");
  let tipShown = $state(false);
  let tipLeft = $state(0);
  let tipTop = $state(0);

  const N_SECTORS = 3;
  const SEG_BINS = 25; // 250m segments for the delta bars

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
        error = r.error.message;
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

  function compareLayout() {
    const dpr = window.devicePixelRatio || 1;
    const cssW = canvas!.clientWidth,
      cssH = 530;
    canvas!.width = cssW * dpr;
    canvas!.height = cssH * dpr;
    const ctx = canvas!.getContext("2d")!;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    const pad = { l: 56, r: 12, t: 8, b: 26 };
    const bins = cmpData!.delta.length;
    const speedH = 210,
      gapH = 24,
      barsH = 80,
      cumH = 140;
    const segs: number[] = [];
    for (let s = 0; s < bins; s += SEG_BINS) {
      segs.push(cmpData!.delta.slice(s, s + SEG_BINS).reduce((x, y) => x + y, 0));
    }
    const cum: number[] = [];
    let acc = 0;
    for (const d of cmpData!.delta) {
      acc += d;
      cum.push(acc);
    }
    return {
      ctx, cssW, cssH, pad, bins, speedH, gapH, barsH, cumH,
      plotW: cssW - pad.l - pad.r,
      yMax: Math.ceil((Math.max(...cmpData!.speedsA, ...cmpData!.speedsB) * SPD().k) / 20) * 20,
      segs,
      segMax: Math.max(0.05, ...segs.map(Math.abs)),
      cum,
      cumMax: Math.max(0.1, ...cum.map(Math.abs)),
    };
  }

  function drawCompare(hoverBin?: number | null) {
    if (!cmpData || !canvas) return;
    const L = compareLayout();
    const { ctx, cssW, pad, bins, plotW, yMax, speedH, gapH, barsH, cumH, segs, segMax, cum, cumMax } = L;
    const css = (n: string) => getComputedStyle(document.documentElement).getPropertyValue(n).trim();
    const x = (bin: number) => pad.l + (bin / (bins - 1)) * plotW;
    const ySpeed = (mph: number) => pad.t + speedH - (mph / yMax) * speedH;
    const bars0 = pad.t + speedH + gapH + barsH / 2;
    const yBars = (d: number) => bars0 - (d / segMax) * (barsH / 2);
    const cumTop = pad.t + speedH + gapH + barsH + gapH;
    const cum0 = cumTop + cumH / 2;
    const yCum = (d: number) => cum0 - (-d / cumMax) * (cumH / 2); // up = B ahead (delta sum negative)
    ctx.clearRect(0, 0, cssW, L.cssH);
    ctx.font = "13px system-ui, sans-serif";

    ctx.lineWidth = 1;
    for (let g = 0; g <= yMax; g += 40) {
      ctx.strokeStyle = css("--grid");
      ctx.beginPath(); ctx.moveTo(pad.l, ySpeed(g)); ctx.lineTo(cssW - pad.r, ySpeed(g)); ctx.stroke();
      ctx.fillStyle = css("--muted"); ctx.textAlign = "right"; ctx.textBaseline = "middle";
      ctx.fillText(`${g}`, pad.l - 8, ySpeed(g));
    }
    ctx.save();
    ctx.translate(11, pad.t + speedH / 2); ctx.rotate(-Math.PI / 2);
    ctx.textAlign = "center"; ctx.fillText(SPD().l, 0, 0);
    ctx.restore();

    (
      [
        [cmpData.speedsA, "#3987e5"],
        [cmpData.speedsB, "#199e70"],
      ] as [number[], string][]
    ).forEach(([speeds, color]) => {
      ctx.strokeStyle = color; ctx.lineWidth = 2; ctx.lineJoin = "round";
      ctx.beginPath();
      speeds.forEach((v, b) =>
        b ? ctx.lineTo(x(b), ySpeed(v * SPD().k)) : ctx.moveTo(x(b), ySpeed(v * SPD().k)),
      );
      ctx.stroke();
    });

    // Per-250m delta bars, in the color of the FASTER setup through that segment.
    ctx.strokeStyle = css("--baseline");
    ctx.beginPath(); ctx.moveTo(pad.l, bars0); ctx.lineTo(cssW - pad.r, bars0); ctx.stroke();
    segs.forEach((d, i) => {
      const x0 = x(i * SEG_BINS),
        x1 = x(Math.min((i + 1) * SEG_BINS, bins - 1));
      ctx.fillStyle = d < 0 ? "#199e70" : "#3987e5"; // negative: B took less time
      const y0 = yBars(Math.max(d, 0)),
        y1 = yBars(Math.min(d, 0));
      ctx.fillRect(x0 + 1, y0, Math.max(1, x1 - x0 - 2), Math.max(1, y1 - y0));
    });
    ctx.fillStyle = css("--muted"); ctx.textAlign = "left"; ctx.textBaseline = "middle";
    ctx.fillText("Δ per 250 m — bar in the color of the faster setup there", pad.l, pad.t + speedH + gapH / 2);
    ctx.textAlign = "right";
    ctx.fillText(`±${segMax.toFixed(2)}s`, pad.l - 8, bars0);

    // Cumulative gap: A is the flat reference line; the curve is B relative to it
    // (above the line = B ahead). Fill tinted by whoever is ahead.
    ctx.fillStyle = css("--muted"); ctx.textAlign = "left"; ctx.textBaseline = "middle";
    ctx.fillText("cumulative gap — A flat, curve = B (above line: B ahead)", pad.l, cumTop - gapH / 2);
    ctx.strokeStyle = "#3987e5"; ctx.lineWidth = 1;
    ctx.beginPath(); ctx.moveTo(pad.l, cum0); ctx.lineTo(cssW - pad.r, cum0); ctx.stroke();
    ctx.fillStyle = css("--muted"); ctx.textAlign = "right"; ctx.textBaseline = "middle";
    ctx.fillText(`+${cumMax.toFixed(2)}s`, pad.l - 8, yCum(-cumMax));
    ctx.fillText(`-${cumMax.toFixed(2)}s`, pad.l - 8, yCum(cumMax));
    ctx.beginPath();
    cum.forEach((d, b) => (b ? ctx.lineTo(x(b), yCum(d)) : ctx.moveTo(x(b), yCum(d))));
    ctx.strokeStyle = "#199e70"; ctx.lineWidth = 2; ctx.lineJoin = "round"; ctx.stroke();
    // sign-split fill between curve and reference
    for (const sign of [1, -1]) {
      ctx.beginPath();
      ctx.moveTo(x(0), cum0);
      cum.forEach((d, b) => {
        const v = sign > 0 ? Math.min(d, 0) : Math.max(d, 0); // d<0 = B ahead
        ctx.lineTo(x(b), yCum(v));
      });
      ctx.lineTo(x(bins - 1), cum0);
      ctx.closePath();
      ctx.fillStyle = sign > 0 ? "rgba(25,158,112,0.18)" : "rgba(57,135,229,0.18)";
      ctx.fill();
    }

    ctx.fillStyle = css("--muted"); ctx.textAlign = "center"; ctx.textBaseline = "top";
    const kmMax = (bins * cmpData.binMeters) / 1000;
    for (let km = 0; km <= kmMax; km += 1) {
      ctx.fillText(`${km} km`, x((km * 1000) / cmpData.binMeters), L.cssH - pad.b + 6);
    }

    if (hoverBin != null) {
      ctx.strokeStyle = css("--baseline");
      ctx.lineWidth = 1;
      ctx.beginPath(); ctx.moveTo(x(hoverBin), pad.t); ctx.lineTo(x(hoverBin), L.cssH - pad.b); ctx.stroke();
    }
  }

  function onMove(ev: MouseEvent) {
    if (!cmpData || !canvas) return;
    const rect = canvas.getBoundingClientRect();
    // compareLayout() resizes the canvas, which CLEARS it — call it once, before
    // drawCompare, never after.
    const { pad, plotW, bins, segs, cum } = compareLayout();
    const px = ev.clientX - rect.left;
    if (px < pad.l || px > pad.l + plotW) {
      drawCompare();
      tipShown = false;
      return;
    }
    const bin = Math.round(((px - pad.l) / plotW) * (bins - 1));
    drawCompare(bin);
    const seg = segs[Math.floor(bin / SEG_BINS)] ?? 0;
    const gap = cum[bin] ?? 0; // negative = B ahead
    tipHtml =
      `<div class="t-head">${((bin * cmpData.binMeters) / 1000).toFixed(2)} km</div>` +
      `<div><span class="chip" style="background:#3987e5"></span>A <span class="num">${(cmpData.speedsA[bin] * SPD().k).toFixed(0)} ${SPD().l}</span></div>` +
      `<div><span class="chip" style="background:#199e70"></span>B <span class="num">${(cmpData.speedsB[bin] * SPD().k).toFixed(0)} ${SPD().l}</span></div>` +
      `<div>this 250 m: <span class="num">${seg < 0 ? "B" : "A"} quicker by ${Math.abs(seg).toFixed(2)}s</span></div>` +
      `<div>gap so far: <span class="num">${gap < 0 ? "B" : "A"} ahead by ${Math.abs(gap).toFixed(2)}s</span></div>`;
    tipShown = true;
    const wrapRect = wrap!.getBoundingClientRect();
    tipLeft = px > wrapRect.width - 200 ? px - (tip?.offsetWidth ?? 200) - 12 : px + 12;
    tipTop = Math.max(0, ev.clientY - rect.top - 20);
  }

  $effect(() => {
    void cmpData;
    void app.unitsTick;
    drawCompare();
  });
</script>

<svelte:window onresize={() => drawCompare()} />

{#if cmpData || error}
  <div class="panel">
    <h2>A/B comparison</h2>
    {#if error}
      <div style="font-size:13px;color:var(--ink-2);margin-bottom:8px">cannot compare: {error}</div>
    {:else if cmpData}
      <div style="font-size:13px;color:var(--ink-2);margin-bottom:8px">
        best lap: <span class="num">{word(cmpData.b.best - cmpData.a.best)}</span> · ideal (spliced):
        <span class="num">{word(cmpData.b.ideal - cmpData.a.ideal)}</span>
        {#if cmpData.unequalLaps}
          · <span style="color:var(--muted)">note: unequal lap counts bias the ideal toward the session with more laps</span>
        {/if}
        {#if cmpData.carMismatch}
          · <span style="color:var(--muted)">note: different cars — this compares cars, not tunes</span>
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
        {#each [["#3987e5", "A", cmpData.a], ["#199e70", "B", cmpData.b]] as [c, tag, s] (tag)}
          <span>
            <span class="chip" style="background:{c}"></span>{tag}: {(s as typeof cmpData.a).file.split("/").pop()} —
            best <span class="num">{fmtLap((s as typeof cmpData.a).best)}</span>, ideal
            <span class="num">{fmtLap((s as typeof cmpData.a).ideal)}</span> ({(s as typeof cmpData.a).laps} laps)
          </span>
        {/each}
      </div>
      <div
        style="position:relative"
        bind:this={wrap}
        onmousemove={onMove}
        onmouseleave={() => { drawCompare(); tipShown = false; }}
        role="img"
      >
        <canvas bind:this={canvas} height="530"></canvas>
        <div
          class="tip"
          bind:this={tip}
          style="display:{tipShown ? 'block' : 'none'};left:{tipLeft}px;top:{tipTop}px"
        >
          <!-- eslint-disable-next-line svelte/no-at-html-tags -->
          {@html tipHtml}
        </div>
      </div>
    {/if}
  </div>
{/if}
