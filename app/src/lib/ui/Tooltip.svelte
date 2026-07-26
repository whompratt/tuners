<script lang="ts">
  import type { Snippet } from "svelte";

  // Chart tooltip: absolutely positioned inside a relative wrap, flipping to
  // the left of the cursor near the right edge. The owner feeds cursor
  // coordinates and the wrap width; content is a snippet.
  let {
    shown,
    x,
    y,
    wrapWidth,
    flipAt = 180,
    children,
  }: {
    shown: boolean;
    x: number;
    y: number;
    wrapWidth: number;
    /** Flip to the cursor's left when closer than this to the right edge. */
    flipAt?: number;
    children: Snippet;
  } = $props();

  let el: HTMLDivElement | undefined = $state();
  let left = $derived(
    x > wrapWidth - flipAt ? x - (el?.offsetWidth ?? flipAt) - 12 : x + 12,
  );
</script>

<div
  class="tip"
  bind:this={el}
  style="display:{shown ? 'block' : 'none'};left:{left}px;top:{Math.max(0, y - 20)}px"
>
  {@render children()}
</div>
