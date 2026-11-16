// One parse per text. The document index, spellcheck and outline all read the
// same LaTeX and Markdown syntax trees, so they share them here. A result is
// remembered by its text, and a file that changes is re-parsed incrementally
// from its previous tree. No messages are imported: this runs in workers too.

import { TreeFragment } from "@lezer/common";
import { parser as latexParser } from "./latex-language/parser.js";
import { loadOrgSyntax, orgSyntax } from "./org-language/module.js";
import { pandocMarkdownParser } from "./pandoc-markdown.js";
import { loadTypstSyntax, typstSyntax } from "./typst-language/module.js";

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

/** Results by parsed text, and the last parse per key for reuse. @type {{ latex: Recent<{ text: string, result: Tree }>, markdown: Recent<{ text: string, result: MarkdownTrees }>, org: Recent<{ text: string, result: Tree }> }} */
const byText = { latex: new Recent(), markdown: new Recent(), org: new Recent() };
/** @type {{ latex: Recent<{ text: string, tree: Tree }>, markdown: Recent<{ text: string, tree: Tree }>, org: Recent<{ text: string, tree: Tree }> }} */
const byKey = { latex: new Recent(), markdown: new Recent(), org: new Recent() };

/**
 * Parses `text`, from the key's previous tree when it has one.
 * @param {"latex" | "markdown" | "org"} kind
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

/**
 * The Org syntax tree of exactly `text`, or null while org.wasm is not loaded
 * (the first call starts loading it). An edit re-parses only the top-level
 * sections it touches; the result is remembered by text.
 * @param {string} text
 * @param {string} [key] usually the file path; lets an edit reuse the old tree
 * @returns {Tree | null}
 */
export function orgTree(text, key) {
  const api = orgSyntax();
  if (!api) {
    startOrgSyntax();
    return null;
  }
  const hit = byText.org.get(text);
  if (hit) return hit.result;
  const tree = parse("org", api, text, key);
  byText.org.set(text, { text, result: tree });
  return tree;
}

/**
 * The Org syntax tree of `text`, once org.wasm has loaded. For paths that must
 * not skip, such as an export. Null when the module cannot be loaded.
 * @param {string} text
 * @param {string} [key]
 * @returns {Promise<Tree | null>}
 */
export async function orgTreeReady(text, key) {
  startOrgSyntax();
  await loadOrgSyntax();
  return orgSyntax() ? orgTree(text, key) : null;
}

/** @type {Promise<void> | null} the one load of org.wasm, started by the first request */
let orgLoading = null;
/** @type {Set<() => void>} */
const orgListeners = new Set();

/** Starts loading org.wasm, once. The listeners run when it arrives. */
function startOrgSyntax() {
  if (orgLoading) return;
  // A load that fails or finds no module is forgotten, so a later call retries.
  orgLoading = loadOrgSyntax().then((api) => {
    if (!api) {
      orgLoading = null;
      return;
    }
    const listeners = [...orgListeners];
    orgListeners.clear();
    for (const listener of listeners) {
      try {
        listener();
      } catch {
        // One listener's failure must not stop the others.
      }
    }
  }, () => {
    orgLoading = null;
  });
}

/**
 * Calls `listener()` once org.wasm has loaded, if it is not loaded yet.
 * Subscribing does not start the load: the first `orgTree` call does. Returns
 * a function that unsubscribes.
 * @param {() => void} listener
 * @returns {() => void}
 */
export function onOrgSyntax(listener) {
  if (orgSyntax()) return () => {};
  orgListeners.add(listener);
  return () => {
    orgListeners.delete(listener);
  };
}

/** @type {Promise<void> | null} the one load of typst-syntax.wasm, started by the first request */
let typstLoading = null;
/** @type {Set<() => void>} */
const typstListeners = new Set();

/** Starts loading typst-syntax.wasm, once. The listeners run when it arrives. */
function startTypstSyntax() {
  if (typstLoading) return;
  // A load that fails or finds no module is forgotten, so a later call retries.
  typstLoading = loadTypstSyntax().then((api) => {
    if (!api) {
      typstLoading = null;
      return;
    }
    const listeners = [...typstListeners];
    typstListeners.clear();
    for (const listener of listeners) {
      try {
        listener();
      } catch {
        // One listener's failure must not stop the others.
      }
    }
  }, () => {
    typstLoading = null;
  });
}

/**
 * The Typst syntax tree of `text`, or null while typst-syntax.wasm is not
 * loaded (the first call starts loading it).
 * @param {string} text
 * @param {string} [key] usually the file path; lets an edit reuse the old tree
 * @returns {import("@lezer/common").Tree | null}
 */
export function typstTree(text, key) {
  const api = typstSyntax();
  if (!api) {
    startTypstSyntax();
    return null;
  }
  return api.tree(text, key);
}

/**
 * Calls `listener()` once typst-syntax.wasm has loaded, if it is not loaded
 * yet. Subscribing does not start the load: the first `typstTree` call does,
 * so a project without Typst files never fetches the module. Returns a
 * function that unsubscribes.
 * @param {() => void} listener
 * @returns {() => void}
 */
export function onTypstSyntax(listener) {
  if (typstSyntax()) return () => {};
  typstListeners.add(listener);
  return () => {
    typstListeners.delete(listener);
  };
}
