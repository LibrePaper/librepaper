<script>
  import { Tabs } from "@skeletonlabs/skeleton-svelte";
  import { m } from "../../lib/paraglide/messages.js";

  // Preview or source, for the phone. It is the same tab strip the side panels
  // use (`.panel-tabs` in the stylesheet, on the same Skeleton Tabs), so the
  // selected tab gets the same primary-coloured underline and nothing else, and
  // the arrow keys move between the two. These tabs control the preview and
  // the editor elsewhere on the page, so this renders no panels of its own; the
  // ids below are what those panels are labelled and paired by. When
  // `collapsed`, the row slides out of view while nobody is using it, which
  // gives the document back its room; it comes back the moment focus lands in
  // it, so a keyboard or switch user is never left without it.
  let { mode = "document", collapsed = false, onchange = () => {} } = $props();
</script>

<div class="mobile-mode-switch" class:collapsed>
  <Tabs
    class="mobile-mode-tabs"
    value={mode}
    onValueChange={(event) => onchange(event.value)}
    ids={{ trigger: (one) => `mode-tab-${one}`, content: (one) => (one === "source" ? "reader-source" : "reader-document") }}
  >
    <Tabs.List class="panel-tabs" aria-label={m.mobilenav_mode_switch()}>
      <Tabs.Trigger value="document" data-mode="document">{m.common_preview()}</Tabs.Trigger>
      <Tabs.Trigger value="source" data-mode="source">{m.common_source()}</Tabs.Trigger>
    </Tabs.List>
  </Tabs>
</div>

<style>
  .mobile-mode-switch {
    position: relative;
    z-index: 1;
    display: flex;
    align-items: stretch;
    /* The 1px divider is added to the row so each tab keeps a 44px target. */
    height: calc(2.75rem + 1px);
    border-bottom: 1px solid var(--color-divider);
    background: var(--color-shell);
  }

  .mobile-mode-switch.collapsed:not(:focus-within) {
    margin-top: calc(-2.75rem - 1px);
    visibility: hidden;
  }

  @media (prefers-reduced-motion: no-preference) {
    .mobile-mode-switch {
      transition: margin-top 180ms ease, visibility 180ms ease;
    }
  }

  /* The root and the list fill the row, so each tab is the full 44px target. */
  .mobile-mode-switch :global(.mobile-mode-tabs) {
    height: 100%;
    width: 100%;
  }

  .mobile-mode-switch :global(.panel-tabs) {
    height: 100%;
    width: 100%;
  }

  .mobile-mode-switch :global([role="tab"]) {
    display: flex;
    align-items: center;
    justify-content: center;
    font-size: 0.875rem;
  }
</style>
