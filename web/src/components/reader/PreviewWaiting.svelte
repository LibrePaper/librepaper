<script>
  // The one waiting display of the preview pane, for every format. The layout
  // is fixed: one spinner that is always there and one stage line. Only the
  // words change as the stages go by, and a stage that can count says how far
  // it has got.
  let { stage, progress = null } = $props();
  const counted = $derived(progress && progress.total > 1
    ? `${stage} (${Math.min(progress.done, progress.total)} of ${progress.total})`
    : stage);
</script>

<div class="notyet waiting" role="status" aria-live="polite">
  <span class="spinner" aria-hidden="true"></span>
  <p class="stage lp-text-secondary text-sm">{counted}</p>
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
</style>
