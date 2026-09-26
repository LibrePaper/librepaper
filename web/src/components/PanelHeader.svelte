<script>
  // The selected rail icon identifies the panel visually. Keep its heading
  // available to assistive technology without reserving a title row.
  let { title, meta = "", actions, children } = $props();
</script>

<h2 class="sr-only">{title}</h2>
{#if meta}<div class="panel-status panel-meta">{meta}</div>{/if}
{#if actions}<div class="panel-actions">{@render actions()}</div>{/if}
{#if children}<div class="panel-intro">{@render children()}</div>{/if}
