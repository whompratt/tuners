// Chart geometry suite, possible at all because layout is now pure
// (geometry/sizing split).

import { describe, expect, it } from "vitest";
import { lapsBinAt, lapsLayout } from "./laps";
import { SEG_BINS, compareBinAt, compareLayout, type Cmp } from "./compare";
import { landscapeLayout, landscapeX, landscapeY } from "./landscape";

const laps = {
  binMeters: 100,
  bestTime: 44.7,
  corroborated: Array(50).fill(true),
  laps: [
    { lap: 2, time: 45.0, standing: false, speeds: Array(50).fill(60) },
    { lap: 3, time: 44.7, standing: false, speeds: Array(50).fill(62) },
  ],
};

describe("laps layout", () => {
  it("derives bins, extent, and headroom-rounded y max", () => {
    const L = lapsLayout(laps, 2.23694, 800);
    expect(L.bins).toBe(50);
    expect(L.kmMax).toBeCloseTo(5.0);
    expect(L.plotW).toBe(800 - L.pad.l - L.pad.r);
    // 62 m/s * 2.23694 = 138.7 mph -> rounded up to 140
    expect(L.yMax).toBe(140);
  });

  it("maps cursor x to bins only inside the plot", () => {
    const L = lapsLayout(laps, 1, 800);
    expect(lapsBinAt(L, L.pad.l - 5)).toBeNull();
    expect(lapsBinAt(L, L.pad.l)).toBe(0);
    expect(lapsBinAt(L, L.pad.l + L.plotW)).toBe(49);
    expect(lapsBinAt(L, L.pad.l + L.plotW / 2)).toBe(Math.round(49 / 2));
  });
});

const cmp: Cmp = {
  binMeters: 100,
  a: { file: "a.ftel", laps: 3, best: 45, median: 45.4, ideal: 44.5, standingOnly: false },
  b: { file: "b.ftel", laps: 3, best: 44.8, median: 45.1, ideal: 44.2, standingOnly: false },
  speedsA: Array(60).fill(50),
  speedsB: Array(60).fill(52),
  timesA: Array(60).fill(0.1),
  delta: Array(60).fill(-0.01),
  verdictDeltaS: -0.3,
  currencies: [-0.3, -0.2, -0.3],
  unequalLaps: false,
  carMismatch: false,
};

describe("compare layout", () => {
  it("segments the delta into 250m bars and accumulates the gap", () => {
    const L = compareLayout(cmp, 1, 900);
    expect(L.segs.length).toBe(Math.ceil(60 / SEG_BINS));
    expect(L.segs[0]).toBeCloseTo(-0.25);
    expect(L.cum[59]).toBeCloseTo(-0.6);
    expect(L.cumMax).toBeCloseTo(0.6);
    // floor keeps tiny deltas from exploding the bar scale
    expect(L.segMax).toBeGreaterThanOrEqual(0.05);
  });

  it("maps cursor x to bins only inside the plot", () => {
    const L = compareLayout(cmp, 1, 900);
    expect(compareBinAt(L, 0)).toBeNull();
    expect(compareBinAt(L, L.pad.l)).toBe(0);
    expect(compareBinAt(L, L.pad.l + L.plotW)).toBe(59);
  });
});

describe("landscape layout", () => {
  const data = {
    nodes: [
      [17, -0.16],
      [18, -0.49],
      [18.5, -0.61],
      [19.5, -0.2],
      [20.7, 0],
    ] as [number, number][],
    fit: null,
    vertex: 18.5,
  };

  it("frames the tried values with zero always in view", () => {
    const L = landscapeLayout(data, 720);
    expect(L.x0).toBe(17);
    expect(L.x1).toBe(20.7);
    expect(L.y0).toBe(-0.61);
    expect(L.y1).toBe(0); // zero line included even when all deltas negative
  });

  it("maps values monotonically into the plot", () => {
    const L = landscapeLayout(data, 720);
    expect(landscapeX(L, 17)).toBeLessThan(landscapeX(L, 20.7));
    // down = faster: more negative delta renders LOWER (greater y)
    expect(landscapeY(L, -0.61)).toBeGreaterThan(landscapeY(L, 0));
  });

  it("pads degenerate (single-value or flat) inputs", () => {
    const L = landscapeLayout({ nodes: [[18, 0], [18, 0]], fit: null, vertex: null }, 720);
    expect(L.padX).toBe(1);
    expect(L.padY).toBe(0.05);
  });
});
