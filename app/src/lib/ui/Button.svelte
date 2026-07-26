<script lang="ts">
  import type { Snippet } from "svelte";
  import type { HTMLButtonAttributes } from "svelte/elements";

  let {
    go = false,
    danger = false,
    children,
    ...rest
  }: { go?: boolean; danger?: boolean; children: Snippet } & HTMLButtonAttributes = $props();
</script>

<button class="btn" class:btn-go={go} class:btn-danger={danger} {...rest}>
  {@render children()}
</button>
