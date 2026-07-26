<script lang="ts">
  import { app, loadAdvice, loadPending, loadSession } from "$lib/app.svelte";
  import { AREA_GROUP, isAccepted, isHold, primaryRec } from "$lib/advice";
  import { advanced } from "$lib/advanced.svelte";
  import { commands, type RecommendationView } from "$lib/bindings";
  import { TUNE_GROUPS } from "$lib/fields";
  import { UNIVERSAL_LIMITS, limToCanon, limToDisp, toCanon, toDisp, unitOf } from "$lib/units";
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
  let limDraft: Record<string, string> = $state({});
  let expanded: Record<string, boolean> = $state({});
  let msg = $state("");

  // Re-seed the draft from the saved version whenever it (or units) change.
  $effect(() => {
    void app.unitsTick;
    const vals = latest;
    const facts = app.session?.facts ?? {};
    const d: Record<string, string> = {};
    const ld: Record<string, string> = {};
    for (const [, fields] of TUNE_GROUPS) {
      for (const [k] of fields) {
        d[k] = toDisp(k, vals?.[k] ?? "");
        ld[k] = limToDisp(k, facts[`limit_${k}`] ?? "");
      }
    }
    draft = d;
    limDraft = ld;
  });

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
      await alertDialog("Save failed", r.error.message);
      return;
    }
    await loadSession();
    await loadPending();
  }

  async function commitLimit(k: string) {
    const raw = String(limDraft[k] ?? "").trim();
    const canon = raw === "" ? "" : limToCanon(k, raw);
    if (raw !== "" && canon === "") {
      msg = `bad range for ${k}: use min..max`;
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
      msg = r.error.message;
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
      await alertDialog("Apply failed", r.error.message);
      return;
    }
    await loadSession();
    await loadPending();
    await loadAdvice();
  }

  let hasPending = $derived(!!app.pending);

  // --- baseline transcription stepper (phase 4): one game screen at a time,
  // in the game's own order, so entry is transcription rather than a wall of
  // 30 inputs. The draft survives step changes; save happens once at the end.
  let bStep = $state(0);
  const enteredCount = (fields: [string, string][]) =>
    fields.filter(([k]) => String(draft[k] ?? "").trim() !== "").length;
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
        <b>Baseline entry — screen {bStep + 1} of {TUNE_GROUPS.length}</b>
        <span style="color:var(--ink-2)">
          copy your tune exactly as the game's tuning screens show it — leave a field empty when the car can't tune it
        </span>
        <span style="flex:1"></span>
        {#if bStep > 0}<Button onclick={() => bStep--}>back</Button>{/if}
        {#if bStep < TUNE_GROUPS.length - 1}
          <Button go onclick={() => bStep++}>next screen</Button>
        {:else}
          <Button go onclick={saveBaseline}>save baseline tune</Button>
        {/if}
        {#if msg}<span style="color:var(--accent)">{msg}</span>{/if}
      </div>
      <div class="b-steps">
        {#each TUNE_GROUPS as [group, fields], i (group)}
          {@const n = enteredCount(fields)}
          <button class:current={i === bStep} class:filled={n > 0} onclick={() => (bStep = i)}>
            {group}{n ? ` ${n}/${fields.length}` : ""}
          </button>
        {/each}
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

    <div class="fam-grid" class:single={baselineMode}>
      {#each baselineMode ? [TUNE_GROUPS[bStep]] : TUNE_GROUPS as [group, fields] (group)}
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
                    <span style="opacity:.45;font-size:11px" title="range fixed across cars">{UNIVERSAL_LIMITS(k)}</span>
                  {:else}
                    <input
                      type="text"
                      placeholder="min..max"
                      title="slider range on this car (for limit-aware advice)"
                      style="width:72px;opacity:.65"
                      bind:value={limDraft[k]}
                      onchange={() => commitLimit(k)}
                    />
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
