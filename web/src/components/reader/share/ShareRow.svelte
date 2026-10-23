<script>
  import Icon from "../../Icon.svelte";
  import IconButton from "../../IconButton.svelte";
  import { displayName, metaOf } from "./shared.js";

  // One link or one agent token. Both are a name over a line of facts with the
  // actions at the right. A token's secret is shown once and never kept, so
  // only a link with a URL offers a copy button. Trash revokes.
  let { kind, item, busy = false, copied = "", oncopy, onrevoke } = $props();
  const noun = $derived(kind === "link" ? "link" : "token");
  const name = $derived(displayName(kind, item));
  const full = $derived(item.url ? new URL(item.url, location.origin).href : "");
</script>

<li class="share-row">
  <span class="share-kind" aria-hidden="true"><Icon name={kind === "link" ? "link" : "bot"} size={15} /></span>
  <span class="share-name" title={name}>{name}</span>
  <span class="share-meta">{metaOf(kind, item)}</span>
  <div class="share-actions">
    {#if full}
      <IconButton icon={copied === full ? "check" : "copy"} label="Copy {name}" title="Copy link" tone="plain" size="btn-icon-sm"
                  disabled={busy} onclick={() => oncopy(full)} />
    {/if}
    <span class="share-revoke">
      <IconButton icon="trash" label="Revoke {name}" title="Revoke {noun}" tone="plain" size="btn-icon-sm"
                  disabled={busy} onclick={onrevoke} />
    </span>
  </div>
</li>

<style>
  .share-row {
    display: grid;
    grid-template-columns: 1rem minmax(0, 1fr) auto;
    grid-template-areas: "kind name actions" "kind meta actions";
    column-gap: calc(var(--spacing) * 2);
    align-items: center;
    min-height: 46px;
    padding-block: calc(var(--spacing) * 1);
    border-top: 1px solid var(--color-divider);
  }
  .share-row:first-child { border-top: 0; }
  .share-kind { grid-area: kind; display: grid; place-items: center; color: var(--panel-muted); }
  .share-name { grid-area: name; overflow: hidden; font-size: 13px; font-weight: 500; line-height: 1.3; text-overflow: ellipsis; white-space: nowrap; }
  .share-meta { grid-area: meta; overflow: hidden; color: var(--panel-muted); font-size: 11.5px; line-height: 1.3; text-overflow: ellipsis; white-space: nowrap; }
  .share-actions { grid-area: actions; display: flex; flex-wrap: nowrap; align-items: center; gap: calc(var(--spacing) * .5); }
  .share-revoke { display: contents; }
  .share-revoke :global(button:hover:not(:disabled)), .share-revoke :global(button:focus-visible) { color: var(--color-error-text); }
</style>
