<script lang="ts">
  import type { Snippet } from "svelte";
  import type { HTMLButtonAttributes } from "svelte/elements";

  let {
    go = false,
    danger = false,
    active = false,
    children,
    ...rest
  }: { go?: boolean; danger?: boolean; active?: boolean; children: Snippet } & HTMLButtonAttributes =
    $props();
</script>

<button
  class="btn"
  class:btn-go={go}
  class:btn-danger={danger}
  class:btn-active={active}
  aria-pressed={active || undefined}
  {...rest}
>
  {@render children()}
</button>
