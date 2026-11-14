// The top-level blocks of an Org source, for visual mode: the Org counterpart
// of blocks.js. Sections are containers, so their headlines and body blocks
// are the blocks, and each is a range of whole lines. A header whose keywords
// set a title, subtitle, author or date is one TitleBlock, which visual mode
// draws as the title block.

import { orgTree } from "../source-trees.js";
import { lineEnd, lineStart } from "./blocks.js";

/** @typedef {{ from: number, to: number, kind: string, render: boolean }} PaperBlock */

// The kinds that make up a document's header: the leading run of these blocks.
export const ORG_HEADER_KINDS = new Set(["TitleBlock", "Keyword", "Comment", "CommentBlock"]);

// A keyword line that includes or sets up another file.
const INCLUDE_KEYWORD = /^\s*#\+(include|setupfile):/i;

// A keyword line that sets a title, subtitle, author or date to a non-empty value.
const TITLE_KEYWORD = /^\s*#\+(title|subtitle|author|date):[ \t]*\S/i;

/** @type {{ text: string, blocks: PaperBlock[] } | null} the last split, by its text */
let last = null;

/** Footnote definitions are read by every block of a document, so they are joined once per split. @type {WeakMap<PaperBlock[], string>} */
const definitionsByBlocks = new WeakMap();

/**
 * Splits an Org source into its top-level blocks, in order, each widened to
 * whole lines. Blocks that would share a line are merged. Keywords, drawers,
 * comments and footnote definitions are kept as source, because the editor
 * never draws them alone. The header's keywords become one TitleBlock, drawn
 * as the title block, when one of them sets a title, subtitle, author or date.
 * Null while the Org syntax module is not loaded.
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
  foldTitleBlock(text, blocks);
  last = { text, blocks };
  return blocks;
}

/**
 * Folds the first stretch of the header's keywords into one TitleBlock, when
 * one of the header's keywords sets a title, subtitle, author or date. The
 * stretch starts at the first keyword that is not an include, and ends at the
 * last keyword before the next include, so an include splits the header in
 * two. Mutates `blocks`.
 * @param {string} text
 * @param {PaperBlock[]} blocks
 */
function foldTitleBlock(text, blocks) {
  const sourceOf = (/** @type {PaperBlock} */ block) => text.slice(block.from, block.to);
  const isKeyword = (/** @type {PaperBlock} */ block) => block.kind === "Keyword";
  const isInclude = (/** @type {PaperBlock} */ block) => isKeyword(block) && INCLUDE_KEYWORD.test(sourceOf(block));
  let leading = 0;
  while (leading < blocks.length && ["Keyword", "Comment", "CommentBlock"].includes(blocks[leading].kind)) leading++;
  const header = blocks.slice(0, leading);
  if (!header.some((block) => isKeyword(block) && TITLE_KEYWORD.test(sourceOf(block)))) return;
  const first = header.findIndex((block) => isKeyword(block) && !isInclude(block));
  if (first === -1) return;
  let end = first;
  while (end + 1 < leading && !isInclude(blocks[end + 1])) end++;
  // The stretch ends on a keyword, not on a trailing comment.
  while (!isKeyword(blocks[end])) end--;
  blocks.splice(first, end - first + 1, { from: blocks[first].from, to: blocks[end].to, kind: "TitleBlock", render: true });
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
 * rendered alone. A title block is the whole header, with no footnote
 * definitions.
 * @param {string} text
 * @param {PaperBlock} block
 * @param {PaperBlock[]} blocks the result of splitOrgBlocks(text)
 * @returns {string}
 */
export function orgBlockSource(text, block, blocks) {
  if (block.kind === "TitleBlock") {
    // An #+INCLUDE: line can split the header, and the title block still shows the
    // title, author and date written after it, so it renders the whole header.
    let count = 0;
    while (count < blocks.length && ORG_HEADER_KINDS.has(blocks[count].kind)) count++;
    return text.slice(blocks[0].from, blocks[count - 1].to);
  }
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
