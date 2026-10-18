<script>
  import { Dialog, Portal } from "@skeletonlabs/skeleton-svelte";
  import IconButton from "./IconButton.svelte";
  import Icon from "./Icon.svelte";

  // Every dialog in the application, so there is one answer to what a dialog
  // is: a card in the middle of a dimmed page, with a heading, whatever it is
  // asking, and its buttons at the bottom right.
  //
  // The behaviour is Skeleton's, which is Zag's: focus moves in on open and
  // back to where it came from on close, it is trapped while the dialog is
  // open, Escape and the cross at the top right close, the page behind does
  // not scroll, and the heading names the dialog for a screen reader. Five hand-written <dialog> elements
  // did some of that and none of them did all of it.
  //
  // `confirm` is how a dialog asks for an answer: give it a label and what to
  // do, and the footer is written here -- Cancel, then the action, in that
  // order, everywhere -- and the action is what has focus when the dialog
  // opens, so a dialog that asks one question is answered with Enter. A
  // dialog whose body is a <form id="..."> passes that id as `confirm.form`
  // instead: the action submits the form, so Enter in any of its fields
  // commits too, and focus is left to the field the form autofocuses.
  //
  // `cancelLabel` renames the way out where "Cancel" would be wrong ("Skip",
  // "Sign in"), and `confirm.cancel: false` drops it for a dialog that only
  // has to be closed. A dialog with no footer at all -- a page, the storage
  // report -- passes no `confirm`.
  let {
    open = $bindable(false),
    title,
    description = null,
    children = undefined,
    confirm = null,
    cancelLabel = "Cancel",
    onclose = undefined,
    wide = false,
    wider = false,
    page = false,
    compact = false,
    sheet = false,
    initialFocus = undefined,
    onEscapeKeyDown = undefined,
  } = $props();

  // Two sizes for a card: a question or a result to read. `wider` is a
  // third, for a palette whose row of tabs does not fit the second. `page`
  // is the other shape: the whole viewport, with a bar at the top holding a
  // Back link. It has no footer and no backdrop. Escape and Back both close it.
  // `compact` is a tighter card for a picker. Its body scrolls inside itself,
  // so the picker decides what scrolls. `sheet` turns the card into a bottom
  // sheet below 480px wide, with a handle that expands it. A drag down on the
  // handle or the header closes it, or shrinks it when it is expanded.
  const width = $derived(compact ? "max-w-[44rem]" : wider ? "max-w-3xl" : wide ? "max-w-xl" : "max-w-lg");
  const cardClass = $derived(compact
    ? `card bg-[var(--color-raised)] flex max-h-full max-h-[75vh] w-full flex-col gap-3 p-4 shadow-xl ${width}`
    : `card bg-[var(--color-raised)] flex max-h-full w-full flex-col gap-4 p-6 shadow-xl ${width}`);
  const contentClass = $derived(`${cardClass}${sheet ? " modal-sheet" : ""}${sheet && expanded ? " modal-sheet-expanded" : ""}`);

  // The cross closes the same way Escape does: the dialog's own change
  // handler runs `onclose`, so closing from here says so too.
  function close() {
    open = false;
    onclose?.();
  }

  // A sheet starts at 70% of the screen. Its handle expands it to the full
  // visual viewport. A drag down on the handle or the header moves the card
  // with the finger. Only the handle and the header start a drag, so scrolling
  // the body never moves the sheet. Drags are ignored at 480px and wider.
  let expanded = $state(false);
  const DRAG_START = 4;
  const DRAG_CLOSE = 80;
  const DRAG_EXPAND = -60;
  let drag = null;

  const phone = () => matchMedia("(max-width: 479px)").matches;

  function startDrag(event) {
    if (!sheet || !phone()) return;
    if (event.target.closest("button") && !event.target.closest(".sheet-handle")) return;
    drag = { id: event.pointerId, y: event.clientY, raw: 0, dy: 0, moved: false, card: event.currentTarget.closest('[data-part="content"]') };
  }

  function moveDrag(event) {
    if (!drag || event.pointerId !== drag.id) return;
    const raw = event.clientY - drag.y;
    if (!drag.moved) {
      if (Math.abs(raw) < DRAG_START) return;
      // Capture waits until the finger has moved, so a tap on the handle
      // still reaches its click handler.
      drag.moved = true;
      event.currentTarget.setPointerCapture(event.pointerId);
    }
    drag.raw = raw;
    drag.dy = Math.max(0, raw);
    if (drag.card) drag.card.style.transform = `translateY(${drag.dy}px)`;
  }

  function endDrag(event) {
    if (!drag || event.pointerId !== drag.id) return;
    const { raw, dy, card, moved } = drag;
    drag = null;
    if (card) card.style.transform = "";
    if (!moved) return;
    if (dy > DRAG_CLOSE) {
      if (expanded) expanded = false;
      else close();
    } else if (raw < DRAG_EXPAND) {
      expanded = true;
    }
  }

  function cancelDrag(event) {
    if (!drag || event.pointerId !== drag.id) return;
    if (drag.card) drag.card.style.transform = "";
    drag = null;
  }

  // The layout viewport does not shrink for the virtual keyboard, the visual
  // viewport does. The sheet follows the visual viewport so its bottom edge
  // stays above the keyboard.
  $effect(() => {
    const vv = window.visualViewport;
    if (!open || !sheet || !vv) return;
    const style = document.documentElement.style;
    const update = () => {
      style.setProperty("--sheet-viewport", `${vv.height}px`);
      style.setProperty("--sheet-keyboard", `${Math.max(0, window.innerHeight - vv.height - vv.offsetTop)}px`);
    };
    update();
    vv.addEventListener("resize", update);
    vv.addEventListener("scroll", update);
    return () => {
      vv.removeEventListener("resize", update);
      vv.removeEventListener("scroll", update);
      style.removeProperty("--sheet-viewport");
      style.removeProperty("--sheet-keyboard");
    };
  });

  let confirmButton = $state(null);
  // A form dialog has a field to type in; Zag's default lands on it. Only the
  // question-shaped dialogs move focus to the action. A dialog whose first
  // control is not what it is for, such as a palette behind its Close button,
  // names its own with `initialFocus`.
  const initialFocusEl = $derived(initialFocus ?? (confirm && !confirm.form ? () => confirmButton : undefined));

  // Closing hands focus back to the opener. Mark that element so it draws no
  // ring until the next key press or until it loses focus.
  function quietReturnedFocus() {
    const mark = (target) => {
      if (!(target instanceof HTMLElement) || target === document.body) return;
      target.dataset.quietFocus = "";
      const clear = () => {
        delete target.dataset.quietFocus;
        target.removeEventListener("blur", clear);
        document.removeEventListener("keydown", clear, true);
      };
      target.addEventListener("blur", clear);
      document.addEventListener("keydown", clear, true);
    };
    const onfocusin = (event) => mark(event.target);
    document.addEventListener("focusin", onfocusin, true);
    setTimeout(() => document.removeEventListener("focusin", onfocusin, true), 300);
  }

  // Runs once per close, however it happens: the cross, Escape, an action,
  // or a parent setting `open` to false.
  let wasOpen = false;
  $effect(() => {
    if (wasOpen && !open) quietReturnedFocus();
    if (open && !wasOpen) expanded = false;
    wasOpen = open;
  });
</script>

<!-- The backdrop and the card are portalled to <body>. `fixed` only means
     "the viewport" when no ancestor has a transform, filter or backdrop
     filter; the navbar has one, so a dialog opened from a navbar menu was
     confined to the navbar and drawn behind the page. Rendering at the body
     removes the question of where the dialog happens to be mounted. -->
<!-- Zag hears Escape on the document before anything inside the dialog does.
     `onEscapeKeyDown` lets the content keep a press by cancelling it. -->
<Dialog {open} {initialFocusEl} {onEscapeKeyDown} onOpenChange={(event) => { open = event.open; if (!open) onclose?.(); }}>
  <Portal>
    {#if !page}
      <Dialog.Backdrop class="fixed inset-0 z-50 bg-black/50 backdrop-blur-xs" />
    {/if}
    <Dialog.Positioner class={page ? "fixed inset-0 z-50 flex" : `fixed inset-0 z-50 flex items-center justify-center p-4${sheet ? " modal-sheet-positioner" : ""}`}>
      <Dialog.Content class={page ? "modal-page flex h-full w-full flex-col overflow-hidden" : contentClass}>
        {#if page}
          <header class="modal-page-bar">
            <div class="modal-page-bar-inner">
              <button type="button" class="modal-back" onclick={close}>
                <Icon name="arrow-left" size={18} />
                Back
              </button>
              <Dialog.Title class="modal-page-title">{title}</Dialog.Title>
            </div>
          </header>
          <div class="modal-page-body">
            {@render children?.()}
          </div>
        {:else}
          <div class="flex shrink-0 flex-col gap-3" class:sheet-grab={sheet}
               onpointerdown={startDrag} onpointermove={moveDrag} onpointerup={endDrag} onpointercancel={cancelDrag}>
            {#if sheet}
              <button type="button" class="sheet-handle" aria-label={expanded ? "Shrink" : "Expand"}
                      onclick={() => (expanded = !expanded)}></button>
            {/if}
            <header class="flex shrink-0 items-start gap-3">
              <div class="min-w-0 flex-1">
                <Dialog.Title class={compact ? "text-base font-semibold" : "h4"}>{title}</Dialog.Title>
                {#if description}
                  <Dialog.Description class="lp-text-secondary text-sm">{description}</Dialog.Description>
                {/if}
              </div>
              <IconButton icon="x" label="Close" tone="plain" size="btn-icon-sm" onclick={close} />
            </header>
          </div>
          <div class={compact ? "flex min-h-0 flex-1 flex-col overflow-hidden" : "flex min-h-0 flex-col gap-3 overflow-y-auto"}>
            {@render children?.()}
          </div>
        {/if}
        {#if confirm && !page}
          <footer class="flex shrink-0 justify-end gap-2 pt-2">
            {#if confirm.cancel !== false}
              <button type="button" class="btn lp-control-tonal-brand" onclick={confirm.oncancel ?? close}>
                {cancelLabel}
              </button>
            {/if}
            <button
              bind:this={confirmButton}
              type={confirm.form ? "submit" : "button"}
              form={confirm.form}
              class="btn {confirm.tone === 'error' ? 'lp-control-danger' : 'lp-control-brand'}"
              disabled={confirm.disabled}
              onclick={confirm.form ? undefined : confirm.onclick}
            >
              {confirm.label}
            </button>
          </footer>
        {/if}
      </Dialog.Content>
    </Dialog.Positioner>
  </Portal>
</Dialog>

<style>
  /* Scoped rules do not reach Dialog.Positioner and Dialog.Content, which are
     components, so the sheet is styled by global class names. */
  .sheet-grab { touch-action: none; }
  .sheet-handle { display: none; }

  @media (max-width: 479px) {
    .sheet-handle {
      display: flex;
      align-self: stretch;
      justify-content: center;
      padding: 0.25rem 0;
      border: 0;
      background: none;
      cursor: grab;
    }
    .sheet-handle::before {
      content: "";
      width: 2.25rem;
      height: 0.25rem;
      border-radius: 9999px;
      background: var(--color-border-strong);
    }

    :global(.modal-sheet-positioner) {
      align-items: flex-end;
      padding: 0;
    }
    :global(.modal-sheet) {
      max-width: none;
      width: 100%;
      height: 70dvh;
      max-height: var(--sheet-viewport, 100dvh);
      margin-bottom: var(--sheet-keyboard, 0px);
      border-top-left-radius: var(--radius-container);
      border-top-right-radius: var(--radius-container);
      border-bottom-left-radius: 0;
      border-bottom-right-radius: 0;
      padding-bottom: calc(1rem + env(safe-area-inset-bottom));
    }
    :global(.modal-sheet.modal-sheet-expanded) {
      height: calc(var(--sheet-viewport, 100dvh) - 0.5rem);
    }
    @media (prefers-reduced-motion: no-preference) {
      :global(.modal-sheet) {
        animation: modal-sheet-up 200ms ease-out;
      }
    }
    @keyframes modal-sheet-up {
      from { transform: translateY(100%); }
    }
  }
</style>