// The stylesheet that gives an editor block the preview's typography. The page
// CSS is written for a whole page, so it is scoped to the block's container,
// and the page's layout (width, margin, paper colour) is undone so the block
// sits in the editor column.

/**
 * The stylesheet for blocks drawn from a page whose CSS is `pageCss`.
 * @param {string} pageCss the text of the page's first `<style>`
 * @returns {string}
 */
export function paperStylesheet(pageCss) {
  // `:root` is the page's own paper; the editor sets its own colours, so the
  // rule goes. `body` becomes the scope root.
  const scoped = pageCss
    .replace(/:root\s*\{[^}]*\}/g, "")
    .replace(/(^|[\s,}])body\s*\{/g, "$1:scope {");
  return [
    `@scope (.cm-lp-paper) {\n${scoped}\n}`,
    // Written after the scoped rules: with equal specificity, the later rule wins.
    `.cm-lp-paper {
  max-width: none;
  margin: 0;
  padding: 0.35em 0;
  background: transparent;
  color: inherit;
}
.cm-lp-paper > :first-child { margin-top: 0; }
.cm-lp-paper > :last-child { margin-bottom: 0; }`,
    // Dark paper mirrors the reader's dark rules: code blocks stay light so
    // their colours keep contrast, and inline code takes the editor's ink.
    `.cm-lp-paper-dark .cm-lp-paper pre { background-color: #f2f4f7; color: #1a1d21; }
.cm-lp-paper-dark .cm-lp-paper :not(pre) > code { background-color: rgb(255 255 255 / 0.1); color: inherit; }
.cm-lp-paper-dark .cm-lp-paper img { background-color: #fff; }
.cm-lp-paper-dark .cm-lp-paper a { color: #7aa2f7; }
.cm-lp-paper-dark .cm-lp-paper :is(table, th, td) { border-color: rgb(255 255 255 / 0.15); }
.cm-lp-paper-dark .cm-lp-paper th { background-color: rgb(255 255 255 / 0.05); }`,
  ].join("\n");
}
