<script lang="ts">
  import { goto } from "$app/navigation";
  import { app, loadAdvice, loadPending } from "$lib/app.svelte";
  import { reopenOnboarding } from "$lib/onboarding.svelte";
  import { isAccepted, isHold, primaryRec } from "$lib/advice";
  import { commands } from "$lib/bindings";
  import { SPD, fmtLap, toDisp } from "$lib/units";
  import Button from "$lib/ui/Button.svelte";

  const STALE_MS = 3000;

  let live = $derived(app.live);
  let driving = $derived(
    !!live?.frame?.raceOn && live.ageMs != null && live.ageMs < STALE_MS,
  );
  let viewOnly = $derived(live?.recorder.mode === "external");

  let lastStep = $derived(app.advice?.steps.at(-1) ?? null);
  let prevStep = $derived(app.advice?.steps.at(-2) ?? null);

  // Verdict headline for the last run, honest about weak comparisons.
  let verdict = $derived.by(() => {
    const st = lastStep;
    if (!st?.outcome) return null;
    const runN = app.advice!.steps.length;
    if ("error" in st.outcome) {
      return { tone: "meh", text: `Run ${runN}: not comparable — ${st.outcome.error}` };
    }
    const d = Math.abs(st.outcome.deltaS ?? 0).toFixed(2);
    const word = st.outcome.word;
    if (word === "improved")
      return { tone: "ok", text: `Run ${runN}: ${d}s faster than run ${runN - 1} ✓ your last change worked` };
    if (word === "WORSE")
      return { tone: "bad", text: `Run ${runN}: ${d}s slower than run ${runN - 1} — your last change hurt` };
    return { tone: "meh", text: `Run ${runN}: within noise of run ${runN - 1} (${d}s) — inconclusive` };
  });

  // Consequence display for compound runs: several families changed at once —
  // say where the time moved and which change that points at. Directional,
  // never absolute (effect vectors may separate these later).
  let consequence = $derived.by(() => {
    const st = lastStep;
    if (!st || st.families.length < 2 || !st.split) return null;
    const [entry, exit, straights] = st.split.map((v) => v ?? 0);
    const corners = entry + exit;
    const channel = Math.abs(corners) >= Math.abs(straights) ? "corners" : "straights";
    const match = st.families.find((f) =>
      channel === "corners" ? f.channel !== "straights" : f.channel === "straights",
    );
    const areas = st.families.map((f) => f.area).join(", ");
    return (
      `${st.families.length} things changed at once (${areas}); the time moved mostly in the ` +
      `${channel} → likely the ${match?.area ?? areas} change. One change at a time gives clean answers.`
    );
  });

  let primary = $derived(primaryRec(app.advice, app.session?.latest));
  let others = $derived(
    app.advice?.recommendations.filter((r) => r !== primary) ?? [],
  );

  let applying = $state(false);
  async function accept(apply: [string, string][]) {
    applying = true;
    await commands.saveTune(apply, true);
    await loadPending();
    await loadAdvice();
    applying = false;
  }

  let confPct = $derived(app.quality ? Math.round(app.quality.confidencePct ?? 0) : null);
</script>

<div class="screen">
  {#if viewOnly}
    <div class="banner">
      view-only — another capture owns the telemetry port, so driving won't record here.
      Close the external capture to arm recording.
    </div>
  {/if}

  {#if !app.booted}
    <div class="hero" style="color:var(--muted)">…</div>
  {:else if !app.entered && app.session && app.session.car != null}
    <div class="hero">
      {app.session.facts.name ? `${app.session.facts.name} · ` : ""}{app.session.carName || `car #${app.session.car}`}
    </div>
    <div style="margin-top:6px;color:var(--muted)">
      {app.session.revisions
        ? `setup version ${app.session.revisions} — pick up where you left off`
        : "no setup on file yet"}
    </div>
    <div class="next-action">
      <div style="display:flex;gap:10px;align-items:center;flex-wrap:wrap">
        <Button go onclick={() => (app.entered = true)}>continue this project</Button>
        <Button onclick={() => goto("/projects")}>switch project…</Button>
      </div>
    </div>
  {:else if !app.session || app.session.car == null}
    <div class="hero">No project yet.</div>
    <div class="next-action">
      Set up your car first — pick it and note what the telemetry can't see (weight, compound, assists).
      <div style="margin-top:10px;display:flex;gap:10px;align-items:center">
        <Button go onclick={() => reopenOnboarding()}>first-time setup guide</Button>
        <Button onclick={() => goto("/projects")}>set up your project</Button>
      </div>
    </div>
  {:else if !app.session.latest}
    <div class="hero">{app.session.carName || `car #${app.session.car}`} — no tune on file.</div>
    <div class="next-action">
      Copy your tune in from the game's tuning screens. Advice starts from your baseline —
      it can't reason about a setup it can't see.
      <div style="margin-top:10px"><Button go onclick={() => goto("/setup")}>enter your tune</Button></div>
    </div>
  {:else if driving}
    <div class="hero">
      Driving — {((live?.frame?.speedMps ?? 0) * SPD().k).toFixed(0)} {SPD().l},
      lap {(live?.frame?.lapNumber ?? 0) + 1}
      {#if live?.frame?.bestLapS}· best {fmtLap(live.frame.bestLapS ?? 0)}{/if}
    </div>
    <div style="margin-top:8px;color:var(--muted)">
      confidence {confPct != null ? `${confPct}%` : "—"} ·
      {app.quality?.band === "good"
        ? "enough for A/B — pit when ready"
        : app.quality?.band === "ok"
          ? "nearly there"
          : "keep driving"}
    </div>
    <div style="margin-top:14px"><a href="/drive">open the Drive view →</a></div>
  {:else}
    {#if app.pending}
      <div class="hero">
        {app.pending.changes.length} change{app.pending.changes.length === 1 ? "" : "s"} pending —
        go drive, recording is armed.
      </div>
      <div style="margin-top:8px;color:var(--ink-2)">
        next run tests: <b style="color:var(--ink)">{app.pending.note}</b>
      </div>
    {:else if verdict}
      <div class="hero"><span class={verdict.tone}>{verdict.text}</span></div>
      {#if consequence}
        <div style="margin-top:8px;color:var(--muted);max-width:640px">{consequence}</div>
      {/if}
      {#if lastStep?.outcome && !("error" in lastStep.outcome) && lastStep.outcome.unequalLaps && prevStep}
        <div style="margin-top:6px;color:var(--muted);font-size:13px">
          ⚠ unequal lap counts ({prevStep.laps} vs {lastStep.laps}) bias this comparison
        </div>
      {/if}
    {:else if app.adviceLoading}
      <div class="hero" style="color:var(--muted)">analyzing your runs…</div>
    {:else if !app.stints.some((s) => s.car === app.session?.car)}
      <!-- Tune on file, nothing driven yet: advise has no data and would
           surface an error — this state is expected, not a fault. -->
      <div class="hero">Tune saved — drive your first run.</div>
      <div style="margin-top:8px;color:var(--muted);max-width:560px">
        Recording is armed whenever the app is open. Start a race, rivals lap, or route
        event in FH6 (free roam isn't recorded) — the verdict lands here when the run ends.
      </div>
      <div style="margin-top:14px"><a href="/drive">open the Drive view →</a></div>
    {:else if app.adviceError}
      <div class="hero" style="color:var(--muted)">{app.adviceError}</div>
    {:else}
      <div class="hero">Ready — drive a run to get your first verdict.</div>
    {/if}

    {#if !app.pending && primary}
      <div class="next-action">
        <div style="color:var(--muted);font-size:12px;text-transform:uppercase;letter-spacing:0.06em">next</div>
        <div style="margin-top:4px">
          {#if primary.suggestion}<b>{primary.suggestion}</b> — {primary.advice}{:else}<b>{primary.area}</b>: {primary.advice}{/if}
        </div>
        <div style="margin-top:10px;display:flex;gap:8px;align-items:center">
          {#if primary.apply.length && !isAccepted(primary.apply as [string, string][], app.session?.latest)}
            <Button go disabled={applying} onclick={() => accept(primary!.apply as [string, string][])}>
              {applying ? "applying…" : "apply"}
            </Button>
          {:else if isHold(primary)}
            <span style="color:var(--ok)">✓ nothing to change here — drive to confirm</span>
          {/if}
          <details style="display:inline-block">
            <summary style="cursor:pointer;color:var(--muted)">why?</summary>
            <div style="margin-top:6px;font-size:13px;color:var(--ink-2)">
              {#each primary.evidence as ev, i (i)}· {ev}<br />{/each}
            </div>
          </details>
        </div>
      </div>
      {#if others.length}
        <details style="margin-top:10px">
          <summary style="cursor:pointer;color:var(--muted)">other suggestions ({others.length}) — alternatives, one change at a time</summary>
          <div style="margin-top:6px;font-size:14px">
            {#each others as r (r.area + r.advice)}
              <div style="padding:4px 0;border-top:1px solid var(--border)">
                <span style="color:var(--muted)">[{r.confidence}]</span>
                {#if r.suggestion}<b>{r.suggestion}</b> — {/if}{r.advice}
              </div>
            {/each}
          </div>
        </details>
      {/if}
    {/if}
    {#if app.pending}
      <div style="margin-top:14px;font-size:14px;color:var(--ink-2)">
        {#each app.pending.changes as c (c.key)}
          <div style="padding:3px 0">
            {c.phrase}: {c.from ? `${toDisp(c.key, c.from)} → ` : ""}<b style="color:var(--ink)">{toDisp(c.key, c.to)}</b>
          </div>
        {/each}
        <div style="margin-top:8px;color:var(--muted)">
          changed your mind? <a href="/setup">edit the setup</a> — reverting a value before you drive nets it out of the history.
        </div>
      </div>
    {/if}
  {/if}
</div>
