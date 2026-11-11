<script>
  import { leaf } from "../../lib/reader/run-directory.js";
  import { m } from "../../lib/paraglide/messages.js";
  import Icon from "../Icon.svelte";

  // The folder a Quarto document's code runs in on this computer, shown beside
  // the Run code switch while that is on. The state is shared with the prompt
  // in the preview; see run-directory.js.
  let { directory: rd, align = "end" } = $props();

  const status = $derived(rd.status);
  const usable = $derived(rd.usable);
  const busy = $derived(rd.busy);
  const error = $derived(rd.error);
  const shorten = (path) => rd.shorten(path);

  let open = $state(false);
  let chip = $state(null);
  let popover = $state(null);

  function close() {
    if (!open) return;
    open = false;
    chip?.focus();
  }

  function toggle() {
    if (open) return close();
    open = true;
    queueMicrotask(() => popover?.querySelector("button:not([disabled])")?.focus());
  }

  function onkeydown(event) {
    if (event.key === "Escape" && open) {
      event.stopPropagation();
      close();
    }
  }

  function outside(event) {
    if (open && !chip?.contains(event.target) && !popover?.contains(event.target)) open = false;
  }

  async function act(call) {
    if (await call()) {
      open = false;
      chip?.focus();
    }
  }
</script>

<svelte:window onpointerdown={outside} {onkeydown} />

{#if rd.loaded}
  <div class="run-directory" role="group" aria-label={m.rundirectorycontrol_label()}>
    {#if status}
      <button
        type="button"
        class="rd-chip"
        class:rd-set={usable}
        class:rd-none={!status.path}
        class:rd-missing={Boolean(status.path) && !usable}
        bind:this={chip}
        title={status.path || undefined}
        aria-haspopup="dialog"
        aria-expanded={open}
        onclick={toggle}
      >
        <span class="rd-icon"><Icon name={status.path && !usable ? "triangle-alert" : "folder"} size="14" /></span>
        <span class="rd-name">{status.path ? leaf(status.path) : m.rundirectorycontrol_choose_chip()}</span>
      </button>
      {#if open}
        <div class="rd-popover" class:rd-start={align === "start"} role="dialog" aria-label={m.rundirectorycontrol_label()} bind:this={popover}>
          {#if usable}
            <p class="rd-text">{m.rundirectorycontrol_runs_in({ path: shorten(status.path) })}</p>
            <div class="rd-actions">
              <button type="button" class="btn btn-sm lp-control-neutral" disabled={Boolean(busy)} onclick={() => act(rd.open)}><Icon name="folder-open" size="14" />{m.rundirectorycontrol_open_folder()}</button>
              <button type="button" class="btn btn-sm lp-control-neutral" disabled={Boolean(busy)} onclick={() => act(rd.choose)}>
                {busy === "choose" ? m.common_choosing() : m.rundirectorycontrol_change()}
              </button>
            </div>
          {:else}
            <p class="rd-text">
              {status.path ? m.rundirectorycontrol_missing({ path: shorten(status.path) }) : m.rundirectorycontrol_choose_prompt()}
            </p>
            <div class="rd-actions">
              <button type="button" class="btn btn-sm lp-control-brand" disabled={Boolean(busy)} title={status.suggested} onclick={() => act(rd.useDefault)}>
                {m.rundirectorycontrol_use_suggested({ suggested: shorten(status.suggested) })}
              </button>
              <button type="button" class="btn btn-sm lp-control-neutral" disabled={Boolean(busy)} onclick={() => act(rd.choose)}>
                {busy === "choose" ? m.common_choosing() : m.backupssettings_choose_folder()}
              </button>
            </div>
          {/if}
          {#if error}<p class="rd-error" role="alert">{error}</p>{/if}
        </div>
      {/if}
    {:else if error}
      <span class="rd-error" role="alert">{error}</span>
    {/if}
  </div>
{/if}

<style>
  .run-directory { position: relative; display: inline-flex; align-items: center; min-width: 0; font-size: var(--text-sm); }
  .rd-chip {
    display: inline-flex; align-items: center; gap: calc(var(--spacing) * 1.5);
    max-width: 10rem; padding: calc(var(--spacing) * 0.5) calc(var(--spacing) * 2);
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
  .rd-popover.rd-start { left: 0; right: auto; }
  .rd-popover {
    position: absolute; top: calc(100% + var(--spacing)); right: 0; z-index: 30;
    box-sizing: border-box; width: max-content; max-width: min(22rem, calc(100vw - 2rem));
    display: flex; flex-direction: column; gap: calc(var(--spacing) * 2);
    padding: calc(var(--spacing) * 3);
    background: var(--color-sidebar); color: var(--color-text);
    border: 1px solid var(--color-divider); border-radius: 4px;
  }
  .rd-text { margin: 0; overflow-wrap: anywhere; }
  .rd-actions { display: flex; flex-direction: column; align-items: stretch; gap: calc(var(--spacing) * 2); }
  .rd-actions :global(.btn) { width: 100%; justify-content: flex-start; text-align: left; }
  .rd-error { margin: 0; color: var(--color-danger, var(--color-text)); }
</style>
