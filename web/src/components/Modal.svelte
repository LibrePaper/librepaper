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
    initialFocus = undefined,
    onEscapeKeyDown = undefined,
  } = $props();

  // Two sizes for a card: a question or a result to read. `wider` is a
  // third, for a palette whose row of tabs does not fit the second. `page`
  // is the other shape: the whole viewport, with a bar at the top holding a
  // Back link. It has no footer and no backdrop. Escape and Back both close it.
  const width = $derived(wider ? "max-w-3xl" : wide ? "max-w-xl" : "max-w-lg");

  // The cross closes the same way Escape does: the dialog's own change
  // handler runs `onclose`, so closing from here says so too.
  function close() {
    open = false;
    onclose?.();
  }

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
    <Dialog.Positioner class={page ? "fixed inset-0 z-50 flex" : "fixed inset-0 z-50 flex items-center justify-center p-4"}>
      <Dialog.Content class={page ? "modal-page flex h-full w-full flex-col overflow-hidden" : `card bg-[var(--color-raised)] flex max-h-full w-full flex-col gap-4 p-6 shadow-xl ${width}`}>
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
          <header class="flex shrink-0 items-start gap-3">
            <div class="min-w-0 flex-1">
              <Dialog.Title class="h4">{title}</Dialog.Title>
              {#if description}
                <Dialog.Description class="lp-text-secondary text-sm">{description}</Dialog.Description>
              {/if}
            </div>
            <IconButton icon="x" label="Close" tone="plain" size="btn-icon-sm" onclick={close} />
          </header>
          <div class="flex min-h-0 flex-col gap-3 overflow-y-auto">
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
