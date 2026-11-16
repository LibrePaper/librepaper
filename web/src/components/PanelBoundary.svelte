<script>
  // A boundary around one part of the reader: the source editor, the preview,
  // a side panel. Boundary.svelte catches everything at once and replaces the
  // whole page; this one catches a single part, so the rest of the reader
  // keeps working while it is down.
  //
  // The first failure is retried by itself after a second, in case it was a
  // transient throw. A failure within ten seconds of an automatic retry is
  // left for the reader to retry, so a part that throws on every render does
  // not loop.
  import { report } from "../lib/crash.js";
  import { copyReport } from "../lib/support/copy.js";
  import { m } from "../lib/paraglide/messages.js";

  let { children, name = "this part" } = $props();

  // Outcome of the last "Copy diagnostics" press, shown under the buttons.
  let copyStatus = $state("");
  let copyFailed = $state(false);

  // No await before copyReport: the clipboard write has to start in the click.
  function copyDiagnostics() {
    copyReport().then(
      () => { copyStatus = m.diagnosticspanel_copied(); copyFailed = false; },
      (error) => { copyStatus = error?.message || m.diagnosticspanel_copy_failed(); copyFailed = true; },
    );
  }

  const RETRY_DELAY = 1000;
  const RETRY_WINDOW = 10000;
  // When the last automatic retry was scheduled, in milliseconds.
  let lastRetry = 0;

  // Attached to the notice, since a snippet cannot hold an effect of its own.
  // Runs each time a failure shows the notice.
  function autoRetry(node, reset) {
    let timer = null;
    const now = Date.now();
    if (now - lastRetry >= RETRY_WINDOW) {
      lastRetry = now;
      timer = setTimeout(reset, RETRY_DELAY);
    }
    return { destroy: () => clearTimeout(timer) };
  }

  const messageOf = (error) =>
    error && typeof error === "object" && "message" in error ? String(error.message) : "";
</script>

<svelte:boundary onerror={(error) => report(error, name)}>
  {@render children()}

  {#snippet failed(error, reset)}
    <div class="panel-failed" role="alert" use:autoRetry={reset}>
      <p>{m.panelboundary_failed()}</p>
      {#if messageOf(error)}<p class="panel-failed-detail">{messageOf(error)}</p>{/if}
      <div class="panel-failed-actions">
        <button type="button" onclick={reset}>{m.common_try_again()}</button>
        <button type="button" onclick={copyDiagnostics}>{m.diagnosticspanel_copy()}</button>
      </div>
      <p class="panel-failed-status" class:panel-failed-error={copyFailed} role="status">{copyStatus}</p>
    </div>
  {/snippet}
</svelte:boundary>

<style>
  /* Inherits the colour and font of the panel it sits in, so it reads the
     same in every theme and needs no token of its own. */
  .panel-failed {
    margin: 1rem;
    font-size: 0.875rem;
    line-height: 1.4;
  }
  .panel-failed p { margin: 0 0 0.5rem; }
  .panel-failed-actions { display: flex; flex-wrap: wrap; gap: 0.5rem; }
  .panel-failed-status { font-size: 0.75rem; }
  .panel-failed-status:empty { margin: 0; }
  .panel-failed-error { font-weight: 600; }
  .panel-failed-detail {
    font-family: ui-monospace, monospace;
    font-size: 0.75rem;
    opacity: 0.7;
    overflow-wrap: anywhere;
  }
  .panel-failed button {
    padding: 0.25rem 0.6rem;
    border: 1px solid currentcolor;
    border-radius: 0.375rem;
    background: none;
    color: inherit;
    cursor: pointer;
    font: inherit;
  }
</style>
