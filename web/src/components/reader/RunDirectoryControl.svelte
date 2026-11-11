<script>
  import { untrack } from "svelte";
  import * as desktopClient from "../../lib/desktop/client.js";
  import { m } from "../../lib/paraglide/messages.js";

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

  async function act(name, call) {
    if (busy) return;
    busy = name;
    error = "";
    try {
      apply(await call(project));
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

{#if loaded}
  <div class="run-directory" role="group" aria-label={m.rundirectorycontrol_label()}>
    {#if usable}
      <span class="run-directory-text" title={status.path}>{m.rundirectorycontrol_runs_in({ path: status.path })}</span>
      <button type="button" class="btn btn-sm lp-control-neutral" disabled={Boolean(busy)} onclick={() => act("open", desktopClient.openRunDirectory)}>{m.rundirectorycontrol_open_folder()}</button>
      <button type="button" class="btn btn-sm lp-control-neutral" disabled={Boolean(busy)} onclick={() => act("choose", desktopClient.chooseRunDirectory)}>
        {busy === "choose" ? m.common_choosing() : m.rundirectorycontrol_change()}
      </button>
    {:else if status}
      <span class="run-directory-text">
        {status.path ? m.rundirectorycontrol_missing({ path: status.path }) : m.rundirectorycontrol_choose_prompt()}
      </span>
      <button type="button" class="btn btn-sm lp-control-brand" disabled={Boolean(busy)} title={status.suggested} onclick={() => act("default", desktopClient.useDefaultRunDirectory)}>
        {m.rundirectorycontrol_use_suggested({ suggested: status.suggested })}
      </button>
      <button type="button" class="btn btn-sm lp-control-neutral" disabled={Boolean(busy)} onclick={() => act("choose", desktopClient.chooseRunDirectory)}>
        {busy === "choose" ? m.common_choosing() : m.backupssettings_choose_folder()}
      </button>
    {/if}
    {#if error}<span class="run-directory-error" role="alert">{error}</span>{/if}
  </div>
{/if}

<style>
  .run-directory { display: flex; align-items: center; flex-wrap: wrap; gap: 0.4rem; min-width: 0; font-size: 0.8125rem; color: var(--color-text-secondary); }
  .run-directory-text { min-width: 0; max-width: 24rem; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .run-directory-error { color: var(--color-danger, var(--color-text)); }
</style>
