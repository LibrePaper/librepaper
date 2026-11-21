<script>
  // A separator that can be dragged, double-clicked to reset, or focused and
  // moved with the arrow keys. An optional `pill` snippet adds a small column
  // of controls sitting on the divider, under a grab handle that drags it.
  //
  // While dragging, only a guide line moves; the real width -- and the iframe
  // reflow that comes with it -- is applied once, on release.
  import { clamp, edgeAt, grows, range, reset as resetOf, sizeAt, snapped, step } from "../lib/panes.js";

  let { pane, label, panes, controls, pill = undefined, onsize, onguide, ongrab } = $props();

  let element = $state(null);
  // Set while a drag is in progress, so the divider stays lit even when the
  // pointer has left the grab handle that started it.
  let dragging = $state(false);
  // What the separator says about itself: where it is, and the two places it
  // cannot be moved past. A splitter that reported 0 and the whole window
  // would have the arrow keys stop somewhere the value never explained.
  const limits = $derived(range(pane, panes));
  const asValue = (size) => Math.round(size * (pane.fraction ? 100 : 1));

  const guideFor = (size) => ({ shown: true, left: edgeAt(pane, size, panes), held: snapped(pane, size) });

  function down(event) {
    event.preventDefault();
    element.setPointerCapture(event.pointerId);
    dragging = true;
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
    dragging = false;
    ongrab?.(false);
    onsize?.(sizeAt(pane, event.clientX, panes));
  }

  // The way back to a sensible split, which is what dragging on its own has
  // never offered.
  function reset() {
    onsize?.(resetOf(pane));
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
  class="grip grip-{pane.name}" class:grip-dragging={dragging}
  role="separator"
  aria-orientation="vertical"
  aria-label={label}
  aria-controls={controls}
  aria-valuenow={asValue(clamp(pane, panes))}
  aria-valuemin={asValue(limits.min)}
  aria-valuemax={asValue(limits.max)}
  tabindex="0"
  onpointerdown={down}
  onpointermove={move}
  onpointerup={finish}
  onpointercancel={finish}
  ondblclick={reset}
  onkeydown={key}
></div>

<!-- The pill is a sibling of the separator, not a child: a separator's
     children are presentational to assistive technology, and the buttons in
     the pill have to stay reachable. The slot takes no room in the row, so the
     panes on either side are laid out as if it were not there. -->
{#if pill}
  <div class="grip-pill-slot">
    <div class="grip-pill">
      <span class="grip-grab" aria-hidden="true" onpointerdown={down} onpointermove={move} onpointerup={finish} onpointercancel={finish} ondblclick={reset}>
        <svg viewBox="0 0 10 14" width="10" height="14" fill="currentColor" focusable="false">
          <circle cx="2.5" cy="2" r="1.1" />
          <circle cx="7.5" cy="2" r="1.1" />
          <circle cx="2.5" cy="7" r="1.1" />
          <circle cx="7.5" cy="7" r="1.1" />
          <circle cx="2.5" cy="12" r="1.1" />
          <circle cx="7.5" cy="12" r="1.1" />
        </svg>
      </span>
      <span class="grip-pill-rule"></span>
      {@render pill()}
    </div>
  </div>
{/if}
