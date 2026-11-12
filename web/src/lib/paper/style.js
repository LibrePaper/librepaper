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
  // The source of the block being edited is set in the page's face too, so
  // revealing a block changes its markup and not its type.
  const font = /(?:^|[\s,}])body\s*\{[^}]*?font:\s*([\d.]+px)\/([\d.]+)\s+([^;}]+)/.exec(pageCss);
  const sourceType = font
    ? `.cm-editor.cm-lp-visual.cm-lp-paper-editor {
  --doc-font: ${font[3].trim()};
  --doc-font-size: ${font[1]};
  --doc-line-height: ${font[2]};
}
.cm-editor.cm-lp-paper-editor .cm-line { padding-inline: 0; }`
    : "";
  // The overrides sit inside the same @scope, after the page rules: a scoped
  // rule beats an unscoped one of equal specificity, so outside it they lose.
  // The rest puts back what the page leaves to the browser's defaults and the
  // app's reset takes away, and stops the editor's pre-wrap from turning the
  // line breaks between rendered tags into blank lines.
  return [
    `@scope (.cm-lp-paper) {\n${scoped}
:scope {
  max-width: none;
  margin: 0;
  padding: 0.35em 0;
  background: transparent;
  color: inherit;
  white-space: normal;
}
:scope > :first-child { margin-top: 0; }
:scope > :last-child { margin-bottom: 0; }
:is(ul, ol) { padding-left: 1.75em; }
ul { list-style: disc; }
ul ul { list-style: circle; }
ol { list-style: decimal; }
li > :is(ul, ol) { margin: 0.2em 0 0; }
a { text-decoration: underline; }
pre { white-space: pre; }
}`,
    sourceType,
    // Dark paper mirrors the reader's dark rules: code blocks stay light so
    // their colours keep contrast, and inline code takes the editor's ink.
    `.cm-lp-paper-dark .cm-lp-paper blockquote { color: rgb(255 255 255 / 0.72); border-left-color: rgb(255 255 255 / 0.25); }
.cm-lp-paper-dark .cm-lp-paper hr { border-top-color: rgb(255 255 255 / 0.2); }
.cm-lp-paper-dark .cm-lp-paper pre { background-color: #f2f4f7; color: #1a1d21; }
.cm-lp-paper-dark .cm-lp-paper :not(pre) > code { background-color: rgb(255 255 255 / 0.1); color: inherit; }
.cm-lp-paper-dark .cm-lp-paper img { background-color: #fff; }
.cm-lp-paper-dark .cm-lp-paper a { color: #7aa2f7; }
.cm-lp-paper-dark .cm-lp-paper :is(table, th, td) { border-color: rgb(255 255 255 / 0.15); }
.cm-lp-paper-dark .cm-lp-paper th { background-color: rgb(255 255 255 / 0.05); }`,
  ].join("\n");
}
