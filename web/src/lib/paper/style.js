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
  // `body` becomes the scope root, and so does `:root`: the page's custom
  // properties (the house colours) must reach the block. The dark theme is
  // the `lp-dark` class on the editor, an ancestor of the scope root, so a
  // selector that names it is rewritten to `.lp-dark :scope`.
  const scoped = pageCss
    .replace(/(^|[\s,}])body\s*\{/g, "$1:scope {")
    .replace(/(?:html|:root)\.lp-dark(?![\w-])|\.lp-dark\s+(?::root|html)(?![\w-])/g, ".lp-dark :scope")
    .replace(/:root(?![\w-])/g, ":scope")
    .replace(/(?<![\w-])\.lp-dark(?!\s*:scope)(?![\w-])/g, ".lp-dark :scope");
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
  ].join("\n");
}
