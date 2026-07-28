<script lang="ts">
  import { type CarView } from "$lib/bindings";
  import { allCars, filterCars } from "$lib/cars";

  let {
    onpick,
    placeholder = "type to search, e.g. ford gt",
    id = undefined,
  }: {
    onpick: (c: CarView) => void;
    placeholder?: string;
    id?: string;
  } = $props();

  let cars: CarView[] = $state([]);
  allCars().then((v) => (cars = v));

  let q = $state("");
  let focused = $state(false);
  let matches = $derived(filterCars(cars, q));

  function pick(c: CarView) {
    onpick(c);
    q = "";
  }
</script>

<div class="car-picker">
  <input
    {id}
    type="text"
    {placeholder}
    autocomplete="off"
    bind:value={q}
    onfocus={() => (focused = true)}
    onblur={() => (focused = false)}
    onkeydown={(e) => {
      if (e.key === "Enter") {
        e.preventDefault();
        if (matches.length) pick(matches[0]);
      } else if (e.key === "Escape") q = "";
    }}
  />
  {#if focused && q.trim()}
    <div class="car-picker-drop">
      {#each matches as c (c.car)}
        <!-- mousedown, not click: blur fires first and would unmount the list -->
        <button type="button" onmousedown={(e) => { e.preventDefault(); pick(c); }}>
          {c.name}
        </button>
      {:else}
        <div class="car-picker-none">no car matches "{q.trim()}"</div>
      {/each}
    </div>
  {/if}
</div>

<style>
  .car-picker { position: relative; }
  .car-picker-drop {
    position: absolute; top: calc(100% + 2px); left: 0; right: 0; z-index: 30;
    background: var(--surface); border: 1px solid var(--border); border-radius: 6px;
    max-height: 240px; overflow-y: auto; box-shadow: 0 6px 18px rgba(0, 0, 0, 0.45);
  }
  .car-picker-drop button {
    display: block; width: 100%; text-align: left; font: 13px var(--font-ui, system-ui);
    background: none; border: none; color: var(--ink-2); padding: 6px 8px; cursor: pointer;
  }
  .car-picker-drop button:hover { background: var(--page); color: var(--ink); }
  .car-picker-none { font-size: 13px; color: var(--muted); padding: 6px 8px; }
</style>
