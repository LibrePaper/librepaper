<script>
  import { Menu } from "@skeletonlabs/skeleton-svelte";
  import ExplorerMenu from "./ExplorerMenu.svelte";
  import { menubar } from "../lib/menubar.svelte.js";

  // One word in the bar and the panel behind it. The open state is the bar's,
  // so a click on File followed by a slide onto Edit reads as one gesture.
  let { id, label, disabled = false, onselect, onopen, children, ...rest } = $props();

  // A menu can leave the bar while it is open -- Edit goes when the source
  // pane closes -- and the bar would otherwise stay engaged, opening every
  // word the pointer crossed afterwards.
  $effect(() => () => menubar.close(id));
</script>

<Menu
  open={menubar.opened === id}
  onOpenChange={(event) => { if (event.open) { menubar.show(id); onopen?.(); } else menubar.close(id); }}
  onSelect={(chosen) => onselect?.(chosen.value)}
>
  <Menu.Trigger
    class="menubar-item"
    data-menubar={id}
    {disabled}
    onpointerenter={() => { if (!disabled) menubar.point(id); }}
    {...rest}
  >{label}</Menu.Trigger>
  <ExplorerMenu>{@render children()}</ExplorerMenu>
</Menu>
