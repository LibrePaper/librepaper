// The stylesheet that gives an editor block the preview's typography. The page
// CSS is written for a whole page, so it is scoped to the block's container,
// and the page's layout (width, margin, paper colour) is undone so the block
// sits in the editor column.

// The values of the `margin` shorthand in the first rule whose selector list
// satisfies `matches`, or null. Only plain lengths are taken.
function pageMargin(pageCss, matches) {
  const length = /^-?[\d.]+(?:px|r?em)$/;
  for (const rule of pageCss.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
    const selectors = rule[1].split(",").map((selector) => selector.trim());
    if (!matches(selectors)) continue;
    const declaration = /(?<![\w-])margin:\s*([^;}]+)/.exec(rule[2]);
    if (!declaration) continue;
    const values = declaration[1].trim().split(/\s+/);
    return values.every((value) => length.test(value)) ? values : null;
  }
  return null;
}

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
  // The face may be a custom property (`var(--lp-serif)`), which is defined
  // inside the scope and so not on the editor: it is resolved here.
  const font = /(?:^|[\s,}])body\s*\{[^}]*?font:\s*([\d.]+(?:px|r?em))\/([\d.]+)\s+([^;}]+)/.exec(pageCss);
  const family = font?.[3].trim().replace(/^var\(\s*(--[\w-]+)\s*(?:,[^)]*)?\)$/, (whole, name) => {
    const defined = new RegExp(`${name}\\s*:\\s*([^;}]+)`).exec(pageCss);
    return defined ? defined[1].trim() : whole;
  });
  // The page's margins, read from its shorthand declarations. A shorthand's
  // first value is the top and its third (or the only) value the bottom.
  const headingTop = pageMargin(pageCss, (selectors) => selectors[0] === "h1" && selectors[1] === "h2");
  const paragraphBottom = pageMargin(pageCss, (selectors) => selectors.includes("p"));
  // A heading that opens a block sits after the previous block's paragraph
  // margin; the extra top margin is the page's gap less that paragraph margin.
  const headingGap =
    headingTop && paragraphBottom
      ? `:scope > :is(h2, h3, h4, h5, h6):first-child { margin-top: calc(${headingTop[0]} - ${paragraphBottom[2] ?? paragraphBottom[0]}); }`
      : "";
  const sourceType = [
    font
      ? `.cm-editor.cm-lp-visual.cm-lp-paper-editor {
  --doc-font: ${family};
  --doc-font-size: ${font[1]};
  --doc-line-height: ${font[2]};
}`
      : "",
    ".cm-editor.cm-lp-paper-editor .cm-line { padding-inline: 0; }",
    // A blank source line between two drawn blocks is collapsed here: the
    // widgets' own margins give the gap the preview has.
    ".cm-editor.cm-lp-paper-editor .cm-line.cm-lp-paper-gap { height: 0; padding: 0; font-size: 0; line-height: 0; overflow: hidden; }",
  ].join("\n");
  // The overrides sit inside the same @scope, after the page rules: a scoped
  // rule beats an unscoped one of equal specificity, so outside it they lose.
  // The rest puts back what the page leaves to the browser's defaults and the
  // app's reset takes away, and stops the editor's pre-wrap from turning the
  // line breaks between rendered tags into blank lines.
  // The widget is a flow root so its children's margins stay inside it: CodeMirror
  // measures the widget's height, and a margin that collapsed out would be missed.
  // The last child keeps its bottom margin, which is the gap to the next block.
  return [
    `@scope (.cm-lp-paper) {\n${scoped}
:scope, .lp-dark :scope {
  --lp-paper: var(--color-shell);
  --lp-ink: var(--color-text);
}
:scope {
  max-width: none;
  margin: 0;
  padding: 0;
  display: flow-root;
  background: transparent;
  color: inherit;
  white-space: normal;
}
:scope > :first-child { margin-top: 0; }
${headingGap}
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
