<script>
  import Icon from "./Icon.svelte";
  let { label = "", tone = "neutral", busy = false, details } = $props();
</script>

{#if label}
  <details class="preview-status" class:warning={tone === "warning"} class:error={tone === "error"}>
    <summary title="Preview status">
      {#if busy}<span class="spinner" aria-hidden="true"></span>
      {:else if tone === "error" || tone === "warning"}<Icon name="triangle-alert" />
      {:else}<Icon name="check" />{/if}
      <span>{label}</span>
      <Icon name="chevron-down" />
    </summary>
    <div class="preview-status-popover">{@render details?.()}</div>
  </details>
{/if}

<style>
  .preview-status { position: relative; flex: none; color: var(--color-surface-700-300); }
  .preview-status summary { display: inline-flex; align-items: center; gap: var(--spacing); padding: calc(var(--spacing) * .5) var(--spacing); border-radius: var(--radius-base); cursor: pointer; list-style: none; font-size: var(--text-xs); }
  .preview-status summary::-webkit-details-marker { display: none; }
  .preview-status summary:hover, .preview-status[open] summary { background: var(--color-surface-100-900); }
  .preview-status summary :global(svg) { width: .875rem; height: .875rem; }
  .preview-status.warning { color: var(--color-warning-700-300); }
  .preview-status.error { color: var(--color-error-600-400); }
  .preview-status-popover { position: absolute; z-index: 30; top: calc(100% + var(--spacing)); right: 0; width: min(32rem, 80vw); padding: calc(var(--spacing) * 3); border: 1px solid var(--color-divider); border-radius: var(--radius-container); background: var(--color-surface-50-950); color: var(--color-surface-800-200); box-shadow: var(--shadow-xl); font-size: var(--text-sm); }
</style>
