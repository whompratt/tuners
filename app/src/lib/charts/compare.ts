// A/B comparison chart: pure geometry + pure draw.
// Speed overlay, per-250m delta bars, cumulative gap curve.

import type { Palette } from "./palette";
import type { UnitDef } from "$lib/units";

export type Cmp = {
  binMeters: number;
  a: { file: string; laps: number; best: number; median: number; ideal: number; standingOnly: boolean };
  b: { file: string; laps: number; best: number; median: number; ideal: number; standingOnly: boolean };
  speedsA: number[];
  speedsB: number[];
  timesA: number[];
  delta: number[];
  // The 2-of-3 vote (median of ideal/best/median-lap deltas) and its
  // component deltas (ideal, best, median lap), B minus A.
  verdictDeltaS: number;
  currencies: [number, number, number];
  unequalLaps: boolean;
  carMismatch: boolean;
};

export const SEG_BINS = 25; // 250m segments for the delta bars
export const A_COLOR = "#3987e5";
export const B_COLOR = "#199e70";

export type CompareLayout = {
  cssW: number;
  cssH: number;
  pad: { l: number; r: number; t: number; b: number };
  bins: number;
  speedH: number;
  gapH: number;
  barsH: number;
  cumH: number;
  plotW: number;
  yMax: number;
  segs: number[];
  segMax: number;
  cum: number[];
  cumMax: number;
};

export function compareLayout(data: Cmp, spdK: number, cssW: number, cssH = 530): CompareLayout {
  const pad = { l: 56, r: 12, t: 8, b: 26 };
  const bins = data.delta.length;
  const segs: number[] = [];
  for (let s = 0; s < bins; s += SEG_BINS) {
    segs.push(data.delta.slice(s, s + SEG_BINS).reduce((x, y) => x + y, 0));
  }
  const cum: number[] = [];
  let acc = 0;
  for (const d of data.delta) {
    acc += d;
    cum.push(acc);
  }
  return {
    cssW,
    cssH,
    pad,
    bins,
    speedH: 210,
    gapH: 24,
    barsH: 80,
    cumH: 140,
    plotW: cssW - pad.l - pad.r,
    yMax: Math.ceil((Math.max(...data.speedsA, ...data.speedsB) * spdK) / 20) * 20,
    segs,
    segMax: Math.max(0.05, ...segs.map(Math.abs)),
    cum,
    cumMax: Math.max(0.1, ...cum.map(Math.abs)),
  };
}

export const compareBinAt = (L: CompareLayout, px: number): number | null =>
  px < L.pad.l || px > L.pad.l + L.plotW
    ? null
    : Math.round(((px - L.pad.l) / L.plotW) * (L.bins - 1));

export function drawCompare(
  ctx: CanvasRenderingContext2D,
  L: CompareLayout,
  data: Cmp,
  spd: UnitDef,
  dist: UnitDef,
  pal: Palette,
  hoverBin?: number | null,
) {
  const { cssW, pad, bins, plotW, yMax, speedH, gapH, barsH, cumH, segs, segMax, cum, cumMax } = L;
  const x = (bin: number) => pad.l + (bin / (bins - 1)) * plotW;
  const ySpeed = (v: number) => pad.t + speedH - (v / yMax) * speedH;
  const bars0 = pad.t + speedH + gapH + barsH / 2;
  const yBars = (d: number) => bars0 - (d / segMax) * (barsH / 2);
  const cumTop = pad.t + speedH + gapH + barsH + gapH;
  const cum0 = cumTop + cumH / 2;
  const yCum = (d: number) => cum0 - (-d / cumMax) * (cumH / 2); // up = B ahead (delta sum negative)
  ctx.font = '13px "JetBrains Mono", ui-monospace, monospace';

  ctx.lineWidth = 1;
  for (let g = 0; g <= yMax; g += 40) {
    ctx.strokeStyle = pal.grid;
    ctx.beginPath(); ctx.moveTo(pad.l, ySpeed(g)); ctx.lineTo(cssW - pad.r, ySpeed(g)); ctx.stroke();
    ctx.fillStyle = pal.muted; ctx.textAlign = "right"; ctx.textBaseline = "middle";
    ctx.fillText(`${g}`, pad.l - 8, ySpeed(g));
  }
  ctx.save();
  ctx.translate(11, pad.t + speedH / 2); ctx.rotate(-Math.PI / 2);
  ctx.textAlign = "center"; ctx.fillText(spd.l, 0, 0);
  ctx.restore();

  (
    [
      [data.speedsA, A_COLOR],
      [data.speedsB, B_COLOR],
    ] as [number[], string][]
  ).forEach(([speeds, color]) => {
    ctx.strokeStyle = color; ctx.lineWidth = 2; ctx.lineJoin = "round";
    ctx.beginPath();
    speeds.forEach((v, b) =>
      b ? ctx.lineTo(x(b), ySpeed(v * spd.k)) : ctx.moveTo(x(b), ySpeed(v * spd.k)),
    );
    ctx.stroke();
  });

  // Per-250m delta bars, in the color of the FASTER setup through that segment.
  ctx.strokeStyle = pal.baseline;
  ctx.beginPath(); ctx.moveTo(pad.l, bars0); ctx.lineTo(cssW - pad.r, bars0); ctx.stroke();
  segs.forEach((d, i) => {
    const x0 = x(i * SEG_BINS),
      x1 = x(Math.min((i + 1) * SEG_BINS, bins - 1));
    ctx.fillStyle = d < 0 ? B_COLOR : A_COLOR; // negative: B took less time
    const y0 = yBars(Math.max(d, 0)),
      y1 = yBars(Math.min(d, 0));
    ctx.fillRect(x0 + 1, y0, Math.max(1, x1 - x0 - 2), Math.max(1, y1 - y0));
  });
  ctx.fillStyle = pal.muted; ctx.textAlign = "left"; ctx.textBaseline = "middle";
  ctx.fillText("Δ per 250 m: bar in the color of the faster setup there", pad.l, pad.t + speedH + gapH / 2);
  ctx.textAlign = "right";
  ctx.fillText(`±${segMax.toFixed(2)}s`, pad.l - 8, bars0);

  // Cumulative gap: A is the flat reference line; the curve is B relative to it
  // (above the line = B ahead). Fill tinted by whoever is ahead.
  ctx.fillStyle = pal.muted; ctx.textAlign = "left"; ctx.textBaseline = "middle";
  ctx.fillText("cumulative gap: A flat, curve = B (above line: B ahead)", pad.l, cumTop - gapH / 2);
  ctx.strokeStyle = A_COLOR; ctx.lineWidth = 1;
  ctx.beginPath(); ctx.moveTo(pad.l, cum0); ctx.lineTo(cssW - pad.r, cum0); ctx.stroke();
  ctx.fillStyle = pal.muted; ctx.textAlign = "right"; ctx.textBaseline = "middle";
  ctx.fillText(`+${cumMax.toFixed(2)}s`, pad.l - 8, yCum(-cumMax));
  ctx.fillText(`-${cumMax.toFixed(2)}s`, pad.l - 8, yCum(cumMax));
  ctx.beginPath();
  cum.forEach((d, b) => (b ? ctx.lineTo(x(b), yCum(d)) : ctx.moveTo(x(b), yCum(d))));
  ctx.strokeStyle = B_COLOR; ctx.lineWidth = 2; ctx.lineJoin = "round"; ctx.stroke();
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

  ctx.fillStyle = pal.muted; ctx.textAlign = "center"; ctx.textBaseline = "top";
  // Distance marks every whole display unit (mi or km, per the speed pref).
  const distMax = bins * data.binMeters * dist.k;
  for (let d = 0; d <= distMax; d += 1) {
    ctx.fillText(`${d} ${dist.l}`, x(d / dist.k / data.binMeters), L.cssH - pad.b + 6);
  }

  if (hoverBin != null) {
    ctx.strokeStyle = pal.baseline;
    ctx.lineWidth = 1;
    ctx.beginPath(); ctx.moveTo(x(hoverBin), pad.t); ctx.lineTo(x(hoverBin), L.cssH - pad.b); ctx.stroke();
  }
}
