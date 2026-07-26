<script lang="ts">
  import { app, show, pick, deleteStint, exportBundle } from "$lib/app.svelte";

  const carLabel = (s: { carName: string; car: number }) => s.carName || `car #${s.car}`;

  // ordinal -> label, insertion-ordered
  let cars = $derived.by(() => {
    const m = new Map<number, string>();
    for (const s of app.stints) m.set(s.car, carLabel(s));
    return m;
  });

  const stamp = (f: string) => (f.match(/(\d{8}-\d{6})/) || [])[1] || "";

  // Scope to the active campaign: stints of the session car from before the
  // campaign started belong to archived sessions — tucked behind a toggle.
  let split = $derived.by(() => {
    let shown = app.stints.filter((s) => app.carFilter === "all" || String(s.car) === app.carFilter);
    let earlier: typeof shown = [];
    if (
      app.session?.campaignStart &&
      String(app.session.car) === app.carFilter &&
      !app.showEarlierStints
    ) {
      const start = app.session.campaignStart;
      earlier = shown.filter((s) => stamp(s.file) < start);
      shown = shown.filter((s) => stamp(s.file) >= start);
    }
    return { shown: [...shown].reverse(), earlier };
  });
</script>

<aside>
  <h1>tuners <span>FH6 tuning assistant</span></h1>
  {#if app.stints.length}
    <select id="car-filter" bind:value={app.carFilter}>
      <option value="all">all cars ({app.stints.length} stints)</option>
      {#each cars as [ord, name] (ord)}
        <option value={String(ord)}>{name}</option>
      {/each}
    </select>
    <div id="sessions">
      {#if !split.shown.length && !split.earlier.length}
        <div class="placeholder">no stints for this car</div>
      {/if}
      {#each split.shown as s (s.file)}
        <div
          class="session"
          class:active={s.file === app.shownFile}
          onclick={() => show(s.file)}
          role="button"
          tabindex="0"
          onkeydown={(e) => e.key === "Enter" && show(s.file)}
        >
          {s.file.split("/").pop()}<br />
          <span class="car">{carLabel(s)}</span> ·
          <span class="size">{(s.bytes / 1e6).toFixed(1)} MB</span>
          <span class="ab">
            <button
              class:on-a={s.file === app.selA}
              title="compare as A"
              onclick={(e) => { e.stopPropagation(); pick("a", s.file); }}>A</button>
            <button
              class:on-b={s.file === app.selB}
              title="compare as B"
              onclick={(e) => { e.stopPropagation(); pick("b", s.file); }}>B</button>
            <button
              class="exp"
              title="export telemetry bundle (raw stint + tune history, no free text)"
              onclick={(e) => { e.stopPropagation(); exportBundle(s.file); }}>⇩</button>
            <button
              class="del"
              title="delete this stint"
              onclick={(e) => { e.stopPropagation(); deleteStint(s.file); }}>×</button>
          </span>
        </div>
      {/each}
      {#if split.earlier.length}
        <div
          class="placeholder"
          style="cursor:pointer"
          onclick={() => (app.showEarlierStints = true)}
          role="button"
          tabindex="0"
          onkeydown={(e) => e.key === "Enter" && (app.showEarlierStints = true)}
        >
          show {split.earlier.length} stint{split.earlier.length === 1 ? "" : "s"} from earlier sessions of this car
        </div>
      {:else if app.showEarlierStints && app.session && String(app.session.car) === app.carFilter}
        <div
          class="placeholder"
          style="cursor:pointer"
          onclick={() => (app.showEarlierStints = false)}
          role="button"
          tabindex="0"
          onkeydown={(e) => e.key === "Enter" && (app.showEarlierStints = false)}
        >
          hide earlier-session stints
        </div>
      {/if}
    </div>
  {:else}
    <div class="placeholder">no stint recordings yet — drive with the app running</div>
  {/if}
</aside>
