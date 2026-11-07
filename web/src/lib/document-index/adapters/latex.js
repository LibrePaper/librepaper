// The LaTeX adapter of the document index. It turns one .tex source into
// IndexItems. Headings come from outline.js, citations from citation-keys.js
// and prose skips from spellcheck/latex.js; everything else is scanned here.
// Comments and the bodies of verbatim-like environments are blanked first, so
// no command inside them is read. Math is not blanked: a \label inside an
// equation counts.

import { extractOutline } from "../../outline.js";
import { citedKeys } from "../../citation-keys.js";
import { skipRanges } from "../../spellcheck/index.js";

/** @typedef {import("../schema.js").IndexItem} IndexItem */
/** @typedef {{ name: string, from: number, to: number }} Piece */
/** @typedef {{ kind: "begin" | "end", name: string, from: number, to: number }} Tag */
/** @typedef {{ name: string, from: number, to: number, beginTo: number, endFrom: number }} Span */
/** @typedef {{ at: number, title: string }} Caption */

// Environments whose body is blanked: nothing inside them is read as LaTeX.
const BLANKED = new Set(["verbatim", "verbatim*", "lstlisting", "minted", "comment"]);
// The verbatim-like environments, which also emit a code item.
const CODE = new Set(["verbatim", "verbatim*", "lstlisting", "minted"]);
const MATH = new Set([
  "equation", "equation*", "align", "align*", "gather", "gather*",
  "multline", "multline*", "displaymath", "math", "eqnarray",
]);

const BEGIN_AT = /\\begin\{([^{}\n]*)\}/y;
const VERB_AT = /\\verb\*?([^A-Za-z\s])/y;
const BLANK_LINE_AT = /\n[ \t]*\n/y;
const TAG = /\\(begin|end)\{([^{}\n]*)\}/g;
const CAPTION = /\\caption\*?(?![A-Za-z@])/g;
const LABEL = /\\label\{([^{}]+)\}/g;
const REF = /\\(?:ref|eqref|pageref|autoref|cref|Cref|nameref|vref|cpageref|labelcref)\*?\{([^{}]*)\}/g;
const INCLUDE = /\\(input|include|subfile)\{([^{}]*)\}/g;
const GRAPHIC = /\\includegraphics(?:\[[^\]]*\])?\{([^{}]*)\}/g;
const BIBLIOGRAPHY = /\\bibliography\{([^{}]*)\}/g;
const ADD_BIBRESOURCE = /\\addbibresource(?:\[[^\]]*\])?\{([^{}]*)\}/g;
const DEFINITION = /\\(?:newcommand|renewcommand|providecommand)\*?\s*(?:\{\s*\\([A-Za-z@]+)\s*\}|\\([A-Za-z@]+))/g;
const OPERATOR = /\\DeclareMathOperator\*?\s*\{\s*\\([A-Za-z@]+)\s*\}/g;
const DEF = /\\def\\([A-Za-z@]+)/g;
const ENVIRONMENT_DEFINITION = /\\(?:newenvironment|renewenvironment)\*?\s*\{\s*([A-Za-z][A-Za-z0-9*]*)\s*\}/g;

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
 * The source with every comment, every verbatim-like body and every \verb span
 * blanked. A % after an even number of backslashes starts a comment; the scan
 * consumes each backslash with the character after it, so an escaped % is
 * never a comment.
 * @param {string} text
 * @returns {string}
 */
function blankInert(text) {
  /** @type {Array<[number, number]>} */
  const spans = [];
  const n = text.length;
  let i = 0;
  while (i < n) {
    const ch = text[i];
    if (ch === "%") {
      const eol = lineEnd(text, i);
      spans.push(pair(i, eol));
      i = eol;
      continue;
    }
    if (ch !== "\\") {
      i++;
      continue;
    }
    BEGIN_AT.lastIndex = i;
    const begin = BEGIN_AT.exec(text);
    if (begin && BLANKED.has(begin[1].trim())) {
      const name = begin[1].trim();
      const from = i + begin[0].length;
      const close = text.indexOf(`\\end{${name}}`, from);
      const to = close < 0 ? n : close;
      spans.push(pair(from, to));
      i = to;
      continue;
    }
    VERB_AT.lastIndex = i;
    const verb = VERB_AT.exec(text);
    if (verb) {
      const close = text.indexOf(verb[1], i + verb[0].length);
      if (close >= 0 && close < lineEnd(text, i)) {
        spans.push(pair(i, close + 1));
        i = close + 1;
        continue;
      }
    }
    i += 2;
  }
  return blank(text, spans);
}

/**
 * Every \begin and \end tag in the blanked source, in order.
 * @param {string} code
 * @returns {Tag[]}
 */
function tagsIn(code) {
  return [...code.matchAll(TAG)].map((match) => {
    const from = match.index ?? 0;
    return {
      kind: /** @type {"begin" | "end"} */ (match[1]),
      name: match[2].trim(),
      from,
      to: from + match[0].length,
    };
  });
}

/**
 * Pair each \end with the nearest open \begin of the same name. Unpaired tags
 * are ignored.
 * @param {Tag[]} tags
 * @returns {Span[]}
 */
function pairTags(tags) {
  /** @type {Tag[]} */
  const open = [];
  /** @type {Span[]} */
  const spans = [];
  for (const tag of tags) {
    if (tag.kind === "begin") {
      open.push(tag);
      continue;
    }
    for (let i = open.length - 1; i >= 0; i--) {
      if (open[i].name !== tag.name) continue;
      const begin = open.splice(i, 1)[0];
      spans.push({ name: tag.name, from: begin.from, to: tag.to, beginTo: begin.to, endFrom: tag.from });
      break;
    }
  }
  return spans;
}

/**
 * The innermost environment that holds an offset, or null.
 * @param {Span[]} spans
 * @param {number} at
 * @returns {Span | null}
 */
function innermost(spans, at) {
  /** @type {Span | null} */
  let best = null;
  for (const span of spans) {
    if (span.from < at && at < span.to && (best === null || span.from > best.from)) best = span;
  }
  return best;
}

/**
 * The offset just past the whitespace at `at`.
 * @param {string} code
 * @param {number} at
 * @returns {number}
 */
function skipSpace(code, at) {
  let k = at;
  while (k < code.length && /\s/.test(code[k])) k++;
  return k;
}

/**
 * The offset of the } that closes the { at `open`, or -1.
 * @param {string} code
 * @param {number} open
 * @returns {number}
 */
function braceEnd(code, open) {
  let depth = 0;
  for (let k = open; k < code.length; k++) {
    const ch = code[k];
    if (ch === "\\") {
      k++;
    } else if (ch === "{") {
      depth++;
    } else if (ch === "}" && --depth === 0) {
      return k;
    }
  }
  return -1;
}

/**
 * Every \caption with a non-empty title: its offset, and the first brace
 * argument after an optional [short] argument, whitespace collapsed.
 * @param {string} code
 * @returns {Caption[]}
 */
function captionsIn(code) {
  /** @type {Caption[]} */
  const out = [];
  for (const match of code.matchAll(CAPTION)) {
    const at = match.index ?? 0;
    let k = skipSpace(code, at + match[0].length);
    if (code[k] === "[") {
      const close = code.indexOf("]", k);
      if (close < 0) continue;
      k = skipSpace(code, close + 1);
    }
    if (code[k] !== "{") continue;
    const close = braceEnd(code, k);
    if (close < 0) continue;
    const title = code.slice(k + 1, close).replace(/\s+/g, " ").trim();
    if (title) out.push({ at, title });
  }
  return out;
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
 * The offset just past the first { of a match, which opens its brace argument.
 * @param {RegExpMatchArray} match
 * @returns {number}
 */
function argumentStart(match) {
  return (match.index ?? 0) + match[0].lastIndexOf("{") + 1;
}

/**
 * The index of the first `closer` at or after `from`, skipping escaped
 * characters, or -1. An inline run stops at a blank line.
 * @param {string} code
 * @param {number} from
 * @param {string} closer
 * @param {boolean} inline
 * @returns {number}
 */
function scanTo(code, from, closer, inline) {
  for (let k = from; k < code.length; k++) {
    if (code.startsWith(closer, k)) return k;
    if (inline && code[k] === "\n") {
      BLANK_LINE_AT.lastIndex = k;
      if (BLANK_LINE_AT.test(code)) return -1;
    }
    if (code[k] === "\\") k++;
  }
  return -1;
}

/**
 * The math runs in the blanked source: $...$, $$...$$ and \[...\]. An escaped
 * dollar is not a delimiter.
 * @param {string} code
 * @returns {Array<[number, number]>}
 */
function mathRuns(code) {
  /** @type {Array<[number, number]>} */
  const out = [];
  let i = 0;
  while (i < code.length) {
    const ch = code[i];
    if (ch === "\\") {
      if (code[i + 1] === "[") {
        const close = scanTo(code, i + 2, "\\]", false);
        if (close >= 0) {
          out.push(pair(i, close + 2));
          i = close + 2;
          continue;
        }
      }
      i += 2;
      continue;
    }
    if (ch === "$") {
      const delimiter = code[i + 1] === "$" ? "$$" : "$";
      const start = i + delimiter.length;
      const close = scanTo(code, start, delimiter, delimiter === "$");
      if (close >= 0) {
        out.push(pair(i, close + delimiter.length));
        i = close + delimiter.length;
        continue;
      }
      i = start;
      continue;
    }
    i++;
  }
  return out;
}

/**
 * Items for one LaTeX source.
 * @param {string} text the file's text
 * @param {string} _path the file's path; the scan reads only the text
 * @returns {IndexItem[]}
 */
export function latexAdapter(text, _path) {
  const code = blankInert(text);
  const tags = tagsIn(code);
  const spans = pairTags(tags);
  const captions = captionsIn(code);
  const headings = extractOutline(text, "latex");
  /** @type {IndexItem[]} */
  const items = [];

  for (const heading of headings) {
    items.push({
      kind: "heading",
      range: pair(heading.from, lineEnd(text, heading.from)),
      level: heading.level,
      title: heading.title,
    });
  }

  for (const cite of citedKeys(text, "latex")) {
    items.push({
      kind: "cite",
      name: cite.key,
      namespace: "cite",
      range: pair(cite.from, cite.to),
      nameRange: pair(cite.from, cite.to),
    });
  }

  for (const [from, to] of skipRanges(text, "latex")) {
    items.push({ kind: "markup", range: pair(from, to), attrs: { prose: "skip" } });
  }

  // Each caption's environment is found once, keeping the first caption per
  // environment. Headings and tags are in source order, so the nearest heading
  // and the first tag after it are found by binary search.
  /** @type {Map<Span | null, string>} */
  const captionOf = new Map();
  for (const caption of captions) {
    const span = innermost(spans, caption.at);
    if (!captionOf.has(span)) captionOf.set(span, caption.title);
  }
  /**
   * Index of the first element whose `from` is >= `at` (elements sorted by from).
   * @param {Array<{ from: number }>} list
   * @param {number} at
   * @returns {number}
   */
  const firstFrom = (list, at) => {
    let low = 0;
    let high = list.length;
    while (low < high) {
      const mid = (low + high) >> 1;
      if (list[mid].from < at) low = mid + 1;
      else high = mid;
    }
    return low;
  };
  /**
   * The title of a label at `at`: the caption in the same innermost
   * environment, else the nearest preceding heading when no \begin or \end
   * lies between the two.
   * @param {number} at
   * @returns {string | undefined}
   */
  const labelTitle = (at) => {
    const span = innermost(spans, at);
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

  for (const match of code.matchAll(LABEL)) {
    const at = match.index ?? 0;
    const name = match[1];
    const nameFrom = argumentStart(match);
    const title = labelTitle(at);
    items.push({
      kind: "label",
      name,
      range: pair(at, at + match[0].length),
      nameRange: pair(nameFrom, nameFrom + name.length),
      ...(title === undefined ? {} : { title }),
    });
  }

  for (const match of code.matchAll(REF)) {
    const at = match.index ?? 0;
    for (const key of pieces(match[1], argumentStart(match))) {
      items.push({
        kind: "ref",
        namespace: "label",
        name: key.name,
        range: pair(at, at + match[0].length),
        nameRange: pair(key.from, key.to),
      });
    }
  }

  for (const match of code.matchAll(INCLUDE)) {
    const at = match.index ?? 0;
    const target = piece(match[2], argumentStart(match));
    if (!target.name) continue;
    items.push({
      kind: "include",
      name: target.name,
      range: pair(at, at + match[0].length),
      nameRange: pair(target.from, target.to),
    });
  }

  for (const match of code.matchAll(GRAPHIC)) {
    const at = match.index ?? 0;
    const target = piece(match[1], argumentStart(match));
    if (!target.name) continue;
    items.push({
      kind: "asset",
      name: target.name,
      range: pair(at, at + match[0].length),
      nameRange: pair(target.from, target.to),
    });
  }

  for (const match of code.matchAll(BIBLIOGRAPHY)) {
    const at = match.index ?? 0;
    for (const name of pieces(match[1], argumentStart(match))) {
      items.push({
        kind: "bibliography",
        name: name.name,
        range: pair(at, at + match[0].length),
        nameRange: pair(name.from, name.to),
      });
    }
  }

  for (const match of code.matchAll(ADD_BIBRESOURCE)) {
    const at = match.index ?? 0;
    const target = piece(match[1], argumentStart(match));
    if (!target.name) continue;
    items.push({
      kind: "bibliography",
      name: target.name,
      range: pair(at, at + match[0].length),
      nameRange: pair(target.from, target.to),
    });
  }

  for (const span of spans) {
    if (MATH.has(span.name)) items.push({ kind: "math", range: pair(span.from, span.to) });
    // A region is foldable only when its \end is on a later line than its \begin.
    const newline = code.indexOf("\n", span.beginTo);
    if (newline < 0 || span.endFrom <= newline) continue;
    const body = pair(newline, span.endFrom);
    items.push({ kind: "block", range: body, name: span.name, attrs: { environment: span.name } });
    if (CODE.has(span.name)) items.push({ kind: "code", range: pair(newline, span.endFrom) });
  }

  for (const [from, to] of mathRuns(code)) items.push({ kind: "math", range: pair(from, to) });

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
