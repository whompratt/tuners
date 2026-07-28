// The bundled car dataset (ordinal -> name), fetched once per app run and
// shared by every picker. Filtering is pure so it can be unit-tested.

import { commands, type CarView } from "./bindings";

let cache: Promise<CarView[]> | null = null;

export function allCars(): Promise<CarView[]> {
  cache ??= commands.carList();
  return cache;
}

/** Every whitespace-separated term must match somewhere in the name
 * ("gt 2005" finds "2005 Ford GT"). Empty query = no matches: the picker
 * shows results only once the user starts typing. */
export function filterCars(cars: CarView[], query: string, limit = 8): CarView[] {
  const terms = query.trim().toLowerCase().split(/\s+/).filter(Boolean);
  if (!terms.length) return [];
  const out: CarView[] = [];
  for (const c of cars) {
    const name = c.name.toLowerCase();
    if (terms.every((t) => name.includes(t))) {
      out.push(c);
      if (out.length >= limit) break;
    }
  }
  return out;
}
