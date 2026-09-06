<script>
  import { Dialog } from "@skeletonlabs/skeleton-svelte";
  import Stack from "./layout/Stack.svelte";

  // Every dialog in the application, so there is one answer to what a dialog
  // is: a card in the middle of a dimmed page, with a heading, whatever it is
  // asking, and its buttons at the bottom right.
  //
  // The behaviour is Skeleton's, which is Zag's: focus moves in on open and
  // back to where it came from on close, it is trapped while the dialog is
  // open, Escape closes, the page behind does not scroll, and the heading
  // names the dialog for a screen reader. Five hand-written <dialog> elements
  // did some of that and none of them did all of it.
  let { open = $bindable(false), title, description = null, children, footer, onclose } = $props();
</script>

<Dialog {open} onOpenChange={(event) => { open = event.open; if (!open) onclose?.(); }}>
  <Dialog.Backdrop class="fixed inset-0 z-50 bg-surface-950/50 backdrop-blur-xs" />
  <Dialog.Positioner class="fixed inset-0 z-50 flex items-center justify-center p-4">
    <Dialog.Content class="card bg-surface-50-950 w-full max-w-lg space-y-4 p-6 shadow-xl">
      <header>
        <Dialog.Title class="h4">{title}</Dialog.Title>
        {#if description}
          <Dialog.Description class="text-surface-600-400 text-sm">{description}</Dialog.Description>
        {/if}
      </header>
      <Stack gap={3}>
        {@render children?.()}
      </Stack>
      {#if footer}
        <footer class="flex justify-end gap-2 pt-2">
          {@render footer()}
        </footer>
      {/if}
    </Dialog.Content>
  </Dialog.Positioner>
</Dialog>
