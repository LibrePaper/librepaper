// The formats the paper layer draws. Each adapter says what the layer needs
// from a format: the blocks of a source, the text to render for one block,
// the renderer module that draws it, and when the format can split text.
// An adapter may also render the whole document at once (renderedBlocks), so
// that footnotes and cross-references number as they do in the preview.

import { blockSource, splitBlocks } from "./blocks.js";
import { orgBlockSource, splitOrgBlocks } from "./org-blocks.js";
import { splitTypstBlocks, typstBlockSource, typstPreamble } from "./typst-blocks.js";
import { loadOrgSyntax } from "../org-language/module.js";
import { loadTypstSyntax } from "../typst-language/module.js";
import { markdownFolds, orgFolds, typstFolds } from "../visual/fold-finders.js";

/**
 * @typedef {{
 *   module: "markdown" | "org" | "typst",
 *   splitBlocks: (text: string) => import("./blocks.js").PaperBlock[] | null,
 *   blockSource: (text: string, block: import("./blocks.js").PaperBlock, blocks: import("./blocks.js").PaperBlock[]) => string,
 *   renderedBlocks?: (text: string, renderer: import("./renderer.js").PaperRenderer, path: string) => import("./blocks.js").PaperBlock[] | null,
 *   ready: () => Promise<void>,
 *   prepare?: (text: string, renderer: any, project: any, onUpdate: () => void) => void,
 *   folds: (text: string) => import("../visual/folds.js").Fold[],
 * }} PaperFormat
 */

/**
 * The line ranges of a text, 1-based by index + 1. Each range is the line's
 * content: `from` is its first offset, `to` the offset of its break (or the
 * text's end), so a line with no trailing newline still ends at the text's end.
 * @param {string} text
 * @returns {{ from: number, to: number }[]}
 */
function lineRanges(text) {
  const lines = [];
  let from = 0;
  const breaks = /\r\n|\r|\n/g;
  let match;
  while ((match = breaks.exec(text))) {
    lines.push({ from, to: match.index });
    from = match.index + match[0].length;
  }
  lines.push({ from, to: text.length });
  return lines;
}

/**
 * The blocks the module renders for a whole document, as paper blocks with
 * their HTML set. Null when the module cannot render the document whole.
 * @param {string} text
 * @param {import("./renderer.js").PaperRenderer} renderer
 * @param {string} path the main file's project path; its name selects the dialect
 * @returns {import("./blocks.js").PaperBlock[] | null}
 */
export function renderedBlocksOf(text, renderer, path) {
  const rendered = renderer.renderBlocks?.(text, path) ?? null;
  if (!rendered) return null;
  const lines = lineRanges(text);
  const blocks = [];
  for (const block of rendered) {
    const first = lines[block.fromLine - 1];
    const last = lines[block.toLine - 1];
    if (!first || !last || block.toLine < block.fromLine) continue;
    blocks.push({ from: first.from, to: last.to, kind: "Block", render: true, html: block.html });
  }
  return blocks;
}

/** @type {Record<string, PaperFormat>} */
const formats = {
  markdown: {
    module: "markdown",
    splitBlocks,
    blockSource,
    renderedBlocks: renderedBlocksOf,
    ready: () => Promise.resolve(),
    folds: markdownFolds,
  },
  quarto: {
    module: "markdown",
    // No fallback: a Quarto file is drawn whole by the module or not at all.
    splitBlocks: () => null,
    blockSource: () => "",
    renderedBlocks: renderedBlocksOf,
    ready: () => Promise.resolve(),
    folds: markdownFolds,
  },
  org: {
    module: "org",
    splitBlocks: splitOrgBlocks,
    blockSource: orgBlockSource,
    ready: () => loadOrgSyntax().then(() => {}),
    folds: orgFolds,
  },
  typst: {
    module: "typst",
    splitBlocks: splitTypstBlocks,
    blockSource: typstBlockSource,
    ready: () => loadTypstSyntax().then(() => {}),
    folds: typstFolds,
    // Fetches the images and packages the project needs, then asks for a redraw.
    prepare(text, renderer, project, onUpdate) {
      const blocks = splitTypstBlocks(text);
      if (!blocks || !renderer.prepare) return;
      renderer.prepare({ ...project, texts: { ...project.texts, [project.main]: text } }, typstPreamble(text, blocks), onUpdate);
    },
  },
};

/** The names of the formats the paper layer draws. */
export const PAPER_FORMATS = new Set(["markdown", "quarto", "org", "typst"]);

/**
 * The adapter for a format, or null when the paper layer does not draw it.
 * @param {string} name "markdown", "quarto", "org" or "typst"
 * @returns {PaperFormat | null}
 */
export function paperFormat(name) {
  return PAPER_FORMATS.has(name) ? formats[name] : null;
}
