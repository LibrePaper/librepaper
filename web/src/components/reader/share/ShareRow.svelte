<script>
  import { Menu } from "@skeletonlabs/skeleton-svelte";
  import Icon from "../../Icon.svelte";
  import IconButton from "../../IconButton.svelte";
  import ExplorerMenu from "../../ExplorerMenu.svelte";
  import MenuIconButton from "../../MenuIconButton.svelte";
  import { displayName, metaOf } from "./shared.js";

  // One link or one agent token. Both are a name over a line of facts with the
  // actions at the right, so they are one component; a token has no URL and
  // so never offers a copy button.
  let { kind, item, busy = false, copied = "", oncopy, onaction } = $props();
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
                  disabled={busy} onclick={() => oncopy(full, "Link copied.")} />
    {/if}
    <Menu onSelect={({ value }) => onaction(value, item)}>
      <MenuIconButton icon="more-horizontal" label="More actions for {name}" size="1rem" data-row-menu={item.id} disabled={busy} />
      <ExplorerMenu>
        <Menu.Item value="edit" class="menuitem">Edit settings</Menu.Item>
        <Menu.Item value="replace" class="menuitem">Replace {noun}</Menu.Item>
        <Menu.Item value="revoke" class="menuitem lp-text-error">Revoke {noun}</Menu.Item>
      </ExplorerMenu>
    </Menu>
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
</style>
