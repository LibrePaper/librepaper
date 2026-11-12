<script module>
  import { installTips } from "../lib/tip.js";

  // The page entries install the tooltip, and so does this, once, for a page
  // put together without them (a test harness that mounts one component).
  if (typeof document !== "undefined") installTips();
</script>

<script>
  import { label } from "../lib/commands.js";

  // What an icon control says about itself: its name, and beside it the
  // shortcut as a keycap when it has one. The tooltip itself is drawn by
  // lib/tip.js, one for the whole page, from the attributes this hands to the
  // control. The caller puts them on the element it draws, so the control
  // stays the trigger and no wrapper breaks the row it sits in.
  //
  // `shortcut` is a binding from lib/commands.js. The caller passes null where
  // the key belongs to a modal editor mode (Vim, Emacs), as the menus do.
  let { text, shortcut = null, children } = $props();
</script>

{@render children({ "data-tip": text, "data-tip-key": shortcut ? label(shortcut) : undefined })}
