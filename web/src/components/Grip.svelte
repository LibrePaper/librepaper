<script>
  // A separator that can be dragged, double-clicked to reset, or focused and
  // moved with the arrow keys.
  //
  // While dragging, only a guide line moves; the real width -- and the iframe
  // reflow that comes with it -- is applied once, on release.
  import { clamp, edgeAt, grows, sizeAt, snapped, step } from "../lib/panes.js";

  let { pane, label, panes, aside, onsize, onguide, ongrab } = $props();

  let element = $state(null);

  const guideFor = (size) => ({ shown: true, left: edgeAt(pane, size, panes), held: snapped(pane, size) });

  function down(event) {
    event.preventDefault();
    element.setPointerCapture(event.pointerId);
    ongrab?.(true);
    onguide?.(guideFor(sizeAt(pane, event.clientX, panes)));
  }

  function move(event) {
    if (!element.hasPointerCapture(event.pointerId)) return;
    onguide?.(guideFor(sizeAt(pane, event.clientX, panes)));
  }

  function finish(event) {
    if (!element.hasPointerCapture(event.pointerId)) return;
    element.releasePointerCapture(event.pointerId);
    ongrab?.(false);
    onsize?.(sizeAt(pane, event.clientX, panes));
  }

  // The way back to a sensible split, which is what dragging on its own has
  // never offered.
  function reset() {
    onsize?.(pane.reset);
  }

  function key(event) {
    if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
    event.preventDefault();
    onsize?.(step(pane, panes, event.key === grows(pane, panes)));
  }
</script>

<!-- A focusable separator is a window splitter, which ARIA makes a widget:
     it takes the focus, it has a value, and the arrow keys move it. Svelte's
     rules read `separator` as non-interactive whatever it carries, so the two
     they raise here are the two that describe a splitter working correctly. -->
<!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
<div
  bind:this={element}
  class="grip grip-{pane.name}"
  role="separator"
  aria-orientation="vertical"
  aria-label={label}
  aria-valuenow={Math.round(clamp(pane, panes) * (pane.fraction ? 100 : 1))}
  aria-valuemin="0"
  aria-valuemax={pane.fraction ? 100 : Math.round(panes.width)}
  tabindex="0"
  onpointerdown={down}
  onpointermove={move}
  onpointerup={finish}
  onpointercancel={finish}
  ondblclick={reset}
  onkeydown={key}
>
  <!-- A quiet sign that something about this separator is not as it usually
       is: today, that clicking in one pane no longer takes the other along. -->
  {#if aside}<span class="grip-aside" aria-hidden="true">{@render aside()}</span>{/if}
</div>
