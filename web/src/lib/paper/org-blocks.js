// The top-level blocks of an Org source, for visual mode: the Org counterpart
// of blocks.js. Sections are containers, so their headlines and body blocks
// are the blocks, and each is a range of whole lines.

import { orgTree } from "../source-trees.js";
import { lineEnd, lineStart } from "./blocks.js";

/** @typedef {{ from: number, to: number, kind: string, render: boolean }} PaperBlock */

/** @type {{ text: string, blocks: PaperBlock[] } | null} the last split, by its text */
let last = null;

/** Footnote definitions are read by every block of a document, so they are joined once per split. @type {WeakMap<PaperBlock[], string>} */
const definitionsByBlocks = new WeakMap();

/**
 * Splits an Org source into its top-level blocks, in order, each widened to
 * whole lines. Blocks that would share a line are merged. Keywords, drawers,
 * comments and footnote definitions are kept as source, because the editor
 * never draws them alone. Null while the Org syntax module is not loaded.
 * @param {string} text
 * @returns {PaperBlock[] | null}
 */
export function splitOrgBlocks(text) {
  if (last && last.text === text) return last.blocks;
  const tree = orgTree(text);
  if (!tree) return null;
  /** @type {PaperBlock[]} */
  const blocks = [];
  /**
   * Emits the blocks under `node`, descending into sections.
   * @param {import("@lezer/common").SyntaxNode | null} node
   */
  const visit = (node) => {
    for (; node; node = node.nextSibling) {
      if (node.name === "Section") {
        visit(node.firstChild);
        continue;
      }
      const from = lineStart(text, node.from);
      const to = lineEnd(text, node.to);
      const kind = node.name;
      const render = !keepsSource(kind);
      const previous = blocks.at(-1);
      // Two blocks on one line cannot both be shown as whole lines, so they become one.
      // A merged block stays source if either part does.
      if (previous && from <= previous.to) {
        previous.to = Math.max(previous.to, to);
        previous.render = previous.render && render;
        continue;
      }
      blocks.push({ from, to, kind, render });
    }
  };
  visit(tree.topNode.firstChild);
  last = { text, blocks };
  return blocks;
}

/**
 * Whether a block of this kind stays as source rather than being drawn.
 * @param {string} kind
 */
function keepsSource(kind) {
  switch (kind) {
    case "Keyword":
    case "PropertyDrawer":
    case "Drawer":
    case "Comment":
    case "CommentBlock":
    case "FootnoteDefinition":
    case "Planning":
      return true;
    default:
      return kind.includes("Drawer");
  }
}

/**
 * The Org to render for one block: its own text, then every footnote
 * definition in the document, so that `[fn:1]` resolves when the block is
 * rendered alone.
 * @param {string} text
 * @param {PaperBlock} block
 * @param {PaperBlock[]} blocks the result of splitOrgBlocks(text)
 * @returns {string}
 */
export function orgBlockSource(text, block, blocks) {
  const own = text.slice(block.from, block.to);
  const definitions = definitionsOf(text, blocks);
  return definitions ? `${own}\n\n${definitions}` : own;
}

/**
 * The footnote definitions of a document, joined, memoized per split.
 * @param {string} text
 * @param {PaperBlock[]} blocks
 */
function definitionsOf(text, blocks) {
  let joined = definitionsByBlocks.get(blocks);
  if (joined === undefined) {
    joined = blocks
      .filter((block) => block.kind === "FootnoteDefinition")
      .map((block) => text.slice(block.from, block.to))
      .join("\n\n");
    definitionsByBlocks.set(blocks, joined);
  }
  return joined;
}
