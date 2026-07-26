<script lang="ts">
  import { onMount } from "svelte";
  import { events } from "$lib/bindings";
  import { app, loadSession, loadStints } from "$lib/app.svelte";
  import Sidebar from "$lib/components/Sidebar.svelte";
  import SessionPanel from "$lib/components/SessionPanel.svelte";
  import LivePanel from "$lib/components/LivePanel.svelte";
  import ComparePanel from "$lib/components/ComparePanel.svelte";
  import AdvicePanel from "$lib/components/AdvicePanel.svelte";
  import LapChartPanel from "$lib/components/LapChartPanel.svelte";

  onMount(() => {
    (async () => {
      await loadStints(true);
      await loadSession();
    })();
    const subs = [
      events.liveStateEvent.listen((e) => (app.live = e.payload)),
      events.qualityEvent.listen((e) => (app.quality = e.payload)),
      // rotation (new recording opened) or a finished run: refresh the sidebar
      events.runsChangedEvent.listen(() => loadStints(true)),
      events.runFinishedEvent.listen(() => loadStints(true)),
    ];
    return () => {
      for (const s of subs) s.then((un) => un());
    };
  });
</script>

<Sidebar />
<main id="top">
  <SessionPanel />
  <LivePanel />
  <ComparePanel />
  <AdvicePanel />
  <LapChartPanel />
  <pre class={app.reportPlaceholder ? "placeholder" : ""}>{app.report}</pre>
</main>
