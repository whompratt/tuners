// Unit-conversion suite.
// Storage is canonical imperial; display prefs must round-trip losslessly
// enough that repeated open/save cycles never drift a value.

import { beforeEach, describe, expect, it } from "vitest";
import {
  UNIT_PRESETS,
  UNIVERSAL_LIMITS,
  activePreset,
  fmtLap,
  limToCanon,
  limToDisp,
  toCanon,
  toDisp,
  unitLabel,
  unitPrefs,
} from "./units";

beforeEach(() => {
  Object.assign(unitPrefs, UNIT_PRESETS.imperial);
});

describe("activePreset", () => {
  it("names the preset that matches every dimension", () => {
    expect(activePreset({ ...UNIT_PRESETS.imperial })).toBe("imperial");
    expect(activePreset({ ...UNIT_PRESETS.metric })).toBe("metric");
    expect(activePreset({ ...UNIT_PRESETS.uk })).toBe("uk");
  });

  it("reads a per-dimension mix as no preset", () => {
    expect(activePreset({ ...UNIT_PRESETS.uk, temp: "f" })).toBe(null);
  });
});

describe("toDisp/toCanon", () => {
  it("is identity under imperial (canonical) prefs", () => {
    expect(toDisp("tire_pressure_f", "30")).toBe("30");
    expect(toCanon("tire_pressure_f", "30")).toBe("30");
    expect(toDisp("springs_f", "550")).toBe("550");
    expect(toDisp("weight", "2622")).toBe("2622");
  });

  it("passes non-numeric and unitless values through untouched", () => {
    expect(toDisp("tire_pressure_f", "")).toBe("");
    expect(toDisp("arb_f", "18.5")).toBe("18.5"); // no display unit
    expect(toCanon("final_drive", "3.95")).toBe("3.95");
  });

  it("stringifies numbers (bind:value on number inputs stores numbers)", () => {
    // Regression: a raw number reaching the IPC boundary fails Rust-side
    // deserialization (Vec<(String, String)>) with an opaque reject.
    expect(toCanon("front_weight_pct", 50 as unknown as string)).toBe("50");
    expect(toCanon("weight", 2622 as unknown as string)).toBe("2622");
    expect(toDisp("arb_f", 18.5 as unknown as string)).toBe("18.5");
  });

  it("round-trips every dimensioned field under metric prefs", () => {
    Object.assign(unitPrefs, UNIT_PRESETS.metric);
    for (const [key, canonical] of [
      ["tire_pressure_f", "28.5"],
      ["springs_r", "612"],
      ["ride_height_f", "4.1"],
      ["aero_r", "210"],
      ["weight", "2622"],
    ] as const) {
      const disp = toDisp(key, canonical);
      expect(disp).not.toBe(canonical); // actually converted
      const back = parseFloat(toCanon(key, disp));
      // Display rounding (dp) bounds the round-trip error.
      expect(Math.abs(back - parseFloat(canonical))).toBeLessThan(
        parseFloat(canonical) * 0.02 + 0.5,
      );
    }
  });

  it("matches the UK preset's mixed units", () => {
    Object.assign(unitPrefs, UNIT_PRESETS.uk);
    expect(unitLabel("tire_pressure_f")).toBe(" (psi)"); // psi stays imperial
    expect(unitLabel("springs_f")).toBe(" (kgf/mm)");
    expect(unitLabel("weight")).toBe(" (kg)");
    expect(toDisp("weight", "2622")).toBe("1189"); // the McLaren's real weight
  });
});

describe("slider limits", () => {
  it("parses min..max in display units to canonical", () => {
    expect(limToCanon("arb_f", "1..65")).toBe("1..65");
    expect(limToCanon("arb_f", " 1 .. 65 ")).toBe("1..65");
    expect(limToCanon("camber_f", "-5..5")).toBe("-5..5");
  });

  it("rejects malformed ranges", () => {
    expect(limToCanon("arb_f", "1-65")).toBe("");
    expect(limToCanon("arb_f", "lots")).toBe("");
  });

  it("converts each side under display prefs", () => {
    Object.assign(unitPrefs, UNIT_PRESETS.metric);
    const canon = limToCanon("tire_pressure_f", "1..3.8"); // bar
    const [lo, hi] = canon.split("..").map(parseFloat);
    expect(lo).toBeCloseTo(14.5, 0);
    expect(hi).toBeCloseTo(55.1, 0);
    expect(limToDisp("tire_pressure_f", canon)).toBe("1..3.8");
  });

  it("keeps typed decimals on ends even when the dimension displays whole numbers", () => {
    // aero displays whole kgf under UK prefs (dp 0); a typed limit like
    // 150.5 must survive the canonical round-trip, not reseed as "151".
    Object.assign(unitPrefs, UNIT_PRESETS.uk);
    const canon = limToCanon("aero_f", "150.5..332");
    expect(limToDisp("aero_f", canon)).toBe("150.5..332");
    const rh = limToCanon("ride_height_f", "12.55..15");
    expect(limToDisp("ride_height_f", rh)).toBe("12.55..15");
  });

  it("knows the universal FH6 slider ranges", () => {
    expect(UNIVERSAL_LIMITS("tire_pressure_f")).toBe("15..55");
    expect(UNIVERSAL_LIMITS("arb_r")).toBe("1..65");
    expect(UNIVERSAL_LIMITS("springs_f")).toBeNull(); // car-specific
  });
});

describe("fmtLap", () => {
  it("formats sub-minute and over-minute laps", () => {
    expect(fmtLap(44.995)).toBe("44.995");
    expect(fmtLap(105.5)).toBe("1:45.500");
    expect(fmtLap(60)).toBe("1:00.000");
  });
});
