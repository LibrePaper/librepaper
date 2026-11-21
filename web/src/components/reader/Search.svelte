<script>
  import { tick } from "svelte";
  import PanelTabs from "../PanelTabs.svelte";
  import IconButton from "../IconButton.svelte";
  import { m } from "../../lib/paraglide/messages.js";
  import { locale } from "../../lib/locale.js";
  import { searchProject, replacementsFor } from "../../lib/project-search.js";

  // Search across every text file in the project. The engine does the matching
  // and the edit planning; this component holds the query, debounces it, shows
  // the results grouped by file, and hands edits back to Reader to apply.
  let {
    files = [], // [{ id, path }], text files only
    textOf = () => "", // (id) => current text of that file
    revision = 0, // changes whenever any file's text may have changed
    canReplace = false, // false for people who may only read
    seed = null, // { text, n }: when n changes, search for text and focus the input
    onopen, // ({ id, path, from, to }) => void
    onreplace, // (id, edits) => void, applied synchronously
  } = $props();

  let input = $state(null);
  let query = $state("");
  let caseSensitive = $state(false);
  let wholeWord = $state(false);
  let regex = $state(false);
  let replacing = $state(false);
  let replacement = $state("");
  // Replaced whole on each run, never edited in place, so it is not deeply reactive.
  let results = $state.raw(null);
  // Paths of the file groups the person has collapsed. Kept across recomputes.
  let collapsed = $state.raw(new Set());

  const fmt = (n) => new Intl.NumberFormat(locale()).format(n);
  const current = () => ({ query, caseSensitive, wholeWord, regex });

  // Runs the engine over the files' current text. `opts` is kept on the result
  // so that a replace applies the options the shown matches came from.
  function run(opts, snapshot) {
    const inputs = snapshot.map((file) => ({ id: file.id, path: file.path, text: textOf(file.id) }));
    return { ...searchProject(inputs, opts, { limit: 1000 }), options: opts };
  }

  function recompute() {
    if (!results) return;
    results = run(current(), files);
  }

  // Debounced: a query, option, file list or revision change recomputes 150 ms
  // after the last one.
  $effect(() => {
    const opts = { query, caseSensitive, wholeWord, regex };
    const snapshot = files;
    void revision;
    if (!opts.query) {
      results = null;
      return;
    }
    const timer = setTimeout(() => {
      results = run(opts, snapshot);
    }, 150);
    return () => clearTimeout(timer);
  });

  // A seed from the editor's selection or the Find command: take its text as
  // the query and focus the input with the text selected.
  $effect(() => {
    const n = seed?.n;
    const text = seed?.text;
    if (n == null || !text) return;
    query = text;
    tick().then(() => {
      input?.focus();
      input?.select();
    });
  });

  const groups = $derived(results?.groups ?? []);
  const invalid = $derived(results?.error === "invalid");
  const hasResults = $derived(!!results && !invalid && results.total > 0);

  function toggleGroup(path) {
    const next = new Set(collapsed);
    if (next.has(path)) next.delete(path);
    else next.add(path);
    collapsed = next;
  }

  function open(group, match) {
    onopen?.({ id: group.id, path: group.path, from: match.from, to: match.to });
  }

  function replaceOne(group, match) {
    onreplace?.(group.id, replacementsFor(textOf(group.id), [match], replacement, results.options));
    recompute();
  }

  function replaceAll() {
    if (!results) return;
    for (const group of results.groups) {
      onreplace?.(group.id, replacementsFor(textOf(group.id), group.matches, replacement, results.options));
    }
    recompute();
  }
</script>

<section class="panel panel-tabbed" aria-label={m.search_section_label()}>
  <PanelTabs id="search" title={m.common_search()}>
  <div class="search-body">
    <input
      bind:this={input}
      type="search"
      class="input input-sm"
      aria-label={m.search_input_label()}
      placeholder={m.search_input_label()}
      bind:value={query}
    />
    <div class="search-row">
      <button type="button" class="btn btn-xs {caseSensitive ? 'lp-control-tonal-brand' : 'lp-control-tonal'}"
              aria-pressed={caseSensitive} onclick={() => (caseSensitive = !caseSensitive)}>{m.search_match_case()}</button>
      <button type="button" class="btn btn-xs {wholeWord ? 'lp-control-tonal-brand' : 'lp-control-tonal'}"
              aria-pressed={wholeWord} onclick={() => (wholeWord = !wholeWord)}>{m.search_whole_word()}</button>
      <button type="button" class="btn btn-xs {regex ? 'lp-control-tonal-brand' : 'lp-control-tonal'}"
              aria-pressed={regex} onclick={() => (regex = !regex)}>{m.search_regex()}</button>
      {#if canReplace}
        <button type="button" class="btn btn-xs lp-control-tonal" aria-expanded={replacing}
                aria-controls="search-replace-row" onclick={() => (replacing = !replacing)}>
          {replacing ? m.search_hide_replace() : m.search_show_replace()}
        </button>
      {/if}
    </div>

    {#if canReplace && replacing}
      <div class="search-row" id="search-replace-row">
        <input
          type="text"
          class="input input-sm"
          aria-label={m.search_replace_label()}
          placeholder={m.search_replace_label()}
          bind:value={replacement}
        />
        <button type="button" class="btn btn-sm lp-control-tonal" disabled={!hasResults}
                onclick={replaceAll}>{m.search_replace_all()}</button>
      </div>
    {/if}

    {#if !query}
      <p class="panel-muted">{m.search_hint()}</p>
    {:else if invalid}
      <p class="lp-tone-error text-sm" role="alert">{m.search_invalid_regex()}</p>
    {:else if results && results.total === 0}
      <p class="panel-muted">{m.search_no_results()}</p>
    {:else if results}
      <p class="panel-meta">{m.search_summary({ count: results.total, files: groups.length })}</p>
      {#if results.capped}
        <p class="panel-meta">{m.search_capped({ count: results.total })}</p>
      {/if}
      <div class="search-results">
        {#each groups as group (group.id)}
          {@const expanded = !collapsed.has(group.path)}
          {@const countLabel = m.search_file_matches({ count: group.matches.length })}
          <div class="search-group">
            <button type="button" class="search-group-head" aria-expanded={expanded}
                    onclick={() => toggleGroup(group.path)}>
              <span class="search-path">{group.path}</span>
              <span class="search-count" aria-label={countLabel} data-tip={countLabel}>{fmt(group.matches.length)}</span>
            </button>
            {#if expanded}
              <ul class="search-matches">
                {#each group.matches as match (match.from)}
                  <li class="search-match-row">
                    <button type="button" class="search-match" onclick={() => open(group, match)}>
                      <span class="search-line">{match.line}</span>
                      <span class="search-text">{match.before}<mark>{match.text}</mark>{match.after}</span>
                    </button>
                    {#if canReplace && replacing}
                      <IconButton icon="check" label={m.search_replace_one()} onclick={() => replaceOne(group, match)} />
                    {/if}
                  </li>
                {/each}
              </ul>
            {/if}
          </div>
        {/each}
      </div>
    {/if}
  </div>
  </PanelTabs>
</section>

<style>
  .search-body { display: flex; flex: 1 1 auto; min-height: 0; flex-direction: column; gap: calc(var(--spacing) * 2); }
  .search-row { display: flex; flex-wrap: wrap; align-items: center; gap: calc(var(--spacing) * 1.5); }
  .search-row .input { flex: 1 1 8rem; min-width: 0; }
  .search-results { flex: 1 1 0; min-height: 0; overflow: auto; }
  .search-group + .search-group { margin-top: calc(var(--spacing) * 2); padding-top: calc(var(--spacing) * 2); border-top: 1px solid var(--color-divider); }
  .search-group-head {
    display: flex; width: 100%; min-width: 0; align-items: baseline; gap: calc(var(--spacing) * 2);
    padding: calc(var(--spacing) * .5) 0; font-size: var(--text-sm); font-weight: 600; text-align: left;
  }
  .search-path { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .search-count { flex: none; color: var(--color-text-secondary); font-size: var(--text-xs); font-variant-numeric: tabular-nums; }
  .search-matches { margin: 0; padding: 0 0 0 calc(var(--spacing) * 2); list-style: none; }
  .search-match-row { display: flex; align-items: center; gap: calc(var(--spacing) * 1); }
  .search-match {
    display: flex; flex: 1 1 auto; min-width: 0; align-items: baseline; gap: calc(var(--spacing) * 2);
    padding: calc(var(--spacing) * .5) var(--spacing); border-radius: var(--radius-base);
    color: var(--color-text-secondary); font-size: var(--text-sm); text-align: left;
  }
  .search-match:hover, .search-match:focus-visible { background: var(--color-divider); }
  .search-match:focus-visible { outline: 2px solid var(--color-focus); outline-offset: -2px; }
  .search-line { flex: none; min-width: 2.5em; text-align: right; font-variant-numeric: tabular-nums; color: var(--color-text-secondary); }
  .search-text { flex: 1 1 auto; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: pre; }
  .search-text mark { padding: 0 1px; border-radius: 2px; background: color-mix(in srgb, var(--color-link) 25%, transparent); color: inherit; }
</style>
