// Setup-history landscape: tried values vs cumulative delta (valley shape:
// down = faster), fitted curve, estimated optimum. Pure geometry + draw.

import type { Palette } from "./palette";

export type LandscapeData = {
  /** (value, cumulative verdict delta s); sanitized, ascending by value. */
  nodes: [number, number][];
  fit: [number, number, number] | null;
  vertex: number | null;
};

export type LandscapeLayout = {
  cssW: number;
  cssH: number;
  x0: number;
  x1: number;
  y0: number;
  y1: number;
  padX: number;
  padY: number;
};

export function landscapeLayout(data: LandscapeData, cssW: number, cssH = 170): LandscapeLayout {
  const xs = data.nodes.map((n) => n[0]),
    ys = data.nodes.map((n) => n[1]);
  const x0 = Math.min(...xs),
    x1 = Math.max(...xs);
  const y0 = Math.min(...ys, 0),
    y1 = Math.max(...ys, 0);
  return {
    cssW,
    cssH,
    x0,
    x1,
    y0,
    y1,
    padX: (x1 - x0) * 0.08 || 1,
    padY: (y1 - y0) * 0.18 || 0.05,
  };
}

export const landscapeX = (L: LandscapeLayout, v: number) =>
  40 + ((v - (L.x0 - L.padX)) / (L.x1 + L.padX - (L.x0 - L.padX))) * (L.cssW - 55);
export const landscapeY = (L: LandscapeLayout, v: number) =>
  12 + ((L.y1 + L.padY - v) / (L.y1 + L.padY - (L.y0 - L.padY))) * (L.cssH - 44);

const sgn = (v: number) => `${v > 0 ? "+" : ""}${v.toFixed(2)}s`;

export function drawLandscape(
  ctx: CanvasRenderingContext2D,
  L: LandscapeLayout,
  data: LandscapeData,
  pal: Palette,
) {
  const { cssW: W, cssH: H } = L;
  ctx.font = '11px "JetBrains Mono", ui-monospace, monospace';
  if (data.nodes.length < 2) {
    ctx.fillStyle = pal.muted;
    ctx.fillText("not enough mapped values to chart. Drive more single-change stints", 10, 24);
    return;
  }
  const X = (v: number) => landscapeX(L, v);
  const Y = (v: number) => landscapeY(L, v);
  ctx.strokeStyle = "rgba(139,139,144,.35)";
  ctx.beginPath(); ctx.moveTo(40, Y(0)); ctx.lineTo(W - 8, Y(0)); ctx.stroke();
  ctx.fillStyle = pal.muted;
  ctx.fillText("0", 28, Y(0) + 3);
  if (data.fit) {
    const [fa, fb, fc] = data.fit;
    ctx.strokeStyle = "rgba(57,135,229,.55)";
    ctx.beginPath();
    for (let i = 0; i <= 60; i++) {
      const x = L.x0 - L.padX + (i / 60) * (L.x1 + L.padX - (L.x0 - L.padX));
      const y = fa * x * x + fb * x + fc;
      if (i) ctx.lineTo(X(x), Y(y));
      else ctx.moveTo(X(x), Y(y));
    }
    ctx.stroke();
  }
  if (data.vertex != null) {
    ctx.strokeStyle = "rgba(25,158,112,.7)";
    ctx.setLineDash([4, 3]);
    ctx.beginPath(); ctx.moveTo(X(data.vertex), 12); ctx.lineTo(X(data.vertex), H - 30); ctx.stroke();
    ctx.setLineDash([]);
    ctx.fillStyle = pal.ok;
    ctx.fillText(`optimum ≈ ${data.vertex}`, Math.min(X(data.vertex) + 5, W - 90), 22);
  }
  for (const [v, cum] of data.nodes) {
    ctx.fillStyle = pal.info;
    ctx.beginPath(); ctx.arc(X(v), Y(cum), 3.5, 0, 7); ctx.fill();
    ctx.fillStyle = pal.ink2;
    ctx.fillText(`${v}`, X(v) - 8, H - 14);
    ctx.fillStyle = pal.muted;
    ctx.fillText(sgn(cum), Math.min(X(v) + 6, W - 45), Y(cum) - 6);
  }
}
