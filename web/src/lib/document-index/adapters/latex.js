// The LaTeX adapter of the document index. It parses one .tex source with the
// Lezer grammar and walks the tree, so comments, \verb spans and verbatim
// bodies are never read as LaTeX, and \begin {x} with a space is an
// environment. Headings still come from outline.js and prose skips from
// spellcheck/latex.js, because other consumers share those scanners and their
// output must not drift. Definitions are read by regex from the source with
// the tree's comments and verbatim text blanked.

import { latexTree } from "../../source-trees.js";
import { extractOutline } from "../../outline.js";
import { skipRanges } from "../../spellcheck/index.js";

/** @typedef {import("../schema.js").IndexItem} IndexItem */
/** @typedef {{ name: string, from: number, to: number }} Piece */
/** @typedef {{ name: string, from: number, to: number, beginTo: number, endFrom: number }} Span */

const DEFINITION = /\\(?:newcommand|renewcommand|providecommand)\*?\s*(?:\{\s*\\([A-Za-z@]+)\s*\}|\\([A-Za-z@]+))/g;
const OPERATOR = /\\DeclareMathOperator\*?\s*\{\s*\\([A-Za-z@]+)\s*\}/g;
const DEF = /\\def\\([A-Za-z@]+)/g;
const ENVIRONMENT_DEFINITION = /\\(?:newenvironment|renewenvironment)\*?\s*\{\s*([A-Za-z][A-Za-z0-9*]*)\s*\}/g;
const CITE = /^\\(?:(?:no|paren|text|auto|foot|smart|super|Paren|Text|Auto|Foot|Smart|Super)?[Cc]ite(?:p|t|author|year)?)$/;

const REFS = new Set([
  "ref", "eqref", "pageref", "autoref", "cref", "Cref", "nameref", "vref", "cpageref", "labelcref",
].map((name) => `\\${name}`));
const INCLUDES = new Set(["\\input", "\\include", "\\subfile"]);
const ENVIRONMENTS = new Set(["Environment", "MathEnvironment", "InnerEnvironment", "VerbatimEnvironment"]);
const BEGIN_TAGS = new Set(["BeginEnv", "MathBeginEnv", "VerbBeginEnv", "ListingBeginEnv"]);

/**
 * @param {number} from
 * @param {number} to
 * @returns {[number, number]}
 */
const pair = (from, to) => [from, to];

/**
 * The offset of the first line break at or after `from`, or the text length.
 * @param {string} text
 * @param {number} from
 * @returns {number}
 */
function lineEnd(text, from) {
  const at = text.indexOf("\n", from);
  return at < 0 ? text.length : at;
}

/**
 * The text with each span replaced by blanks. Line breaks stay, so offsets and
 * line numbers do not move.
 * @param {string} text
 * @param {Array<[number, number]>} spans sorted, non-overlapping
 * @returns {string}
 */
function blank(text, spans) {
  let out = "";
  let cursor = 0;
  for (const [from, to] of spans) {
    out += text.slice(cursor, from) + text.slice(from, to).replace(/[^\n]/g, " ");
    cursor = to;
  }
  return out + text.slice(cursor);
}

/**
 * Comma-separated names with their spans. Blanks around a name are not part of
 * its span; empty names are dropped.
 * @param {string} body
 * @param {number} start offset of the body's first character
 * @returns {Piece[]}
 */
function pieces(body, start) {
  /** @type {Piece[]} */
  const out = [];
  let at = start;
  for (const part of body.split(",")) {
    const item = piece(part, at);
    if (item.name) out.push(item);
    at += part.length + 1;
  }
  return out;
}

/**
 * One name with its span.
 * @param {string} part
 * @param {number} start offset of the part's first character
 * @returns {Piece}
 */
function piece(part, start) {
  const name = part.trim();
  const from = start + (part.length - part.trimStart().length);
  return { name, from, to: from + name.length };
}

/**
 * The brace group that is the argument of a command: the first Group or
 * MathGroup after the anchor, skipping blanks, a star, comments and further
 * [optional] arguments. An optional argument may hold commands and groups, as
 * in \cite[see][\S~3]{key}, so inside an open [ any node is skipped. The
 * anchor is the Command node, or the bare ControlSequence inside math.
 * @param {import("@lezer/common").SyntaxNode} anchor
 * @param {string} text
 * @returns {import("@lezer/common").SyntaxNode | null} a closed group, or null
 */
function argumentOf(anchor, text) {
  let depth = 0;
  for (let node = anchor.nextSibling; node; node = node.nextSibling) {
    if (node.from - anchor.to > 200) return null;
    if (node.name === "Comment") continue;
    // Text mode makes the square brackets nodes of their own.
    if (node.name === "OpenBracket") {
      depth++;
    } else if (node.name === "CloseBracket") {
      if (depth === 0) return null;
      depth--;
    } else if (node.name === "Text") {
      const chunk = text.slice(node.from, node.to);
      if (depth > 0 && /\n\s*\n/.test(chunk)) return null;
      for (const char of chunk) {
        if (char === "[") depth++;
        else if (char === "]" && depth > 0) depth--;
        else if (depth === 0 && !/[\s*]/.test(char)) return null;
      }
    } else if (depth === 0) {
      return (node.name === "Group" || node.name === "MathGroup") && node.lastChild?.name === "CloseBrace" ? node : null;
    }
  }
  return null;
}

/**
 * The source between two offsets with every comment inside the group blanked
 * to spaces, so names never include comment text and offsets stay put.
 * @param {string} text
 * @param {number} from
 * @param {number} to
 * @param {import("@lezer/common").SyntaxNode} group
 * @returns {string}
 */
function maskComments(text, from, to, group) {
  let out = "";
  let cursor = from;
  group.cursor().iterate((ref) => {
    if (ref.name !== "Comment" || ref.from < from || ref.to > to) return;
    out += text.slice(cursor, ref.from) + " ".repeat(ref.to - ref.from);
    cursor = ref.to;
  });
  return out + text.slice(cursor, to);
}

/**
 * The name of an environment, from its \begin or \end node.
 * @param {import("@lezer/common").SyntaxNode | null} tag
 * @param {string} text
 * @returns {string | null}
 */
function envName(tag, text) {
  const name = tag?.getChild("EnvName");
  return name ? text.slice(name.from, name.to).trim() : null;
}

/**
 * The span of a closed environment node whose \end names the same
 * environment as its \begin, or null.
 * @param {import("@lezer/common").SyntaxNode} node
 * @param {string} text
 * @returns {Span | null}
 */
function spanOf(node, text) {
  const begin = node.firstChild;
  const end = node.lastChild;
  if (!begin || end?.name !== "EndEnv") return null;
  const name = envName(begin, text);
  if (name === null || name !== envName(end, text)) return null;
  return { name, from: node.from, to: end.to, beginTo: begin.to, endFrom: end.from };
}

/**
 * Index of the first element whose `from` is >= `at` (elements sorted by from).
 * @param {Array<{ from: number }>} list
 * @param {number} at
 * @returns {number}
 */
function firstFrom(list, at) {
  let low = 0;
  let high = list.length;
  while (low < high) {
    const mid = (low + high) >> 1;
    if (list[mid].from < at) low = mid + 1;
    else high = mid;
  }
  return low;
}

/**
 * Items for one LaTeX source.
 * @param {string} text the file's text
 * @param {string} path the file's path, the key for reusing its last parse
 * @returns {IndexItem[]}
 */
export function latexAdapter(text, path) {
  const tree = latexTree(text, path);
  const headings = extractOutline(text, "latex");
  /** @type {IndexItem[]} */
  const items = [];
  /** @type {Array<[number, number]>} */
  const inert = [];
  /** @type {Array<{ from: number }>} */
  const tags = [];
  /** @type {Span[]} */
  const open = [];
  /** @type {Map<Span, string>} */
  const captionOf = new Map();
  /** @type {Array<{ at: number, span: Span | null, item: IndexItem }>} */
  const labels = [];

  for (const heading of headings) {
    items.push({
      kind: "heading",
      range: pair(heading.from, lineEnd(text, heading.from)),
      level: heading.level,
      title: heading.title,
    });
  }

  for (const [from, to] of skipRanges(text, "latex")) {
    items.push({ kind: "markup", range: pair(from, to), attrs: { prose: "skip" } });
  }

  /**
   * Handle one control word and the brace argument that follows it.
   * @param {import("@lezer/common").SyntaxNodeRef} ref
   */
  const command = (ref) => {
    if (ref.to - ref.from > 20) return;
    const word = text.slice(ref.from, ref.to);
    const isCite = CITE.test(word);
    if (!isCite && word !== "\\label" && word !== "\\caption" && !REFS.has(word) && !INCLUDES.has(word)
      && word !== "\\includegraphics" && word !== "\\bibliography" && word !== "\\addbibresource") return;
    const self = ref.node;
    const anchor = self.parent?.name === "Command" ? self.parent : self;
    const group = argumentOf(anchor, text);
    if (!group) return;
    const at = ref.from;
    const bodyFrom = group.from + 1;
    const body = maskComments(text, bodyFrom, group.to - 1, group);
    // A caption keeps nested groups such as \emph{...} in its title.
    if (word !== "\\caption" && (body.includes("{") || body.includes("}"))) return;
    const range = pair(at, group.to);

    if (isCite) {
      for (const key of pieces(body, bodyFrom)) {
        if (key.name === "*") continue;
        items.push({
          kind: "cite",
          name: key.name,
          namespace: "cite",
          range: pair(key.from, key.to),
          nameRange: pair(key.from, key.to),
        });
      }
    } else if (word === "\\label") {
      const name = piece(body, bodyFrom);
      if (!name.name) return;
      labels.push({
        at,
        span: open.at(-1) ?? null,
        item: { kind: "label", name: name.name, range, nameRange: pair(name.from, name.to) },
      });
    } else if (word === "\\caption") {
      const span = open.at(-1);
      const title = body.replace(/\s+/g, " ").trim();
      if (span && title && !captionOf.has(span)) captionOf.set(span, title);
    } else if (REFS.has(word)) {
      for (const key of pieces(body, bodyFrom)) {
        items.push({ kind: "ref", namespace: "label", name: key.name, range, nameRange: pair(key.from, key.to) });
      }
    } else if (word === "\\bibliography") {
      for (const name of pieces(body, bodyFrom)) {
        items.push({ kind: "bibliography", name: name.name, range, nameRange: pair(name.from, name.to) });
      }
    } else {
      const target = piece(body, bodyFrom);
      if (!target.name) return;
      const kind = INCLUDES.has(word) ? "include" : word === "\\includegraphics" ? "asset" : "bibliography";
      items.push({ kind, name: target.name, range, nameRange: pair(target.from, target.to) });
    }
  };

  tree.iterate({
    enter(ref) {
      const name = ref.name;
      if (name === "ControlSequence") {
        command(ref);
      } else if (name === "Comment" || name === "VerbBody" || name === "Verb") {
        inert.push(pair(ref.from, ref.to));
      } else if (name === "EndEnv" || BEGIN_TAGS.has(name)) {
        tags.push({ from: ref.from });
      } else if (name === "InlineMath" || name === "DisplayMath") {
        if (ref.node.lastChild?.name !== "ImplicitClose") items.push({ kind: "math", range: pair(ref.from, ref.to) });
      } else if (ENVIRONMENTS.has(name)) {
        const span = spanOf(ref.node, text);
        if (!span) return;
        open.push(span);
        if (name === "MathEnvironment") items.push({ kind: "math", range: pair(span.from, span.to) });
        // A region is foldable only when its \end is on a later line than its \begin.
        const newline = text.indexOf("\n", span.beginTo);
        if (newline < 0 || span.endFrom <= newline) return;
        items.push({ kind: "block", range: pair(newline, span.endFrom), name: span.name, attrs: { environment: span.name } });
        if (name === "VerbatimEnvironment" && span.name !== "comment") {
          items.push({ kind: "code", range: pair(newline, span.endFrom) });
        }
      }
    },
    leave(ref) {
      if (ENVIRONMENTS.has(ref.name) && open.length && open[open.length - 1].from === ref.from) open.pop();
    },
  });

  /**
   * The title of a label at `at`: the caption in the same innermost
   * environment, else the nearest preceding heading when no \begin or \end
   * lies between the two.
   * @param {number} at
   * @param {Span | null} span
   * @returns {string | undefined}
   */
  const labelTitle = (at, span) => {
    if (span) {
      const caption = captionOf.get(span);
      if (caption !== undefined) return caption;
    }
    const before = firstFrom(headings, at) - 1;
    if (before < 0) return undefined;
    const heading = headings[before];
    const next = tags[firstFrom(tags, heading.from + 1)];
    const crossed = next !== undefined && next.from < at;
    return crossed ? undefined : heading.title;
  };

  for (const { at, span, item } of labels) {
    const title = labelTitle(at, span);
    items.push(title === undefined ? item : { ...item, title });
  }

  const code = blank(text, inert);
  for (const match of code.matchAll(DEFINITION)) {
    const name = match[1] || match[2];
    if (!name) continue;
    const at = match.index ?? 0;
    const nameFrom = at + match[0].lastIndexOf(name);
    items.push({
      kind: "definition",
      name,
      range: pair(at, at + match[0].length),
      nameRange: pair(nameFrom, nameFrom + name.length),
    });
  }

  for (const match of code.matchAll(OPERATOR)) {
    const at = match.index ?? 0;
    const nameFrom = at + match[0].lastIndexOf(match[1]);
    items.push({
      kind: "definition",
      name: match[1],
      range: pair(at, at + match[0].length),
      nameRange: pair(nameFrom, nameFrom + match[1].length),
    });
  }

  for (const match of code.matchAll(DEF)) {
    const at = match.index ?? 0;
    const nameFrom = at + match[0].lastIndexOf(match[1]);
    items.push({
      kind: "definition",
      name: match[1],
      range: pair(at, at + match[0].length),
      nameRange: pair(nameFrom, nameFrom + match[1].length),
    });
  }

  for (const match of code.matchAll(ENVIRONMENT_DEFINITION)) {
    const at = match.index ?? 0;
    const nameFrom = at + match[0].lastIndexOf(match[1]);
    items.push({
      kind: "definition",
      name: match[1],
      range: pair(at, at + match[0].length),
      nameRange: pair(nameFrom, nameFrom + match[1].length),
    });
  }

  return items;
}
