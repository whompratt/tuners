<script lang="ts">
  import { app, loadSession, loadStints } from "$lib/app.svelte";
  import { commands, type SessionsView, type SharingView } from "$lib/bindings";
  import { COMPOUNDS, FACT_FIELDS, TUNE_FIELDS, TUNE_GROUPS, label } from "$lib/fields";
  import {
    UNITS, UNIT_DIMS, UNIT_PRESETS, UNIVERSAL_LIMITS,
    limToCanon, limToDisp, toCanon, toDisp, unitLabel, unitOf, unitPrefs,
  } from "$lib/units";
  import { alertDialog, confirmDialog } from "$lib/ui/dialogs.svelte";

  let sessionFormOpen = $state(false);
  let tuneFormOpen = $state(false);
  let managerOpen = $state(false);
  let tuneMsg = $state("");
  let panelEl: HTMLDivElement | undefined = $state();

  // --- session form state ---
  let ssName = $state("");
  let ssDescription = $state("");
  let ssCar = $state("");
  let ssCarManual = $state("");
  let ssFacts: Record<string, string> = $state({});
  let ssChecks: Record<string, boolean> = $state({});
  let ssUnits: Record<string, string> = $state({});

  // --- tune form state (display units) ---
  let tfVals: Record<string, string> = $state({});
  let tfLims: Record<string, string> = $state({});

  // --- manager state ---
  let sessList: SessionsView | null = $state(null);
  let sessListError = $state("");
  let sharing: SharingView | null = $state(null);
  let sharingError = $state("");
  let newName = $state("");

  let cars = $derived.by(() => {
    const m = new Map<number, string>();
    for (const st of app.stints) m.set(st.car, st.carName || `car #${st.car}`);
    return m;
  });

  let s = $derived(app.session);
  let active = $derived(!!s && s.car != null);
  // unit_* are display prefs and limit_* are slider ranges (canonical units,
  // shown in the tune form and advice evidence) — neither is a car fact.
  let shownFacts = $derived(
    active && s
      ? Object.entries(s.facts).filter(
          ([k]) => !k.startsWith("unit_") && !k.startsWith("limit_") && k !== "name" && k !== "description",
        )
      : [],
  );

  const trimZeros = (t: string) => t.replace(/\.0+$/, "").replace(/(\.\d*?)0+$/, "$1");

  // One short line instead of the full tune: "baseline", or the delta vs
  // baseline in display units.
  type Chip = { label: string; value: string; muted?: boolean };
  let tuneChips = $derived.by((): Chip[] => {
    void app.unitsTick;
    if (!s || !s.revisions || !s.latest) return [];
    if (s.revisions === 1) return [{ label: "tune", value: "baseline" }];
    const base = s.baseline || {};
    const parts: Chip[] = [];
    for (const [k, v] of Object.entries(s.latest)) {
      const b = base[k];
      const dv = parseFloat(v),
        db = parseFloat(b);
      if (b === undefined) {
        parts.push({ label: label(TUNE_FIELDS, k), value: `${toDisp(k, v)} (new)` });
      } else if (!isNaN(dv) && !isNaN(db)) {
        const u = unitOf(k);
        const d = (dv - db) * (u ? u.k : 1);
        const txt = trimZeros(d.toFixed(u ? u.dp : 2));
        if (parseFloat(txt) !== 0) {
          parts.push({ label: label(TUNE_FIELDS, k), value: `${d > 0 ? "+" : ""}${txt}${u ? " " + u.l : ""}` });
        }
      } else if (v !== b) {
        parts.push({ label: label(TUNE_FIELDS, k), value: `${b} → ${v}` });
      }
    }
    return parts.length ? parts : [{ label: "tune", value: "unchanged from baseline" }];
  });

  function openSessionForm() {
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
    sessionFormOpen = true;
    tuneFormOpen = false;
  }

  // Unit pickers apply live: labels and entered values re-convert immediately
  // (values pass through canonical, so nothing drifts), and charts/live panel
  // preview the choice. Save persists; cancel reloads the saved prefs.
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

  async function saveSession() {
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
    const r = await commands.updateSession(false, ssCarManual || ssCar, facts);
    if (r.status === "error") {
      await alertDialog("Save failed", r.error.message);
      return;
    }
    sessionFormOpen = false;
    await loadSession();
  }

  function openTuneForm() {
    const latest = s?.latest || {};
    tfVals = {};
    tfLims = {};
    for (const [k] of TUNE_FIELDS) {
      tfVals[k] = toDisp(k, latest[k] || "");
      tfLims[k] = limToDisp(k, s?.facts[`limit_${k}`] || "");
    }
    tuneMsg = "";
    tuneFormOpen = true;
    sessionFormOpen = false;
    panelEl?.scrollIntoView({ behavior: "smooth" });
  }

  async function saveTune() {
    // Limits are session facts, not tune revisions: save the changed ones first.
    const limFacts: [string, string][] = [];
    for (const [k] of TUNE_FIELDS) {
      if (UNIVERSAL_LIMITS(k) && !s?.facts[`limit_${k}`]) continue;
      const raw = (tfLims[k] ?? "").trim();
      const canon = raw === "" ? "" : limToCanon(k, raw);
      const cur = s?.facts[`limit_${k}`] || "";
      if (raw !== "" && canon === "") {
        tuneMsg = `bad range for ${k}: use min..max`;
        return;
      }
      if (canon !== cur) limFacts.push([`limit_${k}`, canon]);
    }
    if (limFacts.length) await commands.updateSession(false, null, limFacts);
    const values: [string, string][] = [];
    for (const [k] of TUNE_FIELDS) {
      const v = tfVals[k] ?? "";
      if (v !== "") values.push([k, toCanon(k, v)]);
    }
    const resp = await commands.saveTune(values, false);
    if (resp.status === "error") {
      tuneMsg = resp.error.message;
      return;
    }
    const r = resp.data;
    tuneMsg = r.note
      ? `journaled: ${r.note} — attaches to the next stint`
      : r.changed
        ? "baseline tune saved"
        : "no change from the last revision";
    if (r.changed) setTimeout(() => (tuneFormOpen = false), 1800);
    await loadSession();
  }

  async function toggleManager() {
    if (managerOpen) {
      managerOpen = false;
      return;
    }
    managerOpen = true;
    // Sections render independently: a failure in one shows inline and must
    // never blank the other (or keep the panel from opening at all).
    await renderSessList();
    await renderSharing();
  }
  async function renderSessList() {
    try {
      sessList = await commands.sessions();
      sessListError = "";
    } catch (e) {
      sessList = null;
      sessListError = String(e);
    }
  }
  async function renderSharing() {
    try {
      sharing = await commands.sharing();
      sharingError = "";
    } catch (e) {
      sharing = null;
      sharingError = String(e);
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
  let sharingOverride = $state("");

  // Wired independently of the status render: the toggle must work (or say
  // why it can't) even when the status render failed. Every failure is audible.
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
        await alertDialog("Save failed", r.error.message);
        return;
      }
      await renderSharing();
    } catch (e) {
      await alertDialog("Telemetry sharing", `toggle failed: ${e}`);
    }
  }

  async function shareHistory() {
    try {
      const p = await commands.sharingHistoryPlan();
      const skipped: string[] = [];
      if (p.already) skipped.push(`${p.already} already shared/queued`);
      if (p.unjournaled) skipped.push(`${p.unjournaled} without tune history`);
      if (!p.stints) {
        await alertDialog("Share existing recordings", `Nothing new to share${skipped.length ? ` — skipped: ${skipped.join(", ")}` : ""}.`);
        return;
      }
      const msg =
        `Share ${p.stints} recording${p.stints === 1 ? "" : "s"} from ` +
        `${p.campaigns} session${p.campaigns === 1 ? "" : "s"} (~${(p.mb ?? 0).toFixed(1)} MB raw)` +
        ` recorded before sharing was enabled?\n\n` +
        `Same rules as live sharing: raw telemetry, setup values, and tune deltas only` +
        ` — names, descriptions, and journal notes are stripped. Uploads happen in the` +
        ` background while telemetry is idle.` +
        (skipped.length ? `\n\n(Skipped: ${skipped.join(", ")}.)` : "");
      if (!(await confirmDialog({ title: "Share existing recordings", body: msg, verb: "Share", cancel: "Cancel" }))) return;
      const r = await commands.shareHistory();
      if (r.status === "error") {
        await alertDialog("Save failed", r.error.message);
        return;
      }
      sharingOverride = `bundling ${r.data} recording(s) in the background…`;
      setTimeout(() => {
        sharingOverride = "";
        renderSharing();
      }, 4000);
    } catch (e) {
      await alertDialog("Share existing recordings", `failed: ${e}`);
    }
  }

  async function resumeSession(id: string) {
    const r = await commands.resumeSession(id);
    if (r.status === "error") {
      await alertDialog("Resume failed", r.error.message);
      return;
    }
    await loadSession();
    await renderSessList();
  }

  async function newSession() {
    const r = await commands.newSession(null, newName.trim() || null, null);
    if (r.status === "error") {
      await alertDialog("New session failed", r.error.message);
      return;
    }
    newName = "";
    await loadSession();
    await renderSessList();
    openSessionForm(); // straight into picking the car + facts for the new campaign
  }

  function rowName(r: { name: string | null; carName: string | null; car: number | null }) {
    return r.name || r.carName || (r.car != null ? `car #${r.car}` : "(no car)");
  }
  function rowMeta(r: { name: string | null; carName: string | null; car: number | null; revisions: number; stints: number; id: string | null }) {
    return [
      r.name ? r.carName || (r.car != null ? `car #${r.car}` : null) : null,
      `${r.revisions} revision${r.revisions === 1 ? "" : "s"}`,
      `${r.stints} stint${r.stints === 1 ? "" : "s"}`,
      r.id,
    ]
      .filter(Boolean)
      .join(" · ");
  }

  $effect(() => {
    const open = () => openTuneForm();
    window.addEventListener("tuners:open-tune-form", open);
    return () => window.removeEventListener("tuners:open-tune-form", open);
  });
</script>

<div class="panel" bind:this={panelEl}>
  <div style="display:flex;align-items:baseline;gap:12px;flex-wrap:wrap">
    <h2 style="margin:0">
      {active && s
        ? `Tuning session — ${s.facts.name ? `${s.facts.name} · ` : ""}${s.carName || `car #${s.car}`}`
        : "Tuning session"}
    </h2>
    <span style="color:var(--muted);font-size:13px">
      {active && s
        ? s.revisions
          ? `tune revision ${s.revisions}`
          : "no tune entered yet"
        : "none active — pick a car and enter what the telemetry can't see"}
    </span>
    <span style="flex:1"></span>
    <button class="btn" onclick={toggleManager}>sessions…</button>
    <button class="btn" onclick={openSessionForm}>{active ? "session settings" : "set up session"}</button>
    {#if active}
      <button class="btn" onclick={openTuneForm}>{s?.latest ? "new tune revision" : "enter baseline tune"}</button>
    {/if}
  </div>
  <div id="tune-summary" style="margin-top:8px;font-size:13px;color:var(--ink-2)">
    {#key app.unitsTick}
      {#each shownFacts as [k, v] (k)}
        <span class="kv">{label(FACT_FIELDS, k)} <b>{toDisp(k, v)}{unitLabel(k).replace(/[()]/g, "")}</b></span>
      {/each}
      {#if shownFacts.length && s?.revisions}<br />{/if}
      {#if tuneChips.length && tuneChips[0].value !== "baseline" && tuneChips[0].value !== "unchanged from baseline" && (s?.revisions ?? 0) > 1}
        <span class="kv" style="color:var(--muted)">vs baseline:</span>
      {/if}
      {#each tuneChips as c, i (i)}
        <span class="kv">{c.label} <b>{c.value}</b></span>
      {/each}
    {/key}
  </div>

  {#if managerOpen}
    <div style="margin-top:10px;font-size:13px">
      <div style="display:flex;gap:8px;align-items:center;flex-wrap:wrap;margin-bottom:8px">
        <input type="text" placeholder="name the new session (optional)" style="width:240px" bind:value={newName} />
        <button class="btn btn-go" onclick={newSession}>new session</button>
        <span style="color:var(--muted)">
          archives the current campaign (tune history + journal) whole — resume it below any time. Start one whenever
          the car itself changes (upgrades/drivetrain), not just the tune.
        </span>
      </div>
      <div>
        {#if sessListError}
          <div class="placeholder">session list failed: {sessListError}</div>
        {:else if sessList}
          {#each [sessList.active, ...sessList.archived] as r, i (r.id ?? "active")}
            <div style="display:flex;gap:10px;align-items:baseline;padding:5px 0;border-top:1px solid var(--border)">
              <b>{rowName(r)}</b>
              <span style="color:var(--muted)">{rowMeta(r)}</span>
              {#if r.description}<span style="color:var(--ink-2)">{r.description}</span>{/if}
              <span style="flex:1"></span>
              {#if i === 0}
                <span style="color:var(--accent)">active</span>
              {:else if r.id}
                <button class="btn" onclick={() => resumeSession(r.id!)}>resume</button>
              {/if}
            </div>
          {/each}
        {/if}
      </div>
      <div style="margin-top:12px;padding-top:10px;border-top:1px solid var(--border)">
        <div style="display:flex;gap:10px;align-items:baseline;flex-wrap:wrap">
          <b>telemetry sharing</b>
          <span style="color:var(--muted)">{sharingOverride || sharingStatus}</span>
          <span style="flex:1"></span>
          <button
            class="btn"
            disabled={!sharing?.enabled}
            title={sharing?.enabled
              ? "bundle recordings from before sharing was enabled (asks first)"
              : "turn on sharing first"}
            onclick={shareHistory}>share existing recordings…</button>
          <button class="btn" onclick={toggleSharing}>{sharing?.enabled ? "turn off" : "turn on sharing"}</button>
        </div>
        <div style="color:var(--muted);font-size:12px;margin-top:6px;max-width:640px">
          When on, each finished stint is bundled and uploaded (only while telemetry is idle) to help develop this
          tool: raw driving telemetry, setup values, and tune deltas. No names, no free text — session names,
          descriptions, and journal notes are stripped before anything leaves this machine. Off by default; turn it
          off any time. Quote your sender id to have your data deleted.
        </div>
      </div>
    </div>
  {/if}

  {#if sessionFormOpen}
    <form style="margin-top:12px" onsubmit={(e) => e.preventDefault()}>
      <div style="font-size:13px;color:var(--muted);margin-bottom:8px">
        Pick the car this session is about — stints from other cars are still recorded, but stay outside the session.
      </div>
      <div class="form-grid">
        <div>
          <label for="ss-name">session name</label>
          <input id="ss-name" type="text" placeholder="e.g. rwd no-aero build" bind:value={ssName} />
        </div>
        <div>
          <label for="ss-description">description</label>
          <input id="ss-description" type="text" placeholder="optional — notes to find this campaign later" bind:value={ssDescription} />
        </div>
        <div>
          <label for="ss-car">car (from recorded stints)</label>
          <select id="ss-car" bind:value={ssCar}>
            {#each cars as [o, n] (o)}
              <option value={String(o)}>{n}</option>
            {/each}
          </select>
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
          <button type="button" class="btn" onclick={() => applyPreset("imperial")}>imperial</button>
          <button type="button" class="btn" onclick={() => applyPreset("metric")}>metric</button>
          <button type="button" class="btn" onclick={() => applyPreset("uk")}>UK</button>
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
        <button class="btn btn-go" onclick={saveSession}>save session</button>
        <button class="btn" onclick={() => { sessionFormOpen = false; loadSession(); }}>cancel</button>
      </div>
    </form>
  {/if}

  {#if tuneFormOpen}
    <form style="margin-top:12px" onsubmit={(e) => e.preventDefault()}>
      <div style="font-size:13px;color:var(--muted);margin-bottom:8px">
        Enter the tune as the game shows it. Leave a field empty when the car can't tune it (upgrade limitations) —
        e.g. only rear diff values filled means a rear-diff car. Saving a changed tune journals the difference
        automatically and starts a new stint — change one thing at a time to keep steps attributable.
      </div>
      <div id="tune-fields" class="form-grid">
        {#key app.unitsTick}
          {#each TUNE_GROUPS as [group, fields] (group)}
            <div class="fg-col">
              <div class="fg-head">{group}</div>
              {#each fields as [k, l] (k)}
                <div>
                  <label for="tf-{k}">{l}{unitLabel(k)}</label>
                  <input id="tf-{k}" type="number" step="any" bind:value={tfVals[k]} />
                  {#if UNIVERSAL_LIMITS(k) && !s?.facts[`limit_${k}`]}
                    <span style="opacity:.45;font-size:11px;margin-left:4px" title="range fixed across cars">{UNIVERSAL_LIMITS(k)}</span>
                  {:else}
                    <input
                      type="text"
                      placeholder="min..max"
                      title="slider range on this car (for limit-aware advice)"
                      style="width:76px;opacity:.65;margin-left:4px"
                      bind:value={tfLims[k]}
                    />
                  {/if}
                </div>
              {/each}
            </div>
          {/each}
        {/key}
      </div>
      <div style="margin-top:10px;display:flex;gap:8px;align-items:center">
        <button class="btn btn-go" onclick={saveTune}>save tune</button>
        <button class="btn" onclick={() => (tuneFormOpen = false)}>cancel</button>
        <span style="font-size:13px;color:var(--accent)">{tuneMsg}</span>
      </div>
    </form>
  {/if}
</div>
