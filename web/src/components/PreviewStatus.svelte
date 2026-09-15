<script>
  import { Popover, Portal } from "@skeletonlabs/skeleton-svelte";
  import Icon from "./Icon.svelte";

  // How the last build went, and -- behind it -- what the build actually
  // said. This was a <details> with a panel absolutely positioned under it,
  // which is a disclosure pretending to be a popover: it never flipped when
  // there was no room below, it measured its width against the window rather
  // than the pane it opens in, and <details> closes for neither a click
  // outside it nor Escape, so the panel stayed over the document until the
  // summary was clicked again. All three are Zag's to answer.
  let { label = "", tone = "neutral", busy = false, details } = $props();
</script>

{#if label}
  <Popover
    positioning={{ placement: "bottom-end", gutter: 4, flip: true, fitViewport: true, overflowPadding: 8 }}
  >
    <!-- The button is authored here rather than handed a `class`: a class
         arriving as a prop carries no scope hash, so the rules below would
         have to be global to paint at all. -->
    <Popover.Trigger>
      {#snippet element(attributes)}
        <button {...attributes} class="preview-status-trigger" class:warning={tone === "warning"}
                class:error={tone === "error"} title="Preview status">
          {#if busy}<span class="spinner" aria-hidden="true"></span>
          {:else if tone === "error" || tone === "warning"}<Icon name="triangle-alert" />
          {:else}<Icon name="check" />{/if}
          <span>{label}</span>
          <Icon name="chevron-down" />
        </button>
      {/snippet}
    </Popover.Trigger>
    <!-- Sent to the body: the preview header is a positioned, clipping row,
         and a panel this wide opened from inside it was confined to it. -->
    <Portal>
      <Popover.Positioner class="preview-status-positioner">
        <Popover.Content class="preview-status-popover">
          {@render details?.()}
        </Popover.Content>
      </Popover.Positioner>
    </Portal>
  </Popover>
{/if}

<style>
  .preview-status-trigger { display: inline-flex; align-items: center; gap: var(--spacing); flex: none; padding: calc(var(--spacing) * .5) var(--spacing); border-radius: var(--radius-base); cursor: pointer; font-size: var(--text-xs); color: var(--color-surface-700-300); }
  .preview-status-trigger:hover, .preview-status-trigger[data-state="open"] { background: var(--color-surface-100-900); }
  .preview-status-trigger :global(svg) { width: .875rem; height: .875rem; }
  .preview-status-trigger.warning { color: var(--color-warning-700-300); }
  .preview-status-trigger.error { color: var(--color-error-600-400); }
  /* The panel's width is the pane's business, not the window's: it is capped
     against the positioner's available width, which Zag measures. */
  :global(.preview-status-positioner) { z-index: 30; }
  :global(.preview-status-popover) { width: min(32rem, var(--available-width, 80vw)); max-height: var(--available-height); overflow-y: auto; padding: calc(var(--spacing) * 3); border: 1px solid var(--color-divider); border-radius: var(--radius-container); background: var(--color-surface-50-950); color: var(--color-surface-800-200); box-shadow: var(--shadow-xl); font-size: var(--text-sm); }
</style>
