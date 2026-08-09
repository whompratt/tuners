<script lang="ts">
  import { app, baseName, errMsg, loadAdvice, loadPending, show } from "$lib/app.svelte";
  import { confTag, isAccepted } from "$lib/advice";
  import { prefs } from "$lib/prefs.svelte";
  import { commands } from "$lib/bindings";
  import { fmtLap } from "$lib/units";
  import { drawLandscape, landscapeLayout, type LandscapeData } from "$lib/charts/landscape";
  import { palette } from "$lib/charts/palette";
  import Chart from "$lib/ui/Chart.svelte";

  const OUTCOME_COLOR: Record<string, string> = {
    improved: "#199e70",
    WORSE: "#e66767",
    inconclusive: "var(--muted)",
  };
  const CONF_COLOR: Record<string, string> = {
    high: "#199e70",
    medium: "var(--accent)",
    low: "var(--muted)",
  };

  let a = $derived(app.advice);
  let error = $derived(app.adviceError);
  let loading = $derived(app.adviceLoading);
  let acceptNote = $state("");
  let histSel = $state(0);
  // Pooled state rows expanded to their member runs, keyed by head path.
  let expanded = $state<Record<string, boolean>>({});

  // f32 crosses IPC as `number | null` (NaN honesty); formatters tolerate it.
  const N = (v: number | null | undefined) => v ?? 0;
  const sgn = (v: number | null) => `${N(v) > 0 ? "+" : ""}${N(v).toFixed(2)}s`;
  const dcol = (v: number | null) => (N(v) > 0.02 ? "#e66767" : N(v) < -0.02 ? "#199e70" : "var(--muted)");
  const fl = (v: number | null) => fmtLap(N(v));
  // The verdict is a 2-of-3 vote (ideal / best / median lap); when the
  // spliced ideal loses the vote beyond noise, say so.
  const overruled = (
    v: number | null | undefined,
    c: [number | null, number | null, number | null] | null | undefined,
  ) =>
    v != null &&
    c?.[0] != null &&
    Math.sign(v) !== Math.sign(c[0]) &&
    (Math.abs(v) >= 0.05 || Math.abs(c[0]) >= 0.05);
  // Run identity is the stamp: strip the constant prefix/suffix for display.
  const base = (p: string) => baseName(p).replace(/^stint-/, "").replace(/\.ftel$/, "");

  async function refresh() {
    acceptNote = "";
    histSel = 0;
    await loadAdvice();
  }

  async function accept(apply: [string, string][]) {
    const r = await commands.saveTune(apply, true);
    if (r.status === "error") {
      acceptNote = ` · ${errMsg(r.error)}`;
      return;
    }
    // Recompute against the new version: the accepted suggestion comes back
    // marked "saved", and no stale absolute can be accepted twice.
    const note = r.data.note;
    await loadPending();
    await loadAdvice();
    if (note) acceptNote = ` · recorded: ${note} (attaches to the next run)`;
  }

  let asks = $derived.by(() =>
    a ? a.recommendations.filter((r) => r.suggestion && !r.suggestion.includes("hold")).length : 0,
  );

  // The selected family's measured landscape: tried values vs cumulative
  // delta (valley shape: down = faster), fitted curve, estimated optimum,
  // and the raw measurements behind it.
  let histData = $derived.by((): LandscapeData | null => {
    const l = a?.landscapes[histSel];
    if (!l) return null;
    return {
      nodes: l.nodes.map((nd) => [N(nd[0]), N(nd[1])] as [number, number]),
      provisional: l.provisional.map((p) => [N(p[0]), N(p[1])] as [number, number]),
      fit: l.fit ? [N(l.fit[0]), N(l.fit[1]), N(l.fit[2])] : null,
      vertex: l.vertex,
    };
  });
  let histDraw = $derived.by(() => {
    const data = histData;
    return (ctx: CanvasRenderingContext2D, cssW: number) => {
      if (!data) return;
      drawLandscape(ctx, landscapeLayout(data, cssW), data, palette());
    };
  });

  function jump(path: string) {
    show(path);
  }

</script>

<div class="panel">
  <div style="display:flex;align-items:baseline;gap:12px;flex-wrap:wrap">
    <h2 style="margin:0">Advice</h2>
    <span style="color:var(--muted);font-size:13px">
      {#if a}
        {a.journal
          ? `history: ${a.journal}`
          : "no history yet; it starts with your first setup change"}{acceptNote}
      {/if}
    </span>
    <span style="flex:1"></span>
    <button class="btn btn-go" onclick={refresh}>refresh advice</button>
  </div>
  <div style="margin-top:10px">
    {#if loading}
      <span class="placeholder">analyzing all recorded runs…</span>
    {:else if error}
      <span class="placeholder">{error}</span>
    {:else if a}
      <div style="display:flex;flex-wrap:wrap;gap:8px 32px;align-items:flex-start">
      <div style="flex:2 1 900px;min-width:0">
      {#if a.steps.length}
        <div class="table-scroll">
        <table class="adv-table adv-compact zebra">
          <thead>
            <tr>
              <th></th><th>run</th>
              <th>laps</th>
              <th title="lap-time scatter (sd of flying laps): shrinking scatter at equal pace means the car got easier to drive">scatter</th>
              <th>best</th><th>optimal</th>
              <th title="cornering balance index (+ = understeer); hover a value for the front/rear grip shares">balance</th>
              <th>change</th>
              <th>pos F/R</th><th>outcome</th>
              <th
                title="how much to trust the verdict, from this campaign's own evidence: corroboration on both sides, suspect tags, the measured drift floor, and whether the optimal-lap read won the 2-of-3 vote"
                >conf</th>
              <th title="corner-entry share of the delta">entry</th>
              <th title="corner-exit share of the delta">exit</th>
              <th title="straights share of the delta">straights</th>
            </tr>
          </thead>
          <tbody>
            {#each a.steps as st (st.runs[0].path)}
              {@const headPath = st.runs[0].path}
              {@const pooled = st.runs.length > 1}
              <tr>
                <td class="num">{pooled ? `${st.first}-${st.last}` : st.first}</td>
                <td style="white-space:nowrap">
                  {#if pooled}
                    <a
                      href="#top"
                      title="consecutive same-setup runs pooled into one state; their laps back this row's numbers"
                      onclick={(e) => { e.preventDefault(); expanded[headPath] = !expanded[headPath]; }}
                      >{st.runs.length} runs · {expanded[headPath] ? "hide" : "show"}</a>
                  {:else}
                    <a
                      href="#top"
                      onclick={(e) => { e.preventDefault(); jump(headPath); }}>{base(headPath)}</a>
                  {/if}
                </td>
                <td class="num">{st.laps}</td>
                <td class="num">
                  {#if st.scatterS != null}
                    <span
                      style="color:var(--muted)"
                      title="lap-time scatter (sd of flying laps): shrinking scatter at equal pace means the car got easier to drive"
                      >±{N(st.scatterS).toFixed(2)}s</span>
                  {/if}
                </td>
                <td class="num">{fl(st.bestS)}</td>
                <td class="num">{fl(st.idealS)}</td>
                <td class="num">
                  {#if st.balance}
                    <span
                      title={`front ${(N(st.balance[1]) * 100).toFixed(0)}% / rear ${(N(st.balance[2]) * 100).toFixed(0)}% of grip limit`}
                      >{N(st.balance[0]) > 0 ? "+" : ""}{N(st.balance[0]).toFixed(2)}</span>
                  {:else}–{/if}
                </td>
                <td>
                  {#if st.note}
                    {#each st.note.split("; ") as cl, j (j)}
                      <div style="white-space:nowrap">{cl}</div>
                    {/each}
                  {/if}
                </td>
                <td class="num">
                  {st.pos ? `${N(st.pos[0]) > 0 ? "+" : ""}${st.pos[0]} / ${N(st.pos[1]) > 0 ? "+" : ""}${st.pos[1]}` : ""}
                </td>
                <td>
                  {#if st.outcome}
                    {@const o = st.outcome}
                    {#if "error" in o}
                      <span style="color:var(--muted)">not comparable: {o.error}</span>
                    {:else if o.word === "drift"}
                      <span
                        style="color:var(--muted)"
                        title="same setup as the previous run: a corroboration run, its delta is pure driver/track drift"
                        >same setup · drift {sgn(o.deltaS)}</span>
                    {:else}
                      <span
                        style="color:{OUTCOME_COLOR[o.word] || 'inherit'}"
                        title={st.currencies
                          ? `vote of three: ideal ${sgn(st.currencies[0])} / best ${sgn(st.currencies[1])} / median lap ${sgn(st.currencies[2])}`
                          : undefined}>{o.word} {sgn(o.deltaS)}</span>
                      {#if overruled(o.deltaS, st.currencies)}
                        <br />
                        <span
                          style="color:var(--muted)"
                          title={`the spliced ideal (${sgn(st.currencies![0])}) rewards laps that are fast in different places; best (${sgn(st.currencies![1])}) and median lap (${sgn(st.currencies![2])}) agree against it, so the vote overrules it`}
                        >
                          ideal {sgn(st.currencies![0])} overruled
                        </span>
                      {/if}
                    {/if}
                  {:else}–{/if}
                  {#if st.anchor}
                    <br />
                    {#if st.anchor.areas}
                      <span
                        style="color:var(--muted)"
                        title="honest setup-state verdict: compared against the prior stint with the smallest setup difference"
                      >
                        vs step {st.anchor.vsStep} ({st.anchor.areas}):
                        <span style="color:{OUTCOME_COLOR[st.anchor.word] || 'inherit'}">
                          {st.anchor.word} {sgn(st.anchor.deltaS)}
                        </span>{st.anchor.weak ? " (single-lap)" : ""}
                      </span>
                    {:else}
                      <span style="color:var(--muted)">
                        same setup as step {st.anchor.vsStep}: {sgn(st.anchor.deltaS)} = drift
                      </span>
                    {/if}
                  {/if}
                </td>
                <td>
                  {#if st.outcome && !("error" in st.outcome) && st.outcome.confidence}
                    <span
                      style="color:{CONF_COLOR[st.outcome.confidence] || 'var(--muted)'}"
                      title={st.outcome.why ?? "corroborated both sides, clear of the drift floor, currencies aligned"}
                      >{st.outcome.confidence}</span>
                  {/if}
                </td>
                {#if st.split}
                  {#each st.split as v, j (j)}
                    <td class="num"><span style="color:{dcol(v)}">{sgn(v)}</span></td>
                  {/each}
                {:else}
                  <td></td><td></td><td></td>
                {/if}
              </tr>
              {#if pooled && expanded[headPath]}
                {#each st.runs as r (r.path)}
                  <tr style="color:var(--muted)">
                    <td class="num">{r.n}</td>
                    <td style="white-space:nowrap;padding-left:14px">
                      <a
                        href="#top"
                        onclick={(e) => { e.preventDefault(); jump(r.path); }}>{base(r.path)}</a>
                    </td>
                    <td class="num">{r.laps}</td>
                    <td class="num">
                      {#if r.scatterS != null}±{N(r.scatterS).toFixed(2)}s{/if}
                    </td>
                    <td class="num">{fl(r.bestS)}</td>
                    <td class="num">{fl(r.idealS)}</td>
                    <td class="num">
                      {#if r.balance}
                        <span
                          title={`front ${(N(r.balance[1]) * 100).toFixed(0)}% / rear ${(N(r.balance[2]) * 100).toFixed(0)}% of grip limit`}
                          >{N(r.balance[0]) > 0 ? "+" : ""}{N(r.balance[0]).toFixed(2)}</span>
                      {:else}–{/if}
                    </td>
                    <td>{r.note ?? ""}</td>
                    <td></td>
                    <td>
                      {#if r.driftS != null}
                        <span title="delta vs the previous run of this state: pure driver/track drift"
                          >drift {sgn(r.driftS)}</span>
                      {/if}
                    </td>
                    <td></td>
                    <td></td><td></td><td></td>
                  </tr>
                {/each}
              {/if}
            {/each}
          </tbody>
        </table>
        </div>
      {/if}
      {#if a.inProgress}
        <div style="margin-top:6px;font-size:13px;color:var(--muted)">
          {base(a.inProgress)} is in the history but has no completed laps yet. Its step joins the trajectory once a lap
          completes
        </div>
      {/if}
      {#each a.missing as p (p)}
        <div style="margin-top:6px;font-size:13px;color:var(--muted)">
          {base(p)} is in the history but its recording was deleted: skipped, its setup change merged into the next step
        </div>
      {/each}
      {#each a.noLaps as p (p)}
        <div style="margin-top:6px;font-size:13px;color:var(--muted)">
          {base(p)} has no completed laps (an event entered and abandoned?): skipped
        </div>
      {/each}
      {#if a.anchor}
        <div
          style="margin-top:6px;font-size:13px;color:var(--muted)"
          title="steps record deltas, but honest comparisons are between setup states: this is the prior stint whose setup differs least"
        >
          {#if a.anchor.areas}
            cleanest comparison: vs step {a.anchor.vsStep} (differs only in {a.anchor.areas}: {a.anchor.changes}) →
            <span style="color:{OUTCOME_COLOR[a.anchor.word] || 'inherit'}">{a.anchor.word} {sgn(a.anchor.deltaS)}</span>
            <span title="where the time moved: corner entry / corner exit / straights">
              entry {sgn(a.anchor.split[0])} / exit {sgn(a.anchor.split[1])} / straights {sgn(a.anchor.split[2])}
            </span>{a.anchor.weak ? " (single-lap side)" : ""}{a.anchor.reconciled ? "" : " (multi-area, informational)"}
            {#if a.anchor.pooled}
              <span title="consecutive same-setup runs pool their laps into one side of the comparison">
                · {a.anchor.pooled} pooled</span>
            {/if}
            {#if overruled(a.anchor.deltaS, a.anchor.currencies)}
              <br />
              currencies: ideal {sgn(a.anchor.currencies[0])} overruled by best {sgn(a.anchor.currencies[1])} + median lap
              {sgn(a.anchor.currencies[2])}
            {/if}
          {:else}
            step {a.anchor.vsStep} has the same setup: {sgn(a.anchor.deltaS)} verdict is pure driver/track drift
          {/if}
        </div>
      {/if}
      {#if a.driftFloor}
        <div
          style="margin-top:6px;font-size:13px;color:var(--muted)"
          title="largest ideal-lap difference measured between stints with identical setups: the campaign's own noise floor"
        >
          measured drift floor: ±{N(a.driftFloor[1]).toFixed(2)}s across {a.driftFloor[0]} same-setup pair{a.driftFloor[0] === 1 ? "" : "s"}
          (single-comparison margins below this are noise)
        </div>
      {/if}
      {#if a.aba}
        <div
          style="margin-top:6px;font-size:13px;color:var(--muted)"
          title="the last two steps cancel out, so comparing around them removes driver/track drift"
        >
          A-B-A on {a.aba.families}: drift-corrected cost {sgn(a.aba.effectS ?? 0)} ideal · drift
          {sgn(a.aba.driftS ?? 0)}/stint (outcome margins near that drift are noise)
        </div>
      {/if}
      </div>
      {#if a.landscapes.length}
        <div style="flex:1 1 400px;min-width:340px;max-width:720px">
          <div style="font-size:13px;color:var(--muted)">
            setup history:
            <select bind:value={histSel}>
              {#each a.landscapes as l, i (l.area + l.phrase)}
                <option value={i}>
                  {l.phrase}{l.vertex != null ? ` (optimum ≈ ${l.vertex})` : ""}, {l.measurements.length}
                  measurement{l.measurements.length === 1 ? "" : "s"}
                </option>
              {/each}
            </select>
          </div>
          <div style="margin-top:6px">
            <Chart height={170} draw={histDraw} />
          </div>
          {#if a.landscapes[histSel]}
            {@const land = a.landscapes[histSel]}
            <div style="margin-top:4px" class="table-scroll">
              <table class="adv-table adv-compact">
                <thead>
                  <tr><th>steps</th><th>change</th><th>Δ ideal</th><th>entry</th><th>exit</th><th>straights</th><th></th></tr>
                </thead>
                <tbody>
                  {#each land.measurements as m (m.fromStep + "-" + m.toStep + m.desc)}
                    <tr>
                      <td class="num">{m.fromStep}→{m.toStep}</td>
                      <td>{m.desc}</td>
                      <td class="num" style="color:{dcol(m.deltaS)}">{sgn(m.deltaS)}</td>
                      {#if m.split}
                        {#each m.split as v, j (j)}
                          <td class="num" style="color:{dcol(v)}">{sgn(v)}</td>
                        {/each}
                      {:else}
                        <td></td><td></td><td></td>
                      {/if}
                      <td style="color:var(--muted)">{m.direct ? "direct" : "attributed"}{m.weak ? " (single-lap)" : ""}</td>
                    </tr>
                  {/each}
                </tbody>
              </table>
            </div>
          {/if}
        </div>
      {/if}
      </div>
      <div style="margin-top:12px;font-size:13px;color:var(--muted)">
        advice for {base(a.adviceFor)}:
        {#if asks > 1}
          <span title="applying several suggested values at once cannot be separated afterwards, especially probes">
            suggestions are alternatives: apply ONE per run, drive, refresh
          </span>
        {/if}
      </div>
      {#if a.recommendations.length}
        {#each a.recommendations as r (r.area + r.advice)}
          <details class="adv-rec" open={prefs.expandAdvice}>
            <summary style="cursor:pointer">
              <span class="adv-conf" style="color:{CONF_COLOR[r.confidence]}">[{confTag(r)}]</span>
              {#if r.suggestion}<b>{r.suggestion}</b>: {r.advice}{:else}<b>{r.area}</b>: {r.advice}{/if}
              {#if r.apply.length}
                {#if isAccepted(r.apply as [string, string][], app.session?.latest)}
                  <span
                    style="color:var(--muted);font-size:12px"
                    title="this value is saved on the setup and records against the next run">saved; drive a run</span>
                {:else}
                  <button
                    class="btn"
                    title="save this value onto the current setup; applies before the next run net into one history entry"
                    onclick={(e) => { e.preventDefault(); e.stopPropagation(); accept(r.apply); }}>apply</button>
                {/if}
              {/if}
            </summary>
            <div class="adv-ev">
              {#each r.evidence as ev, i (i)}
                · {ev}<br />
              {/each}
            </div>
          </details>
        {/each}
      {:else}
        <div class="adv-rec">no recommendations: nothing in this run stood out</div>
      {/if}
    {/if}
  </div>
</div>
