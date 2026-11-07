// The typst adapter of the document index. It reads one typst source with the
// existing scanners (the outline, the spellcheck skips) and the masks in
// citation-keys.js, and maps what they find to IndexItems. Synchronous and pure:
// the result depends only on the text. `path` is part of the adapter signature;
// the adapter reads the text alone.

import { extractOutline } from "../../outline.js";
import { skipRanges } from "../../spellcheck/index.js";
import {
  LABEL,
  TYPST_AT,
  TYPST_CITE,
  atKeys,
  typstCodeSpans,
  typstMasked,
} from "../../citation-keys.js";

/** @typedef {import("../schema.js").IndexItem} IndexItem */
/** @typedef {[number, number]} Span */
/** @typedef {{ open: number, close: number }} Args */
/**
 * A closed `( ... )`: where it ends (exclusive), whether its line has a `#`,
 * whether it is a `#figure(` call, and its argument brackets.
 * @typedef {{ end: number, hash: boolean, figure: boolean, args: Args }} ParenInfo
 */
/**
 * An open bracket on the stack.
 * @typedef {{ ch: string, pos: number, hash: boolean, figure: boolean, parenOf: ParenInfo | null }} Opener
 */

const OPENER_OF = { ")": "(", "]": "[", "}": "{" };
const REF_CALL = /#ref\(\s*<([-\p{L}\p{N}_:.]+)>/gu;
const CITE_LABEL_CALL = /#cite\(\s*label\(\s*"([^"\n]+)"\s*\)/gu;
const INCLUDE = /#include\s*\(?\s*"([^"\n]*)"/gu;
const IMAGE = /(?<![\p{L}\p{N}_.-])#?image\(\s*"([^"\n]*)"/gu;
const BIBLIOGRAPHY = /(?<![\p{L}\p{N}_.-])#?bibliography\(([^)]*)\)/gu;
const DEFINITION = /^([ \t]*)#let[ \t]+([\p{L}_][\p{L}\p{N}_-]*)(\()?/gmu;
const CLOSE_PAREN = /\s*\)/y;

/**
 * The typst items of one source. Items are not sorted.
 * @param {string} text
 * @param {string} path
 * @returns {IndexItem[]}
 */
export function typstAdapter(text, path) {
  void path;
  const source = String(text ?? "");
  const masked = typstMasked(source);
  const math = mathSpans(masked);
  const headings = headingItems(source);
  const { blocks, figureEnds } = scanBrackets(masked, math);

  /** @type {IndexItem[]} */
  const items = [...headings];

  for (const range of skipRanges(source, "typst")) {
    items.push({ kind: "markup", range, attrs: { prose: "skip" } });
  }
  for (const range of typstCodeSpans(source)) items.push({ kind: "code", range });
  for (const range of math) items.push({ kind: "math", range });
  for (const range of blocks) items.push({ kind: "block", range });

  for (const { key, from, to } of atKeys(masked, TYPST_AT)) {
    if (spanIn(math, from - 1)) continue;
    items.push({ kind: "ref", range: [from - 1, to], name: key, nameRange: [from, to], namespace: "either" });
  }
  for (const match of masked.matchAll(REF_CALL)) {
    const nameFrom = match.index + match[0].indexOf("<") + 1;
    items.push({
      kind: "ref",
      range: [match.index, closeCall(masked, match.index + match[0].length)],
      name: match[1],
      nameRange: [nameFrom, nameFrom + match[1].length],
      namespace: "label",
    });
  }

  for (const match of masked.matchAll(TYPST_CITE)) {
    const nameFrom = match.index + match[0].indexOf("<") + 1;
    items.push(citeItem(masked, match.index, match[0].length, match[1], nameFrom));
  }
  for (const match of masked.matchAll(CITE_LABEL_CALL)) {
    const nameFrom = match.index + match[0].indexOf('"') + 1;
    items.push(citeItem(masked, match.index, match[0].length, match[1], nameFrom));
  }

  for (const match of masked.matchAll(INCLUDE)) {
    const nameFrom = match.index + match[0].indexOf('"') + 1;
    items.push(quotedItem("include", match.index, match[0].length, match[1], nameFrom));
  }
  for (const match of masked.matchAll(IMAGE)) {
    const nameFrom = match.index + match[0].indexOf('"') + 1;
    items.push(quotedItem("asset", match.index, match[0].length, match[1], nameFrom));
  }
  for (const match of masked.matchAll(BIBLIOGRAPHY)) {
    const groupFrom = match.index + match[0].indexOf("(") + 1;
    for (const literal of match[1].matchAll(/"([^"\n]*)"/g)) {
      const at = groupFrom + literal.index;
      items.push(quotedItem("bibliography", at, literal[0].length, literal[1], at + 1));
    }
  }

  for (const match of masked.matchAll(DEFINITION)) {
    const from = match.index + match[1].length;
    const nameFrom = match.index + match[0].length - (match[3] ? 1 : 0) - match[2].length;
    const nameTo = nameFrom + match[2].length;
    items.push({
      kind: "definition",
      range: [from, nameTo],
      name: match[2],
      nameRange: [nameFrom, nameTo],
      attrs: { type: match[3] ? "function" : "variable" },
    });
  }

  for (const match of masked.matchAll(LABEL)) {
    const from = match.index;
    const to = from + match[0].length;
    /** @type {IndexItem} */
    const item = {
      kind: "label",
      range: [from, to],
      name: match[1],
      nameRange: [from + 1, from + 1 + match[1].length],
    };
    const title = headingTitleAt(source, headings, from, to, match[1]) ?? captionBefore(source, masked, figureEnds, from);
    if (title) item.title = title;
    items.push(item);
  }

  return items;
}

/**
 * Headings from the outline scanner. A heading runs to the end of its line.
 * @param {string} source
 * @returns {IndexItem[]}
 */
function headingItems(source) {
  return extractOutline(source, "typst").map(({ title, level, from }) => {
    let end = from;
    while (end < source.length && source[end] !== "\n" && source[end] !== "\r") end++;
    return { kind: "heading", range: [from, end], level, title };
  });
}

/**
 * Spans of `$...$` math in the masked text, paired in order. An unpaired `$`
 * gives no span. `\$` is not a delimiter.
 * @param {string} masked
 * @returns {Span[]}
 */
function mathSpans(masked) {
  /** @type {Span[]} */
  const spans = [];
  let open = -1;
  for (let i = 0; i < masked.length; i++) {
    if (masked[i] !== "$" || escaped(masked, i)) continue;
    if (open < 0) open = i;
    else {
      spans.push([open, i + 1]);
      open = -1;
    }
  }
  return spans;
}

/**
 * Whether the character at `at` is preceded by an odd number of backslashes.
 * @param {string} text
 * @param {number} at
 * @returns {boolean}
 */
function escaped(text, at) {
  let count = 0;
  while (at - 1 - count >= 0 && text[at - 1 - count] === "\\") count++;
  return count % 2 === 1;
}

/**
 * @param {Span[]} spans
 * @param {number} at
 * @returns {boolean}
 */
function spanIn(spans, at) {
  return spans.some(([from, to]) => at >= from && at < to);
}

/**
 * Whether the line holding `at` has a `#` before `at`.
 * @param {string} text
 * @param {number} at
 * @returns {boolean}
 */
function lineHasHash(text, at) {
  const start = text.lastIndexOf("\n", at - 1) + 1;
  return text.slice(start, at).includes("#");
}

/**
 * The index of the quote that closes the string opened at `open`, on the same
 * line, or -1. Escapes are skipped.
 * @param {string} text
 * @param {number} open
 * @returns {number}
 */
function closingQuote(text, open) {
  for (let j = open + 1; j < text.length && text[j] !== "\n"; j++) {
    if (text[j] === "\\") j++;
    else if (text[j] === '"') return j;
  }
  return -1;
}

/**
 * Walk the masked text once, matching brackets. Brackets in math and strings
 * are skipped. A `[` or `{` is a block when its statement starts with `#`: its
 * own line has a `#`, or it directly follows the `)` of a call whose line has
 * one (`#figure(` ... `)[`). Only blocks that span lines are returned.
 * `figureEnds` maps the offset just after a `)` or `]` that ends a `#figure`
 * call (or its content block) to that call's argument brackets.
 * @param {string} masked
 * @param {Span[]} math
 * @returns {{ blocks: Span[], figureEnds: Map<number, Args> }}
 */
function scanBrackets(masked, math) {
  const mathAt = new Map(math.map(([from, to]) => [from, to]));
  /** @type {Opener[]} */
  const stack = [];
  /** @type {Span[]} */
  const blocks = [];
  /** @type {Map<number, Args>} */
  const figureEnds = new Map();
  /** @type {ParenInfo | null} */
  let lastParen = null;

  for (let i = 0; i < masked.length; i++) {
    const mathEnd = mathAt.get(i);
    if (mathEnd !== undefined) {
      i = mathEnd - 1;
      continue;
    }
    const ch = masked[i];
    if (ch === '"') {
      const close = closingQuote(masked, i);
      if (close >= 0) i = close;
      continue;
    }
    if (ch === "(" || ch === "[" || ch === "{") {
      const parenOf = ch === "[" && lastParen && lastParen.end === i ? lastParen : null;
      stack.push({
        ch,
        pos: i,
        hash: parenOf ? parenOf.hash : lineHasHash(masked, i),
        figure: ch === "(" && /#figure\s*$/.test(masked.slice(Math.max(0, i - 40), i)),
        parenOf,
      });
      continue;
    }
    if (ch !== ")" && ch !== "]" && ch !== "}") continue;
    const top = stack[stack.length - 1];
    if (!top || top.ch !== OPENER_OF[ch]) continue;
    stack.pop();
    if (ch === ")") {
      lastParen = { end: i + 1, hash: top.hash, figure: top.figure, args: { open: top.pos, close: i } };
      if (top.figure) figureEnds.set(i + 1, lastParen.args);
      continue;
    }
    if (top.hash && masked.slice(top.pos, i).includes("\n")) blocks.push([top.pos + 1, i]);
    if (ch === "]" && top.parenOf?.figure) figureEnds.set(i + 1, top.parenOf.args);
  }
  return { blocks, figureEnds };
}

/**
 * The end of a call that may be closed by `)` after `end`, when one follows
 * (with only whitespace between). Otherwise `end`.
 * @param {string} masked
 * @param {number} end
 * @returns {number}
 */
function closeCall(masked, end) {
  CLOSE_PAREN.lastIndex = end;
  return CLOSE_PAREN.exec(masked) ? CLOSE_PAREN.lastIndex : end;
}

/**
 * A `#cite` item. Its range runs to the closing paren of the call.
 * @param {string} masked
 * @param {number} from
 * @param {number} length
 * @param {string} key
 * @param {number} nameFrom
 * @returns {IndexItem}
 */
function citeItem(masked, from, length, key, nameFrom) {
  return {
    kind: "cite",
    range: [from, closeCall(masked, from + length)],
    name: key,
    nameRange: [nameFrom, nameFrom + key.length],
    namespace: "cite",
  };
}

/**
 * A path-carrying item (include, asset, bibliography). Its range is the call
 * head or literal up to the closing quote.
 * @param {"include" | "asset" | "bibliography"} kind
 * @param {number} from
 * @param {number} length
 * @param {string} path
 * @param {number} nameFrom
 * @returns {IndexItem}
 */
function quotedItem(kind, from, length, path, nameFrom) {
  return { kind, range: [from, from + length], name: path, nameRange: [nameFrom, nameFrom + path.length] };
}

/**
 * The heading title when the label ends a heading line, without the label.
 * @param {string} source
 * @param {IndexItem[]} headings
 * @param {number} from
 * @param {number} to
 * @param {string} name
 * @returns {string | null}
 */
function headingTitleAt(source, headings, from, to, name) {
  const heading = headings.find((item) => item.range[0] <= from && from < item.range[1]);
  if (!heading || source.slice(to, heading.range[1]).trim() !== "") return null;
  const title = (heading.title ?? "").split(`<${name}>`).join("").replace(/\s+/g, " ").trim();
  return title || null;
}

/**
 * The caption of the `#figure` the label follows, when the label directly
 * follows a figure call (or its content block).
 * @param {string} source
 * @param {string} masked
 * @param {Map<number, Args>} figureEnds
 * @param {number} labelFrom
 * @returns {string | null}
 */
function captionBefore(source, masked, figureEnds, labelFrom) {
  let end = labelFrom;
  while (end > 0 && /\s/.test(masked[end - 1])) end--;
  const args = figureEnds.get(end);
  if (!args) return null;
  const inner = masked.slice(args.open + 1, args.close);
  const match = /\bcaption\s*:\s*\[/.exec(inner);
  if (!match) return null;
  const open = args.open + 1 + match.index + match[0].length - 1;
  const close = matchingBracket(masked, open);
  if (close < 0) return null;
  return source.slice(open + 1, close).replace(/\s+/g, " ").trim() || null;
}

/**
 * The index of the `]` matching the `[` at `open`, or -1.
 * @param {string} text
 * @param {number} open
 * @returns {number}
 */
function matchingBracket(text, open) {
  let depth = 0;
  for (let i = open; i < text.length; i++) {
    if (text[i] === "[") depth++;
    else if (text[i] === "]" && --depth === 0) return i;
  }
  return -1;
}
