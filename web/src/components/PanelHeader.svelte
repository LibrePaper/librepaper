<script>
  // Every sidebar panel has the same title row, optional count/actions,
  // and a place for its introductory text or transient status.
  let { title, meta = "", actions, children } = $props();
</script>

<header class="panel-header">
  <div class="panel-heading">
    <h2 class="panel-title">{title}</h2>
    {#if meta}<span class="panel-meta">{meta}</span>{/if}
    {#if actions}<div class="panel-actions">{@render actions()}</div>{/if}
  </div>
  {@render children?.()}
</header>
