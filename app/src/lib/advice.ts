// Register-selection helpers over the one reconciled recommendation list
// Home shows the primary entry imperatively, Setup shows each
// family spatially, Analysis shows everything evidentially.

import type { AdviseView, RecommendationView } from "./bindings";

const CONF_RANK: Record<string, number> = { high: 0, medium: 1, low: 2 };

/** A suggestion whose values already sit on the latest saved version is
 * accepted-but-undriven: pending, not re-acceptable. */
export function isAccepted(
  apply: [string, string][],
  latest: Record<string, string> | null | undefined,
): boolean {
  return (
    !!apply.length &&
    !!latest &&
    apply.every(([k, v]) => latest[k] != null && Math.abs(parseFloat(latest[k]) - parseFloat(v)) < 1e-3)
  );
}

/** Bracket tag for a recommendation: probes are data requests, not
 * optimization claims, so the tag carries that instead of the advice text. */
export const confTag = (r: RecommendationView) => (r.probe ? `probe: ${r.confidence}` : r.confidence);

export const isHold = (r: RecommendationView) => {
  const t = `${r.suggestion ?? ""} ${r.advice}`;
  return t.includes("hold") || t.includes("bracketed");
};

/** The one next action: best-confidence actionable suggestion that is not a
 * hold and not already accepted; falls back to the top recommendation. */
export function primaryRec(
  a: AdviseView | null,
  latest: Record<string, string> | null | undefined,
): RecommendationView | null {
  if (!a || !a.recommendations.length) return null;
  const actionable = a.recommendations
    .filter((r) => r.apply.length && !isHold(r) && !isAccepted(r.apply as [string, string][], latest))
    .sort((x, y) => (CONF_RANK[x.confidence] ?? 3) - (CONF_RANK[y.confidence] ?? 3));
  return actionable[0] ?? a.recommendations[0];
}

/** Engine area -> the game-screen family card it belongs on. */
export const AREA_GROUP: Record<string, string> = {
  "tire pressure": "Tire pressures",
  tires: "Tire pressures",
  gearing: "Gearing",
  alignment: "Alignment",
  balance: "Anti-roll bars",
  springs: "Springs",
  bottoming: "Springs",
  damping: "Damping",
  aero: "Aero",
  brakes: "Brakes",
  differential: "Differential",
  traction: "Differential",
  stability: "Differential",
};
