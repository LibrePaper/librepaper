<script>
  import { Menu } from "@skeletonlabs/skeleton-svelte";
  import Icon from "./Icon.svelte";
  import ExplorerMenu from "./ExplorerMenu.svelte";

  let { current, choices, onchoose } = $props();

  const name = $derived(current.slice(current.lastIndexOf("/") + 1));
</script>

<Menu
  onSelect={(chosen) => onchoose?.(chosen.value)}
  positioning={{ placement: "bottom-start", gutter: 4, flip: true, fitViewport: true, overflowPadding: 8 }}
>
  <Menu.Trigger class="preview-target" aria-label="Previewing {name}. Choose the file to preview">
    <Icon name="eye" size="0.875rem" />
    <span class="preview-target-name">{name}</span>
    <Icon name="chevron-down" size="0.875rem" />
  </Menu.Trigger>
  <ExplorerMenu>
    {#each choices as choice (choice.id)}
      <Menu.Item value={choice.id} class="menuitem">
        <span class="menuitem-check">{choice.path === current ? "✓" : ""}</span>{choice.path}
      </Menu.Item>
    {/each}
  </ExplorerMenu>
</Menu>

<style>
  /* The trigger's class is passed as a prop, so these rules are global. */
  :global(.preview-target) {
    position: absolute;
    z-index: 2;
    top: calc(var(--spacing) * 4);
    left: calc(var(--spacing) * 4);
    display: flex;
    align-items: center;
    gap: calc(var(--spacing) * 1.5);
    height: calc(var(--spacing) * 9);
    max-width: calc(50% - var(--spacing) * 6);
    padding-inline: calc(var(--spacing) * 2.5);
    border: 1px solid var(--color-divider);
    border-radius: var(--radius-container);
    background: var(--color-shell);
    box-shadow: var(--shadow-lg);
    color: var(--color-text-secondary);
    font-size: var(--text-xs);
    cursor: pointer;
  }
  :global(.preview-target:hover) {
    background: var(--color-row-hover);
  }
  :global(.preview-target:focus-visible) {
    outline: 2px solid var(--color-focus);
    outline-offset: 2px;
  }
  :global(.preview-target-name) {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--color-text);
    font-weight: 500;
  }
</style>
