<script>
  // The detailed compile status shown from the Preview header. LaTeX is built
  // in the browser and nowhere else, so this reports one backend and
  // identifies browser bibliography work when it runs.
  //
  // Everything here comes from `latex.subscribe`; nothing is stateful on its
  // own. The wording is a pure function in `latex/status-text.js`, checked
  // without a browser in tests/unit/latex-reader.mjs -- this file only draws
  // what it returns.
  import { Progress } from "@skeletonlabs/skeleton-svelte";
  import * as latex from "../lib/latex.js";
  import { backendChip } from "../lib/latex/status-text.js";

  let status = $state(latex.status());
  $effect(() => latex.subscribe((next) => (status = next)));

  const chip = $derived(backendChip(status));
  // A failure keeps the last preview on screen (the reader does that), so the
  // message below is informational, not an error banner.
  const busy = $derived(["loading", "compiling", "browser-biber"].includes(status.phase));
  const tone = $derived(status.phase === "failed" ? "text-error-600-400" : "");
  // What the bar is for, said once: the tooltip and the accessible name are
  // the same sentence.
  const label = $derived(
    status.progress
      ? status.progress.total
        ? `${status.progress.scope}: ${status.progress.done}/${status.progress.total}`
        : status.progress.scope
      : "",
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
      <!-- A measured total is a determinate bar; a scope with no total is the
           indeterminate one, which is what `null` means to Skeleton. Before
           this the bar was a styled span with no role at all, so a screen
           reader was told nothing was happening. -->
      <Progress
        class="latex-progress-root"
        value={status.progress.total ? status.progress.done : null}
        max={status.progress.total || 100}
        translations={{ value: () => label }}
      >
        <Progress.Track class="latex-progress" title={label}>
          <Progress.Range class="bar" />
        </Progress.Track>
      </Progress>
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
     `latex/resources.js`'s `prefetch`. No total is ever invented here: work
     that cannot count itself gets the indeterminate sweep below instead of a
     bar that would claim to be somewhere. */
  .latex-status :global(.latex-progress-root) { display: inline-flex; }

  .latex-status :global(.latex-progress) {
    display: inline-block;
    width: calc(var(--spacing) * 16);
    height: calc(var(--spacing) * 1.5);
    border-radius: var(--radius-container);
    background: var(--color-surface-300);
    overflow: hidden;
    vertical-align: middle;
  }

  .latex-status :global(.latex-progress .bar) {
    height: 100%;
    background: var(--color-primary-500);
    transition: width 120ms linear;
  }

  .latex-status :global(.latex-progress .bar[data-state="indeterminate"]) {
    width: 35%;
    animation: latex-progress-sweep 1.1s ease-in-out infinite;
  }

  @keyframes latex-progress-sweep {
    from { transform: translateX(-100%); }
    to { transform: translateX(285%); }
  }

  @media (prefers-reduced-motion: reduce) {
    .latex-status :global(.latex-progress .bar[data-state="indeterminate"]) { animation: none; width: 100%; opacity: .5; }
  }
</style>
