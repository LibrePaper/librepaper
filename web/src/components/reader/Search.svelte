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
    textsFor = () => ({}), // () => { [id]: current text }, read once per search
    revision = 0, // changes whenever any file's text may have changed
    // False while the panel is hidden: no project-wide search runs until it is shown again.
    active = true,
    canReplace = false, // false for people who may only read
    // { text, n }: each new n asks the panel to focus its input and select it.
    // A non-empty text also becomes the query. An empty text leaves the query as it is.
    seed = null,
    onopen, // ({ id, path, from, to }) => void
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
    results = run(current(), files);
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
      results = null;
      return;
    }
    const timer = setTimeout(() => {
      results = run(opts, snapshot);
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

  function toggleGroup(path) {
    const next = new Set(collapsed);
    if (next.has(path)) next.delete(path);
    else next.add(path);
    collapsed = next;
  }

  function open(group, match) {
    if (fresh) {
      onopen?.({ id: group.id, path: group.path, from: match.from, to: match.to });
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
    onopen?.({ id: group.id, path: group.path, from: best.from, to: best.to });
  }

  async function replaceOne(group, match) {
    if (!fresh || busy) return;
    const opts = results.options;
    const replacementText = replacement;
    busy = true;
    try {
      await onreplace?.(group.id, (text) => replacementsFor(text, [match], replacementText, opts));
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
        <button type="button" class="btn btn-sm lp-control-tonal" disabled={!hasResults || !fresh || busy}
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
                      <IconButton icon="check" label={m.search_replace_one()} disabled={!fresh || busy} onclick={() => replaceOne(group, match)} />
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
