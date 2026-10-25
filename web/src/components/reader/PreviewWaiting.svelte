<script>
  // The one waiting display of the preview pane, for every format. The layout
  // is fixed: one spinner that is always there, one stage line, one bar. Only
  // the words and the bar's fill change as the stages go by.
  let { stage, progress = null } = $props();
  const known = $derived(Boolean(progress && progress.total > 0));
  const percent = $derived(known ? Math.min(100, Math.max(0, (progress.done / progress.total) * 100)) : 0);
</script>

<div class="notyet waiting" role="status" aria-live="polite">
  <span class="spinner" aria-hidden="true"></span>
  <p class="stage lp-text-secondary text-sm">{stage}</p>
  <div class="track" role="progressbar" aria-label={stage} aria-valuemin="0" aria-valuemax={known ? progress.total : undefined} aria-valuenow={known ? progress.done : undefined}>
    <span class="fill" class:indeterminate={!known} style:width={known ? `${percent}%` : null}></span>
  </div>
</div>

<style>
  .waiting {
    align-items: center;
    justify-content: center;
    margin: auto;
    width: 18rem;
    max-width: 100%;
    text-align: center;
  }
  .waiting .spinner {
    width: 1.5rem;
    height: 1.5rem;
    margin-right: 0;
    border-width: 3px;
    color: var(--color-brand);
  }
  .stage {
    margin: 0;
    min-height: 1.5em;
  }
  .track {
    width: 100%;
    height: 4px;
    overflow: hidden;
    border-radius: 2px;
    background: var(--color-border);
  }
  .fill {
    display: block;
    height: 100%;
    background: var(--color-brand);
    transition: width 120ms linear;
  }
  .fill.indeterminate {
    width: 35%;
    animation: waiting-sweep 1.1s ease-in-out infinite;
  }
  @keyframes waiting-sweep {
    from { transform: translateX(-100%); }
    to { transform: translateX(285%); }
  }
  @media (prefers-reduced-motion: reduce) {
    .fill.indeterminate {
      animation: none;
      width: 100%;
      opacity: .5;
    }
  }
</style>
