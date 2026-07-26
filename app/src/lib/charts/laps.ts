// Speed-by-distance lap chart: pure geometry + pure draw (plan 010 phase 2).
// Layout math is DOM-free and unit-tested; the Chart host owns sizing.

import type { Laps } from "$lib/app.svelte";
import { SERIES, type UnitDef } from "$lib/units";
import type { Palette } from "./palette";

export type LapsLayout = {
  cssW: number;
  cssH: number;
  pad: { l: number; r: number; t: number; b: number };
  plotW: number;
  plotH: number;
  bins: number;
  kmMax: number;
  yMax: number;
};

export function lapsLayout(data: Laps, spdK: number, cssW: number, cssH = 320): LapsLayout {
  const pad = { l: 56, r: 12, t: 8, b: 28 };
  const bins = Math.max(...data.laps.map((l) => l.speeds.length));
  const maxDisp = Math.max(...data.laps.flatMap((l) => l.speeds)) * spdK;
  return {
    cssW,
    cssH,
    pad,
    plotW: cssW - pad.l - pad.r,
    plotH: cssH - pad.t - pad.b,
    bins,
    kmMax: (bins * data.binMeters) / 1000,
    yMax: Math.ceil(maxDisp / 20) * 20,
  };
}

export const lapsBinAt = (L: LapsLayout, px: number): number | null =>
  px < L.pad.l || px > L.pad.l + L.plotW
    ? null
    : Math.round(((px - L.pad.l) / L.plotW) * (L.bins - 1));

export function drawLaps(
  ctx: CanvasRenderingContext2D,
  L: LapsLayout,
  data: Laps,
  spd: UnitDef,
  pal: Palette,
  hoverBin?: number | null,
) {
  const { pad, plotW, plotH, bins, kmMax, yMax, cssW } = L;
  const x = (bin: number) => pad.l + (bin / (bins - 1)) * plotW;
  const y = (v: number) => pad.t + plotH - (v / yMax) * plotH;

  ctx.font = "13px system-ui, sans-serif";
  ctx.lineWidth = 1;
  for (let g = 0; g <= yMax; g += 40) {
    ctx.strokeStyle = pal.grid;
    ctx.beginPath(); ctx.moveTo(pad.l, y(g)); ctx.lineTo(cssW - pad.r, y(g)); ctx.stroke();
    ctx.fillStyle = pal.muted;
    ctx.textAlign = "right"; ctx.textBaseline = "middle";
    ctx.fillText(`${g}`, pad.l - 8, y(g));
  }
  ctx.textAlign = "center"; ctx.textBaseline = "top";
  for (let km = 0; km <= kmMax; km += 1) {
    ctx.fillText(`${km} km`, x((km * 1000) / data.binMeters), pad.t + plotH + 8);
  }
  ctx.save();
  ctx.translate(11, pad.t + plotH / 2); ctx.rotate(-Math.PI / 2);
  ctx.fillText(spd.l, 0, 0);
  ctx.restore();
  ctx.strokeStyle = pal.baseline;
  ctx.beginPath(); ctx.moveTo(pad.l, y(0)); ctx.lineTo(cssW - pad.r, y(0)); ctx.stroke();

  data.laps.forEach((lap, i) => {
    ctx.strokeStyle = SERIES[i % SERIES.length];
    ctx.lineWidth = 2; ctx.lineJoin = "round";
    ctx.beginPath();
    lap.speeds.forEach((v, b) =>
      b ? ctx.lineTo(x(b), y(v * spd.k)) : ctx.moveTo(x(b), y(v * spd.k)),
    );
    ctx.stroke();
  });

  // Confidence strip: which bins of the spliced ideal a SECOND lap reproduces
  // (splice tolerance). Green = corroborated, dim = single-lap evidence.
  const corr = data.corroborated || [];
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
    ctx.strokeStyle = pal.baseline;
    ctx.lineWidth = 1;
    ctx.beginPath(); ctx.moveTo(x(hoverBin), pad.t); ctx.lineTo(x(hoverBin), pad.t + plotH); ctx.stroke();
  }
}
