<script>
  import Icon from "../../Icon.svelte";
  import IconButton from "../../IconButton.svelte";
  import { m } from "../../../lib/paraglide/messages.js";
  import { connectionInstructions, displayName, metaOf } from "./shared.js";

  // One link or one agent token. Both are a name over a line of facts with the
  // actions at the right. A link with a URL offers a copy of the URL. A token
  // offers a copy of its connection instructions only when the server sent its
  // full text, which it does for tokens it still holds; older tokens have no
  // copy icon. Trash revokes.
  let { kind, item, busy = false, copied = "", oncopy, onrevoke } = $props();
  const name = $derived(displayName(kind, item));
  const full = $derived(item.url ? new URL(item.url, location.origin).href : "");
  const value = $derived(kind === "token" && item.token ? connectionInstructions(item.token) : "");
</script>

<li class="share-row">
  <span class="share-kind" aria-hidden="true"><Icon name={kind === "link" ? "link" : "bot"} size={15} /></span>
  <span class="share-name" title={name}>{name}</span>
  <span class="share-meta">{metaOf(kind, item)}</span>
  <div class="share-actions">
    {#if full}
      <IconButton icon={copied === full ? "check" : "copy"} label={m.sharerow_copy_label({ name })} title={m.sharerow_copy_link()} tone="plain" size="btn-icon-sm"
                  disabled={busy} onclick={() => oncopy(full)} />
    {/if}
    {#if value}
      <IconButton icon={copied === value ? "check" : "copy"} label={m.sharerow_copy_connection_label({ name })} title={m.tokenreveal_copy_connect()} tone="plain" size="btn-icon-sm"
                  disabled={busy} onclick={() => oncopy(value)} />
    {/if}
    <span class="share-revoke">
      <IconButton icon="trash" label={m.sharerow_revoke_label({ name })} title={kind === "link" ? m.common_revoke_link() : m.common_revoke_token()} tone="plain" size="btn-icon-sm"
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
  /* Phones: each row is a finger's height, and the copy and revoke buttons are
     finger-sized without making the icons larger. */
  @media (max-width: 767px) {
    .share-row { min-height: 2.75rem; padding-block: 0; }
    .share-actions :global(.icon-control) { min-width: 2.75rem; min-height: 2.75rem; }
  }
</style>
