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

  let open = $state(false);
  const close = () => (open = false);

  // Close the popover when focus moves to the preview frame. The preview is
  // a cross-origin iframe, so clicks in it are invisible to Zag's
  // click-outside detection, and the window losing focus to a frame is the
  // only sign the user clicked the preview.
  $effect(() => {
    if (!open) return;
    const listener = () => {
      if (document.activeElement?.tagName === "IFRAME") {
        close();
      }
    };
    window.addEventListener("blur", listener);
    return () => window.removeEventListener("blur", listener);
  });
</script>

{#if label}
  <Popover
    positioning={{ placement: "bottom-end", gutter: 4, flip: true, fitViewport: true, overflowPadding: 8 }}
    {open}
    onOpenChange={(event) => (open = event.open)}
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
          <span class="preview-status-label">{label}</span>
          <Icon name="chevron-down" />
        </button>
      {/snippet}
    </Popover.Trigger>
    <!-- Sent to the body: this sits in the bar along the top of the window,
         a positioned, clipping row, and a panel this wide opened from inside
         it was confined to it. -->
    <Portal>
      <Popover.Positioner>
        <Popover.Content class="preview-status-popover">
          {@render details?.(close)}
        </Popover.Content>
      </Popover.Positioner>
    </Portal>
  </Popover>
{/if}

<style>
  .preview-status-trigger { display: inline-flex; align-items: center; gap: var(--spacing); flex: none; padding: calc(var(--spacing) * .5) var(--spacing); border-radius: var(--radius-base); cursor: pointer; font-size: var(--text-xs); color: var(--color-text-secondary); }
  .preview-status-trigger:hover, .preview-status-trigger[data-state="open"] { background: var(--color-subtle); }
  .preview-status-trigger :global(svg) { width: .875rem; height: .875rem; }
  .preview-status-trigger.warning { color: var(--color-warning-text); }
  .preview-status-trigger.error { color: var(--color-error-text); }
  /* On a narrow bar the icon carries it. The label is the longer half of this
     control, and the bar has a document title to keep. */
  @media (max-width: 600px) {
    .preview-status-label { display: none; }
  }
  /* The panel's width is the pane's business, not the window's: it is capped
     against the positioner's available width, which Zag measures. The
     z-index is on the content because Zag writes the positioner's inline
     from the content's computed value; a class on the positioner lost. */
  :global(.preview-status-popover) { z-index: 30; width: min(32rem, var(--available-width, 80vw)); max-height: var(--available-height); overflow-y: auto; padding: calc(var(--spacing) * 3); border: 1px solid var(--color-divider); border-radius: var(--radius-container); background: var(--color-raised); color: var(--color-text); box-shadow: var(--shadow-xl); font-size: var(--text-sm); }
</style>
