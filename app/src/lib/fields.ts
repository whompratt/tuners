// Tune/fact field definitions, grouped exactly like the game's tuning screens.
// Ported 1:1 from the old dashboard (plan 010 phase 1b).

export const TUNE_GROUPS: [string, [string, string][]][] = [
  ["Tire pressures", [["tire_pressure_f", "front"], ["tire_pressure_r", "rear"]]],
  [
    "Gearing",
    [
      ["final_drive", "final drive"],
      ["gear_1", "1st"], ["gear_2", "2nd"], ["gear_3", "3rd"], ["gear_4", "4th"],
      ["gear_5", "5th"], ["gear_6", "6th"], ["gear_7", "7th"], ["gear_8", "8th"],
      ["gear_9", "9th"], ["gear_10", "10th"],
    ],
  ],
  [
    "Alignment",
    [
      ["camber_f", "front camber"], ["camber_r", "rear camber"],
      ["toe_f", "front toe"], ["toe_r", "rear toe"], ["caster", "caster"],
    ],
  ],
  ["Anti-roll bars", [["arb_f", "front"], ["arb_r", "rear"]]],
  [
    "Springs",
    [
      ["springs_f", "front rate"], ["springs_r", "rear rate"],
      ["ride_height_f", "front ride height"], ["ride_height_r", "rear ride height"],
    ],
  ],
  [
    "Damping",
    [
      ["rebound_f", "front rebound"], ["rebound_r", "rear rebound"],
      ["bump_f", "front bump"], ["bump_r", "rear bump"],
    ],
  ],
  ["Aero", [["aero_f", "front"], ["aero_r", "rear"]]],
  ["Brakes", [["brake_balance", "balance %"], ["brake_pressure", "pressure %"]]],
  [
    "Differential",
    [
      ["diff_accel_f", "front accel %"], ["diff_decel_f", "front decel %"],
      ["diff_accel_r", "rear accel %"], ["diff_decel_r", "rear decel %"],
      ["diff_center", "center balance %"],
    ],
  ],
];

export const TUNE_FIELDS: [string, string][] = TUNE_GROUPS.flatMap(([, fields]) => fields);

// [key, label, type]: number | compound | check (stored as on/off)
export const FACT_FIELDS: [string, string, string][] = [
  ["front_weight_pct", "front weight %", "number"],
  ["weight", "weight", "number"],
  ["tire_compound", "tire compound", "compound"],
  ["abs", "ABS", "check"],
  ["tcs", "TCS", "check"],
  ["stability", "stability control", "check"],
];

export const COMPOUNDS = [
  "stock", "street", "sport", "semi-slick", "slick", "rally",
  "offroad", "snow", "drag", "drift", "vintage",
];

// Full names for summary chips, where the grouped form uses short labels.
const FULL_LABEL: Record<string, string> = {
  tire_pressure_f: "front tire pressure",
  tire_pressure_r: "rear tire pressure",
  arb_f: "front ARB",
  arb_r: "rear ARB",
  springs_f: "front springs",
  springs_r: "rear springs",
  aero_f: "front aero",
  aero_r: "rear aero",
  brake_balance: "brake balance",
  brake_pressure: "brake pressure",
  diff_accel_f: "front diff accel",
  diff_decel_f: "front diff decel",
  diff_accel_r: "rear diff accel",
  diff_decel_r: "rear diff decel",
  diff_center: "center diff",
};
["1st", "2nd", "3rd", "4th", "5th", "6th", "7th", "8th", "9th", "10th"].forEach(
  (o, i) => (FULL_LABEL[`gear_${i + 1}`] = `${o} gear`),
);

export const label = (fields: [string, string][] | [string, string, string][], key: string): string =>
  (fields === TUNE_FIELDS && FULL_LABEL[key]) ||
  ((fields as [string, string][]).find(([k]) => k === key) || [key, key])[1];
