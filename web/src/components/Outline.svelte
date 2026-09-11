<script>
  // This panel deliberately stays small: heading extraction belongs to
  // `lib/outline.js`, while this component only presents a navigable outline
  // and reports the selected source location to Reader.
  let { headings = [], activeFrom = null, onselect } = $props();

  // The current section is the last heading before the caret.
  const firstLevel = $derived(headings.reduce((level, heading) => Math.min(level, heading.level), headings[0]?.level ?? 1));
  const activeHeading = $derived.by(() => {
    if (activeFrom == null) return null;
    let active = null;
    for (const heading of headings) {
      if (heading.from > activeFrom) break;
      active = heading;
    }
    return active?.from ?? null;
  });

  function choose(heading) {
    onselect?.(heading);
  }
</script>

<nav class="outline" aria-label="Document outline">
  <h2 class="sr-only">Document outline</h2>
  {#if headings.length}
    <div class="outline-list">
      {#each headings as heading (heading.from)}
        <button
          type="button"
          class="outline-heading"
          class:active={activeHeading === heading.from}
          style="--outline-indent: {Math.max(0, heading.level - firstLevel)}"
          aria-current={activeHeading === heading.from ? "location" : undefined}
          title={heading.title}
          onclick={() => choose(heading)}
        >
          <span>{heading.title}</span>
        </button>
      {/each}
    </div>
  {:else}
    <p class="outline-empty">No headings in this file.</p>
  {/if}
</nav>

<style>
  .outline { display: flex; flex: 1 1 auto; min-height: 0; flex-direction: column; overflow: hidden; }
  .outline-list { overflow: auto; padding: calc(var(--spacing) * 2) var(--spacing); }
  .outline-heading {
    display: block;
    width: 100%;
    min-width: 0;
    padding: calc(var(--spacing) * .75) var(--spacing) calc(var(--spacing) * .75) calc(var(--spacing) * (2 + 3 * var(--outline-indent)));
    border-radius: var(--radius-base);
    color: var(--color-surface-700-300);
    font-size: var(--text-sm);
    line-height: 1.35;
    text-align: left;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .outline-heading:hover, .outline-heading:focus-visible { background: var(--color-surface-200-800); }
  .outline-heading:focus-visible { outline: 2px solid var(--color-primary-500); outline-offset: -2px; }
  .outline-heading.active { background: var(--color-primary-100-900); color: var(--color-primary-700-300); font-weight: 600; }
  .outline-empty { margin: auto; padding: calc(var(--spacing) * 4); color: var(--color-surface-600-400); font-size: var(--text-sm); text-align: center; }
</style>
