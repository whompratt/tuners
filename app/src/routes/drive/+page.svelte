<script lang="ts">
  import { app } from "$lib/app.svelte";
  import { commands } from "$lib/bindings";
  import { SPD, fmtClock, fmtLap, tempDisp, tempLabel } from "$lib/units";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import Button from "$lib/ui/Button.svelte";

  const STALE_MS = 3000;
  const ARC = Math.PI * 40; // semicircle path length (r=40)

  let rec = $derived(app.live?.recorder ?? { mode: "external", file: null, packets: 0 });
  let stale = $derived(app.live?.ageMs == null || app.live.ageMs > STALE_MS);
  let f = $derived(app.live?.frame ?? null);

  let q = $derived(app.quality);
  let qBand = $derived(q ? q.band : "low");
  let qPct = $derived(q ? (q.confidencePct ?? 0) : 0);

  let onTop = $state(false);
  async function toggleOnTop() {
    onTop = !onTop;
    await getCurrentWindow().setAlwaysOnTop(onTop);
  }

  // The rare manual cut (auto-cutting handles car changes and idle; saving a
  // changed setup cuts too). Nothing here takes a note — setup changes are
  // captured in Setup, where the history entry comes from the diff.
  async function cutRun() {
    await commands.recordSplit(null);
  }
</script>

<div class="screen" class:stale style="transition:opacity 0.3s">
  <div style="display:flex;align-items:baseline;gap:12px;flex-wrap:wrap">
    <h2 style="margin:0;color:var(--ink)">Drive</h2>
    <span style="color:var(--muted);font-size:13px">{app.live?.file || ""}</span>
    <span style="color:var(--muted);font-size:13px">
      {#if rec.mode === "recording"}
        <span class="rec-dot" style="color:var(--danger)">●</span> recording ({rec.packets.toLocaleString()} pkts)
      {:else if rec.mode === "waiting"}
        armed — waiting for race telemetry
      {:else}
        view-only (another capture owns the telemetry port)
      {/if}
    </span>
    <span style="flex:1"></span>
    <label style="font-size:13px;color:var(--muted);display:flex;gap:6px;align-items:center;cursor:pointer">
      <input type="checkbox" style="width:auto" checked={onTop} onchange={toggleOnTop} />
      keep window on top
    </label>
    {#if rec.mode === "recording" || rec.mode === "waiting"}
      <Button onclick={cutRun} title="close this run now — the next race-mode frame starts a fresh one">cut run</Button>
    {/if}
  </div>

  {#if f && stale}
    <div class="banner" style="margin-top:12px">no data for {Math.round((app.live?.ageMs ?? 0) / 1000)}s</div>
  {:else if f && !f.raceOn}
    <div class="banner" style="margin-top:12px">menu / paused</div>
  {/if}

  <div style="display:flex;gap:40px;align-items:flex-start;flex-wrap:wrap;margin-top:10px">
    <div id="live-quality" class={qBand === "good" ? "q-good" : qBand === "ok" ? "q-ok" : ""} style="margin-top:18px">
      <svg viewBox="0 0 100 62" width="230" aria-label="data confidence">
        <path class="q-track" d="M 10 52 A 40 40 0 0 1 90 52" />
        <path
          class="q-fill"
          d="M 10 52 A 40 40 0 0 1 90 52"
          stroke-dasharray="{(ARC * qPct) / 100} 999"
          style="visibility:{qPct > 0 ? 'visible' : 'hidden'}"
        />
        <text id="q-pct" x="50" y="50">{q ? `${Math.round(qPct)}%` : "–"}</text>
      </svg>
      <div id="q-label">
        confidence — {{ good: "enough for A/B", ok: "nearly there", low: "keep driving" }[qBand] ?? "keep driving"}
      </div>
      {#if q}
        <div style="font-size:13px;color:var(--muted)">
          {q.laps} {q.standingOnly ? "standing run(s)" : "flying lap(s)"} · best {fmtLap(q.bestLapS ?? 0)}
        </div>
      {/if}
    </div>

    <div class="drive-grid" style="flex:1;min-width:420px">
      {#key app.unitsTick}
        <div class="readout">
          <div class="label">Speed</div>
          <div class="value">{f ? ((f.speedMps ?? 0) * SPD().k).toFixed(0) : "–"} <span class="unit">{SPD().l}</span></div>
        </div>
        <div class="readout">
          <div class="label">Gear</div>
          <div class="value">{f ? (f.gear === 0 ? "R" : f.gear === 11 ? "–" : f.gear) : "–"}</div>
          <div class="sub">{f ? `${(f.rpm ?? 0).toFixed(0)} / ${(f.maxRpm ?? 0).toFixed(0)} rpm` : ""}</div>
        </div>
        <div class="readout minor">
          <div class="label">Lap {f ? (f.lapNumber ?? 0) + 1 : "–"}</div>
          <div class="value">{f ? fmtClock(f.currentLapS ?? 0) : "–"}</div>
        </div>
        <div class="readout">
          <div class="label">Lap times</div>
          <div class="trow">last <span class="num">{f && (f.lastLapS ?? 0) > 0 ? fmtLap(f.lastLapS ?? 0) : "–"}</span></div>
          <div class="trow">best <span class="num">{f && (f.bestLapS ?? 0) > 0 ? fmtLap(f.bestLapS ?? 0) : "–"}</span></div>
        </div>
        <div class="readout minor">
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
