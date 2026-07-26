// First-run onboarding (plan 010 phase 4): a one-time wizard shown when the
// app boots with no project. Completion (or skipping) persists; users with an
// existing project never see it. Reopenable from Home's no-project state.

const KEY = "tuners-onboarded";

export const onboarding = $state({ open: false });

/** Called once after boot. An existing project counts as onboarded. */
export function initOnboarding(hasProject: boolean) {
  if (localStorage.getItem(KEY) === "1") return;
  if (hasProject) {
    localStorage.setItem(KEY, "1");
    return;
  }
  onboarding.open = true;
}

export function finishOnboarding() {
  onboarding.open = false;
  localStorage.setItem(KEY, "1");
}

export function reopenOnboarding() {
  onboarding.open = true;
}
