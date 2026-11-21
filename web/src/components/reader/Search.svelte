<script>
  import { tick } from "svelte";
  import PanelTabs from "../PanelTabs.svelte";
  import IconButton from "../IconButton.svelte";
  import { m } from "../../lib/paraglide/messages.js";
  import { locale } from "../../lib/locale.js";
  import Icon from "../Icon.svelte";
  import { searchProject, replacementsFor } from "../../lib/project-search.js";
  import { lineRows, splitPath } from "../../lib/search-rows.js";

  // Search across every text file in the project. The engine does the matching
  // and the edit planning; this component holds the query, debounces it, shows
  // the results grouped by file, and hands edits back to Reader to apply.
  let {
    files = [], // [{ id, path }], text files only
    textsFor = () => ({}), // () => { [id]: current text }, read once per search
    revision = 0, // changes whenever any file's text may have changed
    // False while the panel is hidden: no project-wide search runs until it is shown again.
    active = true,
    canReplace = false, // false for people who may only read
    // { text, n }: each new n asks the panel to focus its input and select it.
    // A non-empty text also becomes the query. An empty text leaves the query as it is.
    seed = null,
    // ({ id, path, from, to, focus }) => void. focus is false while stepping with Enter,
    // so the editor does not take the keyboard from the search box.
    onopen,
    // (id, plan) => void | Promise<void>. plan(text) returns the edits for that file's
    // text as it is when the write happens, so the caller plans after any wait.
    onreplace,
  } = $props();

  let input = $state(null);
  let query = $state("");
  let caseSensitive = $state(false);
  let wholeWord = $state(false);
  let regex = $state(false);
  let replacing = $state(false);
  let replacement = $state("");
  // True while replacements are being written; the replace buttons are off meanwhile.
  let busy = $state(false);
  // Replaced whole on each run, never edited in place, so it is not deeply reactive.
  let results = $state.raw(null);
  // Paths of the file groups the person has collapsed. Kept across recomputes.
  let collapsed = $state.raw(new Set());

  const fmt = (n) => new Intl.NumberFormat(locale()).format(n);
  const current = () => ({ query, caseSensitive, wholeWord, regex });
  // True only when the shown results came from the current query, options and file text.
  // During the debounce they are stale, and replacing from them would apply the old query
  // or point at offsets that a collaborator's edit has moved.
  const fresh = $derived(
    !!results &&
      results.options.query === query &&
      results.options.caseSensitive === caseSensitive &&
      results.options.wholeWord === wholeWord &&
      results.options.regex === regex &&
      results.revision === revision,
  );

  // Runs the engine over the files' current text. `opts` is kept on the result
  // so that a replace applies the options the shown matches came from, and
  // `revision` is stamped so that `fresh` can tell when the text has changed since.
  function run(opts, snapshot) {
    const texts = textsFor();
    const inputs = snapshot.map((file) => ({ id: file.id, path: file.path, text: texts[file.id] ?? "" }));
    return { ...searchProject(inputs, opts, { limit: 1000 }), options: opts, revision };
  }

  function recompute() {
    if (!results) return;
    show(run(current(), files));
  }

  // Debounced: a query, option, file list, revision or visibility change recomputes
  // 150 ms after the last one. While hidden it schedules nothing; becoming visible
  // reruns this effect, which then searches the current text.
  $effect(() => {
    const opts = { query, caseSensitive, wholeWord, regex };
    const snapshot = files;
    void revision;
    if (!active) return;
    if (!opts.query) {
      show(null);
      return;
    }
    const timer = setTimeout(() => {
      show(run(opts, snapshot));
    }, 150);
    return () => clearTimeout(timer);
  });

  // A new seed from the editor's selection or the Find command: take its text as
  // the query when there is some, then focus the input with its contents selected.
  // This also runs once on mount, so a seed set before the panel opened still focuses it.
  $effect(() => {
    const n = seed?.n;
    const text = seed?.text;
    if (n == null) return;
    if (text) query = text;
    tick().then(() => {
      input?.focus();
      input?.select();
    });
  });

  const groups = $derived(results?.groups ?? []);
  const invalid = $derived(results?.error === "invalid");
  const hasResults = $derived(!!results && !invalid && results.total > 0);

  // One display row per line, computed once per result set.
  const groupRows = $derived(groups.map((group) => ({ group, rows: lineRows(group.matches) })));
  const stale = $derived(!!query && !!results && !invalid && !fresh);
  const allCollapsed = $derived(groups.length > 0 && groups.every((group) => collapsed.has(group.path)));
  // Every match in result order, for stepping through them with Enter.
  const flat = $derived(groups.flatMap((group) => group.matches.map((match) => ({ group, match }))));

  const PAGE = 300;
  // How many rows are drawn; more are added as the end of the list scrolls into view.
  let limit = $state(PAGE);
  // The last match stepped to or clicked, as { id, path, from }. Null means nothing is selected.
  let at = null;
  let selected = $state(null);
  let resultsEl = $state(null);
  let sentinel = $state(null);

  // The groups to draw, with expanded groups cut to the row budget.
  const visible = $derived.by(() => {
    let left = limit;
    const out = [];
    let more = false;
    for (const entry of groupRows) {
      if (left <= 0) {
        more = true;
        break;
      }
      const expanded = !collapsed.has(entry.group.path);
      const rows = expanded ? entry.rows.slice(0, left) : [];
      if (expanded && rows.length < entry.rows.length) more = true;
      left -= rows.length;
      out.push({ ...entry, expanded, rows });
    }
    return { entries: out, more };
  });

  const rowKey = (group, row) => `${group.id}:${row.matches[0].from}`;

  // A new query starts from a clean list: everything expanded, first page, no selection.
  $effect(() => {
    void query;
    collapsed = new Set();
    limit = PAGE;
  });
  // A different query or option invalidates the position Enter was stepping
  // through. A recompute after an edit or a replace keeps it, so a click on a
  // stale row does not lose its highlight to the search that click runs.
  // The reset happens where results are assigned, not in an effect: Enter can
  // search and step in one go, and an effect would run after the step and undo it.
  let searchKey = "";
  function show(next) {
    const key = next ? JSON.stringify(next.options) : "";
    if (key !== searchKey) {
      searchKey = key;
      at = null;
      selected = null;
    }
    results = next;
  }

  $effect(() => {
    if (!sentinel || !resultsEl || typeof IntersectionObserver === "undefined") return;
    const observer = new IntersectionObserver((entries) => {
      if (entries.some((entry) => entry.isIntersecting)) limit += PAGE;
    }, { root: resultsEl });
    observer.observe(sentinel);
    return () => observer.disconnect();
  });

  function clearQuery() {
    query = "";
    input?.focus();
  }

  function collapseToggle() {
    collapsed = allCollapsed ? new Set() : new Set(groups.map((group) => group.path));
  }

  // Index in `flat` of the match one step from `at`, or -1 when there are none.
  // Matches are ordered by group, then `from`. A group that no longer has results
  // is placed by path, as the engine orders files. From no position, forward goes to
  // the first match and backward to the last. Wraps at both ends.
  function nextIndex(list, order, pos, direction) {
    if (!list.length) return -1;
    if (!pos) return direction > 0 ? 0 : list.length - 1;
    const posGroup = order.findIndex((group) => group.id === pos.id);
    // Negative, zero or positive: where an entry sorts relative to the position.
    const compare = (entry) => {
      if (entry.group.id === pos.id) return entry.match.from - pos.from;
      if (posGroup >= 0) return order.indexOf(entry.group) - posGroup;
      return entry.group.path.localeCompare(pos.path, undefined, { numeric: true });
    };
    if (direction > 0) {
      const found = list.findIndex((entry) => compare(entry) > 0);
      return found < 0 ? 0 : found;
    }
    for (let i = list.length - 1; i >= 0; i--) if (compare(list[i]) < 0) return i;
    return list.length - 1;
  }

  // Enter and Shift+Enter step through every match, wrapping at the ends.
  async function step(direction) {
    const index = nextIndex(flat, results?.groups ?? [], at, direction);
    if (index < 0) return;
    const { group, match } = flat[index];
    at = { id: group.id, path: group.path, from: match.from };
    if (collapsed.has(group.path)) {
      const next = new Set(collapsed);
      next.delete(group.path);
      collapsed = next;
    }
    // Make sure the row is among the drawn ones.
    let rowsBefore = 0;
    for (const entry of groupRows) {
      if (entry.group === group) {
        const index = entry.rows.findIndex((row) => row.matches.includes(match));
        selected = rowKey(group, entry.rows[index]);
        rowsBefore += index + 1;
        break;
      }
      rowsBefore += entry.rows.length;
    }
    if (rowsBefore > limit) limit = rowsBefore;
    open(group, match, false);
    await tick();
    resultsEl?.querySelector(`[data-key="${CSS.escape(selected)}"]`)?.scrollIntoView({ block: "nearest" });
  }

  function onInputKeydown(event) {
    if (event.key !== "Enter" || event.isComposing || !query) return;
    event.preventDefault();
    // During the debounce, or before the first search, the shown matches are not
    // this query's. Search now so Enter steps through the right ones.
    if (!fresh || !results) {
      show(run(current(), files));
      if (invalid || !flat.length) return;
    }
    step(event.shiftKey ? -1 : 1);
  }

  function onResultsKeydown(event) {
    if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) return;
    const items = [...resultsEl.querySelectorAll("[data-nav]")];
    const focused = items.indexOf(document.activeElement);
    if (focused < 0) return;
    event.preventDefault();
    const to = event.key === "Home" ? 0 : event.key === "End" ? items.length - 1 : focused + (event.key === "ArrowDown" ? 1 : -1);
    items[Math.max(0, Math.min(items.length - 1, to))]?.focus();
  }

  function openRow(group, row) {
    selected = rowKey(group, row);
    at = { id: group.id, path: group.path, from: row.matches[0].from };
    open(group, row.matches[0], true);
  }

  function toggleGroup(path) {
    const next = new Set(collapsed);
    if (next.has(path)) next.delete(path);
    else next.add(path);
    collapsed = next;
  }

  function open(group, match, focus) {
    if (fresh) {
      onopen?.({ id: group.id, path: group.path, from: match.from, to: match.to, focus });
      return;
    }
    // The shown matches are stale (the debounce after an edit), so their offsets
    // may point at unrelated text. Search again now and go to the match in this
    // file that is nearest the one clicked. A file with no match now is left alone.
    recompute();
    const matches = results?.groups?.find((entry) => entry.id === group.id)?.matches ?? [];
    if (!matches.length) return;
    let best = matches[0];
    for (const candidate of matches) {
      if (Math.abs(candidate.from - match.from) < Math.abs(best.from - match.from)) best = candidate;
    }
    onopen?.({ id: group.id, path: group.path, from: best.from, to: best.to, focus });
  }

  async function replaceLine(group, row) {
    if (!fresh || busy) return;
    const opts = results.options;
    const replacementText = replacement;
    busy = true;
    try {
      await onreplace?.(group.id, (text) => replacementsFor(text, group.matches.filter((match) => match.line === row.line), replacementText, opts));
    } finally {
      busy = false;
      recompute();
    }
  }

  // Replaces every match in the project, not only the 1,000 the list shows, so
  // it searches again uncapped at click time. Each file's edits are planned by
  // the caller against that file's text at write time.
  async function replaceAll() {
    if (!fresh || busy) return;
    const opts = results.options;
    const replacementText = replacement;
    busy = true;
    try {
      const texts = textsFor();
      const inputs = files.map((file) => ({ id: file.id, path: file.path, text: texts[file.id] ?? "" }));
      const all = searchProject(inputs, opts, { limit: Infinity });
      for (const group of all.groups) {
        await onreplace?.(group.id, (text) => replacementsFor(text, group.matches, replacementText, opts));
      }
    } finally {
      busy = false;
      recompute();
    }
  }
</script>

<section class="panel panel-tabbed" aria-label={m.search_section_label()}>
  <PanelTabs id="search" title={m.common_search()}>
  <div class="search-body" class:has-gutter={canReplace}>
    <div class="search-top">
      {#if canReplace}
        <button type="button" class="search-chevron" aria-expanded={replacing}
                aria-controls="search-replace-row"
                aria-label={replacing ? m.search_hide_replace() : m.search_show_replace()}
                data-tip={replacing ? m.search_hide_replace() : m.search_show_replace()}
                onclick={() => (replacing = !replacing)}>
          <span class="chev" class:open={replacing}><Icon name="chevron-right" /></span>
        </button>
      {/if}
      <div class="search-field" class:invalid>
        <input
          bind:this={input}
          type="text"
          class="search-input"
          aria-label={m.search_input_label()}
          placeholder={m.search_input_label()}
          autocomplete="off"
          spellcheck="false"
          bind:value={query}
          onkeydown={onInputKeydown}
        />
        {#if query}
          <button type="button" class="search-toggle" aria-label={m.search_clear()} data-tip={m.search_clear()}
                  onclick={clearQuery}><Icon name="x" /></button>
        {/if}
        <button type="button" class="search-toggle" aria-pressed={caseSensitive}
                aria-label={m.search_match_case()} data-tip={m.search_match_case()}
                onclick={() => (caseSensitive = !caseSensitive)}><span aria-hidden="true">Aa</span></button>
        <button type="button" class="search-toggle" aria-pressed={wholeWord}
                aria-label={m.search_match_whole_word()} data-tip={m.search_match_whole_word()}
                onclick={() => (wholeWord = !wholeWord)}><span class="underlined" aria-hidden="true">ab</span></button>
        <button type="button" class="search-toggle" aria-pressed={regex}
                aria-label={m.search_use_regex()} data-tip={m.search_use_regex()}
                onclick={() => (regex = !regex)}><span aria-hidden="true">.*</span></button>
      </div>
    </div>
    {#if invalid}
      <p class="search-error lp-tone-error text-sm" role="alert">{m.search_invalid_regex()}</p>
    {/if}

    {#if canReplace && replacing}
      <div class="search-replace" id="search-replace-row">
        <div class="search-field search-replace-field">
          <input
            type="text"
            class="search-input"
            aria-label={m.search_replace_label()}
            placeholder={m.search_replace_label()}
            autocomplete="off"
            spellcheck="false"
            bind:value={replacement}
          />
        </div>
        <button type="button" class="btn btn-sm lp-control-tonal" disabled={!hasResults || !fresh || busy}
                data-tip={!hasResults ? undefined : results.capped ? m.search_replace_all_capped() : m.search_replace_all_scope({ count: results.total, files: groups.length })}
                onclick={replaceAll}>{m.search_replace_all()}</button>
      </div>
    {/if}

    {#if hasResults}
      <div class="search-summary">
        <span class="panel-meta">{stale ? m.search_searching() : m.search_summary({ count: results.total, files: groups.length })}</span>
        <button type="button" class="search-link" onclick={collapseToggle}>
          {allCollapsed ? m.search_expand_all() : m.search_collapse_all()}
        </button>
      </div>
      {#if results.capped}
        <p class="panel-meta search-capped">{m.search_capped({ count: results.total })}</p>
      {/if}
    {:else if query && stale}
      <p class="panel-meta search-summary">{m.search_searching()}</p>
    {:else if query && results && !invalid}
      <p class="panel-muted search-empty">{m.search_no_results()}</p>
    {/if}

    {#if hasResults}
      <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
      <div class="search-results" class:stale bind:this={resultsEl} onkeydown={onResultsKeydown} role="group" aria-label={m.search_section_label()}>
        {#each visible.entries as { group, rows, expanded } (group.id)}
          {@const countLabel = m.search_file_matches({ count: group.matches.length })}
          {@const parts = splitPath(group.path)}
          <div class="search-group">
            <button type="button" class="search-group-head" data-nav aria-expanded={expanded}
                    onclick={() => toggleGroup(group.path)}>
              <span class="chev" class:open={expanded}><Icon name="chevron-right" /></span>
              <span class="search-name">{parts.name}</span>
              {#if parts.dir}<span class="search-dir">{parts.dir}</span>{/if}
              <span class="search-count" aria-label={countLabel} data-tip={countLabel}>{fmt(group.matches.length)}</span>
            </button>
            {#if expanded}
              <ul class="search-matches">
                {#each rows as row (row.matches[0].from)}
                  <li class="search-match-row" class:selected={selected === rowKey(group, row)}>
                    <button type="button" class="search-match" data-nav data-key={rowKey(group, row)}
                            onclick={() => openRow(group, row)}>
                      <span class="search-line">{row.line}</span>
                      <span class="search-snippet">
                        {#if row.before}<span class="search-before"><span>{row.before}</span></span>{/if}
                        <span class="search-segments">{#each row.segments as seg}{#if seg.mark}<mark>{seg.text}</mark>{:else}{seg.text}{/if}{/each}</span>
                      </span>
                    </button>
                    {#if canReplace && replacing}
                      <span class="search-row-replace">
                        <IconButton icon="replace" label={m.search_replace_line()} disabled={!fresh || busy}
                                    onclick={() => replaceLine(group, row)} />
                      </span>
                    {/if}
                  </li>
                {/each}
              </ul>
            {/if}
          </div>
        {/each}
        {#if visible.more}<div class="search-sentinel" bind:this={sentinel}></div>{/if}
      </div>
    {/if}
  </div>
  </PanelTabs>
</section>

<style>
  .search-body {
    --gutter: 0px;
    display: flex; flex: 1 1 auto; min-height: 0; flex-direction: column; gap: calc(var(--spacing) * 2);
  }
  .search-body.has-gutter { --gutter: calc(var(--spacing) * 6 + var(--spacing) * 1); }
  .search-top { container-type: inline-size; display: flex; align-items: center; gap: var(--spacing); min-width: 0; }
  .search-chevron {
    display: inline-flex; flex: none; align-items: center; justify-content: center;
    width: calc(var(--spacing) * 6); height: calc(var(--spacing) * 8); border-radius: var(--radius-base);
    color: var(--color-text-secondary);
  }
  .search-chevron:hover { background: var(--color-divider); color: var(--color-text); }
  .search-chevron:focus-visible, .search-toggle:focus-visible, .search-link:focus-visible,
  .search-group-head:focus-visible, .search-match:focus-visible { outline: 2px solid var(--color-focus); outline-offset: -2px; }
  .chev { display: inline-flex; width: 14px; height: 14px; transition: transform .12s ease; }
  .chev :global(svg) { width: 100%; height: 100%; }
  .chev.open { transform: rotate(90deg); }

  .search-field {
    display: flex; flex: 1 1 auto; min-width: 0; align-items: center; gap: 2px;
    height: calc(var(--spacing) * 9.5); padding: 0 calc(var(--spacing) * 1.5) 0 calc(var(--spacing) * 2.5);
    border: 1px solid var(--color-divider); border-radius: var(--radius-base);
    background: transparent; color: var(--color-text);
  }
  .search-field:focus-within { border-color: var(--color-focus); box-shadow: 0 0 0 1px var(--color-focus); }
  .search-field.invalid { border-color: var(--color-error-border); box-shadow: none; }
  .search-input {
    flex: 1 1 auto; min-width: 0; height: 100%; padding: 0; border: 0; outline: 0; box-shadow: none;
    background: transparent; color: inherit; font: inherit; font-size: var(--text-sm);
  }
  .search-input:focus { outline: 0; box-shadow: none; }
  .search-replace-field { flex: 1 1 8rem; }

  .search-toggle {
    position: relative; display: inline-flex; flex: none; align-items: center; justify-content: center;
    width: 24px; height: 24px; border-radius: 5px; color: var(--color-text-secondary);
    font-size: var(--text-xs); font-weight: 600; line-height: 1;
  }
  .search-toggle::before { content: ""; position: absolute; inset: -4px; }
  .search-toggle :global(svg) { width: 14px; height: 14px; }
  .search-toggle:hover { background: var(--color-divider); color: var(--color-text); }
  .search-toggle[aria-pressed="true"] {
    background: color-mix(in srgb, var(--color-text) 14%, transparent); color: var(--color-text);
  }
  .underlined { text-decoration: underline; text-underline-offset: 2px; }
  @container (max-width: 15rem) { .search-toggle { width: 20px; } }

  .search-error { margin: 0; margin-left: var(--gutter); padding: calc(var(--spacing) * .5) calc(var(--spacing) * 2); border-radius: var(--radius-base); }
  .search-replace { display: flex; flex-wrap: wrap; align-items: center; gap: calc(var(--spacing) * 1.5); margin-left: var(--gutter); }

  .search-summary { display: flex; align-items: baseline; justify-content: space-between; gap: calc(var(--spacing) * 2); margin: calc(var(--spacing) * 1) 0 0; }
  .search-capped { margin: calc(var(--spacing) * -1) 0 0; }
  .search-empty { margin: 0; padding: calc(var(--spacing) * 4) 0; text-align: center; }
  .search-link { color: var(--color-text-secondary); font-size: var(--text-xs); }
  .search-link:hover { color: var(--color-text); text-decoration: underline; }

  .search-results { flex: 1 1 0; min-height: 0; overflow: auto; }
  .search-results.stale { opacity: .6; }
  .search-group + .search-group { margin-top: 2px; }
  .search-group-head {
    display: flex; width: 100%; min-width: 0; align-items: center; gap: calc(var(--spacing) * 1.5);
    height: calc(var(--spacing) * 8); padding: 0 calc(var(--spacing) * 2); border-radius: var(--radius-base);
    text-align: left; font-size: var(--text-sm);
  }
  .search-group-head:hover { background: var(--color-divider); }
  .search-group-head .chev { flex: none; width: 12px; height: 12px; color: var(--color-text-secondary); }
  .search-name { flex: 0 1 auto; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-weight: 500; }
  .search-dir { flex: 1 1 0; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--color-text-secondary); font-size: var(--text-xs); }
  .search-count { flex: none; margin-left: auto; color: var(--color-text-secondary); font-size: var(--text-xs); font-variant-numeric: tabular-nums; }

  .search-matches { margin: 0; padding: 0 0 0 calc(var(--spacing) * 4); list-style: none; }
  .search-match-row { position: relative; display: flex; align-items: center; border-radius: var(--radius-base); }
  .search-match-row:hover { background: var(--color-divider); }
  .search-match-row.selected { background: color-mix(in srgb, var(--color-text) 14%, transparent); }
  .search-match {
    display: flex; flex: 1 1 auto; min-width: 0; align-items: baseline; gap: calc(var(--spacing) * 2);
    min-height: 26px; padding: 2px calc(var(--spacing) * 2); border-radius: var(--radius-base);
    color: var(--color-text-secondary); font-size: var(--text-xs); text-align: left;
  }
  .search-line {
    flex: none; min-width: 3ch; text-align: right; font-variant-numeric: tabular-nums;
    font-family: var(--font-mono, ui-monospace, monospace); color: var(--color-text-secondary); opacity: .8;
  }
  .search-snippet {
    display: flex; flex: 1 1 auto; min-width: 0; white-space: pre;
    font-family: var(--font-mono, ui-monospace, monospace);
  }
  /* The context before a match is end-aligned and clipped at its start, so the
     words next to the match stay in view however narrow the panel gets. */
  .search-before { display: flex; flex: 0 1 auto; justify-content: flex-end; max-width: 40%; min-width: 0; overflow: hidden; }
  .search-before > span { flex: none; }
  .search-segments { flex: 1 1 auto; min-width: 0; overflow: hidden; text-overflow: ellipsis; }
  .search-snippet mark {
    padding: 0 1px; border-radius: 2px; color: var(--color-text); font-weight: 600;
    background: color-mix(in srgb, var(--color-link) 32%, transparent);
  }
  .search-row-replace { flex: none; display: none; }
  .search-match-row:hover .search-row-replace, .search-match-row:focus-within .search-row-replace { display: inline-flex; }
  @media (hover: none) { .search-row-replace { display: inline-flex; } }
  .search-sentinel { height: 1px; }
</style>
