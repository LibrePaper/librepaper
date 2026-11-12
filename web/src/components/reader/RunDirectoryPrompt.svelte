<script>
  import { onMount } from "svelte";
  import { leaf, pageVariant } from "../../lib/reader/run-directory.js";
  import { m } from "../../lib/paraglide/messages.js";
  import Icon from "../Icon.svelte";

  // The Run directory page, shown over the preview: when the button beside Run
  // code is clicked, and on its own while a Quarto document that runs its code
  // on this computer has no usable folder, so nothing can render yet. It reads
  // and acts through the same state as that button. `blocking` is true in the
  // second case, where the page cannot be dismissed.
  let { directory: rd, title = "", blocking = false, onclose = () => {} } = $props();

  const status = $derived(rd.status);
  const busy = $derived(rd.busy);
  const variant = $derived(pageVariant(status, rd.usable));
  // The folder the block describes: the one in use, or the suggestion.
  const shownPath = $derived(variant === "current" ? status.path : variant === "none" ? "" : status?.suggested || "");
  const hasBlock = $derived(Boolean(shownPath));
  const canUse = $derived(variant === "current" || Boolean(status?.suggested));
  const label = $derived(variant === "current" ? leaf(shownPath) : title || (shownPath ? leaf(shownPath) : ""));
  const caption = $derived(variant === "current" ? m.rundirectorycontrol_current_caption() : m.rundirectorycontrol_suggested_caption());

  let primary = $state(null);
  onMount(() => { if (!blocking) primary?.focus(); });

  function onkeydown(event) {
    if (event.key === "Escape" && !blocking) {
      event.stopPropagation();
      onclose();
    }
  }

  async function use() {
    if (variant === "current") return onclose();
    if (await rd.useDefault()) onclose();
  }

  async function choose() {
    const before = status?.path || "";
    if (await rd.choose() && (rd.status?.path || "") !== before) onclose();
  }
</script>

<svelte:window {onkeydown} />

<div class="rd-page" role="group" aria-label={m.rundirectorycontrol_label()}>
  <span class="rd-page-icon"><Icon name={variant === "missing" ? "triangle-alert" : "folder"} size="30" /></span>
  <h2 class="rd-page-title">{m.rundirectorycontrol_prompt_title()}</h2>
  <p class="rd-page-text">
    {variant === "missing" ? m.rundirectorycontrol_missing({ path: rd.shorten(status.path) }) : m.rundirectorycontrol_prompt_text()}
  </p>
  {#if hasBlock}
    <div class="rd-info" data-tip-wait="long" data-tip={shownPath}>
      <span class="rd-info-icon"><Icon name="folder" size="16" /></span>
      <div class="rd-info-body">
        <span class="rd-info-caption">{caption}</span>
        <span class="rd-info-label">{label}</span>
        <span class="rd-info-path">{rd.shorten(shownPath)}</span>
      </div>
    </div>
  {/if}
  <div class="rd-page-actions">
    {#if canUse}
      <button type="button" class="btn btn-sm lp-control-brand" bind:this={primary} disabled={Boolean(busy)} onclick={use}>
        {m.rundirectorycontrol_use_this()}
      </button>
      <button type="button" class="btn btn-sm lp-control-neutral" disabled={Boolean(busy)} onclick={choose}>
        {busy === "choose" ? m.common_choosing() : m.rundirectorycontrol_choose_another()}
      </button>
    {:else}
      <button type="button" class="btn btn-sm lp-control-brand" bind:this={primary} disabled={Boolean(busy)} onclick={choose}>
        {busy === "choose" ? m.common_choosing() : m.rundirectorycontrol_choose_a_folder()}
      </button>
    {/if}
  </div>
  {#if rd.error}<p class="rd-page-error" role="alert">{rd.error}</p>{/if}
</div>

<style>
  .rd-page {
    box-sizing: border-box; display: flex; flex-direction: column; align-items: center;
    gap: calc(var(--spacing) * 3); width: 100%; max-width: 26rem; min-width: 0;
    padding: calc(var(--spacing) * 4); text-align: center;
  }
  .rd-page-icon { display: inline-flex; color: var(--color-text-secondary); }
  .rd-page-title { margin: 0; font-size: var(--text-xl); font-weight: 600; line-height: 1.3; color: var(--color-text); }
  .rd-page-text { margin: 0; font-size: var(--text-sm); color: var(--color-text-secondary); overflow-wrap: anywhere; }
  .rd-info {
    box-sizing: border-box; display: flex; align-items: center; gap: calc(var(--spacing) * 3);
    width: 100%; min-width: 0; padding: calc(var(--spacing) * 3); text-align: left;
    background: var(--color-subtle); border: 1px solid var(--color-border); border-radius: var(--radius-base);
  }
  .rd-info-icon { display: inline-flex; flex: none; color: var(--color-text-secondary); }
  .rd-info-body { display: flex; flex-direction: column; min-width: 0; flex: 1; }
  .rd-info-caption { font-size: var(--text-xs); color: var(--color-text-secondary); }
  .rd-info-label { font-size: var(--text-sm); font-weight: 600; color: var(--color-text); }
  .rd-info-path { font-size: var(--text-xs); color: var(--color-text-secondary); }
  .rd-info-label, .rd-info-path { min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .rd-page-actions { display: flex; flex-direction: column; align-items: stretch; gap: calc(var(--spacing) * 2); width: 100%; }
  .rd-page-actions :global(.btn) { width: 100%; justify-content: center; }
  .rd-page-error { margin: 0; font-size: var(--text-sm); color: var(--color-danger, var(--color-text)); }
</style>
