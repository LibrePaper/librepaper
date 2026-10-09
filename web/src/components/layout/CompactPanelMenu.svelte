<script>
  import { Menu } from "@skeletonlabs/skeleton-svelte";
  import ExplorerMenu from "../ExplorerMenu.svelte";
  import Icon from "../Icon.svelte";
  import { retargetElementAttributes } from "../element-attributes.js";
  import { PANEL_POSITIONING } from "../../lib/menubar.svelte.js";
  import { signInHref, signOut } from "../../lib/api.js";

  let { tabs = [], panel = "", open = false, onselect, onsettings, menus = [], onmenu, me = {} } = $props();

  // The menu is portalled so it cannot inherit a height; measure the room
  // between the top bar and the visual viewport and let the scrolling region
  // take at most that much, so the panel is as tall as its items and no
  // taller, and a press below the last item is a press outside.
  function sizeMenu(element) {
    const trigger = document.querySelector(".compact-panels-trigger");
    const siteBar = document.querySelector("body > nav");
    if (!trigger || !siteBar) return;

    let frame;
    const measure = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => {
        const visual = window.visualViewport;
        const barRect = siteBar.getBoundingClientRect();
        const viewportTop = visual?.offsetTop ?? 0;
        const viewportBottom = viewportTop + (visual?.height ?? window.innerHeight);
        const top = barRect.bottom + 8;
        const bottom = viewportBottom - 8;
        const menu = element.closest(".explorer-menu");
        const style = menu && getComputedStyle(menu);
        const menuInsets = style
          ? parseFloat(style.paddingTop) + parseFloat(style.paddingBottom) + parseFloat(style.borderTopWidth) + parseFloat(style.borderBottomWidth)
          : 0;
        element.style.maxHeight = `${Math.max(0, bottom - top - menuInsets)}px`;
      });
    };
    const observer = new ResizeObserver(measure);
    observer.observe(trigger);
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
    else if (chosen.value.startsWith("menu:")) {
      const id = chosen.value.slice(5);
      // Let this menu finish closing before opening the File/Edit/Insert/View
      // menu, which then opens in this panel's place, hanging from the same
      // trigger.
      const trigger = document.querySelector(".compact-panels-trigger");
      setTimeout(() => onmenu?.(id, trigger), 0);
    }
    else if (chosen.value === "signout") void signOut(me.site);
  }}
  positioning={PANEL_POSITIONING}
>
  <Menu.Trigger
    class={`btn-icon icon-control icon-standard compact-panels-trigger ${open ? "lp-control-tonal-brand" : "icon-plain"}`}
    aria-label="Menu"
  >
    <Icon name="menu" />
    <span class="compact-label" aria-hidden="true">Menu</span>
  </Menu.Trigger>
  <ExplorerMenu>
    <div class="compact-panels-items" use:sizeMenu>
      <!-- At compact widths, the File/Edit/Insert/View menus are accessed through
           the hamburger menu. Menu items appear at the top, above the panel tabs. -->
      {#each menus as menu (menu.id)}
        <Menu.Item value={"menu:" + menu.id} class="menuitem" disabled={menu.disabled} data-menu-id={menu.id}>
          <span class="menuitem-check" aria-hidden="true"></span>
          <span class="menuitem-label">{menu.label}</span>
          <span class="menuitem-chevron" aria-hidden="true">›</span>
        </Menu.Item>
      {/each}
      {#if menus.length}<div class="compact-panels-divider" role="separator"></div>{/if}
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
      {#if me.name}
        {@const shown = me.provider === "github" ? `@${me.name}` : me.name}
        <div class="compact-panels-divider" role="separator"></div>
        <div class="compact-account-who" aria-hidden="true">{shown}</div>
        <Menu.Item value="signout" class="menuitem workspace-action" data-workspace-action="signout">
          <span class="menuitem-check" aria-hidden="true"></span>
          <Icon name="log-out" size="1rem" />
          <span class="menuitem-label">Sign out</span>
        </Menu.Item>
      {:else if me.providers?.length}
        <div class="compact-panels-divider" role="separator"></div>
        <Menu.Item value="signin" class="menuitem workspace-action" data-workspace-action="signin">
          {#snippet element(attributes)}
            <a {...retargetElementAttributes(attributes)} href={signInHref()} class="menuitem workspace-action" data-workspace-action="signin">
              <span class="menuitem-check" aria-hidden="true"></span>
              <Icon name="log-in" size="1rem" />
              <span class="menuitem-label">Sign in</span>
            </a>
          {/snippet}
        </Menu.Item>
      {/if}
    </div>
  </ExplorerMenu>
</Menu>

<style>
  :global(.compact-panels-trigger) {
    display: inline-flex;
    flex: none;
    flex-direction: column;
    gap: 2px;
    width: auto;
    min-width: 3.5rem;
    height: 2.75rem;
    align-items: center;
    justify-content: center;
    padding: 0;
  }

  .compact-label {
    display: block;
    font-size: 11px;
    font-weight: 500;
    line-height: 1.1;
    white-space: nowrap;
  }

  /* Who is signed in: a label above the one action, as in the account menu. */
  .compact-account-who {
    padding: calc(var(--spacing) * 1.5) calc(var(--spacing) * 2) calc(var(--spacing) * 0.5);
    font-size: var(--text-xs);
    color: var(--color-text-secondary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  :global(.compact-panels-trigger[aria-expanded="true"]) {
    background: var(--color-row-selected);
    color: var(--color-brand);
  }

  .compact-panels-items {
    box-sizing: border-box;
    min-height: 0;
    overflow-x: hidden;
    overflow-y: auto;
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

  .menuitem-chevron {
    margin-left: auto;
    padding-left: calc(var(--spacing) * 2);
  }
</style>
