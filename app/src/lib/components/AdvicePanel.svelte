<script lang="ts">
  import { app, loadSession, show } from "$lib/app.svelte";
  import { commands, type AdviseView } from "$lib/bindings";
  import { fmtLap } from "$lib/units";

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

  let a: AdviseView | null = $state(null);
  let error = $state("");
  let loading = $state(false);
  let acceptNote = $state("");
  let histSel = $state(0);
  let histCanvas: HTMLCanvasElement | undefined = $state();

  // f32 crosses IPC as `number | null` (NaN honesty) — formatters tolerate it.
  const N = (v: number | null | undefined) => v ?? 0;
  const sgn = (v: number | null) => `${N(v) > 0 ? "+" : ""}${N(v).toFixed(2)}s`;
  const dcol = (v: number | null) => (N(v) > 0.02 ? "#e66767" : N(v) < -0.02 ? "#199e70" : "var(--muted)");
  const fl = (v: number | null) => fmtLap(N(v));
  const base = (p: string) => p.split("/").pop();

  async function loadAdvice() {
    loading = true;
    error = "";
    acceptNote = "";
    // Fresh session state first: accepted-value detection below compares
    // against the LATEST revision, which an accept just changed.
    await loadSession();
    const r = await commands.advise();
    loading = false;
    if (r.status === "error") {
      a = null;
      error = r.error.message;
      return;
    }
    a = r.data;
    histSel = 0;
  }

  // A suggestion whose values already sit on the latest saved revision is
  // accepted-but-undriven: show it as pending instead of re-acceptable
  // (suggestions are judged against the last STINT's setup, so it still
  // renders until that stint is driven).
  function isAccepted(apply: [string, string][]): boolean {
    const latest = app.session?.latest;
    return (
      !!apply.length &&
      !!latest &&
      apply.every(([k, v]) => latest[k] != null && Math.abs(parseFloat(latest[k]) - parseFloat(v)) < 1e-3)
    );
  }

  async function accept(apply: [string, string][]) {
    const r = await commands.saveTune(apply, true);
    if (r.status === "error") {
      error = r.error.message;
      return;
    }
    // Recompute against the new revision: the accepted suggestion comes
    // back marked "saved", and no stale absolute can be accepted twice.
    const note = r.data.note;
    await loadAdvice();
    if (note) acceptNote = ` · journaled: ${note} — attaches to the next stint`;
  }

  let asks = $derived.by(() =>
    a ? a.recommendations.filter((r) => r.suggestion && !r.suggestion.includes("hold")).length : 0,
  );

  // The selected family's measured landscape: tried values vs cumulative
  // delta (valley shape: down = faster), fitted curve, estimated optimum,
  // and the raw measurements behind it.
  function drawHistory() {
    if (!a || !histCanvas) return;
    const l = a.landscapes[histSel];
    if (!l) return;
    const cv = histCanvas,
      ctx = cv.getContext("2d")!;
    const W = cv.clientWidth,
      H = 170,
      dpr = window.devicePixelRatio || 1;
    cv.width = W * dpr;
    cv.height = H * dpr;
    ctx.scale(dpr, dpr);
    ctx.clearRect(0, 0, W, H);
    ctx.font = "11px system-ui";
    if (l.nodes.length < 2) {
      ctx.fillStyle = "#8b8b90";
      ctx.fillText("not enough mapped values to chart — drive more single-change stints", 10, 24);
      return;
    }
    const xs = l.nodes.map((nd) => N(nd[0])),
      ys = l.nodes.map((nd) => N(nd[1]));
    const x0 = Math.min(...xs),
      x1 = Math.max(...xs);
    const y0 = Math.min(...ys, 0),
      y1 = Math.max(...ys, 0);
    const padX = (x1 - x0) * 0.08 || 1,
      padY = (y1 - y0) * 0.18 || 0.05;
    const X = (v: number) => 40 + ((v - (x0 - padX)) / (x1 + padX - (x0 - padX))) * (W - 55);
    const Y = (v: number) => 12 + ((y1 + padY - v) / (y1 + padY - (y0 - padY))) * (H - 44);
    ctx.strokeStyle = "rgba(139,139,144,.35)";
    ctx.beginPath(); ctx.moveTo(40, Y(0)); ctx.lineTo(W - 8, Y(0)); ctx.stroke();
    ctx.fillStyle = "#8b8b90";
    ctx.fillText("0", 28, Y(0) + 3);
    if (l.fit) {
      const [fa, fb, fc] = [N(l.fit[0]), N(l.fit[1]), N(l.fit[2])];
      ctx.strokeStyle = "rgba(57,135,229,.55)";
      ctx.beginPath();
      for (let i = 0; i <= 60; i++) {
        const x = x0 - padX + (i / 60) * (x1 + padX - (x0 - padX));
        const y = fa * x * x + fb * x + fc;
        if (i) ctx.lineTo(X(x), Y(y));
        else ctx.moveTo(X(x), Y(y));
      }
      ctx.stroke();
    }
    if (l.vertex != null) {
      const vx = N(l.vertex);
      ctx.strokeStyle = "rgba(25,158,112,.7)";
      ctx.setLineDash([4, 3]);
      ctx.beginPath(); ctx.moveTo(X(vx), 12); ctx.lineTo(X(vx), H - 30); ctx.stroke();
      ctx.setLineDash([]);
      ctx.fillStyle = "#199e70";
      ctx.fillText(`optimum ≈ ${l.vertex}`, Math.min(X(vx) + 5, W - 90), 22);
    }
    for (const nd of l.nodes) {
      const v = N(nd[0]), cum = N(nd[1]);
      ctx.fillStyle = "#3987e5";
      ctx.beginPath(); ctx.arc(X(v), Y(cum), 3.5, 0, 7); ctx.fill();
      ctx.fillStyle = "#c9c9ce";
      ctx.fillText(`${v}`, X(v) - 8, H - 14);
      ctx.fillStyle = "#8b8b90";
      ctx.fillText(sgn(cum), Math.min(X(v) + 6, W - 45), Y(cum) - 6);
    }
  }

  function jump(path: string) {
    show(path);
  }

  $effect(() => {
    void histSel;
    void a;
    drawHistory();
  });
</script>

<svelte:window onresize={drawHistory} />

<div class="panel">
  <div style="display:flex;align-items:baseline;gap:12px;flex-wrap:wrap">
    <h2 style="margin:0">Advice</h2>
    <span style="color:var(--muted);font-size:13px">
      {#if a}
        {a.journal
          ? `journal: ${a.journal}`
          : "no journal yet — blind advice on the latest stint (the journal starts with your first tune change)"}{acceptNote}
      {/if}
    </span>
    <span style="flex:1"></span>
    <button class="btn btn-go" onclick={loadAdvice}>get advice</button>
  </div>
  <div style="margin-top:10px">
    {#if loading}
      <span class="placeholder">analyzing all journaled stints…</span>
    {:else if error}
      <span class="placeholder">{error}</span>
    {:else if a}
      {#if a.steps.length}
        <table class="adv-table">
          <thead>
            <tr>
              <th></th><th>stint</th><th>laps</th><th>best</th><th>ideal</th><th>balance</th><th>change</th>
              <th>pos F/R</th><th>outcome</th>
              <th title="corner-entry share of the delta">entry</th>
              <th title="corner-exit share of the delta">exit</th>
              <th title="straights share of the delta">straights</th><th></th>
            </tr>
          </thead>
          <tbody>
            {#each a.steps as st, i (st.path)}
              <tr>
                <td class="num">{i + 1}</td>
                <td>
                  <a
                    href="#top"
                    onclick={(e) => { e.preventDefault(); jump(st.path); }}>{base(st.path)}</a>
                </td>
                <td class="num">{st.laps}</td>
                <td class="num">{fl(st.bestS)}</td>
                <td class="num">{fl(st.idealS)}</td>
                <td>
                  {#if st.balance}
                    {N(st.balance[0]) > 0 ? "+" : ""}{N(st.balance[0]).toFixed(2)}
                    <span style="color:var(--muted)">
                      (F {(N(st.balance[1]) * 100).toFixed(0)}%/R {(N(st.balance[2]) * 100).toFixed(0)}%)
                    </span>
                  {:else}–{/if}
                </td>
                <td>{st.note || ""}</td>
                <td class="num">
                  {st.pos ? `${N(st.pos[0]) > 0 ? "+" : ""}${st.pos[0]} / ${N(st.pos[1]) > 0 ? "+" : ""}${st.pos[1]}` : ""}
                </td>
                <td>
                  {#if st.outcome}
                    {@const o = st.outcome}
                    {#if "error" in o}
                      <span style="color:var(--muted)">not comparable: {o.error}</span>
                    {:else}
                      <span style="color:{OUTCOME_COLOR[o.word] || 'inherit'}">{o.word} {sgn(o.deltaS)}</span>
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
                        </span>{st.anchor.weak ? " ⚠" : ""}
                      </span>
                    {:else}
                      <span style="color:var(--muted)">
                        same setup as step {st.anchor.vsStep}: {sgn(st.anchor.deltaS)} = drift
                      </span>
                    {/if}
                  {/if}
                </td>
                {#if st.split}
                  {#each st.split as v, j (j)}
                    <td class="num"><span style="color:{dcol(v)}">{sgn(v)}</span></td>
                  {/each}
                {:else}
                  <td></td><td></td><td></td>
                {/if}
                <td>
                  {#if st.outcome}
                    {@const o = st.outcome}
                    {#if !("error" in o) && o.unequalLaps}
                      <span style="color:var(--muted)" title="unequal lap counts bias the ideal">⚠</span>
                    {/if}
                  {/if}
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      {/if}
      {#if a.inProgress}
        <div style="margin-top:6px;font-size:13px;color:var(--muted)">
          {base(a.inProgress)} is journaled but has no completed laps yet — its step joins the trajectory once a lap
          completes
        </div>
      {/if}
      {#each a.missing as p (p)}
        <div style="margin-top:6px;font-size:13px;color:var(--muted)">
          {base(p)} is journaled but its recording was deleted — skipped; its tune change merged into the next step
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
            </span>{a.anchor.weak ? " ⚠ single-lap side" : ""}{a.anchor.reconciled ? "" : " (multi-area — informational)"}
          {:else}
            step {a.anchor.vsStep} has the same setup — {sgn(a.anchor.deltaS)} ideal is pure driver/track drift
          {/if}
        </div>
      {/if}
      {#if a.driftFloor}
        <div
          style="margin-top:6px;font-size:13px;color:var(--muted)"
          title="largest ideal-lap difference measured between stints with identical setups — the campaign's own noise floor"
        >
          measured drift floor: ±{N(a.driftFloor[1]).toFixed(2)}s across {a.driftFloor[0]} same-setup pair{a.driftFloor[0] === 1 ? "" : "s"}
          — single-comparison margins below this are noise
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
      {#if a.landscapes.length}
        <div style="margin-top:14px;font-size:13px;color:var(--muted)">
          setup history:
          <select bind:value={histSel}>
            {#each a.landscapes as l, i (l.area + l.phrase)}
              <option value={i}>
                {l.phrase}{l.vertex != null ? ` (optimum ≈ ${l.vertex})` : ""} — {l.measurements.length}
                measurement{l.measurements.length === 1 ? "" : "s"}
              </option>
            {/each}
          </select>
        </div>
        <canvas
          bind:this={histCanvas}
          style="width:100%;max-width:720px;height:170px;display:block;margin-top:6px"
        ></canvas>
        {#if a.landscapes[histSel]}
          {@const land = a.landscapes[histSel]}
          <div style="margin-top:4px">
            <table class="adv-table">
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
                    <td style="color:var(--muted)">{m.direct ? "direct" : "attributed"}{m.weak ? " ⚠ single-lap" : ""}</td>
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        {/if}
      {/if}
      <div style="margin-top:12px;font-size:13px;color:var(--muted)">
        advice for {base(a.adviceFor)}:
        {#if asks > 1}
          <span title="applying several suggested values at once cannot be separated afterwards — especially probes">
            suggestions are alternatives — apply ONE per stint, drive, re-advise
          </span>
        {/if}
      </div>
      {#if a.recommendations.length}
        {#each a.recommendations as r (r.area + r.advice)}
          <details class="adv-rec">
            <summary style="cursor:pointer">
              <span class="adv-conf" style="color:{CONF_COLOR[r.confidence]}">[{r.confidence}]</span>
              {#if r.suggestion}<b>{r.suggestion}</b> — {r.advice}{:else}<b>{r.area}</b>: {r.advice}{/if}
              {#if r.apply.length}
                {#if isAccepted(r.apply)}
                  <span
                    style="color:var(--muted);font-size:12px"
                    title="this value is saved on the tune and journals against the next stint">saved — drive a stint</span>
                {:else}
                  <button
                    class="btn"
                    title="save this value onto the current tune — a partial save; accepts before the next stint net into one journal note"
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
        <div class="adv-rec">no recommendations — nothing in this stint stood out</div>
      {/if}
    {/if}
  </div>
</div>
