<script lang="ts">
  import "../app.css";
  import { onMount } from "svelte";
  import { page } from "$app/state";
  import { getVersion } from "@tauri-apps/api/app";
  import { events } from "$lib/bindings";
  import { app, loadAdvice, loadPending, loadSession, loadStints } from "$lib/app.svelte";
  import { initPrefs } from "$lib/prefs.svelte";
  import { initOnboarding, onboarding } from "$lib/onboarding.svelte";
  import { checkForUpdate } from "$lib/update";
  import DialogHost from "$lib/ui/DialogHost.svelte";
  import Onboarding from "$lib/components/Onboarding.svelte";

  let { children } = $props();
  let version = $state("");

  const NAV = [
    ["/", "Home"],
    ["/drive", "Drive"],
    ["/setup", "Setup"],
    ["/analysis", "Analysis"],
    ["/projects", "Projects"],
    ["/settings", "Settings"],
  ] as const;

  onMount(() => {
    initPrefs();
    // Last-resort net: anything the loaders' own catches miss surfaces as
    // the banner instead of wedging navigation as a silent dead click.
    const onRejection = (e: PromiseRejectionEvent) => {
      console.error("unhandled rejection", e.reason);
      app.loadError = String((e.reason as { message?: string })?.message ?? e.reason);
      e.preventDefault();
    };
    window.addEventListener("unhandledrejection", onRejection);
    getVersion().then((v) => (version = v));
    (async () => {
      await loadStints(true);
      await loadSession();
      await loadPending();
      app.booted = true;
      initOnboarding(!!app.session && app.session.car != null);
      // Advice on launch feeds all three registers; recomputed on run close.
      loadAdvice();
      checkForUpdate();
    })();
    const subs = [
      events.liveStateEvent.listen((e) => (app.live = e.payload)),
      events.qualityEvent.listen((e) => (app.quality = e.payload)),
      // rotation (new recording opened): refresh run lists
      events.runsChangedEvent.listen(() => loadStints(true)),
      // a run closed: its verdict and the pending basket both changed
      events.runFinishedEvent.listen(async () => {
        await loadStints(true);
        await loadAdvice();
      }),
    ];
    return () => {
      window.removeEventListener("unhandledrejection", onRejection);
      for (const s of subs) s.then((un) => un());
    };
  });
</script>

<DialogHost />
{#if app.loadError}
  <div class="load-error" role="alert">
    <span>{app.loadError}</span>
    <button onclick={() => (app.loadError = "")}>dismiss</button>
  </div>
{/if}
{#if onboarding.open}<Onboarding />{/if}
<nav class="rail">
  <div class="rail-brand" title="FH6 tuning assistant">tuners</div>
  {#each NAV as [href, label] (href)}
    <a {href} class:active={page.url.pathname === href}>{label}</a>
  {/each}
  {#if version}<div class="rail-version">v{version}</div>{/if}
</nav>
{@render children()}
