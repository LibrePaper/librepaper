<script>
  // The detailed compile status shown from the Preview header. LaTeX is built
  // in the browser and nowhere else, so this reports one backend and
  // identifies browser bibliography work when it runs.
  //
  // Everything here comes from `latex.subscribe`; nothing is stateful on its
  // own. The wording is a pure function in `latex/status-text.js`, checked
  // without a browser in tests/unit/latex-reader.mjs -- this file only draws
  // what it returns.
  import * as latex from "../lib/latex.js";
  import { backendChip } from "../lib/latex/status-text.js";

  let status = $state(latex.status());
  $effect(() => latex.subscribe((next) => (status = next)));

  const chip = $derived(backendChip(status));
  // A failure keeps the last preview on screen (the reader does that), so the
  // message below is informational, not an error banner.
  const busy = $derived(["loading", "compiling", "browser-biber"].includes(status.phase));
  const tone = $derived(status.phase === "failed" ? "text-error-600-400" : "");
  const share = $derived(
    status.progress?.total ? Math.round((status.progress.done / status.progress.total) * 100) : 0,
  );
</script>

{#if status.phase !== "idle"}
  <span class="latex-status">
    <!-- The compact Preview control opens this readable message. -->
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
