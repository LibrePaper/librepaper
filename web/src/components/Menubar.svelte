<script>
  import { menubar } from "../lib/menubar.svelte.js";

  // The row itself. It owns nothing but the shape and the two arrow keys:
  // which menu is showing is the module's, because the panels are portaled
  // out of here and a key pressed in one of them still has to move the row.
  let { children } = $props();

  function walk(event) {
    if (menubar.opened === null) return;
    if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
    // Captured on the way down: the open panel is portaled to the body and
    // Zag stops these two keys before they bubble back out to the window.
    if (menubar.step(event.key === "ArrowRight" ? 1 : -1)) {
      event.preventDefault();
      event.stopPropagation();
    }
  }
</script>

<svelte:window onkeydowncapture={walk} />

<div class="menubar">{@render children()}</div>
