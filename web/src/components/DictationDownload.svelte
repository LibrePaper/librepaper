<script>
  // Before the first download of a model, the reader
  // sees what is about to happen and can say no. The service knows nothing
  // about dialogs -- it calls whatever `setDownloadConfirmation` last
  // registered (service.js) and awaits a boolean -- so this component is the
  // only place that decision is asked, and mounting it is what turns the
  // question on.
  import { onDestroy, onMount } from "svelte";
  import Modal from "./Modal.svelte";
  import { setDownloadConfirmation } from "../lib/dictation/service.js";

  let open = $state(false);
  let entry = $state(null);
  let resolver = null;

  function megabytes(bytes) {
    return `${Math.round((bytes || 0) / (1024 * 1024))} MB`;
  }

  // Only one question at a time. A second model download landing mid-dialog
  // (there is no surface that can trigger that today, but the service does
  // not prevent it) is declined outright rather than queued or clobbering
  // the one already on screen.
  function confirmDownload(model) {
    if (open) return Promise.resolve(false);
    entry = model;
    open = true;
    return new Promise((resolve) => {
      resolver = resolve;
    });
  }

  function settle(value) {
    // `entry` is left in place rather than cleared here: the dialog is still
    // animating closed, and blanking its body mid-animation would look like
    // a glitch. The next `confirmDownload` call overwrites it before it is
    // ever shown again.
    open = false;
    const resolve = resolver;
    resolver = null;
    resolve?.(value);
  }

  onMount(() => setDownloadConfirmation(confirmDownload));
  onDestroy(() => setDownloadConfirmation(null));
</script>

{#snippet footer()}
  <button type="button" class="btn btn-sm preset-outlined-surface-300-700" onclick={() => settle(false)}>
    Cancel
  </button>
  <button type="button" class="btn btn-sm preset-filled-primary-500" onclick={() => settle(true)}>
    Download
  </button>
{/snippet}

<Modal bind:open title="Download a speech model" onclose={() => settle(false)} {footer}>
  {#if entry}
    <p class="panel-muted">
      This will download {entry.label}, about {megabytes(entry.sizeBytes)}. It downloads once and is
      stored by this browser.
    </p>
    <p class="panel-muted">
      After that, recognition runs on this device: your audio never leaves it.
    </p>
  {/if}
</Modal>
