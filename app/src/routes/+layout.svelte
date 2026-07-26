<script lang="ts">
  import "../app.css";
  import { onMount } from "svelte";
  import { page } from "$app/state";
  import { events } from "$lib/bindings";
  import { app, loadAdvice, loadPending, loadSession, loadStints } from "$lib/app.svelte";
  import { initAdvanced } from "$lib/advanced.svelte";
  import { initOnboarding, onboarding } from "$lib/onboarding.svelte";
  import DialogHost from "$lib/ui/DialogHost.svelte";
  import Onboarding from "$lib/components/Onboarding.svelte";

  let { children } = $props();

  const NAV = [
    ["/", "Home"],
    ["/drive", "Drive"],
    ["/setup", "Setup"],
    ["/analysis", "Analysis"],
    ["/projects", "Projects"],
  ] as const;

  onMount(() => {
    initAdvanced();
    (async () => {
      await loadStints(true);
      await loadSession();
      await loadPending();
      app.booted = true;
      initOnboarding(!!app.session && app.session.car != null);
      // Advice on launch feeds all three registers; recomputed on run close.
      loadAdvice();
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
      for (const s of subs) s.then((un) => un());
    };
  });
</script>

<DialogHost />
{#if onboarding.open}<Onboarding />{/if}
<nav class="rail">
  <div class="rail-brand" title="FH6 tuning assistant">tuners</div>
  {#each NAV as [href, label] (href)}
    <a {href} class:active={page.url.pathname === href}>{label}</a>
  {/each}
</nav>
{@render children()}
