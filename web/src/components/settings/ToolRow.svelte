<script>
  // One program on this computer (a renderer, Zotero, an AI agent): what it
  // is, whether it was found, and, when it has settings of its own, a
  // disclosure that reveals them below the row.
  //
  // The `id` is what the search in the navigation scrolls to.
  import StatusPill from "./StatusPill.svelte";

  let { id = undefined, title, description = "", status = { label: "", tone: "neutral" }, actions = undefined, children = undefined } = $props();
  let open = $state(false);

  const uid = $props.id();
  const detailsId = $derived(id ? `${id}-details` : `tool-row-${uid}-details`);
</script>

<div class="tool-row" {id}>
  <div class="setting-row">
    <div class="setting-words">
      <div class="setting-title-line"><span class="setting-title">{title}</span></div>
      {#if description}<div class="setting-description">{description}</div>{/if}
    </div>
    <div class="setting-control">
      {#if status.label}<StatusPill label={status.label} tone={status.tone} />{/if}
      {@render actions?.()}
      {#if children}
        <button type="button" class="btn btn-sm btn-ghost tool-row-toggle" aria-expanded={open} aria-controls={detailsId} onclick={() => open = !open}>{open ? "Hide details" : "Details"}</button>
      {/if}
    </div>
  </div>
  {#if open && children}
    <div class="tool-row-details" id={detailsId}>{@render children()}</div>
  {/if}
</div>

<style>
  .tool-row-details {
    margin-inline-start: calc(var(--spacing) * 2);
    padding-inline-start: calc(var(--spacing) * 4);
    border-inline-start: 1px solid var(--color-divider);
  }
</style>
