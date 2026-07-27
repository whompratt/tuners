// Shared app state + cross-panel actions (plan 010 phase 1b). One $state
// object mirrors what the old dashboard kept in module-level lets; panels
// read it reactively and call the actions below.

import {
  commands,
  type AdviseView,
  type ApiError,
  type LapsView,
  type LiveStateView,
  type PendingView,
  type QualityView,
  type SessionView,
  type StintRow,
} from "./bindings";

// f32 crosses IPC as `number | null` (NaN serializes to null); chart data is
// sanitized once here so panel math works on plain numbers.
export type Laps = {
  binMeters: number;
  bestTime: number;
  corroborated: boolean[];
  laps: { lap: number; time: number; standing: boolean; speeds: number[] }[];
};
const num = (v: number | null) => v ?? 0;
const sanitizeLaps = (v: LapsView): Laps => ({
  binMeters: num(v.binMeters),
  bestTime: num(v.bestTime),
  corroborated: v.corroborated,
  laps: v.laps.map((l) => ({ lap: l.lap, time: num(l.time), standing: l.standing, speeds: l.speeds.map(num) })),
});
import { UNIT_DIMS, unitPrefs } from "./units";
import { alertDialog, confirmDialog } from "./ui/dialogs.svelte";
import { save } from "@tauri-apps/plugin-dialog";

export const app = $state({
  /** Initial session/stints load finished — before this, "no project" is
   * unknowable, not true, and screens must not claim it. */
  booted: false,
  /** Home starts on a landing gate each launch: the active project is named
   * but not entered until the user says so. Engine-side the project stays
   * active regardless (recording continuity). Session-lifetime only. */
  entered: false,
  stints: [] as StintRow[],
  session: null as SessionView | null,
  live: null as LiveStateView | null,
  quality: null as QualityView | null,
  qualitySeen: false,
  selA: null as string | null,
  selB: null as string | null,
  shownFile: null as string | null,
  report: "select a stint",
  reportPlaceholder: true,
  lapData: null as Laps | null,
  carFilter: "all",
  showEarlierStints: false,
  // bumped whenever unit prefs change so unit-dependent markup re-renders
  unitsTick: 0,
  // Advice is app-wide state: Home renders it imperatively, Setup spatially,
  // Analysis evidentially — one reconciled list, three registers.
  advice: null as AdviseView | null,
  adviceError: "",
  adviceLoading: false,
  // The pending basket: tune edits saved since the last driven run.
  pending: null as PendingView | null,
});

// Failures reaching the frontend are usually typed ApiErrors, but an invoke
// can also reject with a plain string (e.g. argument deserialization) — a
// dialog must never render an empty body.
export const errMsg = (e: ApiError | string | unknown): string =>
  typeof e === "string" ? e : ((e as ApiError)?.message ?? String(e));

export async function loadAdvice() {
  app.adviceLoading = true;
  app.adviceError = "";
  // Fresh session state first: accepted-value detection compares against the
  // LATEST version, which an accept just changed.
  await loadSession();
  await loadPending();
  const r = await commands.advise();
  app.adviceLoading = false;
  if (r.status === "error") {
    app.advice = null;
    app.adviceError = errMsg(r.error);
    return;
  }
  app.advice = r.data;
}

export async function loadPending() {
  app.pending = await commands.pending();
}

export async function loadStints(resetFilter: boolean) {
  app.stints = await commands.stints();
  if (!app.stints.length) return;
  // default: the latest session's car (list is filename-sorted = chronological)
  const valid = new Set(["all", ...app.stints.map((s) => String(s.car))]);
  if (resetFilter || !valid.has(app.carFilter)) {
    app.carFilter = String(app.stints[app.stints.length - 1].car);
  }
}

export async function loadSession() {
  app.session = await commands.session();
  for (const [dim] of UNIT_DIMS) {
    const v = app.session?.facts[`unit_${dim}`];
    if (v) unitPrefs[dim] = v;
  }
  app.unitsTick++;
  // Scope the stint list to the session's car by default.
  if (
    app.session?.car != null &&
    app.stints.some((s) => s.car === app.session!.car)
  ) {
    app.carFilter = String(app.session.car);
  }
}

export async function show(file: string) {
  app.shownFile = file;
  app.reportPlaceholder = false;
  app.report = "analyzing…";
  const [report, laps] = await Promise.all([commands.report(file), commands.laps(file)]);
  app.report = report.status === "ok" ? report.data : errMsg(report.error);
  app.lapData = laps.status === "ok" && laps.data.laps.length ? sanitizeLaps(laps.data) : null;
}

export function pick(side: "a" | "b", file: string) {
  if (side === "a") app.selA = app.selA === file ? null : file;
  else app.selB = app.selB === file ? null : file;
}

export async function deleteStint(file: string) {
  const name = file.split("/").pop()!;
  const ok = await confirmDialog({
    title: "Delete run",
    body: `Delete ${name}?\n\nThis cannot be undone.`,
    verb: "Delete",
    cancel: "Keep",
    danger: true,
  });
  if (!ok) return;
  let r = await commands.deleteStint(name, false);
  if (r.status === "error" && r.error.kind === "conflict") {
    // Journaled stint: the engine wants an explicit force. Advice survives
    // (the step is skipped, its note merged forward) but loses a measurement.
    const force = await confirmDialog({
      title: "Run is in the history",
      body: `${errMsg(r.error)}\n\nDelete anyway?`,
      verb: "Delete anyway",
      cancel: "Keep",
      danger: true,
    });
    if (!force) return;
    r = await commands.deleteStint(name, true);
  }
  if (r.status === "error") {
    await alertDialog("Delete failed", errMsg(r.error));
    return;
  }
  if (app.selA === file) app.selA = null;
  if (app.selB === file) app.selB = null;
  if (app.shownFile === file) {
    app.shownFile = null;
    app.report = "select a stint";
    app.reportPlaceholder = true;
    app.lapData = null;
  }
  await loadStints(true);
}

export async function exportBundle(file: string) {
  const name = file.split("/").pop()!;
  const dest = await save({ defaultPath: name.replace(/\.ftel$/, ".tar.zst") });
  if (!dest) return;
  const r = await commands.exportStint(name, dest);
  if (r.status === "error") await alertDialog("Export failed", errMsg(r.error));
}
