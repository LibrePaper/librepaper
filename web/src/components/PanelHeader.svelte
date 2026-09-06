<script>
  // Every sidebar panel has the same title row, optional count/actions,
  // and a place for its introductory text or transient status.
  let { title, meta = "", actions, children } = $props();
</script>

<header class="panel-header">
  <div class="panel-heading">
    <h2 class="panel-title">{title}</h2>
    {#if actions}<div class="panel-actions">{@render actions()}</div>{/if}
  </div>
  {#if meta}<span class="panel-meta">{meta}</span>{/if}
  {@render children?.()}
</header>
