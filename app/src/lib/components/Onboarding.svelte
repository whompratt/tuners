<script lang="ts">
  import { goto } from "$app/navigation";
  import { app, loadSession } from "$lib/app.svelte";
  import { commands } from "$lib/bindings";
  import { finishOnboarding } from "$lib/onboarding.svelte";
  import { alertDialog } from "$lib/ui/dialogs.svelte";
  import Button from "$lib/ui/Button.svelte";

  const STALE_MS = 3000;

  let step = $state(0);
  let name = $state("");
  let manualCar = $state("");
  let creating = $state(false);

  // Live hookup status: the game streams zeroed packets in menus (raceOn
  // off), so "receiving" means the wiring works and "car detected" needs an
  // actual driving frame.
  let fresh = $derived(app.live?.ageMs != null && app.live.ageMs < STALE_MS);
  let receiving = $derived(fresh && !!app.live?.frame);
  let liveCar = $derived.by(() => {
    const f = app.live?.frame;
    if (!fresh || !f?.raceOn || !f.car) return null;
    return { car: f.car, name: f.carName };
  });

  // Once seen, the detected car sticks (the user pauses the game to click).
  let detected = $state<{ car: number; name: string | null } | null>(null);
  $effect(() => {
    if (liveCar) detected = liveCar;
  });
  // Fallback for step 3 when nothing was detected: the most recent run's car.
  let lastRunCar = $derived.by(() => {
    const s = app.stints.at(-1);
    return s ? { car: s.car, name: s.carName || null } : null;
  });
  let chosen = $derived(detected ?? lastRunCar);
  const carLabel = (c: { car: number; name: string | null }) => c.name || `car #${c.car}`;

  async function createProject() {
    const car = manualCar.trim() || (chosen ? String(chosen.car) : "");
    if (!car) return;
    creating = true;
    const r = await commands.updateSession(false, car, [["name", name.trim()]]);
    creating = false;
    if (r.status === "error") {
      await alertDialog("Create project failed", r.error.message);
      return;
    }
    await loadSession();
    step = 3;
  }

  function enterBaseline() {
    finishOnboarding();
    app.entered = true;
    goto("/setup");
  }
</script>

<div class="wiz-backdrop">
  <div class="wiz" role="dialog" aria-modal="true" aria-label="first-time setup">
    <div class="wiz-progress">
      {#each ["welcome", "telemetry", "project", "baseline"] as label, i (label)}
        <span class:done={i < step} class:current={i === step}>{label}</span>
      {/each}
    </div>

    {#if step === 0}
      <h2>Welcome to tuners</h2>
      <p>
        tuners watches your Forza Horizon 6 driving and tells you which single
        setup change to try next — then measures whether it actually helped.
      </p>
      <p class="muted">
        Three things to set up, once: telemetry from the game, a project for
        your car, and the tune it's currently running.
      </p>
      <div class="wiz-actions">
        <Button go onclick={() => (step = 1)}>get started</Button>
        <button class="wiz-skip" onclick={finishOnboarding}>skip — I know what I'm doing</button>
      </div>
    {:else if step === 1}
      <h2>Hook up the telemetry</h2>
      <p>In Forza Horizon 6, set these once:</p>
      <ol>
        <li>Settings → HUD and Gameplay → <b>Data Out</b> → ON</li>
        <li>IP address <b>127.0.0.1</b> (this PC; use this PC's LAN IP from a console)</li>
        <li>Port <b>20440</b></li>
        <li><b>Restart the game fully</b> — it only honours new Data Out settings after a restart</li>
      </ol>
      <div class="wiz-live" class:ok={receiving}>
        {#if !receiving}
          <span class="pulse">●</span> listening on port 20440…
        {:else if liveCar || detected}
          ✓ receiving — car detected: <b>{carLabel((liveCar ?? detected)!)}</b>
        {:else}
          ✓ receiving — now drive (menus send empty packets; the car shows up on the first driving frame)
        {/if}
      </div>
      {#if app.live?.recorder.mode === "external"}
        <p class="muted">
          Another capture owns the telemetry port right now — close it so the app can record for itself.
        </p>
      {/if}
      <div class="wiz-actions">
        <Button onclick={() => (step = 0)}>back</Button>
        <Button go={!!detected} onclick={() => (step = 2)}>continue</Button>
        <button class="wiz-skip" onclick={finishOnboarding}>skip setup</button>
      </div>
    {:else if step === 2}
      <h2>Create your project</h2>
      <p>
        A project is one build of one car — its setup history and advice live
        here. Start a new project when the car or its upgrades change, not
        just the tune.
      </p>
      {#if chosen}
        <p>Car: <b>{carLabel(chosen)}</b> {detected ? "(detected from telemetry)" : "(from your last recorded run)"}</p>
      {:else}
        <p class="muted">
          No car detected yet — go back and drive a few seconds, or enter the
          car ordinal by hand below.
        </p>
      {/if}
      <div class="wiz-form">
        <label>
          project name (optional)
          <input type="text" placeholder="e.g. rwd no-aero build" bind:value={name} />
        </label>
        {#if !chosen}
          <label>
            car ordinal
            <input type="number" bind:value={manualCar} />
          </label>
        {/if}
      </div>
      <div class="wiz-actions">
        <Button onclick={() => (step = 1)}>back</Button>
        <Button go disabled={creating || (!chosen && !manualCar.trim())} onclick={createProject}>
          {creating ? "creating…" : "create project"}
        </Button>
        <button class="wiz-skip" onclick={finishOnboarding}>skip setup</button>
      </div>
    {:else}
      <h2>Copy your tune in</h2>
      <p>
        Last step: transcribe the car's current tune from the game's tuning
        screens — the Setup screen walks you through them one by one, in the
        same order as the game.
      </p>
      <p class="muted">
        Advice stays locked until this baseline exists: the app can't reason
        about a setup it can't see.
      </p>
      <div class="wiz-actions">
        <Button go onclick={enterBaseline}>enter my tune</Button>
        <button class="wiz-skip" onclick={finishOnboarding}>later</button>
      </div>
    {/if}
  </div>
</div>
