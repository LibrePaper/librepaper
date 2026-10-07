<script>
  import { Menu } from "@skeletonlabs/skeleton-svelte";
  import ExplorerMenu from "./ExplorerMenu.svelte";
  import { menubar, PANEL_POSITIONING } from "../lib/menubar.svelte.js";
  import { untrack } from "svelte";

  // One word in the bar and the panel behind it. The open state is the bar's,
  // so a click on File followed by a slide onto Edit reads as one gesture.
  let { id, label, disabled = false, onselect = undefined, onopen = undefined, children, ...rest } = $props();

  // A menu can leave the bar while it is open -- Edit goes when the source
  // pane closes -- and the bar would otherwise stay engaged, opening every
  // word the pointer crossed afterwards.
  $effect(() => () => menubar.close(id));

  // Call onopen whenever the menu transitions from closed to open, whether it's
  // opened by clicking or by calling menubar.show(id) programmatically. Using
  // untrack prevents the effect from subscribing to onopen's internals.
  let wasOpen = false;
  $effect(() => {
    const isOpen = menubar.opened === id;
    if (isOpen && !wasOpen) {
      untrack(() => onopen?.());
    }
    wasOpen = isOpen;
  });

  // Opened from the Panels trigger the menu hangs from that trigger, in the
  // same box its panel used; returning null when the trigger is gone lets Zag
  // fall back to the menu's own trigger.
  const positioning = $derived(menubar.anchor ? { ...PANEL_POSITIONING, getAnchorRect: () => (menubar.anchor?.isConnected ? menubar.anchor.getBoundingClientRect() : null) } : undefined);
</script>

<Menu
  open={menubar.opened === id}
  onOpenChange={(event) => { if (event.open) { menubar.show(id); } else menubar.close(id); }}
  onSelect={(chosen) => onselect?.(chosen.value)}
  {positioning}
>
  <Menu.Trigger
    class="menubar-item"
    data-menubar={id}
    {disabled}
    onpointerenter={() => { if (!disabled) menubar.point(id); }}
    {...rest}
  >{label}</Menu.Trigger>
  <ExplorerMenu {positioning}>{@render children()}</ExplorerMenu>
</Menu>
