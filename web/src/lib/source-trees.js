// One parse per text. The document index, spellcheck and outline all read the
// same LaTeX and Markdown syntax trees, so they share them here. A result is
// remembered by its text, and a file that changes is re-parsed incrementally
// from its previous tree. No messages are imported: this runs in workers too.

import { TreeFragment } from "@lezer/common";
import { parser as latexParser } from "./latex-language/parser.js";
import { pandocMarkdownParser } from "./pandoc-markdown.js";

/** @typedef {import("@lezer/common").Tree} Tree */
/** @typedef {{ tree: Tree, body: string, frontMatter: [number, number] | null }} MarkdownTrees */

const SIZE = 16;
const LINE_BREAK = /\r\n|\n|\r/g;
const FRONT_MATTER_START = /^---\s*$/;
const FRONT_MATTER_END = /^(?:---|\.\.\.)\s*$/;

/**
 * [0, end] for the YAML block that opens the text, where end is the offset of
 * the end of the closing line; null when there is none. An opener with no
 * closing line is not front matter.
 * @param {string} text
 * @returns {[number, number] | null}
 */
export function frontMatterRange(text) {
  let from = 0;
  let first = true;
  for (const match of text.matchAll(LINE_BREAK)) {
    const line = text.slice(from, match.index);
    if (first && !FRONT_MATTER_START.test(line)) return null;
    if (!first && FRONT_MATTER_END.test(line)) return [0, match.index];
    first = false;
    from = match.index + match[0].length;
  }
  // The last line has no separator after it.
  const line = text.slice(from);
  if (first) return null;
  return FRONT_MATTER_END.test(line) ? [0, text.length] : null;
}

/**
 * The text the Markdown parser reads: front matter blanked, line breaks as
 * the parser expects, same length so every offset is shared.
 * @param {string} text
 * @param {[number, number] | null} frontMatter
 * @returns {string}
 */
function bodyOf(text, frontMatter) {
  let body = text;
  if (frontMatter) {
    body = text.slice(0, frontMatter[1]).replace(/[^\r\n]/g, " ") + text.slice(frontMatter[1]);
  }
  // The parser reads \n only.
  return body.replace(/\r\n?/g, (match) => (match.length === 2 ? " \n" : "\n"));
}

/**
 * The one range that differs between two texts, by common prefix and suffix.
 * @param {string} a
 * @param {string} b
 */
function changeBetween(a, b) {
  const max = Math.min(a.length, b.length);
  let start = 0;
  while (start < max && a.charCodeAt(start) === b.charCodeAt(start)) start++;
  let end = 0;
  while (end < max - start && a.charCodeAt(a.length - 1 - end) === b.charCodeAt(b.length - 1 - end)) end++;
  return { fromA: start, toA: a.length - end, fromB: start, toB: b.length - end };
}

/**
 * Least recently used map: a read or write moves the entry to the end.
 * @template V
 */
class Recent {
  constructor() {
    /** @type {Map<string, V>} */
    this.map = new Map();
  }
  /** @param {string} key @returns {V | undefined} */
  get(key) {
    const value = this.map.get(key);
    if (value !== undefined) {
      this.map.delete(key);
      this.map.set(key, value);
    }
    return value;
  }
  /** @param {string} key @param {V} value */
  set(key, value) {
    this.map.delete(key);
    this.map.set(key, value);
    if (this.map.size > SIZE) this.map.delete(this.map.keys().next().value);
  }
}

/** Results by parsed text, and the last parse per key for reuse. @type {Recent<{ text: string, result: any }>} */
const byText = { latex: new Recent(), markdown: new Recent() };
/** @type {{ latex: Recent<{ text: string, tree: Tree }>, markdown: Recent<{ text: string, tree: Tree }> }} */
const byKey = { latex: new Recent(), markdown: new Recent() };

/**
 * Parses `text`, from the key's previous tree when it has one.
 * @param {"latex" | "markdown"} kind
 * @param {{ parse: (text: string, fragments?: readonly TreeFragment[]) => Tree }} parser
 * @param {string} text what the parser reads
 * @param {string | undefined} key
 * @returns {Tree}
 */
function parse(kind, parser, text, key) {
  const previous = key === undefined ? undefined : byKey[kind].get(key);
  let tree;
  if (previous) {
    const fragments = TreeFragment.applyChanges(TreeFragment.addTree(previous.tree), [
      changeBetween(previous.text, text),
    ]);
    tree = parser.parse(text, fragments);
  } else {
    tree = parser.parse(text);
  }
  if (key !== undefined) byKey[kind].set(key, { text, tree });
  return tree;
}

/**
 * The LaTeX syntax tree of exactly `text`.
 * @param {string} text
 * @param {string} [key] usually the file path; lets an edit reuse the old tree
 * @returns {Tree}
 */
export function latexTree(text, key) {
  const hit = byText.latex.get(text);
  if (hit) return hit.result;
  const tree = parse("latex", latexParser, text, key);
  byText.latex.set(text, { text, result: tree });
  return tree;
}

/**
 * The Pandoc Markdown tree, with the body it was parsed from and the front
 * matter range. Offsets in the tree are offsets in `text`.
 * @param {string} text
 * @param {string} [key] usually the file path; lets an edit reuse the old tree
 * @returns {MarkdownTrees}
 */
export function markdownTree(text, key) {
  const hit = byText.markdown.get(text);
  if (hit) return hit.result;
  const frontMatter = frontMatterRange(text);
  const body = bodyOf(text, frontMatter);
  const tree = parse("markdown", pandocMarkdownParser, body, key);
  const result = { tree, body, frontMatter };
  byText.markdown.set(text, { text, result });
  return result;
}
