<script>
  import Icon from "./Icon.svelte";
  import SymbolTile from "./SymbolTile.svelte";
  import { entrySpellings } from "../lib/symbols/catalog.js";

  // The row under the grid that names the highlighted symbol, lists how it is
  // spelled in the open format, and holds the favorite toggle. The star is the
  // keyboard and touch route to favorites, since a tile itself only inserts.
  /** @typedef {"html" | "quarto" | "markdown" | "latex" | "typst"} SymbolFormat */
  /** @type {{ entry: any, format?: SymbolFormat, favorite?: boolean, ontogglefavorite?: (entry: any) => void, available?: boolean }} */
  let { entry, format = "latex", favorite = false, ontogglefavorite = undefined, available = true } = $props();

  /** @param {string} text */
  const capitalize = (text) => text.charAt(0).toUpperCase() + text.slice(1);
  const spellings = $derived(entry ? entrySpellings(entry, format) : []);
</script>

<div class="details">
  {#if !entry}
    <p class="empty">Hover or focus a symbol to see its name and spelling.</p>
  {:else}
    <SymbolTile symbol={entry} {format} large />
    <div class="text">
      <span class="name">{capitalize(entry.name)}</span>
      <span class="spellings">
        {#each spellings as spelling (spelling.label)}
          <span class="spelling"><span class="label">{spelling.label}</span> <code>{spelling.code}</code></span>
        {/each}
        {#if entry.package}<span class="hint">Needs {entry.package}</span>{/if}
        {#if !available}<span class="hint">Not available in HTML</span>{/if}
      </span>
    </div>
    <button type="button" class="star" class:on={favorite} aria-pressed={favorite} aria-label="Favorite" onclick={() => ontogglefavorite?.(entry)}>
      <Icon name="star" size={16} filled={favorite} />
    </button>
  {/if}
</div>

<style>
  .details { flex: 0 0 auto; display: flex; align-items: center; gap: .75rem; min-height: 3.5rem; max-height: 4.5rem; padding-top: .5rem; border-top: 1px solid var(--color-divider); overflow: hidden; }
  .empty { margin: 0; color: var(--color-text-secondary); font-size: .8rem; }
  .text { flex: 1 1 auto; min-width: 0; display: flex; flex-direction: column; gap: .15rem; }
  .name { font-weight: 600; font-size: .85rem; }
  .spellings { display: flex; flex-wrap: wrap; gap: .1rem .75rem; font-size: .75rem; }
  .label { color: var(--color-text-secondary); }
  .hint { color: var(--color-text-secondary); font-style: italic; }
  code { font-size: .75rem; }
  .star { flex: 0 0 auto; display: inline-flex; align-items: center; justify-content: center; width: 2.25rem; height: 2.25rem; padding: 0; border: 0; border-radius: var(--radius-base); color: var(--color-text-secondary); background: transparent; cursor: pointer; }
  .star:hover { background: var(--color-row-hover); }
  .star:focus-visible { outline: 2px solid var(--color-focus); outline-offset: -2px; }
  .star.on { color: var(--color-brand); }
  @media (max-width: 479px) {
    .details { overflow-x: auto; }
    .text { flex: 0 0 auto; flex-direction: row; align-items: center; gap: .75rem; white-space: nowrap; }
    .spellings { flex-wrap: nowrap; }
  }
</style>
