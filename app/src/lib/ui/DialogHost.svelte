<script lang="ts">
  import { dialogState, settle } from "./dialogs.svelte";
  import Button from "./Button.svelte";

  function onkeydown(e: KeyboardEvent) {
    if (!dialogState.current) return;
    if (e.key === "Escape") settle(false);
    if (e.key === "Enter" && dialogState.current.alert) settle(true);
  }
</script>

<svelte:window {onkeydown} />

{#if dialogState.current}
  {@const d = dialogState.current}
  <!-- Escape closes (window handler); backdrop click cancels. -->
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="dlg-backdrop" onclick={() => settle(false)}>
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
    <div
      class="dlg"
      onclick={(e) => e.stopPropagation()}
      role="dialog"
      aria-modal="true"
      aria-label={d.title}
      tabindex="-1"
    >
      <h3>{d.title}</h3>
      <div class="dlg-body">{d.body}</div>
      <div class="dlg-actions">
        {#if d.alert}
          <Button go onclick={() => settle(true)}>OK</Button>
        {:else}
          <Button onclick={() => settle(false)}>{d.cancel ?? "Cancel"}</Button>
          <Button go={!d.danger} danger={d.danger} onclick={() => settle(true)}>{d.verb}</Button>
        {/if}
      </div>
    </div>
  </div>
{/if}
