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
      class="card toast-card p-4 shadow-lg {toast.type === 'error'
        ? 'lp-control-danger'
        : toast.type === 'success'
          ? 'lp-control-success'
          : 'lp-control-neutral'}"
    >
      <div class="flex items-start justify-between gap-3">
        <Toast.Description class="text-sm">{toast.description}</Toast.Description>
        <Toast.CloseTrigger class="btn-icon btn-icon-sm" aria-label="Dismiss">×</Toast.CloseTrigger>
      </div>
      {#if toast.action}
        <Toast.ActionTrigger class="btn btn-sm lp-control-outline mt-3">{toast.action.label}</Toast.ActionTrigger>
      {/if}
    </Toast>
  {/snippet}
</Toast.Group>
