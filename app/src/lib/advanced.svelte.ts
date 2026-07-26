// Advanced mode (plan 010): ONE boolean that only changes default expansion
// state — purely presentational. Persisted so the choice survives restarts.

const KEY = "tuners-advanced";

export const advanced = $state({ on: false });

export function initAdvanced() {
  advanced.on = localStorage.getItem(KEY) === "1";
}

export function toggleAdvanced() {
  advanced.on = !advanced.on;
  localStorage.setItem(KEY, advanced.on ? "1" : "0");
}
