<script lang="ts">
  import { goto } from "$app/navigation";
  import { app, loadAdvice, loadPending } from "$lib/app.svelte";
  import { reopenOnboarding } from "$lib/onboarding.svelte";
  import { isAccepted, isHold, primaryRec } from "$lib/advice";
  import { commands } from "$lib/bindings";
  import { FACT_FIELDS, label } from "$lib/fields";
  import { SPD, fmtLap, toDisp, unitLabel } from "$lib/units";
  import Button from "$lib/ui/Button.svelte";
  import ConfidenceGauge from "$lib/components/ConfidenceGauge.svelte";

  const STALE_MS = 3000;

  let live = $derived(app.live);
  let rec = $derived(
    live?.recorder ?? { mode: "external", file: null, packets: 0, udpAgeMs: null, udpCar: null, udpCarName: null },
  );
  let driving = $derived(
    !!live?.frame?.raceOn && live.ageMs != null && live.ageMs < STALE_MS,
  );
  let viewOnly = $derived(rec.mode === "external");
  let receiving = $derived(rec.udpAgeMs != null && rec.udpAgeMs < STALE_MS);

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

  let carRuns = $derived(app.stints.filter((s) => s.car === app.session?.car).length);
  // Mini trajectory tail for the Last-run card (newest last, numbered).
  let recentSteps = $derived.by(() => {
    const steps = app.advice?.steps ?? [];
    return steps.slice(-4).map((st, i) => ({ st, n: steps.length - Math.min(4, steps.length) + i + 1 }));
  });
  let shownFacts = $derived(
    app.session
      ? Object.entries(app.session.facts).filter(
          ([k]) => !k.startsWith("unit_") && !k.startsWith("limit_") && k !== "name" && k !== "description",
        )
      : [],
  );
  let dashboard = $derived(
    app.booted && app.entered && !!app.session && app.session.car != null && !!app.session.latest,
  );
</script>

<div class="screen" class:dash={dashboard}>
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
  {:else}
    <!-- The dashboard: the whole loop on one screen — drive status, last
         verdict, next action, project. Screens behind the cards carry the
         depth. -->
    <div class="dash-grid">
      <div class="panel dash-card">
        <div class="dash-head">
          <h2>Drive</h2>
          <a href="/drive">open →</a>
        </div>
        <div class="dash-line">
          {#if rec.mode === "recording"}
            <span style="color:var(--danger)">●</span> recording ({rec.packets.toLocaleString()} pkts)
          {:else if rec.mode === "waiting"}
            {receiving ? "receiving — armed, waiting for an event (free roam isn't recorded)" : "armed — no telemetry"}
          {:else}
            view-only — another capture owns the telemetry port
          {/if}
        </div>
        <div class="dash-mid" style="display:flex;gap:26px;align-items:center;flex-wrap:wrap;padding:12px 0">
          <div style="flex:1;min-width:170px">
            {#if driving}
              <div class="dash-big">{((live?.frame?.speedMps ?? 0) * SPD().k).toFixed(0)} {SPD().l}</div>
              <div class="dash-line">
                lap {(live?.frame?.lapNumber ?? 0) + 1}
                {#if live?.frame?.bestLapS}· best {fmtLap(live.frame.bestLapS ?? 0)}{/if}
              </div>
            {:else if receiving && rec.udpCarName}
              <div class="dash-big" style="font-size:24px">{rec.udpCarName}</div>
              <div class="dash-line">in the garage — start an event to record</div>
            {:else}
              <div class="dash-line dim">waiting for the game</div>
            {/if}
          </div>
          <ConfidenceGauge dim={!driving} width={250} />
        </div>
      </div>

      <div class="panel dash-card">
        <div class="dash-head">
          <h2>Last run</h2>
          <a href="/analysis">evidence →</a>
        </div>
        <div class="dash-mid">
          {#if driving}
            <div class="dash-line">recording — the verdict lands here when the run ends</div>
          {:else if verdict}
            <div class="dash-verdict {verdict.tone}">{verdict.text}</div>
            {#if consequence}
              <div class="dash-line" style="margin-top:6px">{consequence}</div>
            {/if}
            {#if lastStep?.outcome && !("error" in lastStep.outcome) && lastStep.outcome.unequalLaps && prevStep}
              <div class="dash-line">⚠ unequal lap counts ({prevStep.laps} vs {lastStep.laps}) bias this comparison</div>
            {/if}
          {:else if app.adviceLoading}
            <div class="dash-line">analyzing your runs…</div>
          {:else}
            <div class="dash-line">
              no verdict yet — finish a few laps of a race, rivals lap, or route event
            </div>
            {#if app.adviceError}
              <div class="dash-line" style="font-size:12px">analysis said: {app.adviceError}</div>
            {/if}
          {/if}
        </div>
        {#if recentSteps.length}
          <div class="dash-steps">
            {#each recentSteps as { st, n } (n)}
              <div class="row">
                <span style="color:var(--ink-2);flex:none">run {n}</span>
                <span style="overflow:hidden;text-overflow:ellipsis;white-space:nowrap">{st.note || "baseline"}</span>
                <span class="num">
                  {#if st.outcome && !("error" in st.outcome)}
                    {st.outcome.word === "improved" ? "−" : st.outcome.word === "WORSE" ? "+" : "±"}{Math.abs(st.outcome.deltaS ?? 0).toFixed(2)}s
                  {:else if st.bestS}
                    {fmtLap(st.bestS)}
                  {/if}
                </span>
              </div>
            {/each}
          </div>
        {/if}
      </div>

      <div class="panel dash-card" class:priority={!!primary && !app.pending}>
        <div class="dash-head">
          <h2>Next</h2>
          <a href="/setup">setup →</a>
        </div>
        <div class="dash-mid" style="width:100%">
        {#if app.pending}
          <div style="font-size:14px;color:var(--ink)">
            <b>{app.pending.changes.length} change{app.pending.changes.length === 1 ? "" : "s"} pending</b>
            — go drive, recording is armed.
          </div>
          <div class="dash-line">next run tests: <b style="color:var(--ink)">{app.pending.note}</b></div>
          <div style="margin-top:6px;font-size:13px;color:var(--ink-2)">
            {#each app.pending.changes as c (c.key)}
              <div style="padding:2px 0">
                {c.phrase}: {c.from ? `${toDisp(c.key, c.from)} → ` : ""}<b style="color:var(--ink)">{toDisp(c.key, c.to)}</b>
              </div>
            {/each}
          </div>
          <div class="dash-line">
            changed your mind? <a href="/setup">edit the setup</a> — reverting a value before you drive nets it out.
          </div>
        {:else if primary}
          <div style="font-size:14px">
            {#if primary.suggestion}<b>{primary.suggestion}</b> — {primary.advice}{:else}<b>{primary.area}</b>: {primary.advice}{/if}
          </div>
          <div style="margin-top:8px;display:flex;gap:8px;align-items:center;flex-wrap:wrap">
            {#if primary.apply.length && !isAccepted(primary.apply as [string, string][], app.session?.latest)}
              <Button go disabled={applying} onclick={() => accept(primary!.apply as [string, string][])}>
                {applying ? "applying…" : "apply"}
              </Button>
            {:else if isHold(primary)}
              <span style="color:var(--ok);font-size:13px">✓ nothing to change here — drive to confirm</span>
            {/if}
            <details style="display:inline-block">
              <summary style="cursor:pointer;color:var(--muted);font-size:13px">why?</summary>
              <div style="margin-top:6px;font-size:13px;color:var(--ink-2)">
                {#each primary.evidence as ev, i (i)}· {ev}<br />{/each}
              </div>
            </details>
          </div>
          {#if others.length}
            <div class="dash-alt">
              alternatives — one change at a time:
              {#each others.slice(0, 3) as r (r.area + r.advice)}
                <div style="padding:3px 0">
                  <span style="opacity:.7">[{r.confidence}]</span>
                  {#if r.suggestion}<b style="color:var(--ink-2)">{r.suggestion}</b> — {/if}{r.advice}
                </div>
              {/each}
              {#if others.length > 3}
                <a href="/setup">+{others.length - 3} more in Setup</a>
              {/if}
            </div>
          {/if}
        {:else}
          <div class="dash-line">no suggestion yet — verdicts and advice build up as you drive</div>
        {/if}
        </div>
      </div>

      <div class="panel dash-card">
        <div class="dash-head">
          <h2>Project</h2>
          <a href="/projects">manage →</a>
        </div>
        <div style="font-size:16px;color:var(--ink)">
          {app.session.facts.name ? `${app.session.facts.name} · ` : ""}{app.session.carName || `car #${app.session.car}`}
        </div>
        <div class="dash-line">
          setup version {app.session.revisions} · {carRuns} run{carRuns === 1 ? "" : "s"} recorded
        </div>
        <div class="dash-mid">
          {#if shownFacts.length}
            <div style="font-size:14px;color:var(--ink-2);display:flex;gap:8px 18px;flex-wrap:wrap">
              {#key app.unitsTick}
                {#each shownFacts as [k, v] (k)}
                  <span style="white-space:nowrap">{label(FACT_FIELDS, k)} <b style="color:var(--ink)">{toDisp(k, v)}{unitLabel(k).replace(/[()]/g, "")}</b></span>
                {/each}
              {/key}
            </div>
          {/if}
        </div>
        <div class="dash-line" style="padding-top:10px"><a href="/settings">units & sharing in Settings</a></div>
      </div>
    </div>
    {#if viewOnly}
      <div class="banner" style="margin-top:14px">
        view-only — another capture owns the telemetry port, so driving won't record here.
        Close the external capture to arm recording.
      </div>
    {/if}
  {/if}
</div>
