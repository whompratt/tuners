<script lang="ts">
  import { app } from "$lib/app.svelte";
  import { commands } from "$lib/bindings";
  import { SPD, fmtClock, fmtLap, tempDisp, tempLabel } from "$lib/units";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import Button from "$lib/ui/Button.svelte";
  import ConfidenceGauge from "$lib/components/ConfidenceGauge.svelte";

  const STALE_MS = 3000;

  let rec = $derived(
    app.live?.recorder ?? { mode: "external", file: null, packets: 0, udpAgeMs: null, udpCar: null, udpCarName: null },
  );
  let stale = $derived(app.live?.ageMs == null || app.live.ageMs > STALE_MS);
  let f = $derived(app.live?.frame ?? null);
  // Raw datagrams flowing (menus and free roam included): the wiring works,
  // whether or not anything is being recorded.
  let receiving = $derived(rec.udpAgeMs != null && rec.udpAgeMs < STALE_MS);
  // Cold/hot coloring follows the session compound's working band (the same
  // band the advice engine judges pressures against); slick band if no project.
  let band = $derived([app.session?.tempBandF?.[0] ?? 160, app.session?.tempBandF?.[1] ?? 210]);

  // Confidence card: dimmed in menus/idle, full opacity as soon as the car
  // is on a recorded track (out lap included; it shows "–" until a flying
  // lap produces a value). External capture counts via live race frames.
  let onTrack = $derived(!stale && (rec.mode === "recording" || !!f?.raceOn));

  let onTop = $state(false);
  async function toggleOnTop() {
    onTop = !onTop;
    await getCurrentWindow().setAlwaysOnTop(onTop);
  }

  // The rare manual cut (auto-cutting handles car changes and idle; saving a
  // changed setup cuts too). Nothing here takes a note: setup changes are
  // captured in Setup, where the history entry comes from the diff.
  async function cutRun() {
    await commands.recordSplit(null);
  }
</script>

<div class="screen">
  <div style="display:flex;align-items:baseline;gap:12px;flex-wrap:wrap">
    <h2 style="margin:0;color:var(--ink)">Drive</h2>
    <span style="color:var(--muted);font-size:13px">{app.live?.file || ""}</span>
    <span style="color:var(--muted);font-size:13px">
      {#if rec.mode === "recording"}
        <span class="rec-dot" style="color:var(--danger)">●</span> recording ({rec.packets.toLocaleString()} pkts)
      {:else if rec.mode === "waiting"}
        armed, waiting for race telemetry
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
      <Button onclick={cutRun} title="close this run now; the next race-mode frame starts a fresh one">cut run</Button>
    {/if}
  </div>

  {#if rec.mode === "waiting" && receiving}
    <!-- Telemetry flows but nothing records: say so explicitly, and say what
         starts a recording. This is the state free roam and time attack sit
         in, where the dimmed gauge alone reads as a mystery. -->
    <div class="banner" style="margin-top:12px">
      not recording{rec.udpCarName ? ` (receiving telemetry from the ${rec.udpCarName})` : ""}.
      Recording starts by itself when you enter an event: a rivals lap is best
      (identical conditions every run); races and route events work too.
      Free roam and open-world time attack aren't recorded
    </div>
  {:else if !f && app.booted}
    <div class="banner" style="margin-top:12px">
      no telemetry yet. In FH6: Settings → HUD and Gameplay → Data Out → ON, IP 127.0.0.1, port 20440.
      The game only honours new Data Out settings after a full restart.
    </div>
  {:else if f && stale}
    <div class="banner" style="margin-top:12px">no data for {Math.round((app.live?.ageMs ?? 0) / 1000)}s</div>
  {:else if f && !f.raceOn}
    <div class="banner" style="margin-top:12px">menu / paused</div>
  {/if}

  <div style="display:flex;gap:40px;align-items:flex-start;flex-wrap:wrap;margin-top:10px">
    <div style="margin-top:18px">
      <ConfidenceGauge dim={!onTrack} />
    </div>

    <div class="drive-grid" style="flex:1;min-width:420px">
      {#key app.unitsTick}
        <div class="readout">
          <div class="label">Speed</div>
          <div class="value" class:dim={!f}>{f ? ((f.speedMps ?? 0) * SPD().k).toFixed(0) : "–"} <span class="unit">{SPD().l}</span></div>
        </div>
        <div class="readout">
          <div class="label">Gear</div>
          <div class="value" class:dim={!f}>{f ? (f.gear === 0 ? "R" : f.gear === 11 ? "–" : f.gear) : "–"}</div>
          <div class="sub">{f ? `${(f.rpm ?? 0).toFixed(0)} / ${(f.maxRpm ?? 0).toFixed(0)} rpm` : ""}</div>
        </div>
        <div class="readout minor">
          <div class="label">Lap {f ? (f.lapNumber ?? 0) + 1 : "–"}</div>
          <div class="value" class:dim={!f}>{f ? fmtClock(f.currentLapS ?? 0) : "–"}</div>
        </div>
        <div class="readout">
          <div class="label">Lap times</div>
          <div class="trow">last <span class="num" class:dim={!(f && (f.lastLapS ?? 0) > 0)}>{f && (f.lastLapS ?? 0) > 0 ? fmtLap(f.lastLapS ?? 0) : "–"}</span></div>
          <div class="trow">best <span class="num" class:dim={!(f && (f.bestLapS ?? 0) > 0)}>{f && (f.bestLapS ?? 0) > 0 ? fmtLap(f.bestLapS ?? 0) : "–"}</span></div>
        </div>
        <div class="readout minor">
          <div class="label">Fuel</div>
          <div class="value" class:dim={!f}>{f ? `${((f.fuel ?? 0) * 100).toFixed(0)}%` : "–"}</div>
        </div>
        <div class="readout">
          <div class="label">Tires {tempLabel()}</div>
          <div class="temps">
            {#each f ? f.tireTempF : [null, null, null, null] as t, i (i)}
              <!-- thresholds in canonical °F, from the session compound's working band -->
              <div class={t == null ? "dim" : t < band[0] ? "cold" : t > band[1] ? "hot" : ""}>
                {t == null ? "–" : tempDisp(t).toFixed(0)}
              </div>
            {/each}
          </div>
        </div>
      {/key}
    </div>
  </div>
</div>
