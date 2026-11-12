<script>
  import { m } from "../../lib/paraglide/messages.js";
  import { locale } from "../../lib/locale.js";
  import PanelTabs from "../PanelTabs.svelte";
  import {
    OUTLINE_FILTER_ALL,
    OUTLINE_FILTER_OPEN,
    filterOutline,
    hasOutlineFilter,
    outlineTags,
    tagFilter,
  } from "../../lib/outline-filter.js";
  // This panel deliberately stays small: heading extraction belongs to
  // `lib/outline.js` and section word counts to `lib/word-count.js`, while this
  // component only presents a navigable outline with those counts and reports
  // the selected source location to Reader.
  let { headings = [], activeFrom = null, counts = [], onselect } = $props();

  // The filter lives only as long as the panel: it is not remembered.
  let filter = $state(OUTLINE_FILTER_ALL);

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

  // The filter control is offered only when the headings have TODO states or
  // tags. A tag the document no longer has falls back to showing everything.
  const filterable = $derived(hasOutlineFilter(headings));
  const tags = $derived(outlineTags(headings));
  const activeFilter = $derived(
    filter === OUTLINE_FILTER_ALL || filter === OUTLINE_FILTER_OPEN || tags.some((tag) => tagFilter(tag) === filter)
      ? filter
      : OUTLINE_FILTER_ALL,
  );
  const rows = $derived(filterOutline(headings, filterable ? activeFilter : OUTLINE_FILTER_ALL));

  function choose(heading) {
    onselect?.(heading);
  }
</script>

<nav class="outline panel-tabbed" aria-label={m.outline_document()}>
  <PanelTabs id="outline" title={m.common_outline()}>
  {#if headings.length}
    {#if filterable}
      <div class="outline-filter">
        <select class="select" aria-label={m.outline_filter_label()} bind:value={filter}>
          <option value={OUTLINE_FILTER_ALL}>{m.outline_filter_all()}</option>
          <option value={OUTLINE_FILTER_OPEN}>{m.outline_filter_open_todos()}</option>
          {#each tags as tag (tag)}
            <option value={tagFilter(tag)}>{m.outline_filter_tag({ tag })}</option>
          {/each}
        </select>
      </div>
    {/if}
    {#if rows.length}
      <div class="outline-list">
        {#each rows as row (row.heading.from)}
          {@const heading = row.heading}
          <button
            type="button"
            class="outline-heading"
            class:outline-heading-context={row.context}
            style="--outline-indent: {Math.max(0, heading.level - firstLevel)}"
            aria-current={activeHeading === heading.from ? "location" : undefined}
            data-tip-wait="long" data-tip={heading.title}
            onclick={() => choose(heading)}
          >
            {#if heading.todo}<span class="outline-todo" class:outline-todo-done={heading.done}>{heading.todo}</span>{/if}
            {#if heading.priority}<span class="outline-priority">{`[#${heading.priority}]`}</span>{/if}
            <span class="outline-title">{heading.title}</span>
            {#if heading.tags?.length}
              <span class="outline-tags">
                {#each heading.tags as tag (tag)}<span class="outline-tag">{tag}</span>{/each}
              </span>
            {/if}
            {#if counts[row.index] != null}<span class="outline-count">{counts[row.index].toLocaleString(locale())}</span>{/if}
          </button>
        {/each}
      </div>
    {:else}
      <p class="outline-empty">{m.outline_filter_no_match()}</p>
    {/if}
  {:else}
    <p class="outline-empty">{m.outline_no_headings()}</p>
  {/if}
  </PanelTabs>
</nav>

<style>
  .outline { display: flex; flex: 1 1 auto; min-height: 0; flex-direction: column; overflow: hidden; }
  .outline-filter { padding-bottom: calc(var(--spacing) * 2); }
  .outline-filter .select { width: 100%; font-size: var(--text-sm); }
  /* The pane supplies the padding; the list only scrolls. */
  .outline-list { flex: 1 1 0; min-height: 0; overflow: auto; padding: 0; }
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
  /* A heading shown only for the headings below it that match a filter. */
  .outline-heading-context { opacity: .55; }
  .outline-title { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .outline-count { flex: none; font-variant-numeric: tabular-nums; }
  /* Org TODO states use the colours of the source editor's TODO markup. */
  .outline-todo {
    flex: none;
    padding: 0 calc(var(--spacing) * 2);
    border-radius: var(--radius-base);
    background: color-mix(in srgb, var(--color-link) 15%, transparent);
    color: var(--color-link);
    font-size: var(--text-xs);
    font-weight: 600;
  }
  .outline-todo-done {
    background: var(--color-subtle);
    color: var(--color-text-secondary);
    text-decoration: line-through;
  }
  .outline-priority { flex: none; color: var(--color-text-secondary); font-size: var(--text-xs); font-variant-numeric: tabular-nums; }
  .outline-tags { display: flex; min-width: 0; flex: 0 1 auto; flex-wrap: wrap; gap: calc(var(--spacing) * .5); justify-content: flex-end; }
  .outline-tag {
    padding: 0 calc(var(--spacing) * 1.5);
    border: 1px solid var(--color-divider);
    border-radius: var(--radius-base);
    color: var(--color-text-secondary);
    font-size: var(--text-xs);
    white-space: nowrap;
  }
  .outline-heading:hover, .outline-heading:focus-visible { background: var(--color-divider); }
  .outline-heading:focus-visible { outline: 2px solid var(--color-focus); outline-offset: -2px; }
  .outline-empty { margin: auto; padding: calc(var(--spacing) * 4); color: var(--color-text-secondary); font-size: var(--text-sm); text-align: center; }
  /* Phones: a heading is a finger's height. Indentation keeps three levels of
     hierarchy and stops there, so a deep heading keeps its words at 320px. */
  @media (max-width: 767px) {
    .outline-heading {
      min-height: 2.75rem;
      align-items: center;
      padding-inline-start: calc(var(--spacing) * (2 + 3 * min(var(--outline-indent), 3)));
    }
  }
</style>
