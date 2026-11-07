<script>
  import { menubar } from "../lib/menubar.svelte.js";

  // The row itself. It owns the shape and the arrow keys along it: which menu
  // is showing is the module's, because the panels are portaled out of here and
  // a key pressed in one of them still has to move the row. With no menu open,
  // the arrows move the focus between the words; with one open, they open the
  // neighbour and put the keyboard in it. Escape is Zag's: it closes the menu
  // and returns the focus to its word.
  let { children } = $props();

  function walk(event) {
    if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
    const delta = event.key === "ArrowRight" ? 1 : -1;
    if (menubar.opened === null) {
      // Captured on the way down for the same reason as below: the words are
      // the bar's, and nothing else on the page should see the arrows move.
      if (!menubar.focusStep(delta)) return;
      event.preventDefault();
      event.stopPropagation();
      return;
    }
    // Right opens a highlighted sub-menu and Left closes an open one. Only
    // those presses are the sub-menu's; every other press walks the row.
    const open = document.querySelector('[data-scope="menu"][data-part="trigger-item"][data-state="open"]');
    const highlighted = document.querySelector('[data-scope="menu"][data-part="trigger-item"][data-highlighted]');
    if (event.key === "ArrowLeft" ? open : (open || highlighted)) return;
    // Captured on the way down: the open panel is portaled to the body and
    // Zag stops these two keys before they bubble back out to the window.
    if (menubar.step(delta)) {
      menubar.enter();
      event.preventDefault();
      event.stopPropagation();
    }
  }
</script>

<svelte:window onkeydowncapture={walk} />

<div class="menubar">{@render children()}</div>
