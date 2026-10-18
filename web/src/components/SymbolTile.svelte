<script>
  import { symbolText } from "../lib/symbols/index.js";
  import { loadKatex } from "../lib/katex.js";

  // One symbol as a button. The palette and the search field both draw their
  // tiles here, so a tile looks the same wherever it is shown.
  /** @typedef {"html" | "quarto" | "markdown" | "latex" | "typst"} SymbolFormat */
  /** @type {{ symbol: any, format?: SymbolFormat, onclick?: (event: MouseEvent) => void }} */
  let { symbol, format = "latex", onclick = undefined } = $props();

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

  // The title is the bare spelling, so hovering a tile shows what will be
  // typed for the document's format. The package is named because the
  // spelling does not work without it.
  const title = $derived.by(() => {
    const bare = symbolText(symbol, format, { inMath: true });
    return symbol.package ? `${bare}, needs ${symbol.package}` : bare;
  });

  /** @param {any} katex @param {any} symbol @returns {string} */
  function renderMath(katex, symbol) {
    // An accent is drawn over a placeholder letter, so the tile shows the
    // accent on an x rather than on the raw "#1" argument.
    const source = symbol.kind === "accent" ? symbol.latex.replaceAll("#1", "x") : symbol.latex;
    try { return katex.renderToString(source, { throwOnError: false, output: "html" }); }
    catch { return ""; }
  }
  const markup = $derived(math && katex ? renderMath(katex, symbol) : "");
</script>

<button type="button" class="symbol-tile" aria-label={symbol.name} {title} data-symbol={symbol.id} {onclick}>
  {#if markup}<span class="symbol-math">{@html markup}</span>{:else}<span class="symbol-glyph">{symbol.glyph}</span>{/if}
</button>

<style>
  .symbol-tile { display: inline-flex; flex: 0 0 auto; align-items: center; justify-content: center; width: 2.25rem; height: 2.25rem; padding: 0; color: var(--color-text); background: transparent; border-radius: var(--radius-base); cursor: pointer; }
  .symbol-tile:hover, .symbol-tile:focus-visible { background: var(--color-subtle); }
  .symbol-glyph { font-size: 1.1rem; line-height: 1; }
  .symbol-math :global(.katex) { font-size: 1.05em; }
</style>
