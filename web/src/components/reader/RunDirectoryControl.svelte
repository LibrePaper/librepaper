<script>
  import { leaf } from "../../lib/reader/run-directory.js";
  import { m } from "../../lib/paraglide/messages.js";
  import Icon from "../Icon.svelte";

  // The folder a Quarto document's code runs in on this computer, shown beside
  // the Run code switch while that is on. Clicking it opens the Run directory
  // page in the preview; the state is shared with that page.
  let { directory: rd, onopen } = $props();

  const status = $derived(rd.status);
  const usable = $derived(rd.usable);
  const error = $derived(rd.error);
</script>

{#if rd.loaded}
  <div class="run-directory">
    {#if status}
      <button
        type="button"
        class="rd-chip"
        class:rd-set={usable}
        class:rd-none={!status.path}
        class:rd-missing={Boolean(status.path) && !usable}
        title={status.path || undefined}
        aria-label={m.rundirectorycontrol_change_aria()}
        onclick={() => onopen?.()}
      >
        <span class="rd-icon"><Icon name={status.path && !usable ? "triangle-alert" : "folder"} size="14" /></span>
        <span class="rd-name">{status.path ? leaf(status.path) : m.rundirectorycontrol_choose_chip()}</span>
        <span class="rd-icon"><Icon name="chevron-down" size="14" /></span>
      </button>
    {:else if error}
      <span class="rd-error" role="alert">{error}</span>
    {/if}
  </div>
{/if}

<style>
  .run-directory { display: inline-flex; align-items: center; min-width: 0; font-size: var(--text-sm); }
  .rd-chip {
    display: inline-flex; align-items: center; gap: calc(var(--spacing) * 1.5);
    max-width: 12rem; padding: calc(var(--spacing) * 0.5) calc(var(--spacing) * 2);
    font-size: var(--text-sm); line-height: 1.25rem; cursor: pointer;
    color: var(--color-text-secondary); background: transparent;
    border: 1px solid var(--color-border); border-radius: 999px;
  }
  .rd-chip:hover { background: var(--color-subtle); }
  .rd-chip:focus-visible { outline: 2px solid var(--color-brand); outline-offset: 1px; }
  .rd-chip.rd-none { color: var(--color-on-brand); background: var(--color-brand); border-color: var(--color-brand); }
  .rd-chip.rd-none:hover { background: var(--color-brand-hover); }
  .rd-chip.rd-missing { color: var(--color-warning-text); border-color: var(--color-warning-solid); }
  .rd-icon { display: inline-flex; flex: none; }
  .rd-name { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .rd-error { margin: 0; color: var(--color-danger, var(--color-text)); }
</style>
