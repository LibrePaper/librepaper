// Decides whether a LaTeX edit can be shown at once by patching the preview
// that is already on screen, and describes the patch. The real compile then
// replaces the patched preview. Being wrong shows the reader a wrong preview
// for a few seconds, so everything here is conservative: when in doubt the
// answer is null and the preview simply waits for the compile.

import { latexTree } from "../source-trees.js";

/**
 * @typedef {{ kind: "prose", file: string, line: number, before: string, deleted: string, inserted: string, after: string }} ProsePatch
 * @typedef {{ kind: "math", file: string, line: number, oldTex: string, tex: string, display: boolean }} MathPatch
 * @typedef {{ kind: "paragraph", file: string, anchorLine: number, text: string }} ParagraphPatch
 * @typedef {ProsePatch | MathPatch} Patch
 * @typedef {{ from: number, oldTo: number, newTo: number }} Span
 * @typedef {import("@lezer/common").SyntaxNode} SyntaxNode
 * @typedef {import("@lezer/common").Tree} Tree
 */

/// Characters whose rendering is the character itself.
const PLAIN_CHAR = /^[\p{L}\p{N} .,;:!?()\/*+=@'`~-]$/u;
const CONTEXT_MAX = 32;

/// Environments whose direct content is running text.
const TEXT_ENVIRONMENTS = new Set([
  "document", "itemize", "enumerate", "description", "abstract", "quote",
  "quotation", "center", "flushleft", "flushright",
]);
/// Commands whose mandatory argument is running text.
const TEXT_COMMANDS = new Set([
  "emph", "textbf", "textit", "textsl", "textsc", "texttt", "textup", "textmd",
  "textrm", "textsf", "underline", "section", "subsection", "subsubsection",
  "paragraph", "subparagraph", "chapter", "part", "caption", "footnote",
]);
/// Nodes that may sit right beside an edit: text, and the braces around it.
const NEIGHBOR_OK = new Set([
  "Text", "Ligature", "Tie", "OpenBrace", "CloseBrace", "Document", "Environment", "Group",
]);
/// Leaf nodes the edit position itself may be on.
const POSITION_LEAF_OK = new Set(["Text", "Ligature", "Tie", "CloseBrace"]);

const MATH_NODES = new Set(["InlineMath", "DisplayMath", "MathEnvironment"]);
const MATH_ENVIRONMENTS = new Set(["equation", "equation*", "displaymath"]);
/// [open, close, display]; `$$` must be tried before `$`.
/** @type {Array<[string, string, boolean]>} */
const MATH_DELIMITERS = [
  ["$$", "$$", true], ["$", "$", false], ["\\(", "\\)", false], ["\\[", "\\]", true],
];

/**
 * Whether `s` has only characters that render as themselves.
 * @param {string} s
 */
function isPlain(s) {
  for (const char of s) if (!PLAIN_CHAR.test(char)) return false;
  return true;
}

/**
 * The one changed span between two texts: common prefix and common suffix.
 * Null when the texts are equal.
 * @param {string} a
 * @param {string} b
 * @returns {Span | null}
 */
function diffSpan(a, b) {
  const max = Math.min(a.length, b.length);
  let from = 0;
  while (from < max && a[from] === b[from]) from++;
  let tail = 0;
  while (tail < max - from && a[a.length - 1 - tail] === b[b.length - 1 - tail]) tail++;
  let oldTo = a.length - tail;
  let newTo = b.length - tail;
  // Never cut a surrogate pair in two.
  if (from > 0 && (a.charCodeAt(from - 1) & 0xfc00) === 0xd800) from--;
  if (tail > 0 && (a.charCodeAt(oldTo) & 0xfc00) === 0xdc00) { oldTo++; newTo++; }
  if (from === oldTo && from === newTo) return null;
  return { from, oldTo, newTo };
}

/**
 * The 1-based line that holds offset `at`.
 * @param {string} text
 * @param {number} at
 */
function lineOf(text, at) {
  let line = 1;
  for (let i = text.indexOf("\n"); i !== -1 && i < at; i = text.indexOf("\n", i + 1)) line++;
  return line;
}

/**
 * The name of the environment a Begin node opens, e.g. "equation*".
 * @param {string} text
 * @param {SyntaxNode} env an Environment or MathEnvironment node
 * @returns {string | null}
 */
function envName(text, env) {
  const begin = env.firstChild;
  if (!begin) return null;
  for (let child = begin.firstChild; child; child = child.nextSibling) {
    if (child.name === "EnvName") return text.slice(child.from, child.to).trim();
  }
  return null;
}

/**
 * The name of the command a group is the argument of, without its star.
 * Null when the group does not directly follow a command.
 * @param {string} text
 * @param {SyntaxNode} group
 */
function commandBefore(text, group) {
  const prev = group.prevSibling;
  if (!prev || prev.name !== "Command") return null;
  const match = /^\\([A-Za-z]+)\*?/.exec(text.slice(prev.from, prev.to));
  return match ? match[1] : null;
}

/**
 * Whether the characters on both sides of `pos` are text or braces. A command,
 * comment or delimiter next to the edit could be extended or merged by it.
 * @param {Tree} tree
 * @param {number} pos
 */
function neighborsOk(tree, pos) {
  return NEIGHBOR_OK.has(tree.resolveInner(pos, -1).name) && NEIGHBOR_OK.has(tree.resolveInner(pos, 1).name);
}

/**
 * The node that says where `pos` sits. The node to the right usually does,
 * except at an opening brace, where the position is outside the group.
 * @param {Tree} tree
 * @param {number} pos
 * @returns {SyntaxNode | null}
 */
function positionNode(tree, pos) {
  const right = tree.resolveInner(pos, 1);
  if (right.name !== "OpenBrace") return right;
  const left = tree.resolveInner(pos, -1);
  // `}|{`: between two groups, too ambiguous.
  return left.name === "CloseBrace" ? null : left;
}

/**
 * Whether `pos` is in running text: only allowed environments and the
 * argument groups of text commands lie between it and the top. For the main
 * file it must also be inside the `document` environment.
 * @param {Tree} tree
 * @param {string} text
 * @param {number} pos
 * @param {boolean} isMain
 */
function inRunningText(tree, text, pos, isMain) {
  const start = positionNode(tree, pos);
  if (!start) return false;
  let inDocument = false;
  for (let node = start; node; node = node.parent) {
    if (node.name === "Document") continue;
    if (node.name === "Environment") {
      const name = envName(text, node);
      if (!name || !TEXT_ENVIRONMENTS.has(name)) return false;
      if (name === "document") inDocument = true;
    } else if (node.name === "Group") {
      const name = commandBefore(text, node);
      if (!name || !TEXT_COMMANDS.has(name)) return false;
    } else if (node !== start || !POSITION_LEAF_OK.has(node.name)) {
      return false;
    }
  }
  return isMain ? inDocument : true;
}

/**
 * The plain run next to an edit, cut at 32 characters and at a blank line.
 * Single line breaks and runs of blanks become one space.
 * @param {string} text
 * @param {number} at where the run starts (forward) or ends (backward)
 * @param {boolean} backward
 */
function plainRun(text, at, backward) {
  const ok = (char) => char === "\n" || PLAIN_CHAR.test(char);
  let i = at;
  if (backward) while (i > 0 && at - i < CONTEXT_MAX && ok(text[i - 1])) i--;
  else while (i < text.length && i - at < CONTEXT_MAX && ok(text[i])) i++;
  const raw = backward ? text.slice(i, at) : text.slice(at, i);
  const parts = raw.split(/\n[ \n]*\n/);
  return (backward ? parts[parts.length - 1] : parts[0]).replace(/\s+/g, " ");
}

/**
 * The text as LaTeXML prints it, for the single-character cases. Pairs such
 * as `` and -- are refused before this is called.
 * @param {string} s
 */
function render(s) {
  return s.replace(/'/g, "\u2019").replace(/`/g, "\u2018").replace(/~/g, "\u00a0");
}

/**
 * @param {{ path: string, oldText: string, newText: string, isMain: boolean }} edit
 * @param {Span} span
 * @returns {ProsePatch | null}
 */
function prosePatch({ path, oldText, newText, isMain }, { from, oldTo, newTo }) {
  const deleted = oldText.slice(from, oldTo);
  const inserted = newText.slice(from, newTo);
  if (!isPlain(deleted) || !isPlain(inserted)) return null;
  const oldTree = latexTree(oldText, path);
  const newTree = latexTree(newText, path);
  /** @type {Array<[Tree, string, number]>} */
  const sides = [
    [oldTree, oldText, oldTo],
    [newTree, newText, newTo],
  ];
  for (const [tree, text, end] of sides) {
    if (!neighborsOk(tree, from) || !neighborsOk(tree, end)) return null;
    if (!inRunningText(tree, text, from, isMain) || !inRunningText(tree, text, end, isMain)) return null;
  }
  // The window can end inside the blank line next to a paragraph; the
  // paragraph's own text has no space there.
  const rawBefore = plainRun(oldText, from, true).replace(/^\s+/, "");
  const rawAfter = plainRun(oldText, oldTo, false).replace(/\s+$/, "");
  // A ligature split by the edit boundary shows up only in the joined string.
  const ligature = /--|``|''|[!?]`/;
  if (ligature.test(rawBefore + deleted + rawAfter) || ligature.test(rawBefore + inserted + rawAfter)) return null;
  const before = render(rawBefore);
  const after = render(rawAfter);
  // Too little context to find the place in the preview.
  if (before.length + after.length < 6) return null;
  return {
    kind: "prose", file: path, line: lineOf(oldText, from),
    before, deleted: render(deleted), inserted: render(inserted), after,
  };
}

/**
 * The innermost math node around `pos`, or null.
 * @param {Tree} tree
 * @param {number} pos
 */
function mathAround(tree, pos) {
  for (let node = tree.resolveInner(pos, 1); node; node = node.parent) {
    if (MATH_NODES.has(node.name)) return node;
  }
  return null;
}

/**
 * Where a math node's content lies between its delimiters. Null for a node
 * that is unclosed or is not a supported kind.
 * @param {string} text
 * @param {SyntaxNode} node
 * @returns {{ from: number, to: number, display: boolean } | null}
 */
function mathContent(text, node) {
  if (node.name === "MathEnvironment") {
    const begin = node.firstChild;
    const end = node.lastChild;
    if (!begin || !end || begin === end || end.name !== "EndEnv") return null;
    const name = envName(text, node);
    if (!name || !MATH_ENVIRONMENTS.has(name)) return null;
    return { from: begin.to, to: end.from, display: true };
  }
  const display = node.name === "DisplayMath";
  const delimiter = MATH_DELIMITERS.find(([open, , disp]) => disp === display && text.startsWith(open, node.from));
  if (!delimiter) return null;
  const [open, close] = delimiter;
  if (node.to - node.from < open.length + close.length) return null;
  if (text.slice(node.to - close.length, node.to) !== close) return null;
  return { from: node.from + open.length, to: node.to - close.length, display };
}

/**
 * Whether `pos` lies inside the `document` environment.
 * @param {SyntaxNode} node
 * @param {string} text
 */
function inDocumentEnvironment(node, text) {
  for (let up = node.parent; up; up = up.parent) {
    if (up.name === "Environment" && envName(text, up) === "document") return true;
  }
  return false;
}

/**
 * The formula source without labels, which the preview does not show.
 * @param {string} content
 */
function cleanTex(content) {
  return content.replace(/\\label\s*\{[^}]*\}/g, "").trim();
}

/**
 * Whether braces pair up, so the formula is complete enough to render.
 * @param {string} tex
 */
function balanced(tex) {
  let depth = 0;
  for (let i = 0; i < tex.length; i++) {
    if (tex[i] === "\\") i++;
    else if (tex[i] === "{") depth++;
    else if (tex[i] === "}" && --depth < 0) return false;
  }
  return depth === 0;
}

/**
 * @param {{ path: string, oldText: string, newText: string, isMain: boolean }} edit
 * @param {Span} span
 * @param {SyntaxNode} oldNode the math node around the edit in the old text
 * @returns {MathPatch | null}
 */
function mathPatch({ path, oldText, newText, isMain }, { from, oldTo, newTo }, oldNode) {
  const newNode = mathAround(latexTree(newText, path), from);
  if (!newNode) return null;
  const oldContent = mathContent(oldText, oldNode);
  const newContent = mathContent(newText, newNode);
  if (!oldContent || !newContent) return null;
  // Same node: it starts where it did, its end moved only by the edit, and
  // its delimiters are the same.
  const shift = newTo - oldTo;
  if (newNode.name !== oldNode.name || newNode.from !== oldNode.from) return null;
  if (newNode.to - oldNode.to !== shift) return null;
  if (newContent.from !== oldContent.from || newContent.to - oldContent.to !== shift) return null;
  if (oldContent.display !== newContent.display) return null;
  if (from < oldContent.from || oldTo > oldContent.to) return null;
  if (isMain && !inDocumentEnvironment(oldNode, oldText)) return null;
  const oldSource = oldText.slice(oldContent.from, oldContent.to);
  const newSource = newText.slice(newContent.from, newContent.to);
  if (oldSource.includes("\\tag") || newSource.includes("\\tag")) return null;
  const oldTex = cleanTex(oldSource);
  const tex = cleanTex(newSource);
  // A half-typed formula would show an error; a comment or stray $ is not math.
  if (!tex || tex === oldTex || !balanced(tex) || /[$%]/.test(tex.replace(/\\[$%]/g, ""))) return null;
  return { kind: "math", file: path, line: lineOf(oldText, oldNode.from), oldTex, tex, display: oldContent.display };
}

/**
 * The patch that shows an edit in the rendered preview, or null when the edit
 * is not safe to patch and the preview should wait for the compile.
 * @param {{ path: string, oldText: string, newText: string, isMain: boolean }} edit
 * @returns {Patch | null}
 */
export function livePatch(edit) {
  const { path, oldText, newText } = edit;
  if (!path.endsWith(".tex")) return null;
  const span = diffSpan(oldText, newText);
  if (!span) return null;
  if (oldText.slice(span.from, span.oldTo).includes("\n") || newText.slice(span.from, span.newTo).includes("\n")) return null;
  const oldNode = mathAround(latexTree(oldText, path), span.from);
  return oldNode ? mathPatch(edit, span, oldNode) : prosePatch(edit, span);
}

/**
 * The paragraph around offset `at` of the new text, as the preview would show
 * it: the lines between the blank lines above and below, collapsed to one
 * line. Null unless every character renders as itself and `at` is in running
 * text.
 * @param {{ path: string, text: string, at: number, isMain: boolean }} edit
 * @returns {{ startLine: number, endLine: number, text: string } | null}
 */
export function paragraphText({ path, text, at, isMain }) {
  if (!/\.tex$/i.test(path)) return null;
  const lines = text.split("\n");
  const index = lineOf(text, at) - 1;
  if (index < 0 || index >= lines.length || lines[index].trim() === "") return null;
  let first = index;
  while (first > 0 && lines[first - 1].trim() !== "") first--;
  let last = index;
  while (last < lines.length - 1 && lines[last + 1].trim() !== "") last++;
  const paragraph = lines.slice(first, last + 1).join("\n");
  for (const char of paragraph) if (char !== "\n" && !PLAIN_CHAR.test(char)) return null;
  if (/--|``|''|[!?]`/.test(paragraph)) return null;
  if (!inRunningText(latexTree(text, path), text, at, isMain)) return null;
  const collapsed = paragraph.replace(/\s+/g, " ").trim();
  if (collapsed === "") return null;
  return { startLine: first + 1, endLine: last + 1, text: render(collapsed) };
}
