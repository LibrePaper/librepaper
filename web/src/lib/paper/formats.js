// The formats the paper layer draws. Each adapter says what the layer needs
// from a format: the blocks of a source, the text to render for one block,
// the renderer module that draws it, and when the format can split text.

import { blockSource, splitBlocks } from "./blocks.js";
import { orgBlockSource, splitOrgBlocks } from "./org-blocks.js";
import { loadOrgSyntax } from "../org-language/module.js";

/**
 * @typedef {{
 *   module: "markdown" | "org",
 *   splitBlocks: (text: string) => import("./blocks.js").PaperBlock[] | null,
 *   blockSource: (text: string, block: import("./blocks.js").PaperBlock, blocks: import("./blocks.js").PaperBlock[]) => string,
 *   ready: () => Promise<void>,
 * }} PaperFormat
 */

/** @type {Record<string, PaperFormat>} */
const formats = {
  markdown: {
    module: "markdown",
    splitBlocks,
    blockSource,
    ready: () => Promise.resolve(),
  },
  org: {
    module: "org",
    splitBlocks: splitOrgBlocks,
    blockSource: orgBlockSource,
    ready: () => loadOrgSyntax().then(() => {}),
  },
};

/** The names of the formats the paper layer draws. */
export const PAPER_FORMATS = new Set(["markdown", "org"]);

/**
 * The adapter for a format, or null when the paper layer does not draw it.
 * @param {string} name "markdown" or "org"
 * @returns {PaperFormat | null}
 */
export function paperFormat(name) {
  return PAPER_FORMATS.has(name) ? formats[name] : null;
}
