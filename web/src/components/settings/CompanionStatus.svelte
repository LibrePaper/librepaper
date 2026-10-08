<script>
  // The settings sidebar footer: whether the companion is running, as one line
  // that opens the Companion page. It watches the status and never probes.
  import StatusPill from "./StatusPill.svelte";
  import { companion } from "../../lib/companion/status.svelte.js";
  import { STATES } from "../../lib/companion/states.js";

  let { onopen = undefined } = $props();

  $effect(() => companion.watch());
  // No version here: the sidebar is narrow, and the Companion page shows it.
  const wording = $derived(STATES[companion.status?.state] || STATES.unknown);
</script>

<button type="button" id="settings-companion" class="companion-status" onclick={() => onopen?.()}>
  <span class="companion-status-label">Companion</span>
  <StatusPill label={wording.says} tone={wording.tone} accessibleLabel={`LibrePaper Companion: ${wording.says}`} />
</button>

<style>
  .companion-status {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: calc(var(--spacing) * 1.5);
    width: 100%;
    padding: calc(var(--spacing) * 1.5) calc(var(--spacing) * 1);
    border: 0;
    border-radius: var(--radius-base);
    background: none;
    font: inherit;
    font-size: var(--text-sm);
    text-align: left;
    cursor: pointer;
  }
  .companion-status:hover { background-color: var(--color-row-hover); }
  .companion-status-label { color: var(--color-text-secondary); }
  .companion-status :global(.setting-status-pill) { white-space: nowrap; flex-shrink: 0; }
</style>
