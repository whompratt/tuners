<script lang="ts">
  import { app } from "$lib/app.svelte";
  import Sidebar from "$lib/components/Sidebar.svelte";
  import ComparePanel from "$lib/components/ComparePanel.svelte";
  import AdvicePanel from "$lib/components/AdvicePanel.svelte";
  import LapChartPanel from "$lib/components/LapChartPanel.svelte";
</script>

<div class="screen-cols">
  <Sidebar />
  <main class="screen">
    {#if app.booted && !app.stints.length}
      <div class="hero">No runs recorded yet.</div>
      <div style="margin-top:8px;color:var(--muted);max-width:560px">
        Recording is armed whenever the app is open. Drive in FH6 and each
        outing shows up here as a run: lap chart, A/B comparison, and the
        evidence behind every suggestion. Check the <a href="/drive">Drive view</a>
        to confirm telemetry is arriving.
      </div>
    {:else}
      <ComparePanel />
      <AdvicePanel />
      <LapChartPanel />
      <pre class={app.reportPlaceholder ? "placeholder" : ""}>{app.report}</pre>
    {/if}
  </main>
</div>
