<script>
  import { Menu } from "@skeletonlabs/skeleton-svelte";
  import ExplorerMenu from "../ExplorerMenu.svelte";
  import Icon from "../Icon.svelte";
  import { retargetElementAttributes } from "../element-attributes.js";

  let { tabs = [], panel = "", open = false, onselect, onsettings } = $props();

  // The menu is portalled, so its height cannot be inherited from the reader
  // or its bottom bar. Measure the available space between both bars in the
  // visual viewport and give the menu's scrolling region all of it.
  function sizeMenu(element) {
    const trigger = document.querySelector(".compact-panels-trigger");
    const bottomBar = trigger?.closest(".mobile-pane-nav");
    const siteBar = document.querySelector("body > nav");
    if (!trigger || !bottomBar || !siteBar) return;

    let frame;
    const measure = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => {
        const visual = window.visualViewport;
        const bottomRect = bottomBar.getBoundingClientRect();
        const barRect = siteBar.getBoundingClientRect();
        const viewportTop = visual?.offsetTop ?? 0;
        const viewportBottom = viewportTop + (visual?.height ?? window.innerHeight);
        const top = Math.max(viewportTop, barRect.bottom) + 8;
        const bottom = Math.min(viewportBottom, bottomRect.top - 4);
        const menu = element.closest(".explorer-menu");
        const style = menu && getComputedStyle(menu);
        const menuInsets = style
          ? parseFloat(style.paddingTop) + parseFloat(style.paddingBottom) + parseFloat(style.borderTopWidth) + parseFloat(style.borderBottomWidth)
          : 0;
        element.style.height = `${Math.max(0, bottom - top - menuInsets)}px`;
      });
    };
    const observer = new ResizeObserver(measure);
    observer.observe(trigger);
    observer.observe(bottomBar);
    observer.observe(siteBar);
    window.addEventListener("resize", measure);
    window.visualViewport?.addEventListener("resize", measure);
    window.visualViewport?.addEventListener("scroll", measure);
    measure();
    return {
      update: measure,
      destroy() {
        cancelAnimationFrame(frame);
        observer.disconnect();
        window.removeEventListener("resize", measure);
        window.visualViewport?.removeEventListener("resize", measure);
        window.visualViewport?.removeEventListener("scroll", measure);
      },
    };
  }
</script>

<Menu
  onSelect={(chosen) => {
    if (tabs.some((tab) => tab.id === chosen.value)) onselect?.(chosen.value);
    // Let the menu finish closing before opening a focus-trapping dialog.
    else if (chosen.value === "settings") setTimeout(() => onsettings?.(), 0);
  }}
  positioning={{ placement: "top-start", gutter: 4, flip: true, fitViewport: true, overflowPadding: 8 }}
>
  <Menu.Trigger
    class={`btn-icon icon-control icon-standard compact-panels-trigger ${open ? "lp-control-tonal-brand" : "icon-plain"}`}
    aria-label="Panels"
  >
    <Icon name="menu" />
  </Menu.Trigger>
  <ExplorerMenu>
    <div class="compact-panels-items" use:sizeMenu>
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
      <div class="compact-panels-divider" role="separator"></div>
      <Menu.Item value="settings" class="menuitem workspace-action" data-workspace-action="settings">
        <span class="menuitem-check" aria-hidden="true"></span>
        <Icon name="settings" size="1rem" />
        <span class="menuitem-label">Settings</span>
      </Menu.Item>
      <Menu.Item value="home" class="menuitem workspace-action" data-workspace-action="home">
        {#snippet element(attributes)}
          <a {...retargetElementAttributes(attributes)} href="/" class="menuitem workspace-action" data-workspace-action="home">
            <span class="menuitem-check" aria-hidden="true"></span>
            <Icon name="home" size="1rem" />
            <span class="menuitem-label">Home</span>
          </a>
        {/snippet}
      </Menu.Item>
      <Menu.Item value="docs" class="menuitem workspace-action" data-workspace-action="docs">
        {#snippet element(attributes)}
          <a {...retargetElementAttributes(attributes)} href="/documentation" class="menuitem workspace-action" data-workspace-action="docs">
            <span class="menuitem-check" aria-hidden="true"></span>
            <Icon name="help" size="1rem" />
            <span class="menuitem-label">Docs</span>
          </a>
        {/snippet}
      </Menu.Item>
    </div>
  </ExplorerMenu>
</Menu>

<style>
  :global(.compact-panels-trigger) {
    display: inline-flex;
    flex: none;
    width: 2.75rem;
    height: 2.75rem;
    align-items: center;
    justify-content: center;
    padding-inline: 0;
  }

  :global(.compact-panels-trigger[aria-expanded="true"]) {
    background: var(--color-row-selected);
    color: var(--color-brand);
  }

  .compact-panels-items {
    box-sizing: border-box;
    min-height: 0;
    overflow-x: hidden;
    overflow-y: scroll;
    overscroll-behavior: contain;
    touch-action: pan-y;
    -webkit-overflow-scrolling: touch;
  }

  .compact-panels-items :global(.compact-panel-item) {
    min-height: 2.75rem;
    white-space: normal;
  }

  .compact-panels-divider {
    height: 1px;
    margin: calc(var(--spacing) * 1) calc(var(--spacing) * 1.5);
    background: var(--color-divider);
  }

  .compact-panels-items :global(.workspace-action) {
    min-height: 2.75rem;
    white-space: normal;
  }
</style>
