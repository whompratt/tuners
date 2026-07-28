<script lang="ts">
  import { goto } from "$app/navigation";
  import { app, errMsg, loadSession } from "$lib/app.svelte";
  import { commands, type EffectMapStatus, type SharingView } from "$lib/bindings";
  import { UNITS, UNIT_DIMS, UNIT_PRESETS, unitPrefs } from "$lib/units";
  import { advanced, toggleAdvanced } from "$lib/advanced.svelte";
  import { reopenOnboarding } from "$lib/onboarding.svelte";
  import { alertDialog, confirmDialog } from "$lib/ui/dialogs.svelte";
  import Button from "$lib/ui/Button.svelte";

  // --- display units (stored on the active project so exports/analysis and
  // new projects inherit them; the engine's storage stays canonical) ---
  let units: Record<string, string> = $state({});
  $effect(() => {
    void app.unitsTick;
    const u: Record<string, string> = {};
    for (const [dim] of UNIT_DIMS) u[dim] = unitPrefs[dim];
    units = u;
  });
  async function saveUnits() {
    const facts: [string, string][] = UNIT_DIMS.map(([dim]) => [`unit_${dim}`, units[dim]]);
    const r = await commands.updateSession(false, null, facts);
    if (r.status === "error") {
      await alertDialog("Display units", errMsg(r.error));
      return;
    }
    await loadSession();
  }
  function applyPreset(p: string) {
    for (const [dim, u] of Object.entries(UNIT_PRESETS[p])) units[dim] = u;
    saveUnits();
  }

  // --- telemetry sharing (moved here from Projects) ---
  let sharing: SharingView | null = $state(null);
  let sharingError = $state("");
  let sharingOverride = $state("");

  // --- effect map (built in the background from all tuning history) ---
  let mapStatus: EffectMapStatus | null | undefined = $state(undefined);

  $effect(() => {
    refreshSharing();
    commands
      .effectMapStatus()
      .then((s) => (mapStatus = s))
      .catch(() => (mapStatus = null));
  });

  async function refreshSharing() {
    try {
      sharing = await commands.sharing();
      sharingError = "";
    } catch (e) {
      sharing = null;
      sharingError = String(e);
    }
  }

  async function toggleSharing() {
    try {
      const d = await commands.sharing();
      let discard = false;
      if (
        d.enabled &&
        d.queued &&
        (await confirmDialog({
          title: "Telemetry sharing",
          body: `${d.queued} bundle(s) are still queued.\n\nDelete them, or keep them? Kept bundles upload if you re-enable.`,
          verb: "Delete queued",
          cancel: "Keep queued",
          danger: true,
        }))
      ) {
        discard = true;
      }
      const r = await commands.setSharing(!d.enabled, null, discard);
      if (r.status === "error") {
        await alertDialog("Telemetry sharing", errMsg(r.error));
        return;
      }
      await refreshSharing();
    } catch (e) {
      await alertDialog("Telemetry sharing", `toggle failed: ${e}`);
    }
  }

  async function shareHistory() {
    try {
      const p = await commands.sharingHistoryPlan();
      const skipped: string[] = [];
      if (p.already) skipped.push(`${p.already} already shared/queued`);
      if (p.unjournaled) skipped.push(`${p.unjournaled} without setup history`);
      if (!p.stints) {
        await alertDialog("Share existing recordings", `Nothing new to share${skipped.length ? ` (skipped: ${skipped.join(", ")})` : ""}.`);
        return;
      }
      const msg =
        `Share ${p.stints} recording${p.stints === 1 ? "" : "s"} from ` +
        `${p.campaigns} project${p.campaigns === 1 ? "" : "s"} (~${(p.mb ?? 0).toFixed(1)} MB raw)` +
        ` recorded before sharing was enabled?\n\n` +
        `Same rules as live sharing: raw telemetry, setup values, and setup deltas only;` +
        ` names, descriptions, and notes are stripped. Uploads happen in the` +
        ` background while telemetry is idle.` +
        (skipped.length ? `\n\n(Skipped: ${skipped.join(", ")}.)` : "");
      if (!(await confirmDialog({ title: "Share existing recordings", body: msg, verb: "Share", cancel: "Cancel" }))) return;
      const r = await commands.shareHistory();
      if (r.status === "error") {
        await alertDialog("Share existing recordings", errMsg(r.error));
        return;
      }
      sharingOverride = `bundling ${r.data} recording(s) in the background…`;
      setTimeout(() => {
        sharingOverride = "";
        refreshSharing();
      }, 4000);
    } catch (e) {
      await alertDialog("Share existing recordings", `failed: ${e}`);
    }
  }

  let sharingStatus = $derived.by(() => {
    if (!sharing) return sharingError ? `status unavailable (${sharingError})` : "";
    const bits: string[] = [];
    if (sharing.enabled) {
      bits.push(`on, sender ${sharing.sender}`);
      bits.push(`${sharing.queued} bundle${sharing.queued === 1 ? "" : "s"} queued`);
      if (sharing.rejected) bits.push(`${sharing.rejected} rejected (see outbox/rejected)`);
    } else {
      bits.push("off");
      if (sharing.queued) bits.push(`${sharing.queued} still queued from before`);
    }
    return bits.join(" · ");
  });

  function openGuide() {
    reopenOnboarding();
    goto("/");
  }
</script>

<div class="screen">
  <div class="panel">
    <div style="display:flex;gap:8px;align-items:baseline;flex-wrap:wrap;margin-bottom:8px">
      <h2 style="margin:0">Display units</h2>
      <span style="color:var(--muted);font-size:13px">
        display only: values are stored in the game's native units, switch any time
      </span>
    </div>
    <div style="display:flex;gap:8px;align-items:baseline;margin-bottom:10px">
      <Button onclick={() => applyPreset("imperial")}>imperial</Button>
      <Button onclick={() => applyPreset("metric")}>metric</Button>
      <Button onclick={() => applyPreset("uk")}>UK</Button>
    </div>
    <div class="form-grid" style="max-width:760px">
      {#each UNIT_DIMS as [dim, l] (dim)}
        <div>
          <label for="set-{dim}">{dim === "temp" ? "temperature" : l}</label>
          {#if dim === "temp"}
            <select id="set-{dim}" bind:value={units[dim]} onchange={saveUnits}>
              <option value="f">°F</option>
              <option value="c">°C</option>
            </select>
          {:else}
            <select id="set-{dim}" bind:value={units[dim]} onchange={saveUnits}>
              {#each Object.entries(UNITS[dim]) as [u, d] (u)}
                <option value={u}>{d.l}</option>
              {/each}
            </select>
          {/if}
        </div>
      {/each}
    </div>
  </div>

  <div class="panel">
    <div style="display:flex;gap:10px;align-items:baseline;flex-wrap:wrap">
      <h2 style="margin:0">Telemetry sharing</h2>
      <span style="color:var(--muted);font-size:13px">{sharingOverride || sharingStatus}</span>
      <span style="flex:1"></span>
      <Button
        disabled={!sharing?.enabled}
        title={sharing?.enabled
          ? "bundle recordings from before sharing was enabled (asks first)"
          : "turn on sharing first"}
        onclick={shareHistory}>share existing recordings…</Button>
      <Button onclick={toggleSharing}>{sharing?.enabled ? "turn off" : "turn on sharing"}</Button>
    </div>
    <div style="color:var(--muted);font-size:12px;margin-top:6px;max-width:640px">
      When on, each finished run is bundled and uploaded (only while telemetry is idle) to help develop this
      tool: raw driving telemetry, setup values, and setup deltas. No names, no free text: project names,
      descriptions, and notes are stripped before anything leaves this machine. Off by default; turn it
      off any time. Quote your sender id to have your data deleted.
    </div>
  </div>

  <div class="panel">
    <div style="display:flex;gap:10px;align-items:baseline;flex-wrap:wrap">
      <h2 style="margin:0">Effect map</h2>
      <span style="color:var(--muted);font-size:13px">
        {#if mapStatus === undefined}
          &nbsp;
        {:else if mapStatus === null}
          not built yet — it appears once a project has measured tune changes
        {:else}
          {mapStatus.samples} measurement{mapStatus.samples === 1 ? "" : "s"} across
          {mapStatus.campaigns} campaign{mapStatus.campaigns === 1 ? "" : "s"}
          {#if mapStatus.updatedMs != null}
            · updated {new Date(mapStatus.updatedMs).toLocaleString()}
          {/if}
        {/if}
      </span>
    </div>
    <div style="color:var(--muted);font-size:12px;margin-top:6px;max-width:640px">
      Every measured tune change across your projects (and any shared data you ingest) is pooled into a
      map of what each adjustment does. Advice uses it to suggest untried adjustments, clearly marked as
      coming from the map rather than from this project's own runs. Kept fresh automatically in the
      background.
    </div>
  </div>

  <div class="panel">
    <h2 style="margin:0 0 8px">Interface</h2>
    <label style="display:flex;align-items:center;gap:8px;font-size:14px;color:var(--ink-2);cursor:pointer">
      <input type="checkbox" style="width:auto" checked={advanced.on} onchange={toggleAdvanced} />
      advanced mode: expand expert detail (evidence, measurements, slider ranges) by default
    </label>
    <div style="margin-top:12px">
      <Button onclick={openGuide}>show the first-time setup guide again</Button>
    </div>
  </div>
</div>
