<script>
  import { Menu } from "@skeletonlabs/skeleton-svelte";
  import ExplorerMenu from "../ExplorerMenu.svelte";
  import Icon from "../Icon.svelte";

  let { tabs = [], panel = "", open = false, onselect } = $props();
</script>

<Menu
  onSelect={(chosen) => onselect?.(chosen.value)}
  positioning={{ placement: "top-start", gutter: 4, flip: true, fitViewport: true, overflowPadding: 8 }}
>
  <Menu.Trigger class="btn btn-sm lp-control-outline compact-panels-trigger" aria-label="Panels">
    <Icon name="menu" size="1.25rem" />
    <span>Panels</span>
  </Menu.Trigger>
  <ExplorerMenu>
    <div class="compact-panels-items">
      {#each tabs as tab (tab.id)}
        {@const selected = open && panel === tab.id}
        <Menu.Item
          value={tab.id}
          class="menuitem compact-panel-item"
          data-panel-id={tab.id}
          aria-label={tab.says}
          aria-current={selected ? "true" : undefined}
        >
          <span class="menuitem-check" aria-hidden="true">{selected ? "✓" : ""}</span>
          <Icon name={tab.icon} size="1rem" />
          <span class="menuitem-label">{tab.says}</span>
        </Menu.Item>
      {/each}
    </div>
  </ExplorerMenu>
</Menu>

<style>
  :global(.compact-panels-trigger) {
    display: inline-flex;
    min-height: 2.75rem;
    align-items: center;
    justify-content: center;
    gap: calc(var(--spacing) * 2);
    padding-inline: calc(var(--spacing) * 3);
  }

  .compact-panels-items {
    max-height: calc(100dvh - 6rem);
    overflow-y: auto;
    overscroll-behavior: contain;
  }

  .compact-panels-items :global(.compact-panel-item) {
    min-height: 2.75rem;
    white-space: normal;
  }
</style>
