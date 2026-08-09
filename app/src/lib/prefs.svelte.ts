// Interface preferences: purely presentational booleans, persisted per
// install so the choice survives restarts.

const EXPAND_KEY = "tuners-expand-advice";
// Pre-rename key ("advanced mode"); read once as a fallback so existing
// installs keep their choice.
const LEGACY_KEY = "tuners-advanced";

export const prefs = $state({ expandAdvice: false });

export function initPrefs() {
  prefs.expandAdvice =
    (localStorage.getItem(EXPAND_KEY) ?? localStorage.getItem(LEGACY_KEY)) === "1";
}

export function toggleExpandAdvice() {
  prefs.expandAdvice = !prefs.expandAdvice;
  localStorage.setItem(EXPAND_KEY, prefs.expandAdvice ? "1" : "0");
}
