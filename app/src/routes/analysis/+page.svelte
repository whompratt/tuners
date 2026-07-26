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
    <ComparePanel />
    <AdvicePanel />
    <LapChartPanel />
    <pre class={app.reportPlaceholder ? "placeholder" : ""}>{app.report}</pre>
  </main>
</div>
