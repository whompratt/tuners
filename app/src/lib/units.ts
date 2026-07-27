// Units: storage is canonical imperial (FH6 internals); prefs are pure
// display formatting, switchable any time. k converts canonical -> display.
// Ported 1:1 from the old dashboard.

export type UnitDef = { k: number; dp: number; l: string };

export const UNITS: Record<string, Record<string, UnitDef>> = {
  pressure: { psi: { k: 1, dp: 1, l: "psi" }, bar: { k: 0.0689476, dp: 2, l: "bar" } },
  springs: { lbin: { k: 1, dp: 0, l: "lb/in" }, kgfmm: { k: 0.017858, dp: 2, l: "kgf/mm" } },
  length: { in: { k: 1, dp: 1, l: "in" }, cm: { k: 2.54, dp: 1, l: "cm" } },
  force: { lb: { k: 1, dp: 0, l: "lb" }, kgf: { k: 0.453592, dp: 0, l: "kgf" } },
  mass: { lb: { k: 1, dp: 0, l: "lb" }, kg: { k: 0.453592, dp: 0, l: "kg" } },
  speed: { mph: { k: 2.23694, dp: 0, l: "mph" }, kmh: { k: 3.6, dp: 0, l: "km/h" } }, // from m/s
};

export const FIELD_UNIT: Record<string, string> = {
  tire_pressure_f: "pressure",
  tire_pressure_r: "pressure",
  springs_f: "springs",
  springs_r: "springs",
  ride_height_f: "length",
  ride_height_r: "length",
  aero_f: "force",
  aero_r: "force",
  weight: "mass",
};

export const UNIT_PRESETS: Record<string, Record<string, string>> = {
  imperial: { pressure: "psi", springs: "lbin", length: "in", force: "lb", mass: "lb", speed: "mph", temp: "f" },
  metric: { pressure: "bar", springs: "kgfmm", length: "cm", force: "kgf", mass: "kg", speed: "kmh", temp: "c" },
  uk: { pressure: "psi", springs: "kgfmm", length: "cm", force: "kgf", mass: "kg", speed: "mph", temp: "c" },
};

export const UNIT_DIMS: [string, string][] = [
  ["pressure", "tire pressure"],
  ["springs", "spring rate"],
  ["length", "ride height"],
  ["force", "aero"],
  ["mass", "weight"],
  ["speed", "speed"],
  ["temp", "temperature"],
];

// Reactive unit preferences (Svelte 5 runes in a .ts module are not allowed,
// so this is a plain mutable object; panels re-render off explicit signals).
export const unitPrefs: Record<string, string> = { ...UNIT_PRESETS.imperial };

export const unitOf = (key: string): UnitDef | null => {
  const dim = FIELD_UNIT[key];
  if (!dim) return null;
  return UNITS[dim][unitPrefs[dim]] || UNITS[dim][Object.keys(UNITS[dim])[0]];
};

const trimZeros = (s: string) => s.replace(/\.0+$/, "").replace(/(\.\d*?)0+$/, "$1");

// canonical string -> display string (for form/summary), and back.
// Accepts numbers too: Svelte's bind:value on type="number" inputs stores
// numbers at runtime, and the IPC contract is strings — everything that
// crosses the boundary must come through here already stringified.
export const toDisp = (key: string, v: string | number): string => {
  const u = unitOf(key);
  const n = parseFloat(String(v));
  return !u || isNaN(n) ? String(v ?? "") : trimZeros((n * u.k).toFixed(u.dp));
};
export const toCanon = (key: string, v: string | number): string => {
  const u = unitOf(key);
  const n = parseFloat(String(v));
  return !u || isNaN(n) ? String(v ?? "") : String(Math.round((n / u.k) * 1e4) / 1e4);
};
export const unitLabel = (key: string): string => {
  const u = unitOf(key);
  return u ? ` (${u.l})` : "";
};

// slider ranges fixed across cars (FH6 UI); car-specific fields need input
export const UNIVERSAL_LIMITS = (k: string): string | null =>
  k.startsWith("tire_pressure") ? "15..55"
  : k === "final_drive" ? "2.2..6.1"
  : k.startsWith("gear_") ? "0.48..6"
  : k === "caster" ? "1..7"
  : /^(camber|toe)_/.test(k) ? "-5..5"
  : /^arb_/.test(k) ? "1..65"
  : /^(rebound|bump)_/.test(k) ? "1..20"
  : k === "brake_pressure" ? "0..200"
  : /^diff_/.test(k) || k === "brake_balance" ? "0..100"
  : null;

// slider-limit facts hold "min..max" in canonical units; convert per side
export const limToDisp = (k: string, v: string): string =>
  v ? v.split("..").map((x) => toDisp(k, x)).join("..") : "";
export const limToCanon = (k: string, v: string): string => {
  const m = v.trim().match(/^(-?[\d.]+)\s*\.\.\s*(-?[\d.]+)$/);
  return m ? `${toCanon(k, m[1])}..${toCanon(k, m[2])}` : "";
};

// telemetry display: speed from m/s, temps from °F
export const SPD = (): UnitDef => UNITS.speed[unitPrefs.speed] || UNITS.speed.mph;
export const tempDisp = (f: number): number => (unitPrefs.temp === "c" ? (f - 32) / 1.8 : f);
export const tempLabel = (): string => (unitPrefs.temp === "c" ? "°C" : "°F");

export const fmtLap = (s: number): string => {
  const ms = Math.round(s * 1000),
    m = Math.floor(ms / 60000),
    r = ms % 60000;
  const sec = (r / 1000).toFixed(3).padStart(6, "0");
  return m > 0 ? `${m}:${sec}` : (r / 1000).toFixed(3);
};

export const fmtClock = (s: number): string =>
  s >= 60 ? `${Math.floor(s / 60)}:${(s % 60).toFixed(1).padStart(4, "0")}` : `${s.toFixed(1)}s`;

// reference dark categorical palette (validated, fixed order — never cycled)
export const SERIES = ["#3987e5", "#008300", "#d55181", "#c98500", "#199e70", "#d95926", "#9085e9", "#e66767"];
