<script lang="ts">
  import { app, errMsg, loadAdvice, loadPending, loadSession } from "$lib/app.svelte";
  import { AREA_GROUP, isAccepted, isHold, primaryRec } from "$lib/advice";
  import { advanced } from "$lib/advanced.svelte";
  import { commands, type RecommendationView } from "$lib/bindings";
  import { TUNE_GROUPS } from "$lib/fields";
  import { UNIT_PRESETS, UNIVERSAL_LIMITS, limToCanon, limToDisp, toCanon, toDisp, unitOf, unitPrefs } from "$lib/units";
  import { drawLandscape, landscapeLayout, type LandscapeData } from "$lib/charts/landscape";
  import { palette } from "$lib/charts/palette";
  import Chart from "$lib/ui/Chart.svelte";
  import Button from "$lib/ui/Button.svelte";
  import { alertDialog } from "$lib/ui/dialogs.svelte";

  let latest = $derived(app.session?.latest ?? null);
  let baselineMode = $derived(!!app.session && app.session.car != null && !latest);

  // Draft edits (display units), keyed by slider. In living-tune mode a field
  // commits on change; in baseline mode the whole draft saves at once.
  let draft: Record<string, string> = $state({});
  // Limits edit as separate min/max boxes; storage stays the engine's
  // canonical "min..max" fact string.
  let limDraft: Record<string, { min: string; max: string }> = $state({});
  let expanded: Record<string, boolean> = $state({});
  let msg = $state("");

  // Re-seed the draft from the saved version whenever it (or units) change.
  // In baseline mode the draft IS the user's in-progress transcription and
  // there is no saved version to reseed from — seed once per car and never
  // clobber it (applyUnits re-expresses it in place instead).
  let baselineSeededFor = $state<number | null>(null);
  $effect(() => {
    void app.unitsTick;
    const vals = latest;
    const facts = app.session?.facts ?? {};
    const car = app.session?.car ?? null;
    if (baselineMode && baselineSeededFor === car) return;
    baselineSeededFor = baselineMode ? car : null;
    const d: Record<string, string> = {};
    const ld: Record<string, { min: string; max: string }> = {};
    for (const [, fields] of TUNE_GROUPS) {
      for (const [k] of fields) {
        d[k] = toDisp(k, vals?.[k] ?? "");
        const [min = "", max = ""] = limToDisp(k, facts[`limit_${k}`] ?? "").split("..");
        ld[k] = { min, max };
      }
    }
    draft = d;
    limDraft = ld;
  });

  /** Baseline units: re-express the in-progress draft under the new preset
   * (via canonical, so nothing drifts) and persist the prefs to the project.
   * Getting this right BEFORE saving matters — save interprets the numbers
   * under the active prefs. */
  async function applyUnits(preset: string) {
    const canon: Record<string, string> = {};
    const limCanon: Record<string, { min: string; max: string }> = {};
    for (const [, fields] of TUNE_GROUPS) {
      for (const [k] of fields) {
        canon[k] = String(draft[k] ?? "").trim() === "" ? "" : toCanon(k, draft[k]);
        limCanon[k] = {
          min: String(limDraft[k]?.min ?? "").trim() === "" ? "" : toCanon(k, limDraft[k].min),
          max: String(limDraft[k]?.max ?? "").trim() === "" ? "" : toCanon(k, limDraft[k].max),
        };
      }
    }
    for (const [dim, u] of Object.entries(UNIT_PRESETS[preset])) unitPrefs[dim] = u;
    for (const [, fields] of TUNE_GROUPS) {
      for (const [k] of fields) {
        draft[k] = canon[k] === "" ? "" : toDisp(k, canon[k]);
        limDraft[k] = {
          min: limCanon[k].min === "" ? "" : toDisp(k, limCanon[k].min),
          max: limCanon[k].max === "" ? "" : toDisp(k, limCanon[k].max),
        };
      }
    }
    const facts: [string, string][] = Object.entries(UNIT_PRESETS[preset]).map(([dim, u]) => [`unit_${dim}`, u]);
    await commands.updateSession(false, null, facts);
    await loadSession();
  }

  const dirty = (k: string) => {
    const saved = toDisp(k, latest?.[k] ?? "");
    return String(draft[k] ?? "") !== saved;
  };

  /** Living tune: an edited field commits immediately as a partial save; the
   * engine nets consecutive saves into ONE history entry, so typing the old
   * value back before driving erases the change entirely. */
  async function commitField(k: string) {
    if (baselineMode || !dirty(k)) return;
    const v = String(draft[k] ?? "").trim();
    if (v === "") {
      draft[k] = toDisp(k, latest?.[k] ?? "");
      return;
    }
    const r = await commands.saveTune([[k, toCanon(k, v)]], true);
    if (r.status === "error") {
      await alertDialog("Save failed", errMsg(r.error));
      return;
    }
    await loadSession();
    await loadPending();
  }

  async function commitLimit(k: string) {
    const mn = String(limDraft[k]?.min ?? "").trim();
    const mx = String(limDraft[k]?.max ?? "").trim();
    // Half-entered = mid-typing (or mid-clearing) — commit when both settle.
    if ((mn === "") !== (mx === "")) return;
    const canon = mn === "" ? "" : limToCanon(k, `${mn}..${mx}`);
    if (mn !== "" && canon === "") {
      msg = `bad range for ${k}: min and max must be numbers`;
      return;
    }
    const cur = app.session?.facts[`limit_${k}`] ?? "";
    if (canon === cur) return;
    await commands.updateSession(false, null, [[`limit_${k}`, canon]]);
    await loadSession();
  }

  async function saveBaseline() {
    const values: [string, string][] = [];
    for (const [, fields] of TUNE_GROUPS) {
      for (const [k] of fields) {
        const v = String(draft[k] ?? "").trim();
        if (v !== "") values.push([k, toCanon(k, v)]);
      }
    }
    if (!values.length) {
      msg = "enter at least one value";
      return;
    }
    const r = await commands.saveTune(values, false);
    if (r.status === "error") {
      msg = errMsg(r.error);
      return;
    }
    msg = "baseline saved — advice unlocks after your first run";
    await loadSession();
    await loadAdvice();
  }

  // --- the spatial register: recommendations per family card ---
  let primary = $derived(primaryRec(app.advice, latest));
  let recsByGroup = $derived.by(() => {
    const m = new Map<string, RecommendationView[]>();
    for (const r of app.advice?.recommendations ?? []) {
      const g = AREA_GROUP[r.area];
      if (!g) continue;
      m.set(g, [...(m.get(g) ?? []), r]);
    }
    return m;
  });
  let landscapeByGroup = $derived.by(() => {
    const m = new Map<string, LandscapeData>();
    for (const l of app.advice?.landscapes ?? []) {
      const g = AREA_GROUP[l.area];
      if (!g || m.has(g)) continue;
      m.set(g, {
        nodes: l.nodes.map((nd) => [nd[0] ?? 0, nd[1] ?? 0] as [number, number]),
        fit: l.fit ? [l.fit[0] ?? 0, l.fit[1] ?? 0, l.fit[2] ?? 0] : null,
        vertex: l.vertex,
      });
    }
    return m;
  });
  const landscapeDraw = (data: LandscapeData) => (ctx: CanvasRenderingContext2D, cssW: number) =>
    drawLandscape(ctx, landscapeLayout(data, cssW, 100), data, palette());

  async function accept(apply: [string, string][]) {
    const r = await commands.saveTune(apply, true);
    if (r.status === "error") {
      await alertDialog("Apply failed", errMsg(r.error));
      return;
    }
    await loadSession();
    await loadPending();
    await loadAdvice();
  }

  let hasPending = $derived(!!app.pending);

  // Baseline entry shows every card at once — the cards mirror the game's
  // tuning screens in order, so transcription is one tab-through of the
  // whole form (a per-group stepper was tried and felt tedious).
  let baselineEntered = $derived(
    TUNE_GROUPS.flatMap(([, fields]) => fields).filter(([k]) => String(draft[k] ?? "").trim() !== "").length,
  );
</script>

<div class="screen">
  {#if !app.booted}
    <div class="hero" style="color:var(--muted)">…</div>
  {:else if !app.session || app.session.car == null}
    <div class="hero">No project yet.</div>
    <div style="margin-top:8px;color:var(--muted)">
      <a href="/projects">Set up your project</a> first — then copy your tune in here.
    </div>
  {:else}
    {#if baselineMode}
      <div class="pending-bar">
        <b>Baseline entry</b>
        <span style="color:var(--ink-2)">
          the cards mirror the game's tuning screens in order — tab through and copy your tune exactly;
          leave a field empty when the car can't tune it
        </span>
        <span style="flex:1"></span>
        {#if baselineEntered}<span style="color:var(--muted)">{baselineEntered} entered</span>{/if}
        <Button go onclick={saveBaseline}>save baseline tune</Button>
        {#if msg}<span style="color:var(--accent)">{msg}</span>{/if}
        <div style="flex-basis:100%;display:flex;gap:8px;align-items:baseline;margin-top:6px">
          <span style="font-size:12px;color:var(--muted)">units — match what the game shows you:</span>
          <Button onclick={() => applyUnits("imperial")}>imperial</Button>
          <Button onclick={() => applyUnits("metric")}>metric</Button>
          <Button onclick={() => applyUnits("uk")}>UK</Button>
          <span style="font-size:12px;color:var(--muted)">
            switching re-converts anything already typed; per-field prefs live in Projects
          </span>
        </div>
      </div>
    {:else if app.pending}
      <div class="pending-bar">
        <b>{app.pending.changes.length} change{app.pending.changes.length === 1 ? "" : "s"} pending</b>
        <span style="color:var(--ink-2)">next run tests: {app.pending.note}</span>
        <span style="flex:1"></span>
        <span style="color:var(--muted)">drive to lock it into the history — or edit a value back to cancel it</span>
      </div>
    {/if}
    {#if msg && !baselineMode}<div class="banner">{msg}</div>{/if}

    <div class="fam-grid">
      {#each TUNE_GROUPS as [group, fields] (group)}
        {@const recs = recsByGroup.get(group) ?? []}
        {@const isPriority = !!primary && recs.includes(primary)}
        {@const hold = recs.find((r) => isHold(r))}
        {@const land = landscapeByGroup.get(group)}
        <div class="fam-card" class:priority={isPriority}>
          <h3>
            {group}
            {#if isPriority}<span class="badge" style="color:var(--accent)">next priority</span>{/if}
            {#if hold}<span class="badge" style="color:var(--ok)">✓ bracketed — hold</span>{/if}
          </h3>
          {#each fields as [k, l] (k)}
            {#key app.unitsTick}
              <div class="fam-row">
                <span class="lbl">{l}</span>
                <input
                  type="number"
                  step="any"
                  class:dirty={dirty(k)}
                  bind:value={draft[k]}
                  onchange={() => commitField(k)}
                />
                <span class="unit">{unitOf(k)?.l ?? ""}</span>
                {#if advanced.on}
                  {#if UNIVERSAL_LIMITS(k) && !app.session?.facts[`limit_${k}`]}
                    <span style="opacity:.45;font-size:11px" title="range fixed across cars">
                      {UNIVERSAL_LIMITS(k)?.replace("..", " – ")}
                    </span>
                  {:else if limDraft[k]}
                    <span class="lim" title="slider range on this car (for limit-aware advice)">
                      <input type="number" step="any" placeholder="min" bind:value={limDraft[k].min} onchange={() => commitLimit(k)} />
                      –
                      <input type="number" step="any" placeholder="max" bind:value={limDraft[k].max} onchange={() => commitLimit(k)} />
                    </span>
                  {/if}
                {/if}
              </div>
            {/key}
          {/each}
          {#if !baselineMode && recs.length}
            {#each recs as r (r.area + r.advice)}
              {@const accepted = isAccepted(r.apply as [string, string][], latest)}
              <div class="fam-sub" class:muted={r !== primary}>
                {#if r !== primary && !isHold(r)}
                  <span style="font-size:11px;text-transform:uppercase;letter-spacing:0.05em">alternative — one change at a time</span><br />
                {/if}
                {#if r.suggestion}<b>{r.suggestion}</b> — {/if}{r.advice}
                {#if r.apply.length}
                  {#if accepted}
                    <div style="margin-top:4px;color:var(--muted)">saved — drive a run</div>
                  {:else}
                    <div style="margin-top:6px">
                      <Button
                        go={r === primary}
                        title={hasPending && r !== primary ? "adds a 2nd change — the next run can't separate them" : undefined}
                        onclick={() => accept(r.apply as [string, string][])}
                      >
                        {r === primary ? "apply" : hasPending ? "adds a 2nd change ⚠" : "apply instead"}
                      </Button>
                    </div>
                  {/if}
                {/if}
              </div>
            {/each}
          {/if}
          {#if !baselineMode && land}
            <div class="fam-sub muted">
              <button
                style="all:unset;cursor:pointer;color:var(--muted)"
                onclick={() => (expanded[group] = !expanded[group])}
              >
                {expanded[group] ? "▾" : "▸"} measured landscape
              </button>
              {#if expanded[group]}
                <div style="margin-top:6px"><Chart height={100} draw={landscapeDraw(land)} /></div>
              {/if}
            </div>
          {/if}
        </div>
      {/each}
    </div>
  {/if}
</div>
