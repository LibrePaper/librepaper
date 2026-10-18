<script>
  import { symbolText } from "../lib/symbols/index.js";
  import { entrySpellings } from "../lib/symbols/catalog.js";
  import { loadKatex } from "../lib/katex.js";

  // One symbol as a button. The palette and the search field both draw their
  // tiles here, so a tile looks the same wherever it is shown. The large form
  // is a plain preview for the details row and is not interactive. Extra props
  // such as tabindex and pointer handlers go to the button.
  /** @typedef {"html" | "quarto" | "markdown" | "latex" | "typst"} SymbolFormat */
  /** @type {{ symbol: any, format?: SymbolFormat, onclick?: (event: MouseEvent) => void, large?: boolean, [key: string]: any }} */
  let { symbol, format = "latex", onclick = undefined, large = false, ...rest } = $props();

  // Math is drawn with KaTeX so a tile looks like the output. Text and emoji
  // are plain glyphs, and so is any math tile until KaTeX has arrived or if
  // it cannot be loaded.
  const math = $derived(symbol.category !== "text" && symbol.category !== "emoji");
  /** @type {any} */
  let katex = $state(null);
  // KaTeX is large, so it is fetched the first time a math tile is drawn.
  // Every tile after that shares the same load.
  $effect(() => {
    if (!math || katex) return;
    let live = true;
    loadKatex().then((module) => { if (live) katex = module; }, () => {});
    return () => { live = false; };
  });

  /** @param {string} text */
  const capitalize = (text) => text.charAt(0).toUpperCase() + text.slice(1);

  // The title is the bare spelling, so hovering a tile shows what will be
  // typed for the document's format. The package is named because the
  // spelling does not work without it.
  const title = $derived.by(() => {
    const bare = symbol.kind === "pair" || symbol.kind === "template"
      ? (entrySpellings(symbol, format)[0]?.code ?? "")
      : symbolText(symbol, format, { inMath: true });
    const head = `${capitalize(symbol.name)}: ${bare}`;
    return symbol.package ? `${head}, needs ${symbol.package}` : head;
  });

  /** @param {any} katex @param {any} symbol @returns {string} */
  function renderMath(katex, symbol) {
    // An accent is drawn over a placeholder letter, so the tile shows the
    // accent on an x rather than on the raw "#1" argument.
    const raw = symbol.preview ?? symbol.latex;
    const source = symbol.kind === "accent" ? raw.replaceAll("#1", "x") : raw;
    try { return katex.renderToString(source, { throwOnError: false, output: "html" }); }
    catch { return ""; }
  }
  const markup = $derived(math && katex ? renderMath(katex, symbol) : "");
  // Templates and pairs are wider than a single symbol, so they are drawn
  // smaller to fit the tile.
  const wide = $derived(symbol.kind === "template" || symbol.kind === "pair");
</script>

{#snippet face()}
  {#if markup}<span class="symbol-math" class:wide>{@html markup}</span>{:else}<span class="symbol-glyph" class:emoji={symbol.category === "emoji"}>{symbol.glyph}</span>{/if}
{/snippet}

{#if large}
  <span class="symbol-tile large" aria-hidden="true">{@render face()}</span>
{:else}
  <button type="button" class="symbol-tile" aria-label={capitalize(symbol.name)} {title} data-symbol={symbol.id} {onclick} {...rest}>
    {@render face()}
  </button>
{/if}

<style>
  .symbol-tile { display: inline-flex; flex: 0 0 auto; align-items: center; justify-content: center; width: 2.625rem; height: 2.625rem; padding: 0; overflow: hidden; border: 0; color: var(--color-text); background: transparent; border-radius: var(--radius-base); cursor: pointer; }
  button.symbol-tile:hover { background: var(--color-row-hover); }
  button.symbol-tile:focus-visible { outline: 2px solid var(--color-focus); outline-offset: -2px; }
  .symbol-tile[aria-disabled="true"] { opacity: 0.45; }
  .symbol-tile.large { width: 3rem; height: 3rem; cursor: default; }
  .symbol-glyph { font-size: 1.1rem; line-height: 1; }
  .symbol-glyph.emoji { font-family: "Apple Color Emoji", "Segoe UI Emoji", "Noto Color Emoji", sans-serif; }
  .large .symbol-glyph { font-size: 1.6rem; }
  .symbol-math { display: inline-flex; max-width: 100%; align-items: center; justify-content: center; overflow: hidden; }
  .symbol-math :global(.katex) { font-size: 1.05em; }
  .large .symbol-math :global(.katex) { font-size: 1.5em; }
  .symbol-math.wide :global(.katex) { font-size: 0.75em; }
  .large .symbol-math.wide :global(.katex) { font-size: 1em; }
  @media (pointer: coarse) {
    .symbol-tile:not(.large) { width: 2.75rem; height: 2.75rem; }
  }
</style>
