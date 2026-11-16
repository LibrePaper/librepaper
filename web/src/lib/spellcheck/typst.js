// Typst source is prose in markup and code after `#`. The spellchecker reads
// only prose, so this lists the markup ranges it must skip. The syntax tree
// comes from Typst's own parser, so prose is what the parser calls markup: the
// text and spaces that sit directly in a Markup node.

import { typstTree } from "../source-trees.js";

/**
 * Ranges of a Typst source that are markup rather than prose, for the
 * spellchecker to skip. Half-open [from, to) offsets into `text` (UTF-16, as
 * JavaScript strings index). Sorted, disjoint and not touching.
 *
 * Prose is the `Text`, `Space` and `SmartQuote` leaves whose parent is a
 * `Markup` node: the root, and the markup of headings, strong and emphasis,
 * list items, and content blocks (including those inside code after `#`).
 * Raw and equation contents are never prose. Everything else is skipped.
 * When the parser is not loaded yet, the whole text is skipped.
 * @param {string} text
 * @param {string} [key] names the document, so an edit re-parses from its previous tree
 * @returns {Array<[number, number]>}
 */
export function typstSkips(text, key) {
  if (typeof text !== "string" || text.length === 0) return [];
  const tree = typstTree(text, key);
  if (!tree) return [[0, text.length]];
  /** @type {Array<[number, number]>} */
  const skips = [];
  // End of the last prose leaf. Whatever lies between two prose leaves is skipped.
  let at = 0;
  tree.iterate({
    enter(node) {
      if (node.name === "Raw" || node.name === "Equation") return false;
      if (
        (node.name === "Text" || node.name === "Space" || node.name === "SmartQuote") &&
        node.node.parent?.name === "Markup"
      ) {
        if (node.from > at) skips.push([at, node.from]);
        at = node.to;
      }
    },
  });
  if (at < text.length) skips.push([at, text.length]);
  return skips;
}
