<script lang="ts">
  import type { Snippet } from "svelte";

  // Card primitive: optional header row (title + trailing actions) over a
  // free-form body. Panels with bespoke headers keep using .panel directly.
  let {
    title,
    sub,
    actions,
    children,
  }: { title?: string; sub?: string; actions?: Snippet; children: Snippet } = $props();
</script>

<div class="panel">
  {#if title || actions}
    <div style="display:flex;align-items:baseline;gap:12px;flex-wrap:wrap">
      {#if title}<h2 style="margin:0">{title}</h2>{/if}
      {#if sub}<span style="color:var(--muted);font-size:13px">{sub}</span>{/if}
      <span style="flex:1"></span>
      {@render actions?.()}
    </div>
  {/if}
  {@render children()}
</div>
