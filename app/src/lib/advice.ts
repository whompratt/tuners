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

/** Bracket tag for a recommendation: probes are data requests and
 * explores are crowd-led experiments, not detection claims; holds ask for
 * nothing, so confidence there would be noise. The tag says which. */
export const confTag = (r: RecommendationView) =>
  r.kind === "hold"
    ? "hold"
    : r.probe
      ? `probe: ${r.confidence}`
      : r.kind === "explore"
        ? `explore: ${r.confidence}`
        : r.confidence;

export const isHold = (r: RecommendationView) => r.kind === "hold";

// Mirrors Kind::rank_group: fixes and hones pool (confidence decides),
// explores backstop, holds sink.
const KIND_RANK: Record<string, number> = { fix: 0, hone: 0, explore: 1, hold: 2 };

/** The one next action: highest-tier actionable suggestion that is not a
 * hold and not already accepted (the backend list is already tier-then-
 * confidence sorted); falls back to the top recommendation. */
export function primaryRec(
  a: AdviseView | null,
  latest: Record<string, string> | null | undefined,
): RecommendationView | null {
  if (!a || !a.recommendations.length) return null;
  const actionable = a.recommendations
    .filter((r) => r.apply.length && !isHold(r) && !isAccepted(r.apply as [string, string][], latest))
    .sort(
      (x, y) =>
        (KIND_RANK[x.kind] ?? 4) - (KIND_RANK[y.kind] ?? 4) ||
        (CONF_RANK[x.confidence] ?? 3) - (CONF_RANK[y.confidence] ?? 3),
    );
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
