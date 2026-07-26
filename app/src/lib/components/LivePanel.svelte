<script lang="ts">
  import { app } from "$lib/app.svelte";
  import { commands } from "$lib/bindings";
  import { SPD, fmtClock, fmtLap, tempDisp, tempLabel } from "$lib/units";

  const STALE_MS = 3000;
  const ARC = Math.PI * 40; // semicircle path length (r=40)

  let splitFormOpen = $state(false);
  let sfFamily = $state("front arb");
  let sfDelta = $state("");
  let sfText = $state("");

  let rec = $derived(app.live?.recorder ?? { mode: "external", file: null, packets: 0 });
  // Recorder armed or data seen: show the panel so the user knows where things stand.
  let visible = $derived(!!app.live && (!!app.live.frame || rec.mode !== "external"));
  let stale = $derived(app.live?.ageMs == null || app.live.ageMs > STALE_MS);
  let f = $derived(app.live?.frame ?? null);
  // Visible whenever the recorder is armed: mid-session it splits; between
  // sessions (waiting) it just stores the note for the next session to open.
  let canSplit = $derived(rec.mode === "recording" || rec.mode === "waiting");
  let recStatus = $derived(
    {
      recording: `● recording (${rec.packets.toLocaleString()} pkts)`,
      waiting: "armed — waiting for race telemetry",
      external: "view-only (external capture owns the UDP port)",
    }[rec.mode] || "",
  );

  let q = $derived(app.quality);
  let qBand = $derived(q ? q.band : "low");
  let qPct = $derived(q ? (q.confidencePct ?? 0) : 0);
  let qLabel = $derived(
    "confidence — " + ({ good: "enough for A/B", ok: "nearly there", low: "keep driving" }[qBand] ?? "keep driving"),
  );

  // The split button expands into a small form capturing the tune delta in
  // slider units (journal v2) — or free text — before cutting the session.
  // With a session tune on file, the tune form IS the change capture — the
  // journal note comes from the diff. The inline delta form is blind-mode only.
  function splitClick() {
    if (app.session?.latest) {
      window.dispatchEvent(new CustomEvent("tuners:open-tune-form"));
      return;
    }
    splitFormOpen = true;
  }

  async function doSplit(note: string) {
    await commands.recordSplit(note || null);
    // The recorder closes the file now; the next race-mode frame opens a fresh
    // one, and the note (if any) is journaled against it.
    splitFormOpen = false;
    sfDelta = "";
    sfText = "";
  }

  function sfGo() {
    const note = sfFamily
      ? sfDelta
        ? `${sfFamily} ${parseFloat(sfDelta) > 0 ? "+" : ""}${sfDelta}`
        : ""
      : sfText.trim();
    doSplit(note);
  }

  $effect(() => {
    if (!canSplit && splitFormOpen) splitFormOpen = false;
  });
</script>

{#if visible}
  <div id="live-panel" class="panel" class:stale>
    <div class="live-head">
      <span class="dot"></span>
      <h2 style="margin:0">Live</h2>
      <span style="font-size:13px;color:var(--muted)">{app.live?.file || ""}</span>
      <span style="font-size:13px;color:var(--muted)">
        {#if f}
          {stale ? `no data for ${Math.round((app.live?.ageMs ?? 0) / 1000)}s` : f.raceOn ? "" : "menu / paused"}
        {/if}
      </span>
      <span style="flex:1"></span>
      <span id="rec-status" style="font-size:13px;color:var(--muted)">
        {#if rec.mode === "recording"}<span class="rec-dot">●</span> recording ({rec.packets.toLocaleString()} pkts)
        {:else}{recStatus}{/if}
      </span>
      {#if canSplit && !splitFormOpen}
        <button id="rec-split" onclick={splitClick}>tune changed…</button>
      {/if}
      {#if splitFormOpen}
        <span id="split-form">
          <select id="sf-family" bind:value={sfFamily}>
            <option value="front arb">front ARB</option>
            <option value="rear arb">rear ARB</option>
            <option value="front springs">front springs</option>
            <option value="rear springs">rear springs</option>
            <option value="">other (free text)</option>
          </select>
          {#if sfFamily}
            <input id="sf-delta" type="number" step="0.5" placeholder="Δ (− = softer)" bind:value={sfDelta} />
          {:else}
            <input id="sf-text" type="text" placeholder="what changed?" bind:value={sfText} />
          {/if}
          <button id="sf-go" onclick={sfGo}>save</button>
          <button title="cut the session without a journal note" onclick={() => doSplit("")}>no note</button>
        </span>
      {/if}
    </div>
    <div class="live-body">
      <div id="live-quality" class={qBand === "good" ? "q-good" : qBand === "ok" ? "q-ok" : ""}>
        <svg viewBox="0 0 100 62" width="190" aria-label="data confidence">
          <path class="q-track" d="M 10 52 A 40 40 0 0 1 90 52" />
          <path
            class="q-fill"
            d="M 10 52 A 40 40 0 0 1 90 52"
            stroke-dasharray="{(ARC * qPct) / 100} 999"
            style="visibility:{qPct > 0 ? 'visible' : 'hidden'}"
          />
          <text id="q-pct" x="50" y="50">{q ? `${qPct}%` : "–"}</text>
        </svg>
        <div id="q-label">{qLabel}</div>
        <div id="q-tip" class="tip">
          {#if q}
            <span class="num">{q.laps}</span>
            {q.standingOnly ? "standing run(s)" : "flying lap(s)"} · best
            <span class="num">{fmtLap(q.bestLapS ?? 0)}</span> · spread
            <span class="num">{(q.spreadPct ?? 0).toFixed(1)}%</span> ·
            <span class="num">{(q.sharedKm ?? 0).toFixed(2)} km</span><br />
          {:else}
            No complete laps yet.
          {/if}
          Confidence is the share of the ideal lap reproduced by a second lap. It rises when sections of track are
          driven the same way at least twice; sections driven differently every lap stay unconfirmed. Green from 75%.
        </div>
      </div>
      <div class="live-grid">
        {#key app.unitsTick}
        <div class="readout">
          <div class="label">Speed</div>
          <div class="value">
            <span>{f ? ((f.speedMps ?? 0) * SPD().k).toFixed(0) : "–"}</span>
            <span class="unit">{SPD().l}</span>
          </div>
        </div>
        <div class="readout">
          <div class="label">Gear</div>
          <div class="value">{f ? (f.gear === 0 ? "R" : f.gear === 11 ? "–" : f.gear) : "–"}</div>
          <div class="sub"><span>{f ? (f.rpm ?? 0).toFixed(0) : "–"}</span>{f ? ` / ${(f.maxRpm ?? 0).toFixed(0)}` : ""} rpm</div>
        </div>
        <div class="readout">
          <div class="label">Lap</div>
          <div class="value">{f ? f.lapNumber + 1 : "–"}</div>
          <div class="sub">{f ? fmtClock(f.currentLapS ?? 0) : ""}</div>
        </div>
        <div class="readout">
          <div class="label">Lap times</div>
          <div class="trow">last <span class="num">{f && (f.lastLapS ?? 0) > 0 ? fmtLap(f.lastLapS ?? 0) : "–"}</span></div>
          <div class="trow">best <span class="num">{f && (f.bestLapS ?? 0) > 0 ? fmtLap(f.bestLapS ?? 0) : "–"}</span></div>
        </div>
        <div class="readout">
          <div class="label">Fuel</div>
          <div class="value">{f ? `${((f.fuel ?? 0) * 100).toFixed(0)}%` : "–"}</div>
        </div>
        <div class="readout">
          <div class="label">Tires {tempLabel()}</div>
          <div class="temps">
            {#each f ? f.tireTempF : [null, null, null, null] as t, i (i)}
              <!-- thresholds in canonical °F -->
              <div class={t == null ? "" : t < 160 ? "cold" : t > 240 ? "hot" : ""}>
                {t == null ? "–" : tempDisp(t).toFixed(0)}
              </div>
            {/each}
          </div>
        </div>
        {/key}
      </div>
    </div>
  </div>
{/if}
