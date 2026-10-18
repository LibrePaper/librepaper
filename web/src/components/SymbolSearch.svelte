<script>
  import SymbolTile from "./SymbolTile.svelte";
  import { CATEGORIES, searchSymbols } from "../lib/symbols/index.js";

  // A search field over a list of symbols, with the matches as tiles. The
  // palette uses it for the whole sidebar search, and the math forms use it
  // to find one symbol at a time. `grouped` labels each run of matches with
  // its category, which only the palette wants.
  /** @typedef {"html" | "quarto" | "markdown" | "latex" | "typst"} SymbolFormat */
  /** @type {{ symbols?: any[], format?: SymbolFormat, onpick?: (symbol: any, event?: Event) => void, placeholder?: string, autofocus?: boolean, grouped?: boolean, query?: string }} */
  let {
    symbols = [],
    format = "latex",
    onpick = undefined,
    placeholder = "Search symbols",
    autofocus = false,
    grouped = false,
    query = $bindable(""),
  } = $props();

  const labels = new Map(CATEGORIES.map((category) => [category.id, category.label]));

  /** @type {HTMLInputElement | null} */
  let input = $state(null);
  $effect(() => {
    if (autofocus && input) input.focus();
  });

  export function focus() {
    input?.focus();
  }

  const trimmed = $derived(query.trim());
  const results = $derived(trimmed ? searchSymbols(trimmed, symbols) : []);

  // Each category keeps the rank it earned in the search. The groups run in
  // the order their best match appeared, so the strongest match leads.
  const groups = $derived.by(() => {
    /** @type {string[]} */
    const order = [];
    /** @type {Map<string, any[]>} */
    const byCategory = new Map();
    for (const symbol of results) {
      if (!byCategory.has(symbol.category)) {
        byCategory.set(symbol.category, []);
        order.push(symbol.category);
      }
      byCategory.get(symbol.category)?.push(symbol);
    }
    return order.map((id) => ({ id, label: labels.get(id) ?? id, symbols: byCategory.get(id) ?? [] }));
  });

  /** @param {KeyboardEvent} event */
  function onkeydown(event) {
    if (event.key === "Enter" && results.length) {
      event.preventDefault();
      onpick?.(results[0], event);
    } else if (event.key === "Escape" && query) {
      // Escape clears the query only. With nothing typed it is left alone so
      // the surrounding panel still gets it.
      event.preventDefault();
      query = "";
    }
  }
</script>

<div class="symbol-search">
  <input bind:this={input} class="input" type="search" {placeholder} aria-label={placeholder} bind:value={query} {onkeydown} />
  {#if trimmed}
    <div class="results">
      {#if !results.length}
        <p class="panel-muted">No symbols match "{trimmed}".</p>
      {:else if grouped}
        {#each groups as group (group.id)}
          <p class="group-label">{group.label}</p>
          <div class="symbol-grid" role="group" aria-label={group.label}>
            {#each group.symbols as symbol (symbol.id)}
              <SymbolTile {symbol} {format} onclick={(event) => onpick?.(symbol, event)} />
            {/each}
          </div>
        {/each}
      {:else}
        <div class="symbol-grid">
          {#each results as symbol (symbol.id)}
            <SymbolTile {symbol} {format} onclick={(event) => onpick?.(symbol, event)} />
          {/each}
        </div>
      {/if}
    </div>
  {/if}
</div>

<style>
  .symbol-search { flex: 1 1 auto; display: flex; flex-direction: column; gap: calc(var(--spacing) * 2); min-height: 0; }
  .results { flex: 1 1 auto; min-height: 0; overflow-y: auto; }
  .symbol-grid { display: flex; flex-wrap: wrap; gap: 2px; }
  .group-label { margin: .5rem 0 .2rem; color: var(--color-text-secondary); font-size: .7rem; font-weight: 700; text-transform: uppercase; letter-spacing: .05em; opacity: .7; }
</style>
