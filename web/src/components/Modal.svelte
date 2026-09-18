<script>
  import { Dialog, Portal } from "@skeletonlabs/skeleton-svelte";
  import IconButton from "./IconButton.svelte";

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
  // has to be closed. A dialog with no footer at all -- the settings page,
  // the storage report -- passes no `confirm`.
  let {
    open = $bindable(false),
    title,
    description = null,
    children,
    confirm = null,
    cancelLabel = "Cancel",
    onclose,
    wide = false,
    full = false,
  } = $props();

  // Three sizes: a question, a result to read, or a page -- the settings --
  // which is as wide as a page and a fixed height, so what is inside it can
  // keep its own columns and scroll them itself rather than being scrolled.
  const width = $derived(full ? "max-w-5xl modal-page" : wide ? "max-w-xl" : "max-w-lg");

  // The cross closes the same way Escape does: the dialog's own change
  // handler runs `onclose`, so closing from here says so too.
  function close() {
    open = false;
    onclose?.();
  }

  let confirmButton = $state(null);
  // A form dialog has a field to type in; Zag's default lands on it. Only the
  // question-shaped dialogs move focus to the action.
  const initialFocusEl = $derived(confirm && !confirm.form ? () => confirmButton : undefined);
</script>

<!-- The backdrop and the card are portalled to <body>. `fixed` only means
     "the viewport" when no ancestor has a transform, filter or backdrop
     filter; the navbar has one, so a dialog opened from a navbar menu was
     confined to the navbar and drawn behind the page. Rendering at the body
     removes the question of where the dialog happens to be mounted. -->
<Dialog {open} {initialFocusEl} onOpenChange={(event) => { open = event.open; if (!open) onclose?.(); }}>
  <Portal>
    <Dialog.Backdrop class="fixed inset-0 z-50 bg-surface-950/50 backdrop-blur-xs" />
    <Dialog.Positioner class="fixed inset-0 z-50 flex items-center justify-center p-4">
      <Dialog.Content class="card bg-surface-50-950 flex max-h-full w-full flex-col gap-4 p-6 shadow-xl {width}">
        <header class="flex shrink-0 items-start gap-3">
          <div class="min-w-0 flex-1">
            <Dialog.Title class="h4">{title}</Dialog.Title>
            {#if description}
              <Dialog.Description class="text-surface-600-400 text-sm">{description}</Dialog.Description>
            {/if}
          </div>
          <IconButton icon="x" label="Close" tone="plain" size="btn-icon-sm" onclick={close} />
        </header>
        <div class="flex min-h-0 flex-col gap-3 {full ? 'flex-1 overflow-hidden' : 'overflow-y-auto'}">
          {@render children?.()}
        </div>
        {#if confirm}
          <footer class="flex shrink-0 justify-end gap-2 pt-2">
            {#if confirm.cancel !== false}
              <button type="button" class="btn preset-outlined-surface-300-700" onclick={confirm.oncancel ?? close}>
                {cancelLabel}
              </button>
            {/if}
            <button
              bind:this={confirmButton}
              type={confirm.form ? "submit" : "button"}
              form={confirm.form}
              class="btn {confirm.tone === 'error' ? 'preset-filled-error-500' : 'preset-filled-primary-500'}"
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
