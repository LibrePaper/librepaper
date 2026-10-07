<script>
  // One row of the settings dialog, so every setting is the same shape: what
  // it is called and one line on what it does at the left, the control at the
  // right. A control too wide for that -- a textarea, a table, a form --
  // takes the whole width below the words instead.
  //
  // A `note` is one more line under both, across the whole row, for words the
  // reader has to see (a failure) and that would not fit beside the control.
  //
  // The `id` is what the search in the navigation scrolls to.
  let { id = undefined, title, description = "", stacked = false, children, note = undefined } = $props();
</script>

<div class="setting-row" class:stacked {id}>
  <div class="setting-words">
    <div class="setting-title-line"><span class="setting-title">{title}</span></div>
    {#if description}<div class="setting-description">{description}</div>{/if}
  </div>
  <div class="setting-control">{@render children?.()}</div>
  {#if note}<div class="setting-note">{@render note()}</div>{/if}
</div>
