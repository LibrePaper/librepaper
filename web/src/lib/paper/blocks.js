// The top-level blocks of a Markdown source, for visual mode. Each block is
// drawn as the HTML the preview would show, so a block is a range of whole
// lines: the editor reveals a block's source by line, and a block that began
// mid-line would leave half a line drawn.

import { markdownTree } from "../source-trees.js";

/** @typedef {{ from: number, to: number, kind: string, render: boolean }} PaperBlock */

/** @type {{ text: string, blocks: PaperBlock[] } | null} the last split, by its text */
let last = null;

/** Definitions are read by every block of a document, so they are joined once per split. @type {WeakMap<PaperBlock[], string>} */
const definitionsByBlocks = new WeakMap();

/**
 * The start of the line that holds `at`.
 * @param {string} text
 * @param {number} at
 */
export function lineStart(text, at) {
  let i = at;
  while (i > 0 && text[i - 1] !== "\n" && text[i - 1] !== "\r") i--;
  return i;
}

/**
 * The end of the line that holds `at`: its line break, or the text's end.
 * @param {string} text
 * @param {number} at
 */
export function lineEnd(text, at) {
  let i = at;
  while (i < text.length && text[i] !== "\n" && text[i] !== "\r") i++;
  return i;
}

/**
 * Splits a Markdown source into its top-level blocks, in order, each widened
 * to whole lines. Blocks that would share a line are merged. Definitions and
 * comments are kept as source, because the editor never draws them alone.
 * @param {string} text
 * @returns {PaperBlock[]}
 */
export function splitBlocks(text) {
  if (last && last.text === text) return last.blocks;
  /** @type {PaperBlock[]} */
  const blocks = [];
  const { tree, frontMatter } = markdownTree(text);
  if (frontMatter) {
    blocks.push({ from: 0, to: lineEnd(text, frontMatter[1]), kind: "FrontMatter", render: false });
  }
  for (let node = tree.topNode.firstChild; node; node = node.nextSibling) {
    const from = lineStart(text, node.from);
    const to = lineEnd(text, node.to);
    const kind = node.name;
    const render = !keepsSource(text, kind, from, to);
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
  last = { text, blocks };
  return blocks;
}

/**
 * Whether a block of this kind stays as source rather than being drawn.
 * @param {string} text
 * @param {string} kind
 * @param {number} from
 * @param {number} to
 */
function keepsSource(text, kind, from, to) {
  if (kind === "FrontMatter") return true;
  if (kind.includes("LinkReference") || kind.includes("FootnoteDefinition")) return true;
  if (kind.includes("CommentBlock")) return true;
  // An HTML comment is an HTML block whose text opens with `<!--`.
  return kind === "HTMLBlock" && /^ {0,3}<!--/.test(text.slice(from, to));
}

/**
 * The Markdown to render for one block: its own text, then every link
 * reference and footnote definition in the document, so that `[x][ref]` and
 * `[^1]` resolve when the block is rendered alone.
 * @param {string} text
 * @param {PaperBlock} block
 * @param {PaperBlock[]} blocks the result of splitBlocks(text)
 * @returns {string}
 */
export function blockSource(text, block, blocks) {
  const own = text.slice(block.from, block.to);
  const definitions = definitionsOf(text, blocks);
  return definitions ? `${own}\n\n${definitions}` : own;
}

/**
 * The definitions of a document, joined, memoized per split.
 * @param {string} text
 * @param {PaperBlock[]} blocks
 */
function definitionsOf(text, blocks) {
  let joined = definitionsByBlocks.get(blocks);
  if (joined === undefined) {
    joined = blocks
      .filter((block) => block.kind.includes("LinkReference") || block.kind.includes("FootnoteDefinition"))
      .map((block) => text.slice(block.from, block.to))
      .join("\n\n");
    definitionsByBlocks.set(blocks, joined);
  }
  return joined;
}
