<script>
  import { Tooltip } from "@skeletonlabs/skeleton-svelte";
  import { label } from "../lib/commands.js";

  // The tooltip every icon control shows: what it is, and beneath it the
  // shortcut as a keycap when it has one. Its trigger is the control itself, so
  // the caller hands the trigger's attributes to the element it draws.
  //
  // `shortcut` is a binding from lib/commands.js. The caller passes null where
  // the key belongs to a modal editor mode (Vim, Emacs), as the menus do.
  let { text, shortcut = null, children } = $props();
</script>

<Tooltip openDelay={150} closeDelay={100} positioning={{ placement: "bottom" }}>
  <Tooltip.Trigger>
    {#snippet element(attributes)}
      {@render children(attributes)}
    {/snippet}
  </Tooltip.Trigger>
  <Tooltip.Positioner class="z-50">
    <Tooltip.Content class="card lp-control-neutral px-2 py-1 text-xs shadow-lg">
      <span class="tip-text">{text}</span>{#if shortcut}<kbd class="tip-key">{label(shortcut)}</kbd>{/if}
    </Tooltip.Content>
  </Tooltip.Positioner>
</Tooltip>

<style>
  .tip-text { white-space: nowrap; }
  .tip-key {
    margin-left: calc(var(--spacing) * 2);
    padding: 0 calc(var(--spacing) * 1);
    border: 1px solid var(--color-border);
    border-bottom-width: 2px;
    border-radius: var(--radius-base);
    background: var(--color-raised);
    color: inherit;
    font-family: inherit;
    font-size: var(--text-xs);
    line-height: 1.5;
    white-space: nowrap;
  }
</style>
