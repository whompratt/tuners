<script lang="ts">
  import { goto } from "$app/navigation";
  import { app, detectedCar, errMsg, loadAdvice, loadSession, loadStints } from "$lib/app.svelte";
  import { commands, type CarView, type SessionsView } from "$lib/bindings";
  import { allCars } from "$lib/cars";
  import CarPicker from "$lib/components/CarPicker.svelte";
  import { COMPOUNDS, FACT_FIELDS, label } from "$lib/fields";
  import { toCanon, toDisp, unitLabel } from "$lib/units";
  import { alertDialog, confirmDialog } from "$lib/ui/dialogs.svelte";
  import Button from "$lib/ui/Button.svelte";

  let formOpen = $state(false);

  // --- project form state (facts; display units live in Settings) ---
  let ssName = $state("");
  let ssDescription = $state("");
  let ssCar = $state("");
  let ssFacts: Record<string, string> = $state({});
  let ssChecks: Record<string, boolean> = $state({});

  // Full dataset, for naming a car chosen by search/detection that has no
  // recorded runs yet.
  let carsAll: CarView[] = $state([]);
  allCars().then((v) => (carsAll = v));

  // --- library state ---
  let list: SessionsView | null = $state(null);
  let listError = $state("");
  let newName = $state("");

  let cars = $derived.by(() => {
    const m = new Map<number, string>();
    for (const st of app.stints) m.set(st.car, st.carName || `car #${st.car}`);
    return m;
  });
  const carLabelFor = (o: number) => cars.get(o) ?? carsAll.find((c) => c.car === o)?.name ?? `car #${o}`;
  let chosenLabel = $derived(ssCar ? carLabelFor(Number(ssCar)) : null);
  let liveCar = $derived(formOpen ? detectedCar() : null);
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

  function openForm() {
    const cur = s?.car;
    ssName = s?.facts.name || "";
    ssDescription = s?.facts.description || "";
    ssCar = cur != null ? String(cur) : "";
    ssFacts = {};
    ssChecks = {};
    for (const [k, , type] of FACT_FIELDS) {
      const v = s?.facts[k] || "";
      if (type === "check") ssChecks[k] = v === "on";
      else ssFacts[k] = type === "number" ? toDisp(k, v) : v;
    }
    formOpen = true;
  }

  async function saveProject() {
    const facts: [string, string][] = [];
    facts.push(["name", ssName.trim()]);
    facts.push(["description", ssDescription.trim()]);
    for (const [k, , type] of FACT_FIELDS) {
      facts.push([k, type === "check" ? (ssChecks[k] ? "on" : "off") : toCanon(k, ssFacts[k] ?? "")]);
    }
    const r = await commands.updateSession(false, ssCar, facts);
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

  // --- duplicate: new project seeded from a source project's setup ---
  let dupOpen = $state(false);
  let dupRowKey = $state("");
  let dupName = $state("");
  let dupSourceId: string | null = $state(null);

  function openDuplicate(r: { id: string | null; name: string | null; carName: string | null; car: number | null }) {
    dupSourceId = r.id;
    dupRowKey = r.id ?? "active";
    dupName = `${rowName(r)} copy`;
    dupOpen = true;
  }

  async function duplicateProject() {
    const r = await commands.duplicateSession(dupSourceId, dupName.trim() || null, null);
    if (r.status === "error") {
      await alertDialog("Duplicate failed", errMsg(r.error));
      return;
    }
    dupOpen = false;
    await loadSession();
    await refreshList();
    await loadAdvice();
    await loadStints(true);
    goto("/setup"); // review the copied tune, save it as the baseline
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

  async function deleteProject(r: { id: string; name: string | null; carName: string | null; car: number | null }) {
    const p = await commands.sessionDeletePlan(r.id);
    if (p.status === "error") {
      await alertDialog("Delete failed", errMsg(p.error));
      return;
    }
    const plan = p.data;
    const ok = await confirmDialog({
      title: "Delete project",
      body: `Delete "${rowName(r)}"? Its setup history and notes are removed.\n\nThis cannot be undone.`,
      verb: "Delete",
      danger: true,
    });
    if (!ok) return;
    // Second decision, not a veto: cancel here still deletes the project,
    // it just leaves the recordings on disk.
    let deleteRuns = false;
    if (plan.runs > 0) {
      const kept = [
        plan.shared ? `${plan.shared} used by other projects are kept` : "",
        plan.missing ? `${plan.missing} already gone` : "",
      ]
        .filter(Boolean)
        .join("; ");
      deleteRuns = await confirmDialog({
        title: "Delete its recordings too?",
        body: `${plan.runs} recorded run${plan.runs === 1 ? "" : "s"} (${(plan.mb ?? 0).toFixed(1)} MB) belong only to this project.${kept ? `\n${kept}.` : ""}`,
        verb: "Delete recordings",
        cancel: "Keep recordings",
        danger: true,
      });
    }
    const d = await commands.deleteSession(r.id, deleteRuns);
    if (d.status === "error") {
      await alertDialog("Delete failed", errMsg(d.error));
      return;
    }
    await refreshList();
    if (deleteRuns) await loadStints(true);
  }

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
          ? `Project: ${s.facts.name ? `${s.facts.name} · ` : ""}${s.carName || `car #${s.car}`}`
          : "Project"}
      </h2>
      <span style="color:var(--muted);font-size:13px">
        {active && s
          ? s.revisions
            ? `setup version ${s.revisions}`
            : "no setup on file yet; enter it in Setup"
          : "none active (a project is one build of one car)"}
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
          Pick the car this project is about. Runs from other cars are still recorded, but stay outside the project.
          Start a NEW project whenever the car itself changes (upgrades/drivetrain), not just the setup.
        </div>
        <div class="form-grid">
          <div>
            <label for="ss-name">project name</label>
            <input id="ss-name" type="text" placeholder="e.g. rwd no-aero build" bind:value={ssName} />
          </div>
          <div>
            <label for="ss-description">description</label>
            <input id="ss-description" type="text" placeholder="optional notes to find this project later" bind:value={ssDescription} />
          </div>
          <div style="grid-column:1/-1;font-size:14px">
            car:
            {#if chosenLabel}
              <b>{chosenLabel}</b>
            {:else}
              <span style="color:var(--muted)">not chosen yet — pick one of these ways:</span>
            {/if}
            {#if liveCar && String(liveCar.car) !== ssCar}
              <span style="margin-left:12px;color:var(--muted)">
                in the game now: <b style="color:var(--ink)">{liveCar.name ?? `car #${liveCar.car}`}</b>
              </span>
              <Button onclick={() => (ssCar = String(liveCar!.car))}>use this car</Button>
            {/if}
          </div>
          <div>
            <label for="ss-car">previously driven</label>
            <select
              id="ss-car"
              value={cars.has(Number(ssCar)) ? ssCar : ""}
              onchange={(e) => (ssCar = e.currentTarget.value)}
              disabled={!cars.size}
            >
              <option value="" disabled>{cars.size ? "pick from your runs…" : "no recorded runs yet"}</option>
              {#each cars as [o, n] (o)}
                <option value={String(o)}>{n}</option>
              {/each}
            </select>
          </div>
          <div>
            <label for="ss-car-search">search all cars</label>
            <CarPicker id="ss-car-search" onpick={(c) => (ssCar = String(c.car))} />
          </div>
          {#if !liveCar}
            <div style="grid-column:1/-1;font-size:12px;color:var(--muted);margin-top:-6px">
              or just get in the car in game — with telemetry hooked up it's detected automatically
            </div>
          {/if}
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
        </div>
        <div style="margin-top:10px;display:flex;gap:8px">
          <Button go disabled={!ssCar} onclick={saveProject}>save project</Button>
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
      archiving keeps the whole project (setup history + notes); resume it any time
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
            {#if r.car != null}
              <Button onclick={() => openDuplicate(r)}>duplicate</Button>
            {/if}
            {#if i === 0}
              <span style="color:var(--accent)">active</span>
            {:else if r.id}
              <Button onclick={() => resumeProject(r.id!)}>resume</Button>
              <Button danger onclick={() => deleteProject({ ...r, id: r.id! })}>delete</Button>
            {/if}
          </div>
          {#if dupOpen && dupRowKey === (r.id ?? "active")}
            <div style="display:flex;gap:8px;align-items:center;flex-wrap:wrap;padding:6px 0 8px;color:var(--muted)">
              <span>
                copies the setup (car, facts, limits, tune) into a new project — no runs or notes come along;
                the first save in Setup is the new baseline
              </span>
              <input
                type="text"
                placeholder="new project name"
                style="width:220px;background:var(--page);color:var(--ink-2);border:1px solid var(--border);border-radius:6px;padding:5px 8px"
                bind:value={dupName}
              />
              <Button go onclick={duplicateProject}>create</Button>
              <Button onclick={() => (dupOpen = false)}>cancel</Button>
            </div>
          {/if}
        {/each}
      {/if}
    </div>
  </div>

</div>
