<script>
  import { leaf, pageVariant } from "../../lib/reader/run-directory.js";
  import { m } from "../../lib/paraglide/messages.js";
  import { storageBytes } from "../../lib/quota-preferences.js";
  import Icon from "../Icon.svelte";

  // The body of the Run directory dialog: opened when the button beside Run
  // code is clicked, and on its own while a Quarto document that runs its code
  // on this computer has no usable folder. It reads and acts through the same
  // state as that button. The dialog supplies the heading, Escape and focus.
  // The share section shows only when `onshare` is given.
  let { directory: rd, title = "", onclose = () => {}, share = true, onshare = undefined, shareStatus = { kind: "idle" }, pdfOutput = false } = $props();

  const status = $derived(rd.status);
  const busy = $derived(rd.busy);
  const variant = $derived(pageVariant(status, rd.usable));
  // The folder the block describes: the one in use, or the suggestion.
  const shownPath = $derived(variant === "current" ? status.path : variant === "none" ? "" : status?.suggested || "");
  const hasBlock = $derived(Boolean(shownPath));
  const canUse = $derived(variant === "current" || Boolean(status?.suggested));
  const label = $derived(variant === "current" ? leaf(shownPath) : title || (shownPath ? leaf(shownPath) : ""));
  const caption = $derived(variant === "current" ? m.rundirectorycontrol_current_caption() : m.rundirectorycontrol_suggested_caption());

  // Shared results: the line under the checkbox, if there is anything to say.
  const shareLine = $derived.by(() => {
    if (!share) return "";
    if (pdfOutput) return m.rundirectorycontrol_html_only();
    if (shareStatus.kind === "shared") return m.rundirectorycontrol_shared({ size: storageBytes(shareStatus.bytes ?? 0) });
    if (shareStatus.kind === "failed") {
      if (shareStatus.reason === "storage") return m.rundirectorycontrol_not_shared_full();
      if (shareStatus.reason === "unmatched") return m.rundirectorycontrol_not_shared_unmatched();
      return m.rundirectorycontrol_not_shared_failed();
    }
    return "";
  });

  async function use() {
    if (variant === "current") return onclose();
    if (await rd.useDefault()) onclose();
  }

  async function choose() {
    const before = status?.path || "";
    if (await rd.choose() && (rd.status?.path || "") !== before) onclose();
  }
</script>

<div class="rd-page" role="group" aria-label={m.rundirectorycontrol_label()}>
  <span class="rd-page-icon"><Icon name={variant === "missing" ? "triangle-alert" : "folder"} size="30" /></span>
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
      <button type="button" class="btn btn-sm lp-control-brand" disabled={Boolean(busy)} onclick={use}>
        {m.rundirectorycontrol_use_this()}
      </button>
      <button type="button" class="btn btn-sm lp-control-neutral" disabled={Boolean(busy)} onclick={choose}>
        {busy === "choose" ? m.common_choosing() : m.rundirectorycontrol_choose_another()}
      </button>
    {:else}
      <button type="button" class="btn btn-sm lp-control-brand" disabled={Boolean(busy)} onclick={choose}>
        {busy === "choose" ? m.common_choosing() : m.rundirectorycontrol_choose_a_folder()}
      </button>
    {/if}
  </div>
  {#if rd.error}<p class="rd-page-error" role="alert">{rd.error}</p>{/if}
  {#if onshare}
    <div class="rd-share-section">
      <label class="rd-share">
        <input type="checkbox" checked={share} onchange={(event) => onshare(event.currentTarget.checked)} />
        <span>{m.rundirectorycontrol_share_results()}</span>
      </label>
      <p class="rd-share-hint">{m.rundirectorycontrol_share_hint()}</p>
      {#if shareLine}<p class="rd-share-hint" role="status">{shareLine}</p>{/if}
    </div>
  {/if}
</div>

<style>
  .rd-page {
    box-sizing: border-box; display: flex; flex-direction: column; align-items: center;
    gap: calc(var(--spacing) * 3); width: 100%; min-width: 0; text-align: center;
  }
  .rd-page-icon { display: inline-flex; color: var(--color-text-secondary); }
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
  .rd-share-section { display: flex; flex-direction: column; gap: calc(var(--spacing) * 1); width: 100%; text-align: left; }
  .rd-share { display: inline-flex; align-items: center; gap: calc(var(--spacing) * 2); font-size: var(--text-sm); color: var(--color-text); cursor: pointer; }
  .rd-share-hint { margin: 0; font-size: var(--text-xs); color: var(--color-text-secondary); }
</style>
