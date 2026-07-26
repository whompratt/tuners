<script lang="ts">
  import { onMount } from "svelte";
  import { commands, events, type SessionView, type StintRow, type LiveStateView } from "$lib/bindings";

  // Phase 1a placeholder: proves the typed command/event wiring end to end.
  // The real screens land with the parity port (plan 010 phase 1b).
  let session: SessionView | null = $state(null);
  let stints: StintRow[] = $state([]);
  let live: LiveStateView | null = $state(null);

  onMount(() => {
    commands.session().then((s) => (session = s));
    commands.stints().then((rows) => (stints = rows));
    const un = events.liveStateEvent.listen((e) => (live = e.payload));
    return () => {
      un.then((f) => f());
    };
  });
</script>

<main>
  <h1>tuners</h1>
  <p>
    {#if session?.carName}
      session: {session.carName}
    {:else}
      no active session
    {/if}
    · {stints.length} runs on disk
  </p>
  <p>
    recorder: {live?.recorder.mode ?? "starting…"}
    {#if live?.recorder.file}
      — {live.recorder.file} ({live.recorder.packets} packets)
    {/if}
  </p>
</main>

<style>
  main {
    font-family: system-ui, sans-serif;
    padding: 2rem;
  }
</style>
