<script>
  // The detailed compile status shown from the Preview header. It
  // distinguishes browser and local output and identifies browser bibliography
  // work when used.
  //
  // Everything here comes from `latex.subscribe`; nothing is stateful on its
  // own. The wording and the action list are pure functions in
  // `latex/status-text.js`, checked without a browser in
  // tests/unit/latex-reader.mjs -- this file only draws what they return and
  // wires the buttons to `latex.js`.
  import * as latex from "../lib/latex.js";
  import { actionsFor, backendChip, fallbackExplanation, failureHint } from "../lib/latex/status-text.js";

  // `onconnect` opens Settings' local-compilation section, which carries the
  // fuller connection flow (address, capabilities, doctor output) than a
  // preview popover has room for. `onretrybrowser` lets the reader kick off the
  // compile `latex.tryBrowser()` makes eligible again -- resetting the
  // session-native route is not itself a compile.
  let { onconnect, onretrybrowser } = $props();

  let status = $state(latex.status());
  $effect(() => latex.subscribe((next) => (status = next)));

  const chip = $derived(backendChip(status));
  const actions = $derived(actionsFor(status));
  // A failure keeps the last preview on screen (the reader does that), so
  // the hint below is informational, not an error banner; a successful
  // fallback gets the same quiet treatment, one line explaining why.
  const hint = $derived(status.phase === "failed" ? failureHint(status.lastResult?.failure) : "");
  const explanation = $derived(status.phase === "ready" ? fallbackExplanation(status.lastResult?.attempts) : "");
  const busy = $derived(
    ["loading", "compiling", "browser-biber", "checking-local", "local-biber", "native"].includes(
      status.phase,
    ),
  );
  const tone = $derived(
    status.phase === "failed" ? "text-error-600-400"
      : status.phase === "local-needed" ? "text-warning-600-400"
      : "",
  );
  const share = $derived(
    status.progress?.total ? Math.round((status.progress.done / status.progress.total) * 100) : 0,
  );

  // `latex.local.openApp()` answers with the CLI instructions to show once
  // the application-protocol attempt has been made; there is nothing further
  // to await, so this is not async.
  let instructions = $state("");
  function openApp() {
    instructions = latex.local.openApp() || "";
  }

  function tryBrowser() {
    latex.tryBrowser();
    onretrybrowser?.();
  }
</script>

{#if status.phase !== "idle"}
  <span class="latex-status">
    <!-- The compact Preview control opens this readable message, hint and
         action list. -->
    <span class="latex-message {tone}">
      {#if busy}<span class="spinner" aria-hidden="true"></span>{/if}
      {status.message}{#if chip}<span class="latex-backend"> · {chip}</span>{/if}
    </span>
    {#if status.progress}
      <span
        class="latex-progress"
        title={status.progress.total
          ? `${status.progress.scope}: ${status.progress.done}/${status.progress.total}`
          : status.progress.scope}
      >
        <span class="bar" style:width="{share}%"></span>
      </span>
    {/if}
    {#if hint}<span class="text-surface-600-400">{hint}</span>{/if}
    {#if explanation}<span class="text-surface-600-400">{explanation}</span>{/if}
    {#if actions.length}
      <span class="latex-actions">
        {#if actions.includes("connect")}
          <button type="button" class="btn btn-sm preset-outlined-surface-300-700" onclick={() => onconnect?.()}>
            Connect local LibrePaper
          </button>
        {/if}
        {#if actions.includes("retry")}
          <button type="button" class="btn btn-sm preset-outlined-surface-300-700" onclick={() => latex.local.retry()}>
            Retry connection
          </button>
        {/if}
        {#if actions.includes("open-app")}
          <button type="button" class="btn btn-sm preset-outlined-surface-300-700" onclick={openApp}>
            Open LibrePaper
          </button>
        {/if}
        {#if actions.includes("try-browser")}
          <button type="button" class="btn btn-sm preset-outlined-surface-300-700" onclick={tryBrowser}>
            Try browser compilation
          </button>
        {/if}
        {#if actions.includes("doctor")}
          <span class="text-surface-600-400">Run <code>librepaper local doctor</code> for setup help.</span>
        {/if}
        {#if actions.includes("diagnostics")}
          <span class="text-surface-600-400">See Diagnostics for what the local build said.</span>
        {/if}
      </span>
    {/if}
    {#if instructions}<span class="text-surface-600-400">{instructions}</span>{/if}
  </span>
{/if}

<style>
  .latex-status {
    display: inline-flex;
    align-items: center;
    flex-wrap: wrap;
    gap: calc(var(--spacing) * 2);
  }

  .latex-message {
    display: inline-flex;
    align-items: center;
    gap: var(--spacing);
  }

  .latex-actions {
    display: inline-flex;
    align-items: center;
    flex-wrap: wrap;
    gap: calc(var(--spacing) * 1);
  }

  /* The one place progress is drawn with a real, measured total -- see
     `latex/resources.js`'s `prefetch`. No total is ever invented here. */
  .latex-progress {
    display: inline-block;
    width: calc(var(--spacing) * 16);
    height: calc(var(--spacing) * 1.5);
    border-radius: var(--radius-container);
    background: var(--color-surface-300);
    overflow: hidden;
    vertical-align: middle;
  }

  .latex-progress .bar {
    height: 100%;
    background: var(--color-primary-500);
    transition: width 120ms linear;
  }
</style>
