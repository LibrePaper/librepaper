<script>
  import PanelTabs from "./PanelTabs.svelte";
  import SymbolSearch from "./SymbolSearch.svelte";
  import SymbolTile from "./SymbolTile.svelte";
  import { CATEGORIES, SYMBOLS, loadEmoji, symbolInsertion, symbolText } from "../lib/symbols/index.js";

  /** @typedef {import("../lib/insert.js").InsertContext} InsertContext */
  /** @typedef {"html" | "quarto" | "markdown" | "latex" | "typst"} SymbolFormat */

  // The symbols overlay. A click inserts the symbol into the open file and
  // then calls onfocus, which the reader answers by closing the dialog and
  // handing the keyboard back to the editor. Shift-click copies the bare
  // spelling instead, for pasting into prose or a different file.
  let { getContext, oninsert, onfocus, format = "" } = $props();

  const RECENT_KEY = "lp:symbols:recent";
  const RECENT_LIMIT = 20;
  // Below this width the category tabs fold into a select, which fits a narrow
  // sidebar better than a row of tabs that would scroll sideways.
  const NARROW_WIDTH = 480;

  /** @returns {string[]} */
  function readRecent() {
    try {
      const value = JSON.parse(localStorage.getItem(RECENT_KEY) || "[]");
      return Array.isArray(value) ? value.filter((id) => typeof id === "string").slice(0, RECENT_LIMIT) : [];
    } catch {
      // Storage can be blocked or hold something that is not JSON. The row
      // simply starts empty then.
      return [];
    }
  }

  /** @type {string[]} */
  let recentIds = $state(readRecent());
  let query = $state("");
  let tab = $state(CATEGORIES[0].id);
  /** @type {HTMLElement | null} */
  let root = $state(null);
  let narrow = $state(false);
  /** @type {{ groups: { id: string, label: string }[], symbols: any[] } | null} */
  let emoji = $state(null);
  /** @type {Promise<void> | null} */
  let emojiLoad = null;
  let emojiGroup = $state("");
  let note = $state("");
  /** @type {any} */
  let search = $state(null);

  // The format is a prop, so reading it never captures an insertion target.
  // The context is read only when a tile is picked.
  // With no document open the bare spelling falls back to LaTeX, so the tiles
  // still have titles.
  /** @returns {SymbolFormat} */
  const formatNow = () => /** @type {SymbolFormat} */ (format || "latex");
  const noticeShown = () => format === "latex" && tab === "emoji";

  const everything = $derived(emoji ? [...SYMBOLS, ...emoji.symbols] : SYMBOLS);
  const byId = $derived(new Map(everything.map((symbol) => [symbol.id, symbol])));
  const recentSymbols = $derived(recentIds.map((id) => byId.get(id)).filter(Boolean));
  const searching = $derived(query.trim() !== "");

  const activeGroup = $derived(emojiGroup || emoji?.groups[0]?.id || "");
  const gridSymbols = $derived(tab === "emoji"
    ? (emoji ? emoji.symbols.filter((symbol) => symbol.group === activeGroup) : [])
    : SYMBOLS.filter((symbol) => symbol.category === tab));

  // The width is measured on the panel itself, so the fold follows the space
  // the panel really has rather than the window.
  $effect(() => {
    if (!root) return;
    const element = root;
    const measure = () => { narrow = element.clientWidth < NARROW_WIDTH; };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  });

  // The note clears itself, so a stale warning does not stay under the search
  // box after the user has moved on.
  $effect(() => {
    if (!note) return;
    const timer = setTimeout(() => { note = ""; }, 4000);
    return () => clearTimeout(timer);
  });

  export function focus() {
    search?.focus();
  }

  // What the overlay focuses on opening, and whether an Escape is the
  // search's to clear rather than the overlay's to close on.
  export const focusTarget = () => root?.querySelector('input[type="search"]') ?? null;
  export const claimsEscape = () => query !== "";

  /** @param {string} id */
  function chooseTab(id) {
    tab = id;
    if (id === "emoji") loadEmojiOnce();
  }

  // The emoji set is large and most sessions never open that tab, so it is
  // fetched on the first visit only. A failed load is retried on the next
  // visit.
  function loadEmojiOnce() {
    if (emoji || emojiLoad) return;
    emojiLoad = loadEmoji().then(
      (data) => { emoji = data; },
      () => { emojiLoad = null; }
    );
  }

  /** @param {string} id */
  function remember(id) {
    recentIds = [id, ...recentIds.filter((other) => other !== id)].slice(0, RECENT_LIMIT);
    try {
      localStorage.setItem(RECENT_KEY, JSON.stringify(recentIds));
    } catch {
      // Without storage the row still works for this visit.
    }
  }

  /**
   * @param {any} symbol
   * @param {MouseEvent | KeyboardEvent} [event]
   */
  function pick(symbol, event) {
    const context = getContext?.();
    if (!context) return;
    if (event?.shiftKey) {
      navigator.clipboard?.writeText(symbolText(symbol, /** @type {SymbolFormat} */ (context.format))).catch(() => {});
      return;
    }
    const result = symbolInsertion(symbol, context);
    oninsert?.(result, context);
    remember(symbol.id);
    if (result.notes?.length) note = result.notes.join(" ");
    onfocus?.();
  }
</script>

<div class="symbol-palette" bind:this={root}>
  <div class="search-wrap" class:searching>
    <SymbolSearch
      bind:this={search}
      bind:query
      symbols={everything}
      format={formatNow()}
      grouped
      onpick={pick}
    />
  </div>

  {#if note}<p class="symbol-note panel-muted" role="status">{note}</p>{/if}

  {#if !searching}
    {#if recentSymbols.length}
      <div class="recent" role="group" aria-label="Recent symbols">
        {#each recentSymbols as symbol (symbol.id)}
          <SymbolTile {symbol} format={formatNow()} onclick={(event) => pick(symbol, event)} />
        {/each}
      </div>
    {/if}

    {#if narrow}
      <select class="select" aria-label="Symbol category" value={tab} onchange={(event) => chooseTab(event.currentTarget.value)}>
        {#each CATEGORIES as category (category.id)}
          <option value={category.id}>{category.label}</option>
        {/each}
      </select>
    {:else}
      <PanelTabs id="symbols" label="Symbol categories" tabs={CATEGORIES} value={tab} onchange={chooseTab} />
    {/if}

    <!-- The pane the selected tab controls. The strip names it
         `symbols-pane-<tab>`, and a selected tab whose pane nothing answers
         to is an invalid aria-controls. The select at narrow widths names
         no pane, so the body carries none there. -->
    <div class="symbol-body" id={narrow ? undefined : `symbols-pane-${tab}`} role={narrow ? undefined : "tabpanel"} aria-labelledby={narrow ? undefined : `symbols-tab-${tab}`}>
      {#if tab === "emoji"}
        {#if noticeShown()}
          <p class="panel-muted format-notice">Emoji in LaTeX need LuaLaTeX and the emoji package, which is added on insert.</p>
        {/if}
        {#if !emoji}
          <p class="panel-muted" role="status">Loading emoji...</p>
        {:else}
          <div class="group-chips" role="group" aria-label="Emoji groups">
            {#each emoji.groups as group (group.id)}
              <button type="button" class="btn btn-sm lp-control-tonal-brand" aria-pressed={group.id === activeGroup}
                onclick={() => { emojiGroup = group.id; }}>{group.label}</button>
            {/each}
          </div>
        {/if}
      {/if}

      <div class="symbol-grid">
        {#each gridSymbols as symbol (symbol.id)}
          <SymbolTile {symbol} format={formatNow()} onclick={(event) => pick(symbol, event)} />
        {/each}
      </div>
    </div>
  {/if}
</div>

<style>
  .symbol-palette { display: flex; flex-direction: column; gap: calc(var(--spacing) * 2); height: 100%; min-height: 0; }
  .search-wrap { flex: 0 0 auto; display: flex; flex-direction: column; min-height: 0; }
  .search-wrap.searching { flex: 1 1 auto; }
  .symbol-note { flex: 0 0 auto; font-size: .8rem; }
  /* The shared tab root grows to fill a sidebar column. Here the grid below it does the growing. */
  .symbol-palette > :global(.panel-tabs-root) { flex: 0 0 auto; }
  .recent { flex: 0 0 auto; display: flex; flex-wrap: wrap; gap: 2px; }
  .symbol-body { flex: 1 1 auto; min-height: 0; display: flex; flex-direction: column; gap: calc(var(--spacing) * 2); }
  .format-notice { flex: 0 0 auto; font-size: .8rem; }
  .group-chips { flex: 0 0 auto; display: flex; flex-wrap: wrap; gap: .25rem; }
  .symbol-grid { flex: 1 1 auto; min-height: 0; display: flex; flex-wrap: wrap; align-content: flex-start; gap: 2px; overflow-y: auto; overscroll-behavior: contain; }
</style>
