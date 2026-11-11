<script>
  import { leaf } from "../../lib/reader/run-directory.js";
  import { m } from "../../lib/paraglide/messages.js";
  import Icon from "../Icon.svelte";

  // The bottom of the Files panel says where a Quarto project's scripts and
  // data live: on this computer, in the Run directory. Once a folder is in use
  // the row names it and opens it; before that it says how to choose one. It
  // shares its state with the Run code button and the Run directory page.
  let { directory: rd, offered = false } = $props();
</script>

{#if rd.usable && rd.status?.path}
  <div class="rd-row" title={rd.status.path}>
    <span class="rd-row-icon"><Icon name="folder" size="14" /></span>
    <span class="rd-row-body">
      <span class="rd-row-caption">{m.rundirectoryrow_caption()}</span>
      <span class="rd-row-name">{leaf(rd.status.path)}</span>
    </span>
    <button type="button" class="btn btn-sm lp-control-neutral" disabled={Boolean(rd.busy)} onclick={() => rd.open()}>{m.rundirectoryrow_open()}</button>
  </div>
  {#if rd.error}<p class="rd-row-text" role="alert">{rd.error}</p>{/if}
{:else}
  <p class="rd-row-text">{offered ? m.rundirectoryrow_choose_with_run_code() : m.rundirectoryrow_needs_desktop()}</p>
{/if}

<style>
  .rd-row { display: flex; align-items: center; gap: calc(var(--spacing) * 2); min-width: 0; }
  .rd-row-icon { display: inline-flex; flex: none; color: var(--color-text-secondary); }
  .rd-row-body { display: flex; flex-direction: column; min-width: 0; flex: 1; }
  .rd-row-caption { font-size: var(--text-xs); color: var(--color-text-secondary); }
  .rd-row-name { font-size: var(--text-sm); color: var(--color-text); min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .rd-row .btn { flex: none; }
  .rd-row-text { margin: 0; font-size: var(--text-xs); color: var(--color-text-secondary); overflow-wrap: anywhere; }
</style>
