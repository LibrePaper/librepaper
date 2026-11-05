<script>
  import { Dialog, Portal } from "@skeletonlabs/skeleton-svelte";
  import Icon from "../Icon.svelte";
  import { locale } from "../../lib/locale.js";
  import { m } from "../../lib/paraglide/messages.js";

  // The phone's More menu: everything the bottom bar does not hold, as a
  // sheet that rises from the bottom. Sections are { label, rows }; a row is
  // a button that reports its id, or a link that navigates and also reports.
  //
  // Dialog gives the sheet the same focus trap, Escape, backdrop dismissal,
  // focus return and scroll lock as Modal.svelte. Its handle and body can be
  // dragged down to close, which Skeleton does not do, so that is added here.
  let {
    open = $bindable(false),
    sections = [],
    onchoose = () => {},
    onclose = undefined,
  } = $props();

  function dismiss() {
    open = false;
    onclose?.();
  }

  function choose(row) {
    // A row that navigates closes the sheet too; the link still goes ahead.
    open = false;
    onchoose(row.id);
  }

  // The sheet card is the Dialog.Content element, which is a component and
  // cannot take `use:`. Both the handle and the body find it from inside.
  function sheetCardOf(node) {
    return node.closest('[data-part="content"]') ?? node;
  }

  // Finger swipe. A drag counts if it starts on the handle, or in the list
  // while the list is at its top. Once it moves down it moves the sheet; if
  // the finger moves up first, the touch is left to scroll the list. The
  // listeners are non-passive so preventDefault can stop the page scrolling
  // behind the sheet during the drag.
  function swipe(node) {
    let touch = null;
    const list = () => node.querySelector(".mobile-sheet-list");

    function start(event) {
      const target = event.target;
      const fromHandle = !!target.closest(".sheet-handle");
      const inList = !!target.closest(".mobile-sheet-list");
      if (!fromHandle && !(inList && list().scrollTop <= 0)) return;
      touch = { y: event.touches[0].clientY, dy: 0, dragging: false };
    }

    function move(event) {
      if (!touch) return;
      const dy = event.touches[0].clientY - touch.y;
      if (!touch.dragging) {
        if (dy <= 0) {
          touch = null;
          return;
        }
        touch.dragging = true;
      }
      event.preventDefault();
      touch.dy = Math.max(0, dy);
      sheetCardOf(node).style.transform = `translateY(${touch.dy}px)`;
    }

    function end() {
      if (!touch) return;
      const { dragging, dy } = touch;
      touch = null;
      sheetCardOf(node).style.transform = "";
      if (dragging && dy > 80) dismiss();
    }

    function cancel() {
      touch = null;
      sheetCardOf(node).style.transform = "";
    }

    const options = { passive: false };
    node.addEventListener("touchstart", start, options);
    node.addEventListener("touchmove", move, options);
    node.addEventListener("touchend", end, options);
    node.addEventListener("touchcancel", cancel, options);
    return {
      destroy() {
        node.removeEventListener("touchstart", start, options);
        node.removeEventListener("touchmove", move, options);
        node.removeEventListener("touchend", end, options);
        node.removeEventListener("touchcancel", cancel, options);
      },
    };
  }

  // A mouse has no touch to drag with, so a mouse drag on the handle does the
  // same by pointer. Capture starts only after 4px of movement, so a plain
  // click still reaches the handle's click handler and closes the sheet.
  let mouse = null;
  let justDragged = false;

  function handleDown(event) {
    justDragged = false;
    if (event.pointerType !== "mouse") return;
    mouse = { id: event.pointerId, y: event.clientY, dy: 0, moved: false };
  }

  function handleMove(event) {
    if (!mouse || event.pointerId !== mouse.id) return;
    const raw = event.clientY - mouse.y;
    if (!mouse.moved) {
      if (Math.abs(raw) < 4) return;
      mouse.moved = true;
      event.currentTarget.setPointerCapture(event.pointerId);
    }
    mouse.dy = Math.max(0, raw);
    sheetCardOf(event.currentTarget).style.transform = `translateY(${mouse.dy}px)`;
  }

  function handleUp(event) {
    if (!mouse || event.pointerId !== mouse.id) return;
    const { moved, dy } = mouse;
    mouse = null;
    sheetCardOf(event.currentTarget).style.transform = "";
    if (!moved) return;
    // The release of a drag is followed by a click on the handle; it must
    // not close the sheet a second time.
    justDragged = true;
    if (dy > 80) dismiss();
  }

  function handleCancel(event) {
    if (!mouse || event.pointerId !== mouse.id) return;
    mouse = null;
    sheetCardOf(event.currentTarget).style.transform = "";
  }

  function handleClick() {
    if (justDragged) {
      justDragged = false;
      return;
    }
    dismiss();
  }

  function onOpenChange(event) {
    open = event.open;
    if (!open) onclose?.();
  }
</script>

{#snippet rowContent(row)}
  <span class="row-icon"><Icon name={row.icon} size={20} /></span>
  <span class="row-label">{row.label}</span>
  {#if row.count !== undefined && row.count !== null}
    <span class="row-count">{row.count.toLocaleString(locale())}</span>
  {/if}
  {#if row.dot}<span class="row-dot" aria-hidden="true"></span>{/if}
{/snippet}

<Dialog {open} {onOpenChange}>
  <Portal>
    <Dialog.Backdrop class="mobile-sheet-backdrop bg-black/35" />
    <Dialog.Positioner class="mobile-sheet-positioner">
      <Dialog.Content class="mobile-sheet">
        <div class="mobile-sheet-body" use:swipe>
          <Dialog.Title class="sr-only">{m.mobilenav_more_sheet()}</Dialog.Title>
          <button
            type="button"
            class="sheet-handle"
            aria-label={m.mobilenav_drag_to_close()}
            onpointerdown={handleDown}
            onpointermove={handleMove}
            onpointerup={handleUp}
            onpointercancel={handleCancel}
            onclick={handleClick}
          ></button>
          <div class="mobile-sheet-list">
            {#each sections as section, index (index)}
              {#if section.label}
                <div class="mobile-sheet-label">{section.label}</div>
              {/if}
              {#each section.rows as row (row.id)}
                {#if row.href && !row.disabled}
                  <a
                    class="mobile-sheet-row"
                    data-row={row.id}
                    href={row.href}
                    target={row.external ? "_blank" : undefined}
                    rel={row.external ? "noreferrer" : undefined}
                    onclick={() => choose(row)}
                  >
                    {@render rowContent(row)}
                  </a>
                {:else}
                  <button
                    type="button"
                    class="mobile-sheet-row"
                    data-row={row.id}
                    disabled={row.disabled}
                    onclick={() => choose(row)}
                  >
                    {@render rowContent(row)}
                  </button>
                {/if}
              {/each}
            {/each}
          </div>
        </div>
      </Dialog.Content>
    </Dialog.Positioner>
  </Portal>
</Dialog>

<style>
  /* Dialog's own elements are rendered by Skeleton components, so scoped
     rules do not reach them; they are styled by global class names, as
     Modal.svelte does. */
  :global(.mobile-sheet-backdrop) {
    position: fixed;
    inset: 0;
    z-index: 50;
  }

  :global(.mobile-sheet-positioner) {
    position: fixed;
    inset: 0;
    z-index: 50;
    display: flex;
    align-items: flex-end;
  }

  :global(.mobile-sheet) {
    display: flex;
    flex-direction: column;
    width: 100%;
    max-height: 85dvh;
    padding: 0;
    padding-bottom: env(safe-area-inset-bottom);
    border-top: 1px solid var(--color-pane-edge);
    border-radius: 16px 16px 0 0;
    background: var(--color-raised);
  }

  @media (prefers-reduced-motion: no-preference) {
    :global(.mobile-sheet) {
      animation: mobile-sheet-up 200ms ease-out;
    }
  }

  @keyframes -global-mobile-sheet-up {
    from { transform: translateY(100%); }
  }

  .mobile-sheet-body {
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
  }

  .sheet-handle {
    display: flex;
    flex: none;
    align-items: center;
    justify-content: center;
    width: 100%;
    height: 44px;
    padding: 0;
    border: 0;
    background: none;
    cursor: grab;
    touch-action: none;
  }

  .sheet-handle::before {
    content: "";
    width: 36px;
    height: 4px;
    border-radius: 9999px;
    background: var(--color-border-strong);
  }

  .sheet-handle:focus-visible,
  .mobile-sheet-row:focus-visible {
    outline: 2px solid var(--color-focus);
    outline-offset: -2px;
  }

  .mobile-sheet-list {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    overscroll-behavior: contain;
    padding-bottom: 0.5rem;
  }

  .mobile-sheet-label {
    padding: 0.75rem 1rem 0.25rem;
    font-size: 0.75rem;
    color: var(--color-text-secondary);
  }

  .mobile-sheet-row {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    width: 100%;
    min-height: 2.75rem;
    padding-inline: 1rem;
    border: 0;
    background: none;
    color: var(--color-text);
    font: inherit;
    font-size: 0.875rem;
    text-align: start;
    text-decoration: none;
    cursor: pointer;
  }

  .mobile-sheet-row:hover,
  .mobile-sheet-row:active {
    background: var(--color-row-hover);
  }

  .mobile-sheet-row:disabled {
    opacity: 0.5;
    cursor: default;
    background: none;
  }

  .row-icon {
    display: flex;
    flex: none;
    width: 1.25rem;
    color: var(--color-text-secondary);
  }

  .row-label {
    flex: 1;
    min-width: 0;
  }

  .row-count {
    margin-left: auto;
    color: var(--color-text-secondary);
    font-variant-numeric: tabular-nums;
  }

  .row-dot {
    flex: none;
    width: 7px;
    height: 7px;
    margin-left: auto;
    border-radius: 9999px;
    background: var(--color-brand);
  }

  .row-count + .row-dot {
    margin-left: 0.5rem;
  }
</style>
