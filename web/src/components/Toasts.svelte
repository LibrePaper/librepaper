<script>
  import { Toast } from "@skeletonlabs/skeleton-svelte";
  import { toaster } from "../lib/toast.svelte.js";

  // Where what the application has to say appears. One of these per page, at
  // the bottom, out of the way of both the document and the comment column.
</script>

<Toast.Group {toaster} class="fixed right-4 bottom-4 z-100 flex flex-col gap-2">
  {#snippet children(toast)}
    <Toast
      {toast}
      class="card w-80 p-4 shadow-lg {toast.type === 'error'
        ? 'preset-filled-error-500'
        : toast.type === 'success'
          ? 'preset-filled-success-500'
          : 'preset-filled-surface-200-800'}"
    >
      <div class="flex items-start justify-between gap-3">
        <Toast.Description class="text-sm" />
        <Toast.CloseTrigger class="btn-icon btn-icon-sm" aria-label="Dismiss">×</Toast.CloseTrigger>
      </div>
    </Toast>
  {/snippet}
</Toast.Group>
