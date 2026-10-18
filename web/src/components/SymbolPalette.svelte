<script>
  import Icon from "./Icon.svelte";
  import SymbolDetails from "./SymbolDetails.svelte";
  import SymbolTile from "./SymbolTile.svelte";
  import { loadEmoji, searchSymbols } from "../lib/symbols/index.js";
  import { PICKER_CATEGORIES, PICKER_SYMBOLS, entryAvailable, entryInsertion, entrySpellings } from "../lib/symbols/catalog.js";
  import { readRecent, readFavorites, withRecent, withFavorite, saveRecent, saveFavorites } from "../lib/symbols/memory.js";

  /** @typedef {"html" | "quarto" | "markdown" | "latex" | "typst"} SymbolFormat */

  // The symbols picker. A click inserts the symbol into the open file and the
  // picker stays open for the next one, so the reader owns neither closing nor
  // focus. Oninsert returns normally or throws an Error whose message is shown
  // here. Shift-click copies the bare spelling instead, for pasting into prose
  // or a different file.
  let { getContext, oninsert, oncancel, format = "" } = $props();

  const RESULT_LIMIT = 300;
  const LONG_PRESS_MS = 500;
  const LONG_PRESS_SLOP = 10;
  // The coarse pointer and width test keep the virtual keyboard from opening on
  // its own on a phone or tablet.
  const FOCUS_WIDTH = 480;

  // The glyph shown beside each category that has no icon of its own.
  const GLYPHS = {
    greek: "α", operators: "±", relations: "≤", arrows: "→", logic: "∈",
    accents: "â", delimiters: "⟨⟩", templates: "√", text: "§", emoji: "☺",
  };

  /** @type {string[]} */
  let recent = $state(readRecent());
  /** @type {string[]} */
  let favorites = $state(readFavorites());
  let query = $state("");
  let tab = $state(readRecent().length ? "recent" : "greek");
  /** @type {HTMLElement | null} */
  let root = $state(null);
  /** @type {HTMLInputElement | null} */
  let input = $state(null);
  /** @type {HTMLElement | null} */
  let scroller = $state(null);
  /** @type {{ groups: { id: string, label: string }[], symbols: any[] } | null} */
  let emoji = $state(null);
  /** @type {Promise<void> | null} */
  let emojiLoad = null;
  let emojiGroup = $state("");
  let note = $state("");
  let active = $state(0);
  /** @type {any} */
  let shown = $state(null);

  // The format is a prop, so reading it never captures an insertion target.
  // The context is read only when a tile is picked.
  // With no document open the bare spelling falls back to LaTeX, so the tiles
  // still have titles.
  /** @returns {SymbolFormat} */
  const formatNow = () => /** @type {SymbolFormat} */ (format || "latex");

  const everything = $derived(emoji ? [...PICKER_SYMBOLS, ...emoji.symbols] : PICKER_SYMBOLS);
  const byId = $derived(new Map(everything.map((entry) => [entry.id, entry])));
  const trimmed = $derived(query.trim());
  const searching = $derived(trimmed !== "");
  const results = $derived(searching ? searchSymbols(trimmed, everything).slice(0, RESULT_LIMIT) : []);
  const categoryLabel = $derived(PICKER_CATEGORIES.find((category) => category.id === tab)?.label ?? "");
  const activeGroup = $derived(emojiGroup || emoji?.groups[0]?.id || "");
  const noticeShown = $derived(format === "latex" && tab === "emoji" && !searching);

  /** @param {string} id */
  const inCategory = (id) => PICKER_SYMBOLS.filter((entry) => entry.category === id);

  // The grid is one or two labelled runs of tiles. Each run records where it
  // starts in the flat tile order, which the roving tabindex counts in.
  const sections = $derived.by(() => {
    /** @type {{ label: string, entries: any[] }[]} */
    let runs;
    if (searching) runs = [{ label: "Results", entries: results }];
    else if (tab === "recent") runs = [{ label: categoryLabel, entries: recent.map((id) => byId.get(id)).filter(Boolean) }];
    else if (tab === "favorites") runs = [{ label: categoryLabel, entries: favorites.map((id) => byId.get(id)).filter(Boolean) }];
    else if (tab === "emoji") runs = [{ label: categoryLabel, entries: emoji ? emoji.symbols.filter((entry) => entry.group === activeGroup) : [] }];
    else if (tab === "delimiters") {
      const all = inCategory("delimiters");
      runs = [
        { label: "Pairs", entries: all.filter((entry) => entry.kind === "pair") },
        { label: "Single", entries: all.filter((entry) => entry.kind !== "pair") },
      ];
    } else runs = [{ label: categoryLabel, entries: inCategory(tab) }];
    let offset = 0;
    return runs.filter((run) => run.entries.length).map((run) => {
      const placed = { ...run, offset };
      offset += run.entries.length;
      return placed;
    });
  });
  const total = $derived(sections.reduce((sum, run) => sum + run.entries.length, 0));
  const activeIndex = $derived(Math.min(active, Math.max(total - 1, 0)));

  const emptyText = $derived.by(() => {
    if (searching) return `No symbols match "${trimmed}". Try a name like alpha, a command like \\alpha, or the character itself.`;
    if (tab === "recent") return "Symbols you insert appear here.";
    if (tab === "favorites") return "Star a symbol in the details row to keep it here.";
    return "";
  });

  // A remembered emoji can only be drawn once the emoji set is in, so it is
  // fetched up front when the remembered lists hold one.
  $effect(() => {
    if ([...recent, ...favorites].some((id) => id.startsWith("emoji-"))) loadEmojiOnce();
  });

  // The note clears itself, so a stale warning does not stay under the search
  // box after the user has moved on.
  $effect(() => {
    if (!note) return;
    const timer = setTimeout(() => { note = ""; }, 4000);
    return () => clearTimeout(timer);
  });

  export function focus() {
    input?.focus();
  }

  // What the overlay focuses on opening, and whether an Escape is the
  // search's to clear rather than the overlay's to close on.
  export const focusTarget = () =>
    !matchMedia("(pointer: coarse)").matches && innerWidth >= FOCUS_WIDTH ? input : null;
  export const claimsEscape = () => query !== "";

  /** @param {string} id */
  function chooseTab(id) {
    tab = id;
    active = 0;
    if (id === "emoji") loadEmojiOnce();
  }

  // The emoji set is large and most sessions never open that category, so it
  // is fetched on the first visit or first search only. A failed load is
  // retried on the next visit.
  function loadEmojiOnce() {
    if (emoji || emojiLoad) return;
    emojiLoad = loadEmoji().then(
      (data) => { emoji = data; },
      () => { emojiLoad = null; }
    );
  }

  function onsearchinput() {
    active = 0;
    if (query.trim() !== "") loadEmojiOnce();
  }

  /** @param {any} entry */
  const availableIn = (entry) => entryAvailable(entry, formatNow());

  /**
   * @param {any} entry
   * @param {MouseEvent | KeyboardEvent} [event]
   */
  function pick(entry, event) {
    const context = getContext?.();
    if (!context) {
      note = "Open a file you can edit to insert symbols.";
      return;
    }
    if (!entryAvailable(entry, context.format)) {
      note = "Not available in HTML.";
      oncancel?.(context);
      return;
    }
    if (event?.shiftKey) {
      const code = entrySpellings(entry, context.format)[0]?.code ?? "";
      if (code) navigator.clipboard?.writeText(code).catch(() => {});
      oncancel?.(context);
      return;
    }
    const result = entryInsertion(entry, context);
    if (!result.text) {
      note = (result.notes ?? []).join(" ");
      oncancel?.(context);
      return;
    }
    try {
      oninsert?.(result, context);
    } catch (error) {
      note = error instanceof Error ? error.message : String(error);
      oncancel?.(context);
      return;
    }
    recent = withRecent(recent, entry.id);
    saveRecent(recent);
    shown = entry;
    if (result.notes?.length) note = result.notes.join(" ");
  }

  /** @param {any} entry */
  function toggleFavorite(entry) {
    favorites = withFavorite(favorites, entry.id, !favorites.includes(entry.id));
    saveFavorites(favorites);
  }

  // A touch long-press shows the details instead of inserting. The click that
  // ends the press is swallowed, and the context menu is suppressed so the
  // browser does not offer to save or search the glyph.
  /** @type {ReturnType<typeof setTimeout> | undefined} */
  let pressTimer;
  let pressed = false;
  let touching = false;
  let pressX = 0;
  let pressY = 0;

  function cancelPress() {
    clearTimeout(pressTimer);
    pressTimer = undefined;
  }

  /** @param {PointerEvent} event @param {any} entry */
  function onpointerdown(event, entry) {
    pressed = false;
    touching = event.pointerType === "touch";
    if (!touching) return;
    pressX = event.clientX;
    pressY = event.clientY;
    cancelPress();
    pressTimer = setTimeout(() => {
      pressTimer = undefined;
      pressed = true;
      shown = entry;
    }, LONG_PRESS_MS);
  }

  /** @param {PointerEvent} event */
  function onpointermove(event) {
    if (pressTimer === undefined) return;
    if (Math.hypot(event.clientX - pressX, event.clientY - pressY) > LONG_PRESS_SLOP) cancelPress();
  }

  /** @param {MouseEvent} event @param {any} entry */
  function onclick(event, entry) {
    if (pressed) {
      pressed = false;
      return;
    }
    pick(entry, event);
  }

  /** @param {any} entry */
  function onenter(entry) {
    if (matchMedia("(hover: hover)").matches) shown = entry;
  }

  /** @param {any} entry @param {number} index */
  function onfocusTile(entry, index) {
    active = index;
    shown = entry;
  }

  /** @returns {HTMLElement[]} */
  const tiles = () => /** @type {HTMLElement[]} */ ([...(scroller?.querySelectorAll("[data-symbol]") ?? [])]);

  // Up and down move to the nearest tile in the next row, so the move holds
  // across the break between two labelled runs.
  /** @param {HTMLElement[]} list @param {number} from @param {1 | -1} direction */
  function neighbourRow(list, from, direction) {
    const here = list[from].getBoundingClientRect();
    let rowTop = null;
    for (const tile of list) {
      const top = tile.getBoundingClientRect().top;
      if (direction > 0 ? top > here.top + 1 : top < here.top - 1) {
        if (rowTop === null || (direction > 0 ? top < rowTop : top > rowTop)) rowTop = top;
      }
    }
    if (rowTop === null) return -1;
    let best = -1;
    let distance = Infinity;
    list.forEach((tile, index) => {
      const rect = tile.getBoundingClientRect();
      if (Math.abs(rect.top - rowTop) > 1) return;
      const gap = Math.abs(rect.left - here.left);
      if (gap < distance) { distance = gap; best = index; }
    });
    return best;
  }

  /** @param {KeyboardEvent} event */
  function ongridkeydown(event) {
    const current = /** @type {HTMLElement | null} */ (/** @type {HTMLElement} */ (event.target).closest?.("[data-symbol]"));
    if (!current) return;
    const list = tiles();
    const index = list.indexOf(current);
    let next = -1;
    if (event.key === "ArrowRight") next = Math.min(index + 1, list.length - 1);
    else if (event.key === "ArrowLeft") next = Math.max(index - 1, 0);
    else if (event.key === "ArrowDown") next = neighbourRow(list, index, 1);
    else if (event.key === "ArrowUp") next = neighbourRow(list, index, -1);
    else if (event.key === "Home") next = 0;
    else if (event.key === "End") next = list.length - 1;
    else return;
    event.preventDefault();
    if (next >= 0) list[next]?.focus();
  }

  /** @param {KeyboardEvent} event */
  function onsearchkeydown(event) {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      tiles()[activeIndex]?.focus();
    } else if (event.key === "Enter" && results.length) {
      event.preventDefault();
      pick(results[0], event);
    }
  }

  /** @param {KeyboardEvent} event */
  function onrootkeydown(event) {
    if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "f") {
      event.preventDefault();
      event.stopPropagation();
      input?.focus();
      input?.select();
    } else if (event.key === "Escape" && query !== "") {
      // Escape clears the query only. With nothing typed it is left alone so
      // the surrounding dialog still gets it.
      event.preventDefault();
      event.stopPropagation();
      query = "";
      active = 0;
    }
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="symbol-palette" bind:this={root} onkeydown={onrootkeydown}>
  <input bind:this={input} class="input" type="search" placeholder="Search symbols" aria-label="Search symbols"
    bind:value={query} oninput={onsearchinput} onkeydown={onsearchkeydown} />

  <div class="body">
    <nav class="categories" aria-label="Symbol categories">
      {#each PICKER_CATEGORIES as category (category.id)}
        <button type="button" class="category" class:current={!searching && tab === category.id}
          aria-current={tab === category.id ? "true" : undefined} onclick={() => chooseTab(category.id)}>
          <span class="category-icon" class:text-emoji={category.id === "emoji"} aria-hidden="true">
            {#if category.id === "recent"}<Icon name="history" size={14} />
            {:else if category.id === "favorites"}<Icon name="star" size={14} />
            {:else}{GLYPHS[category.id]}{/if}
          </span>
          <span class="category-label">{category.label}</span>
        </button>
        {#if category.id === "favorites"}<span class="separator" role="separator"></span>{/if}
      {/each}
    </nav>

    <div class="content">
      {#if noticeShown}
        <p class="panel-muted notice">Emoji in LaTeX need LuaLaTeX and the emoji package, which is added on insert.</p>
      {/if}
      {#if note}<p class="panel-muted notice" role="status">{note}</p>{/if}

      {#if tab === "emoji" && !searching}
        {#if !emoji}
          <p class="panel-muted" role="status">Loading emoji...</p>
        {:else}
          <div class="group-strip" role="group" aria-label="Emoji groups">
            {#each emoji.groups as group (group.id)}
              <button type="button" class="btn btn-sm lp-control-tonal-brand" aria-pressed={group.id === activeGroup}
                onclick={() => { emojiGroup = group.id; active = 0; }}>{group.label}</button>
            {/each}
          </div>
        {/if}
      {/if}

      <!-- svelte-ignore a11y_no_static_element_interactions -->
      <div class="grid-scroll" bind:this={scroller} onkeydown={ongridkeydown}>
        {#each sections as run (run.label)}
          <p class="sub-label">{run.label}</p>
          <div class="grid" role="group" aria-label={run.label}>
            {#each run.entries as entry, i (entry.id)}
              <SymbolTile
                symbol={entry}
                format={formatNow()}
                tabindex={run.offset + i === activeIndex ? 0 : -1}
                aria-disabled={availableIn(entry) ? undefined : "true"}
                onclick={(event) => onclick(event, entry)}
                onfocus={() => onfocusTile(entry, run.offset + i)}
                onmouseenter={() => onenter(entry)}
                onpointerdown={(event) => onpointerdown(event, entry)}
                onpointermove={onpointermove}
                onpointerup={cancelPress}
                onpointercancel={cancelPress}
                oncontextmenu={(event) => { if (touching) event.preventDefault(); }}
              />
            {/each}
          </div>
        {/each}
        {#if !total && emptyText}<p class="panel-muted">{emptyText}</p>{/if}
      </div>

      <SymbolDetails
        entry={shown}
        format={formatNow()}
        favorite={shown ? favorites.includes(shown.id) : false}
        ontogglefavorite={toggleFavorite}
        available={shown ? availableIn(shown) : true}
      />
    </div>
  </div>
</div>

<style>
  .symbol-palette { display: flex; flex-direction: column; gap: calc(var(--spacing) * 2); height: 100%; min-height: 0; }
  .symbol-palette > .input { flex: 0 0 auto; }
  .body { flex: 1 1 auto; min-height: 0; display: flex; flex-direction: column; gap: calc(var(--spacing) * 2); }
  .categories { flex: 0 0 auto; display: flex; flex-direction: row; align-items: center; gap: .25rem; overflow-x: auto; scrollbar-width: none; }
  .categories::-webkit-scrollbar { display: none; }
  .category { flex: 0 0 auto; display: flex; align-items: center; gap: .4rem; padding: .3rem .6rem; border: 0; border-radius: var(--radius-base); color: var(--color-text); background: transparent; font-size: .8rem; white-space: nowrap; cursor: pointer; }
  .category:hover { background: var(--color-row-hover); }
  .category.current { background: var(--color-row-selected); }
  .category:focus-visible { outline: 2px solid var(--color-focus); outline-offset: -2px; }
  .category-icon { display: inline-flex; align-items: center; justify-content: center; flex: 0 0 1.25rem; min-width: 1.25rem; color: var(--color-text-secondary); font-size: .85rem; line-height: 1; }
  .category-icon.text-emoji { font-variant-emoji: text; }
  .separator { flex: 0 0 1px; align-self: stretch; width: 1px; margin: .2rem .25rem; background: var(--color-divider); }
  .content { flex: 1 1 auto; min-width: 0; min-height: 0; display: flex; flex-direction: column; gap: calc(var(--spacing) * 2); }
  .notice { flex: 0 0 auto; margin: 0; font-size: .8rem; }
  .group-strip { flex: 0 0 auto; display: flex; flex-wrap: nowrap; gap: .25rem; overflow-x: auto; scrollbar-width: none; }
  .group-strip::-webkit-scrollbar { display: none; }
  .group-strip > :global(.btn) { flex: 0 0 auto; white-space: nowrap; }
  .grid-scroll { flex: 1 1 auto; min-height: 0; overflow-y: auto; overscroll-behavior: contain; }
  .sub-label { margin: .25rem 0 .3rem; color: var(--color-text-secondary); font-size: .7rem; font-weight: 700; text-transform: uppercase; letter-spacing: .05em; }
  .grid { display: grid; grid-template-columns: repeat(auto-fill, 2.625rem); grid-auto-rows: 2.625rem; gap: 4px; }
  /* A template is wider than one symbol, so its tile takes two cells. */
  .grid > :global([data-kind="template"]) { grid-column: span 2; width: auto; }
  @media (pointer: coarse) {
    .grid { grid-template-columns: repeat(auto-fill, 2.75rem); grid-auto-rows: 2.75rem; }
  }
  @media (min-width: 768px) {
    .symbol-palette { min-height: 22rem; }
    .body { flex-direction: row; gap: calc(var(--spacing) * 3); }
    .categories { flex: 0 0 9.5rem; width: 9.5rem; flex-direction: column; align-items: stretch; gap: 1px; overflow-x: visible; overflow-y: auto; }
    .category { white-space: normal; text-align: left; }
    .separator { flex: 0 0 1px; align-self: stretch; width: auto; height: 1px; margin: .3rem .25rem; }
  }
</style>
