<script>
  import { untrack } from "svelte";
  import * as desktopClient from "../../lib/desktop/client.js";
  import { m } from "../../lib/paraglide/messages.js";
  import Icon from "../Icon.svelte";

  // The folder a Quarto document's code runs in on this computer. It is shown
  // beside the Run code switch while that is on, and it tells the page whether
  // a render may start: `onchange(path)` with the folder once one that exists is set, or "" when none is.
  let { project, onchange = () => {} } = $props();

  let status = $state(null);
  let busy = $state("");
  let error = $state("");
  let loaded = $state(false);

  const usable = $derived(Boolean(status?.path) && status.exists !== false);

  function apply(next) {
    status = next;
    onchange(next?.path && next.exists !== false ? next.path : "");
  }

  // The path as shown: the home directory is `~`. The home is what is left of
  // the suggested folder, which is <home>/LibrePaper/<project>.
  const home = $derived.by(() => {
    const found = /^(.*)[\\/]LibrePaper[\\/][^\\/]+[\\/]?$/.exec(status?.suggested || "");
    return found?.[1] || "";
  });
  const shorten = (path) => {
    if (!home || !path) return path || "";
    if (path === home) return "~";
    return path.startsWith(home) && /[\\/]/.test(path[home.length]) ? "~" + path.slice(home.length) : path;
  };
  const leaf = (path) => path.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || path;

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

  async function act(name, call) {
    if (busy) return;
    busy = name;
    error = "";
    try {
      apply(await call(project));
      open = false;
      chip?.focus();
    } catch (cause) {
      error = cause?.message || "";
    } finally {
      busy = "";
    }
  }

  $effect(() => {
    const slug = project;
    let current = true;
    loaded = false;
    untrack(() => onchange(""));
    desktopClient.runDirectory(slug).then(
      (next) => { if (current) { loaded = true; apply(next); } },
      (cause) => { if (current) { loaded = true; error = cause?.message || ""; } },
    );
    return () => { current = false; };
  });
</script>

<svelte:window onpointerdown={outside} />

{#if loaded}
  <div class="run-directory" role="group" aria-label={m.rundirectorycontrol_label()} {onkeydown}>
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
        <div class="rd-popover" role="dialog" aria-label={m.rundirectorycontrol_label()} bind:this={popover}>
          {#if usable}
            <p class="rd-text">{m.rundirectorycontrol_runs_in({ path: shorten(status.path) })}</p>
            <div class="rd-actions">
              <button type="button" class="btn btn-sm lp-control-neutral" disabled={Boolean(busy)} onclick={() => act("open", desktopClient.openRunDirectory)}><Icon name="folder-open" size="14" />{m.rundirectorycontrol_open_folder()}</button>
              <button type="button" class="btn btn-sm lp-control-neutral" disabled={Boolean(busy)} onclick={() => act("choose", desktopClient.chooseRunDirectory)}>
                {busy === "choose" ? m.common_choosing() : m.rundirectorycontrol_change()}
              </button>
            </div>
          {:else}
            <p class="rd-text">
              {status.path ? m.rundirectorycontrol_missing({ path: shorten(status.path) }) : m.rundirectorycontrol_choose_prompt()}
            </p>
            <div class="rd-actions">
              <button type="button" class="btn btn-sm lp-control-brand" disabled={Boolean(busy)} title={status.suggested} onclick={() => act("default", desktopClient.useDefaultRunDirectory)}>
                {m.rundirectorycontrol_use_suggested({ suggested: shorten(status.suggested) })}
              </button>
              <button type="button" class="btn btn-sm lp-control-neutral" disabled={Boolean(busy)} onclick={() => act("choose", desktopClient.chooseRunDirectory)}>
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
  .rd-chip.rd-none { color: var(--color-brand); border-color: var(--color-brand); }
  .rd-chip.rd-missing { color: var(--color-warning-text); border-color: var(--color-warning-solid); }
  .rd-icon { display: inline-flex; flex: none; }
  .rd-name { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .rd-popover {
    position: absolute; top: calc(100% + var(--spacing)); right: 0; z-index: 30;
    box-sizing: border-box; width: max-content; max-width: min(22rem, calc(100vw - 2rem));
    display: flex; flex-direction: column; gap: calc(var(--spacing) * 2);
    padding: calc(var(--spacing) * 3);
    background: var(--color-sidebar); color: var(--color-text);
    border: 1px solid var(--color-divider); border-radius: 4px;
  }
  .rd-text { margin: 0; overflow-wrap: anywhere; }
  .rd-actions { display: flex; flex-wrap: wrap; gap: calc(var(--spacing) * 2); }
  .rd-error { margin: 0; color: var(--color-danger, var(--color-text)); }
</style>
