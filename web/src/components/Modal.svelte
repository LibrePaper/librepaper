<script>
  import { Dialog } from "@skeletonlabs/skeleton-svelte";

  // Every dialog in the application, so there is one answer to what a dialog
  // is: a card in the middle of a dimmed page, with a heading, whatever it is
  // asking, and its buttons at the bottom right.
  //
  // The behaviour is Skeleton's, which is Zag's: focus moves in on open and
  // back to where it came from on close, it is trapped while the dialog is
  // open, Escape closes, the page behind does not scroll, and the heading
  // names the dialog for a screen reader. Five hand-written <dialog> elements
  // did some of that and none of them did all of it.
  let { open = $bindable(false), title, description = null, children, footer, onclose, wide = false, full = false } = $props();

  // Three sizes: a question, a result to read, or a page -- the settings --
  // which is as wide as a page and a fixed height, so what is inside it can
  // keep its own columns and scroll them itself rather than being scrolled.
  const width = $derived(full ? "max-w-5xl modal-page" : wide ? "max-w-xl" : "max-w-lg");
</script>

<Dialog {open} onOpenChange={(event) => { open = event.open; if (!open) onclose?.(); }}>
  <Dialog.Backdrop class="fixed inset-0 z-50 bg-surface-950/50 backdrop-blur-xs" />
  <Dialog.Positioner class="fixed inset-0 z-50 flex items-center justify-center p-4">
    <Dialog.Content class="card bg-surface-50-950 flex max-h-full w-full flex-col gap-4 p-6 shadow-xl {width}">
      <header class="shrink-0">
        <Dialog.Title class="h4">{title}</Dialog.Title>
        {#if description}
          <Dialog.Description class="text-surface-600-400 text-sm">{description}</Dialog.Description>
        {/if}
      </header>
      <div class="flex min-h-0 flex-col gap-3 {full ? 'flex-1 overflow-hidden' : 'overflow-y-auto'}">
        {@render children?.()}
      </div>
      {#if footer}
        <footer class="flex shrink-0 justify-end gap-2 pt-2">
          {@render footer()}
        </footer>
      {/if}
    </Dialog.Content>
  </Dialog.Positioner>
</Dialog>
