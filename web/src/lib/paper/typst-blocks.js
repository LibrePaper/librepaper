// The top-level blocks of a Typst source, for visual mode: the Typst
// counterpart of blocks.js. Typst markup is flat, so a block is a run of
// markup between blank lines, or a heading, and each is a range of whole
// lines. A block is compiled alone, so what the rest of the document gives it
// (settings, definitions, numbers, reference text) is put in front of it.

import { typstTree } from "../source-trees.js";
import { typstFacts } from "../visual-markup/typst-facts.js";
import { lineEnd, lineStart } from "./blocks.js";

/** @typedef {{ from: number, to: number, kind: string, render: boolean }} PaperBlock */

/** @type {{ text: string, blocks: PaperBlock[] } | null} the last split, by its text */
let last = null;

/** What a document gives every block, computed once per split. @type {WeakMap<PaperBlock[], { text: string, refs: string, numbered: any[] }>} */
const contexts = new WeakMap();

// Rules that set the document up. They are never drawn and go in front of every block.
const PREAMBLE = new Set(["SetRule", "ShowRule", "LetBinding", "ModuleImport", "ModuleInclude"]);
const TRIVIA = new Set(["Space", "Parbreak", "LineComment", "BlockComment", "Label"]);
const COMMENTS = new Set(["LineComment", "BlockComment"]);

/**
 * Splits a Typst source into its top-level blocks, in order, each widened to
 * whole lines. Blocks that would share a line are merged. Set, show, let,
 * import and include rules and comments are kept as source, because the editor
 * never draws them alone. Null while typst-syntax.wasm is not loaded.
 * @param {string} text
 * @returns {PaperBlock[] | null}
 */
export function splitTypstBlocks(text) {
  if (last && last.text === text) return last.blocks;
  const tree = typstTree(text);
  if (!tree) return null;
  /** @type {PaperBlock[]} */
  const blocks = [];
  /** @param {number} from @param {number} to @param {string} kind @param {boolean} render @param {boolean} [trivia] */
  const emit = (from, to, kind, render, trivia = false) => {
    from = lineStart(text, from);
    to = lineEnd(text, to);
    const previous = blocks.at(-1);
    // Two blocks on one line cannot both be shown as whole lines, so they become one.
    // A merged block stays source if either part does, except that a label
    // or comment sharing the line (`= Intro <sec:intro>`) changes nothing.
    if (previous && from <= previous.to) {
      previous.to = Math.max(previous.to, to);
      if (!trivia) previous.render = previous.render && render;
      return;
    }
    blocks.push({ from, to, kind, render });
  };

  // The run of markup being collected: from its first non-space child to its last.
  /** @type {{ from: number, to: number, kind: string, content: boolean } | null} */
  let run = null;
  const flush = () => {
    if (run) {
      const comment = run.kind !== "" && !run.content;
      emit(run.from, run.to, comment ? "Comment" : run.kind, run.content, !run.content);
    }
    run = null;
  };

  const markup = tree.topNode.firstChild;
  for (let node = markup?.firstChild ?? null; node; node = node.nextSibling) {
    const name = node.name;
    if (name === "Space") continue;
    if (name === "Parbreak") {
      flush();
      continue;
    }
    if (name === "Hash" && node.nextSibling && PREAMBLE.has(node.nextSibling.name)) {
      flush();
      const rule = node.nextSibling;
      emit(node.from, rule.to, rule.name, false);
      node = rule;
      continue;
    }
    if (name === "Heading") {
      flush();
      emit(node.from, node.to, name, true);
      continue;
    }
    const content = !TRIVIA.has(name);
    // A `#` and the code it starts are one thing, named by the code.
    const kind = name === "Hash" && node.nextSibling ? node.nextSibling.name : name;
    if (!run) run = { from: node.from, to: node.to, kind, content };
    else {
      run.to = node.to;
      run.content = run.content || content;
      if (content && COMMENTS.has(run.kind)) run.kind = kind;
    }
  }
  flush();
  last = { text, blocks };
  return blocks;
}

// The word a reference to this kind of label reads.
const KIND_WORDS = { section: "Section", equation: "Equation", figure: "Figure", table: "Table", listing: "Listing" };

// The kind of figure a counter update names, in Typst's own spelling.
const FIGURE_KINDS = { figure: "image", table: "table", listing: "raw" };

/** @param {string} kind */
function kindWord(kind) {
  return KIND_WORDS[/** @type {keyof typeof KIND_WORDS} */ (kind)] ?? kind.charAt(0).toUpperCase() + kind.slice(1);
}

/** What the whole document gives every block: its preamble-independent facts. */
function contextOf(text, blocks) {
  let context = contexts.get(blocks);
  if (context) return context;
  const facts = typstFacts({ "main.typ": text }, "main.typ", "main.typ", []);
  // A reference alone in a block cannot see its target, so the text it would
  // read ("Equation 1") is given to every block as a table. A label with no
  // number reads as its key in brackets, which is also how a citation looks.
  const entries = [...facts.labels].map(
    ([label, { number, kind }]) => `${JSON.stringify(label)}: ${JSON.stringify(`${kindWord(kind)} ${number}`)}`,
  );
  const refs =
    `#let __lp_refs = (${entries.length ? entries.join(", ") : ":"})\n` +
    '#show ref: it => link("#" + str(it.target), __lp_refs.at(str(it.target), default: "[" + str(it.target) + "]"))\n';
  context = { text, refs, numbered: facts.numbered };
  contexts.set(blocks, context);
  return context;
}

/**
 * The counter updates that put the numbering where it stands before `block`:
 * the heading numbers in force, and how many figures of each kind and display
 * equations have gone by.
 * @param {any[]} numbered
 * @param {PaperBlock} block
 */
function counterUpdates(numbered, block) {
  const headings = [0, 0, 0, 0, 0, 0];
  /** @type {Map<string, number>} */
  const counts = new Map();
  for (const item of numbered) {
    if (item.at >= block.from) break;
    if (item.type === "heading") {
      headings[item.level - 1] += 1;
      headings.fill(0, item.level);
    } else {
      const key = item.type === "equation" ? "equation" : item.kind;
      counts.set(key, (counts.get(key) ?? 0) + 1);
    }
  }
  let out = "";
  let depth = headings.length;
  while (depth > 0 && headings[depth - 1] === 0) depth--;
  if (depth > 0) out += `#counter(heading).update((${headings.slice(0, depth).join(", ")},))\n`;
  for (const [key, count] of counts) {
    if (key === "equation") out += `#counter(math.equation).update(${count})\n`;
    else {
      const kind = FIGURE_KINDS[/** @type {keyof typeof FIGURE_KINDS} */ (key)] ?? JSON.stringify(key);
      out += `#counter(figure.where(kind: ${kind})).update(${count})\n`;
    }
  }
  return out;
}

/**
 * The Typst to compile for one block: every preamble rule before it, then the
 * reference table and the counter updates, then its own text.
 * @param {string} text
 * @param {PaperBlock} block
 * @param {PaperBlock[]} blocks the result of splitTypstBlocks(text)
 * @returns {string}
 */
export function typstBlockSource(text, block, blocks) {
  const context = contextOf(text, blocks);
  let preamble = "";
  for (const other of blocks) {
    if (other.from >= block.from) break;
    if (!other.render && other.kind !== "Comment") preamble += text.slice(other.from, other.to) + "\n";
  }
  return preamble + context.refs + counterUpdates(context.numbered, block) + text.slice(block.from, block.to);
}
