<script>
  import { Menu, Portal } from "@skeletonlabs/skeleton-svelte";

  let { children: content, positioning = undefined } = $props();

  // Skeleton replaces the positioner's style when Zag publishes its placement,
  // removing the coordinates Zag just wrote. Position again after that render;
  // keep Zag responsible for the anchor, viewport bounds, and flipping.
  // Zag's reposition builds its own anchor callback for context menus and lays
  // it over the configured positioning even when it is undefined, so the menu's
  // positioning is handed back to it here. A menu opened from the Panels trigger
  // keeps hanging from that trigger, and a menu with no positioning of its own
  // repositions from its trigger as before.
  function positionMenu(_element, parameter) {
    let frame;
    function position({ api, positioning }) {
      cancelAnimationFrame(frame);
      if (api.open) frame = requestAnimationFrame(() => api.reposition(positioning));
    }
    position(parameter);
    return { update: position, destroy: () => cancelAnimationFrame(frame) };
  }
</script>

<Menu.Context>
  {#snippet children(menu)}
    <Portal>
      <Menu.Positioner>
        {#snippet element(attributes)}
          <div {...attributes} style:transform="translate3d(var(--x, -100vw), var(--y, -100vh), 0)" use:positionMenu={{ api: menu(), positioning }}>
            <!-- Author the content element here rather than passing `class` to
                 Menu.Content: a class arriving as a prop carries no scope hash,
                 so the rule below would have to be global to paint at all. -->
            <Menu.Content>
              {#snippet element(attributes)}
                <div {...attributes} class="explorer-menu">{@render content()}</div>
              {/snippet}
            </Menu.Content>
          </div>
        {/snippet}
      </Menu.Positioner>
    </Portal>
  {/snippet}
</Menu.Context>

<style>
  .explorer-menu {
    z-index: 50;
    box-sizing: border-box;
    width: max-content;
    min-width: 13rem;
    max-width: calc(100vw - calc(var(--spacing) * 4));
    padding: calc(var(--spacing) * 1);
    border: 1px solid var(--color-divider);
    border-radius: var(--radius-base);
    background: var(--color-raised);
    box-shadow: var(--shadow-xl);
    overflow-x: hidden;
    /* Zag measures the room left below the anchor and writes it on the
       positioner, and a custom property inherits; a panel with more items
       than that scrolls instead of running off the bottom of a phone held
       sideways. */
    max-height: var(--available-height);
    overflow-y: auto;
    /* A menu is not a place to select text from; dragging across it should
       feel like sliding along the items, not like sweeping up their names. */
    user-select: none;
  }
</style>
