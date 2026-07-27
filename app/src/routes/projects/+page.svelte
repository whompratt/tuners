<script lang="ts">
  import { app, errMsg, loadAdvice, loadSession, loadStints } from "$lib/app.svelte";
  import { commands, type SessionsView, type SharingView } from "$lib/bindings";
  import { COMPOUNDS, FACT_FIELDS, label } from "$lib/fields";
  import { UNITS, UNIT_DIMS, UNIT_PRESETS, toCanon, toDisp, unitLabel, unitPrefs } from "$lib/units";
  import { alertDialog, confirmDialog } from "$lib/ui/dialogs.svelte";
  import Button from "$lib/ui/Button.svelte";

  let formOpen = $state(false);

  // --- project form state (facts + units) ---
  let ssName = $state("");
  let ssDescription = $state("");
  let ssCar = $state("");
  let ssCarManual = $state("");
  let ssFacts: Record<string, string> = $state({});
  let ssChecks: Record<string, boolean> = $state({});
  let ssUnits: Record<string, string> = $state({});

  // --- library + sharing state ---
  let list: SessionsView | null = $state(null);
  let listError = $state("");
  let sharing: SharingView | null = $state(null);
  let sharingError = $state("");
  let sharingOverride = $state("");
  let newName = $state("");

  let cars = $derived.by(() => {
    const m = new Map<number, string>();
    for (const st of app.stints) m.set(st.car, st.carName || `car #${st.car}`);
    return m;
  });
  let s = $derived(app.session);
  let active = $derived(!!s && s.car != null);
  let shownFacts = $derived(
    active && s
      ? Object.entries(s.facts).filter(
          ([k]) => !k.startsWith("unit_") && !k.startsWith("limit_") && k !== "name" && k !== "description",
        )
      : [],
  );

  $effect(() => {
    refreshList();
    refreshSharing();
  });

  async function refreshList() {
    try {
      list = await commands.sessions();
      listError = "";
    } catch (e) {
      list = null;
      listError = String(e);
    }
  }
  async function refreshSharing() {
    try {
      sharing = await commands.sharing();
      sharingError = "";
    } catch (e) {
      sharing = null;
      sharingError = String(e);
    }
  }

  function openForm() {
    const cur = s?.car;
    ssName = s?.facts.name || "";
    ssDescription = s?.facts.description || "";
    ssCar = cur != null && cars.has(cur) ? String(cur) : cars.size ? String([...cars.keys()][0]) : "";
    ssCarManual = cur != null && !cars.has(cur) ? String(cur) : "";
    ssFacts = {};
    ssChecks = {};
    for (const [k, , type] of FACT_FIELDS) {
      const v = s?.facts[k] || "";
      if (type === "check") ssChecks[k] = v === "on";
      else ssFacts[k] = type === "number" ? toDisp(k, v) : v;
    }
    ssUnits = {};
    for (const [dim] of UNIT_DIMS) ssUnits[dim] = unitPrefs[dim];
    formOpen = true;
  }

  // Unit pickers apply live: values re-convert through canonical so nothing
  // drifts; save persists, cancel reloads the saved prefs.
  function refreshFormUnits() {
    const canon: Record<string, string> = {};
    for (const [k, , type] of FACT_FIELDS) if (type === "number") canon[k] = toCanon(k, ssFacts[k] ?? "");
    for (const [dim] of UNIT_DIMS) unitPrefs[dim] = ssUnits[dim];
    for (const [k, , type] of FACT_FIELDS) if (type === "number") ssFacts[k] = toDisp(k, canon[k]);
    app.unitsTick++;
  }
  function applyPreset(preset: string) {
    for (const [dim, u] of Object.entries(UNIT_PRESETS[preset])) ssUnits[dim] = u;
    refreshFormUnits();
  }

  async function saveProject() {
    const facts: [string, string][] = [];
    facts.push(["name", ssName.trim()]);
    facts.push(["description", ssDescription.trim()]);
    // Units first, so toCanon below converts under the prefs being saved.
    for (const [dim] of UNIT_DIMS) {
      unitPrefs[dim] = ssUnits[dim];
      facts.push([`unit_${dim}`, ssUnits[dim]]);
    }
    for (const [k, , type] of FACT_FIELDS) {
      facts.push([k, type === "check" ? (ssChecks[k] ? "on" : "off") : toCanon(k, ssFacts[k] ?? "")]);
    }
    // bind:value on the number input stores a number — the IPC arg is a string.
    const r = await commands.updateSession(false, String(ssCarManual || ssCar), facts);
    if (r.status === "error") {
      await alertDialog("Save failed", errMsg(r.error));
      return;
    }
    formOpen = false;
    await loadSession();
    await refreshList();
  }

  async function newProject() {
    const r = await commands.newSession(null, newName.trim() || null, null);
    if (r.status === "error") {
      await alertDialog("New project failed", errMsg(r.error));
      return;
    }
    newName = "";
    await loadSession();
    await refreshList();
    openForm(); // straight into picking the car + facts for the new project
  }

  async function resumeProject(id: string) {
    const r = await commands.resumeSession(id);
    if (r.status === "error") {
      await alertDialog("Resume failed", errMsg(r.error));
      return;
    }
    await loadSession();
    await refreshList();
    await loadAdvice();
    await loadStints(true);
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
        await alertDialog("Share existing recordings", `Nothing new to share${skipped.length ? ` — skipped: ${skipped.join(", ")}` : ""}.`);
        return;
      }
      const msg =
        `Share ${p.stints} recording${p.stints === 1 ? "" : "s"} from ` +
        `${p.campaigns} project${p.campaigns === 1 ? "" : "s"} (~${(p.mb ?? 0).toFixed(1)} MB raw)` +
        ` recorded before sharing was enabled?\n\n` +
        `Same rules as live sharing: raw telemetry, setup values, and setup deltas only` +
        ` — names, descriptions, and notes are stripped. Uploads happen in the` +
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
      bits.push(`on — sender ${sharing.sender}`);
      bits.push(`${sharing.queued} bundle${sharing.queued === 1 ? "" : "s"} queued`);
      if (sharing.rejected) bits.push(`${sharing.rejected} rejected (see outbox/rejected)`);
    } else {
      bits.push("off");
      if (sharing.queued) bits.push(`${sharing.queued} still queued from before`);
    }
    return bits.join(" · ");
  });

  const rowName = (r: { name: string | null; carName: string | null; car: number | null }) =>
    r.name || r.carName || (r.car != null ? `car #${r.car}` : "(no car)");
  const rowMeta = (r: { name: string | null; carName: string | null; car: number | null; revisions: number; stints: number; id: string | null }) =>
    [
      r.name ? r.carName || (r.car != null ? `car #${r.car}` : null) : null,
      `${r.revisions} version${r.revisions === 1 ? "" : "s"}`,
      `${r.stints} run${r.stints === 1 ? "" : "s"}`,
      r.id,
    ]
      .filter(Boolean)
      .join(" · ");
</script>

<div class="screen">
  <div class="panel">
    <div style="display:flex;align-items:baseline;gap:12px;flex-wrap:wrap">
      <h2 style="margin:0">
        {active && s
          ? `Project — ${s.facts.name ? `${s.facts.name} · ` : ""}${s.carName || `car #${s.car}`}`
          : "Project"}
      </h2>
      <span style="color:var(--muted);font-size:13px">
        {active && s
          ? s.revisions
            ? `setup version ${s.revisions}`
            : "no setup on file yet — enter it in Setup"
          : "none active — a project is one build of one car"}
      </span>
      <span style="flex:1"></span>
      <Button onclick={openForm}>{active ? "project settings" : "set up project"}</Button>
    </div>
    <div style="margin-top:8px;font-size:13px;color:var(--ink-2)">
      {#key app.unitsTick}
        {#each shownFacts as [k, v] (k)}
          <span class="kv">{label(FACT_FIELDS, k)} <b>{toDisp(k, v)}{unitLabel(k).replace(/[()]/g, "")}</b></span>
        {/each}
      {/key}
    </div>

    {#if formOpen}
      <form style="margin-top:12px" onsubmit={(e) => e.preventDefault()}>
        <div style="font-size:13px;color:var(--muted);margin-bottom:8px">
          Pick the car this project is about — runs from other cars are still recorded, but stay outside the project.
          Start a NEW project whenever the car itself changes (upgrades/drivetrain), not just the setup.
        </div>
        <div class="form-grid">
          <div>
            <label for="ss-name">project name</label>
            <input id="ss-name" type="text" placeholder="e.g. rwd no-aero build" bind:value={ssName} />
          </div>
          <div>
            <label for="ss-description">description</label>
            <input id="ss-description" type="text" placeholder="optional — notes to find this project later" bind:value={ssDescription} />
          </div>
          <div>
            <label for="ss-car">car (from recorded runs)</label>
            <select id="ss-car" bind:value={ssCar} disabled={!cars.size}>
              {#if !cars.size}
                <option value="">no recorded runs yet</option>
              {:else}
                {#each cars as [o, n] (o)}
                  <option value={String(o)}>{n}</option>
                {/each}
              {/if}
            </select>
            {#if !cars.size}
              <div style="font-size:12px;color:var(--muted);margin-top:2px">
                this list fills from recordings — drive once with the app open, or enter the ordinal manually
              </div>
            {/if}
          </div>
          <div>
            <label for="ss-car-manual">or car ordinal (manual)</label>
            <input id="ss-car-manual" type="number" bind:value={ssCarManual} />
          </div>
          {#each FACT_FIELDS as [k, l, type] (k)}
            {#if type === "check"}
              <div style="display:flex;align-items:flex-end;padding-bottom:4px">
                <label style="margin:0;display:flex;align-items:center;gap:6px;font-size:14px;color:var(--ink-2)">
                  <input type="checkbox" style="width:auto" bind:checked={ssChecks[k]} />
                  {l}
                </label>
              </div>
            {:else if type === "compound"}
              <div>
                <label for="ss-{k}">{l}</label>
                <select id="ss-{k}" bind:value={ssFacts[k]}>
                  <option value=""></option>
                  {#each COMPOUNDS as c (c)}
                    <option value={c}>{c}</option>
                  {/each}
                </select>
              </div>
            {:else}
              <div>
                {#key app.unitsTick}
                  <label for="ss-{k}">{l}{unitLabel(k)}</label>
                {/key}
                <input id="ss-{k}" type="number" step="any" bind:value={ssFacts[k]} />
              </div>
            {/if}
          {/each}
          <div style="grid-column:1/-1;margin-top:6px;display:flex;gap:8px;align-items:baseline">
            <span style="font-size:12px;color:var(--muted)">units</span>
            <Button onclick={() => applyPreset("imperial")}>imperial</Button>
            <Button onclick={() => applyPreset("metric")}>metric</Button>
            <Button onclick={() => applyPreset("uk")}>UK</Button>
            <span style="font-size:12px;color:var(--muted)">display only — values are stored in the game's native units, switch any time</span>
          </div>
          {#each UNIT_DIMS as [dim, l] (dim)}
            <div>
              <label for="up-{dim}">{dim === "temp" ? "temperature" : l}</label>
              {#if dim === "temp"}
                <select id="up-{dim}" bind:value={ssUnits[dim]} onchange={refreshFormUnits}>
                  <option value="f">°F</option>
                  <option value="c">°C</option>
                </select>
              {:else}
                <select id="up-{dim}" bind:value={ssUnits[dim]} onchange={refreshFormUnits}>
                  {#each Object.entries(UNITS[dim]) as [u, d] (u)}
                    <option value={u}>{d.l}</option>
                  {/each}
                </select>
              {/if}
            </div>
          {/each}
        </div>
        <div style="margin-top:10px;display:flex;gap:8px">
          <Button go onclick={saveProject}>save project</Button>
          <Button onclick={() => { formOpen = false; loadSession(); }}>cancel</Button>
        </div>
      </form>
    {/if}
  </div>

  <div class="panel">
    <div style="display:flex;gap:8px;align-items:center;flex-wrap:wrap;margin-bottom:8px">
      <h2 style="margin:0">Project library</h2>
      <span style="flex:1"></span>
      <input type="text" placeholder="name the new project (optional)" style="width:240px;background:var(--page);color:var(--ink-2);border:1px solid var(--border);border-radius:6px;padding:5px 8px" bind:value={newName} />
      <Button go onclick={newProject}>new project</Button>
    </div>
    <div style="font-size:13px;color:var(--muted);margin-bottom:6px">
      archiving keeps the whole project (setup history + notes) — resume it any time
    </div>
    <div style="font-size:13px">
      {#if listError}
        <div class="placeholder">project list failed: {listError}</div>
      {:else if list}
        {#each [list.active, ...list.archived] as r, i (r.id ?? "active")}
          <div style="display:flex;gap:10px;align-items:baseline;padding:5px 0;border-top:1px solid var(--border)">
            <b>{rowName(r)}</b>
            <span style="color:var(--muted)">{rowMeta(r)}</span>
            {#if r.description}<span style="color:var(--ink-2)">{r.description}</span>{/if}
            <span style="flex:1"></span>
            {#if i === 0}
              <span style="color:var(--accent)">active</span>
            {:else if r.id}
              <Button onclick={() => resumeProject(r.id!)}>resume</Button>
            {/if}
          </div>
        {/each}
      {/if}
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
      tool: raw driving telemetry, setup values, and setup deltas. No names, no free text — project names,
      descriptions, and notes are stripped before anything leaves this machine. Off by default; turn it
      off any time. Quote your sender id to have your data deleted.
    </div>
  </div>
</div>
