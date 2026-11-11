<script>
  import { m } from "../../lib/paraglide/messages.js";
  import Icon from "../Icon.svelte";

  // Shown over the preview while a Quarto document that runs its code on this
  // computer has no usable Run directory, so nothing can render yet. It reads
  // and acts through the same state as the chip beside Run code.
  let { directory: rd } = $props();

  const status = $derived(rd.status);
  const missing = $derived(Boolean(status?.path) && !rd.usable);
  const busy = $derived(rd.busy);
</script>

<div class="rd-prompt" role="group" aria-label={m.rundirectorycontrol_label()}>
  <span class="rd-prompt-icon"><Icon name={missing ? "triangle-alert" : "folder"} size="28" /></span>
  <h2 class="rd-prompt-title">{m.rundirectorycontrol_prompt_title()}</h2>
  <p class="rd-prompt-text">
    {missing ? m.rundirectorycontrol_missing({ path: rd.shorten(status.path) }) : m.rundirectorycontrol_prompt_text()}
  </p>
  <div class="rd-prompt-actions">
    {#if status?.suggested}
      <button type="button" class="btn btn-sm lp-control-brand" disabled={Boolean(busy)} title={status.suggested} onclick={() => rd.useDefault()}>
        {m.rundirectorycontrol_use_suggested({ suggested: rd.shorten(status.suggested) })}
      </button>
    {/if}
    <button type="button" class="btn btn-sm lp-control-neutral" disabled={Boolean(busy)} onclick={() => rd.choose()}>
      {busy === "choose" ? m.common_choosing() : m.backupssettings_choose_folder()}
    </button>
  </div>
  {#if rd.error}<p class="rd-prompt-error" role="alert">{rd.error}</p>{/if}
</div>

<style>
  .rd-prompt { display: flex; flex-direction: column; align-items: center; gap: calc(var(--spacing) * 2); max-width: 24rem; padding: calc(var(--spacing) * 4); text-align: center; }
  .rd-prompt-icon { display: inline-flex; color: var(--color-text-secondary); }
  .rd-prompt-title { margin: 0; font-size: var(--text-base); font-weight: 600; color: var(--color-text); }
  .rd-prompt-text { margin: 0; font-size: var(--text-sm); color: var(--color-text-secondary); overflow-wrap: anywhere; }
  .rd-prompt-actions { display: flex; flex-wrap: wrap; justify-content: center; gap: calc(var(--spacing) * 2); }
  .rd-prompt-error { margin: 0; font-size: var(--text-sm); color: var(--color-danger, var(--color-text)); }
</style>
