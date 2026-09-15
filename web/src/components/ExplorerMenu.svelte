<script>
  import { Menu, Portal } from "@skeletonlabs/skeleton-svelte";

  let { children: content } = $props();

  // Skeleton replaces the positioner's style when Zag publishes its placement,
  // removing the coordinates Zag just wrote. Position again after that render;
  // keep Zag responsible for the anchor, viewport bounds, and flipping.
  function positionMenu(_element, menu) {
    let frame;
    function position(api) {
      cancelAnimationFrame(frame);
      if (api.open) frame = requestAnimationFrame(() => api.reposition());
    }
    position(menu);
    return { update: position, destroy: () => cancelAnimationFrame(frame) };
  }
</script>

<Menu.Context>
  {#snippet children(menu)}
    <Portal>
      <Menu.Positioner>
        {#snippet element(attributes)}
          <div {...attributes} style:transform="translate3d(var(--x, -100vw), var(--y, -100vh), 0)" use:positionMenu={menu()}>
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
    min-width: 11rem;
    max-width: calc(100vw - calc(var(--spacing) * 4));
    padding: calc(var(--spacing) * 1.5);
    border: 1px solid var(--color-divider);
    border-radius: var(--radius-base);
    background: var(--color-surface-50-950);
    box-shadow: var(--shadow-xl);
    overflow-x: hidden;
  }
</style>
