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
            <Menu.Content class="explorer-menu card bg-surface-50-950 z-50 w-52 p-1 shadow-xl">
              {@render content()}
            </Menu.Content>
          </div>
        {/snippet}
      </Menu.Positioner>
    </Portal>
  {/snippet}
</Menu.Context>
