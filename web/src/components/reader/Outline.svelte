<script>
  // This panel deliberately stays small: heading extraction belongs to
  // `lib/outline.js` and section word counts to `lib/word-count.js`, while this
  // component only presents a navigable outline with those counts and reports
  // the selected source location to Reader.
  let { headings = [], activeFrom = null, counts = [], onselect } = $props();

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
      {#each headings as heading, i (heading.from)}
        <button
          type="button"
          class="outline-heading"
          class:active={activeHeading === heading.from}
          style="--outline-indent: {Math.max(0, heading.level - firstLevel)}"
          aria-current={activeHeading === heading.from ? "location" : undefined}
          title={heading.title}
          onclick={() => choose(heading)}
        >
          <span class="outline-title">{heading.title}</span>
          {#if counts[i] != null}<span class="outline-count">{counts[i].toLocaleString()}</span>{/if}
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
    display: flex;
    width: 100%;
    min-width: 0;
    align-items: baseline;
    gap: calc(var(--spacing) * 2);
    padding: calc(var(--spacing) * .75) var(--spacing) calc(var(--spacing) * .75) calc(var(--spacing) * (2 + 3 * var(--outline-indent)));
    border-radius: var(--radius-base);
    color: var(--color-text-secondary);
    font-size: var(--text-sm);
    line-height: 1.35;
    text-align: left;
    overflow: hidden;
  }
  .outline-title { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .outline-count { flex: none; font-variant-numeric: tabular-nums; }
  .outline-heading:hover, .outline-heading:focus-visible { background: var(--color-divider); }
  .outline-heading:focus-visible { outline: 2px solid var(--color-focus); outline-offset: -2px; }
  .outline-heading.active { background: var(--color-row-selected); color: var(--color-link); font-weight: 600; }
  .outline-empty { margin: auto; padding: calc(var(--spacing) * 4); color: var(--color-text-secondary); font-size: var(--text-sm); text-align: center; }
</style>
