<script lang="ts">
  import type { Snippet } from "svelte";

  // The blessed chart pattern (plan 010 phase 2): THIS host is the only code
  // that ever sizes the canvas — sizing clears it, so draw functions must be
  // pure renderers. `draw` closes over data + hover state; a new closure
  // identity (or a resize) triggers a full re-render.
  let {
    height,
    draw,
    onmove,
    onleave,
    children,
  }: {
    height: number;
    draw: (ctx: CanvasRenderingContext2D, cssW: number) => void;
    /** Cursor moved: px/py relative to the canvas, plus its current width. */
    onmove?: (px: number, py: number, cssW: number) => void;
    onleave?: () => void;
    children?: Snippet;
  } = $props();

  let canvas: HTMLCanvasElement | undefined = $state();

  function render() {
    if (!canvas) return;
    const dpr = window.devicePixelRatio || 1;
    const cssW = canvas.clientWidth;
    canvas.width = cssW * dpr;
    canvas.height = height * dpr;
    const ctx = canvas.getContext("2d")!;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, cssW, height);
    draw(ctx, cssW);
  }

  $effect(() => {
    void draw;
    render();
  });

  $effect(() => {
    if (!canvas) return;
    const ro = new ResizeObserver(() => render());
    ro.observe(canvas);
    return () => ro.disconnect();
  });
</script>

<div
  style="position:relative"
  onmousemove={(e) => {
    if (!canvas || !onmove) return;
    const r = canvas.getBoundingClientRect();
    onmove(e.clientX - r.left, e.clientY - r.top, canvas.clientWidth);
  }}
  onmouseleave={() => onleave?.()}
  role="img"
>
  <canvas bind:this={canvas} style="height:{height}px"></canvas>
  {@render children?.()}
</div>
