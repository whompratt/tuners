// Startup update check against the GitHub releases latest.json feed.
// Failures are silent by design: offline, a draft-only release history, or a
// bundle format without updater support (deb/rpm) must never block launch.
import { check } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { confirmDialog } from "$lib/ui/dialogs.svelte";

export async function checkForUpdate(): Promise<void> {
  if (import.meta.env.DEV) return;
  try {
    const update = await check();
    if (!update) return;
    const ok = await confirmDialog({
      title: "Update available",
      body: `tuners ${update.version} is available (you have ${update.currentVersion}). Download and install it now? The app restarts once it's done.`,
      verb: "Update",
      cancel: "Not now",
    });
    if (!ok) return;
    await update.downloadAndInstall();
    // On Windows the installer exits the app itself; elsewhere we relaunch.
    await relaunch();
  } catch {
    // ignore — try again next launch
  }
}
