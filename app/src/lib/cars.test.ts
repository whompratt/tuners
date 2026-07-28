// Car search: every typed term must match, in any order, case-insensitively.

import { describe, expect, it } from "vitest";
import { filterCars } from "./cars";

const CARS = [
  { car: 348, name: "2005 Ford GT" },
  { car: 2937, name: "2017 Ford Fiesta GRC" },
  { car: 1478, name: "1986 Audi Sport Quattro S2" },
  { car: 3405, name: "2018 McLaren 570S Coupe" },
];

describe("filterCars", () => {
  it("matches all terms in any order, ignoring case", () => {
    expect(filterCars(CARS, "gt ford").map((c) => c.car)).toEqual([348]);
    expect(filterCars(CARS, "FORD")).toHaveLength(2);
    expect(filterCars(CARS, "quattro 1986").map((c) => c.car)).toEqual([1478]);
  });

  it("returns nothing for an empty or unmatched query", () => {
    expect(filterCars(CARS, "")).toEqual([]);
    expect(filterCars(CARS, "   ")).toEqual([]);
    expect(filterCars(CARS, "ferrari")).toEqual([]);
  });

  it("caps the result list", () => {
    expect(filterCars(CARS, "20", 2)).toHaveLength(2);
  });
});
