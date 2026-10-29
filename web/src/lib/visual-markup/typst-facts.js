// The facts of a Typst project that the visual layer reads, in the shape of
// documentFacts (facts.js): the numbers of headings, figures and display
// equations, the labels bound to them, the citations and the citation style.
// It is one pass over each file in include order. The pass reads a copy of each
// source in which comments, raw text and math are blanked, so offsets still
// point into the source.

import { rankEntries } from "../bibliography.js";
import { citationFacts } from "./facts.js";

const NAME = String.raw`[\p{L}\p{N}_][\p{L}\p{N}_:.-]*`;

// A label definition, <name>, that is not the argument of a cite or a ref.
const LABEL_DEFINITION = new RegExp(String.raw`(?<!#(?:cite|ref)\(\s*)<(${NAME})>`, "gu");
// A label at the end of a heading line, and a label right after an element.
const LABEL_END = new RegExp(String.raw`<(${NAME})>\s*$`, "u");
const LABEL_AT = new RegExp(String.raw`<(${NAME})>`, "uy");

const HEADING_LINE = /^[ \t]*(=+)[ \t]+(.*)$/gm;
const HEADING_CALL = /#heading(?![\w.-])/g;
const FIGURE_CALL = /#figure(?![\w.-])/g;
const SET_RULE = /#set\s+(heading|figure|math\.equation|cite)(?![\w.-])\s*\(/g;
const BIBLIOGRAPHY = /#bibliography(?![\w.-])\s*\(/g;
const CITE = new RegExp(String.raw`#cite\(\s*<(${NAME})>`, "gu");
// A reference @key, but not one inside a word, an e-mail address or a URL.
const AT_KEY = new RegExp(String.raw`(?<![\p{L}\p{N}_@/])@([\p{L}\p{N}_](?:[\p{L}\p{N}_:-]|\.(?=[\p{L}\p{N}_]))*)`, "gu");
const INCLUDE = /#include\s*"([^"]*)"/g;

const TICK_RUN = /`+/g;
const TICK_START = /`+/y;

// Counting symbols of a numbering pattern. Other characters are literal.
const SYMBOLS = "1aAiI";
const TRIM = /^[^\p{L}\p{N}]+|[^\p{L}\p{N}]+$/gu;
const NAMED_KIND = /(?<![\w.-])kind\s*:\s*(?:"([^"]*)"|([\p{L}_][\p{L}\p{N}_.-]*))/u;
const KIND_NAMES = new Map([
  ["image", "figure"],
  ["table", "table"],
  ["raw", "listing"],
]);

// Styles that cite as author and year. The others, the numeric ones, are the
// default, as Typst's own default style (IEEE) is numeric.
const AUTHOR_YEAR = /^(?:apa|american-psychological|chicago|harvard|mla|modern-language|elsevier-harvard|council-of-science|american-sociological)|author-(?:date|year)/;

// Joins and resolves . and .. so that paths compare as project paths.
function normalize(path) {
  const parts = [];
  for (const part of path.split("/")) {
    if (part === "" || part === ".") continue;
    if (part === "..") parts.pop();
    else parts.push(part);
  }
  return parts.join("/");
}

function folderOf(path) {
  return path.slice(0, path.lastIndexOf("/") + 1);
}

// Blanks comments, raw text and math, keeping newlines and offsets. Display
// math (its content starts and ends with whitespace) is also listed, with the
// offsets of its dollar signs, so that a label after it can bind to it.
function maskTypst(source) {
  const chars = source.split("");
  const maths = [];
  const blank = (from, to) => {
    for (let k = from; k < to; k++) if (chars[k] !== "\n") chars[k] = " ";
  };
  let i = 0;
  while (i < source.length) {
    const c = source[i];
    if (c === "\\") {
      i += 2;
    } else if (c === "/" && source[i + 1] === "/" && source[i - 1] !== ":") {
      // A // after a colon is the // of a URL, not a comment.
      const eol = source.indexOf("\n", i);
      const end = eol === -1 ? source.length : eol;
      blank(i, end);
      i = end;
    } else if (c === "/" && source[i + 1] === "*") {
      let depth = 0;
      let j = i;
      while (j < source.length) {
        if (source.startsWith("/*", j)) {
          depth += 1;
          j += 2;
        } else if (source.startsWith("*/", j)) {
          depth -= 1;
          j += 2;
          if (depth === 0) break;
        } else {
          j += 1;
        }
      }
      blank(i, j);
      i = j;
    } else if (c === "`") {
      TICK_START.lastIndex = i;
      const run = TICK_START.exec(source)[0].length;
      let end = -1;
      TICK_RUN.lastIndex = i + run;
      for (let match; (match = TICK_RUN.exec(source)); ) {
        if (match[0].length === run) {
          end = match.index + run;
          break;
        }
      }
      if (end === -1) {
        i += run;
      } else {
        blank(i, end);
        i = end;
      }
    } else if (c === "$") {
      let j = i + 1;
      while (j < source.length && source[j] !== "$") j += source[j] === "\\" ? 2 : 1;
      if (j >= source.length) {
        i += 1;
      } else {
        const content = source.slice(i + 1, j);
        maths.push({
          start: i,
          end: j + 1,
          display: content.length > 1 && /^\s/.test(content) && /\s$/.test(content),
        });
        blank(i, j + 1);
        i = j + 1;
      }
    } else {
      i += 1;
    }
  }
  return { text: chars.join(""), maths };
}

// The index after the bracket that closes the one at `open`, or -1. A string
// is skipped whole, so a bracket inside it does not count.
function closeOf(text, open) {
  let depth = 0;
  for (let i = open; i < text.length; i++) {
    const c = text[i];
    if (c === '"') {
      i += 1;
      while (i < text.length && text[i] !== '"') i += text[i] === "\\" ? 2 : 1;
    } else if ("([{".includes(c)) {
      depth += 1;
    } else if (")]}".includes(c) && --depth === 0) {
      return i + 1;
    }
  }
  return -1;
}

// The parenthesized arguments at `at` (null when there are none) and the
// content blocks that follow them directly, with the index after the last one.
function parseCall(text, at) {
  let i = at;
  let args = null;
  if (text[i] === "(") {
    const close = closeOf(text, i);
    if (close < 0) return null;
    args = text.slice(i + 1, close - 1);
    i = close;
  }
  const bodies = [];
  while (text[i] === "[") {
    const close = closeOf(text, i);
    if (close < 0) break;
    bodies.push(text.slice(i + 1, close - 1));
    i = close;
  }
  return { args, bodies, end: i };
}

function isCall(call) {
  return call !== null && (call.args !== null || call.bodies.length > 0);
}

// A label that follows `at` after spaces or tabs only, as in "#figure(...) <fig>".
function labelAfter(text, at) {
  let i = at;
  while (text[i] === " " || text[i] === "\t") i += 1;
  LABEL_AT.lastIndex = i;
  return LABEL_AT.exec(text)?.[1] ?? null;
}

// The value of `numbering:` in an argument list. Undefined when the argument is
// absent, null for `none` or for a function (which is not read), else the pattern.
function numberingArg(args) {
  const match = /(?<![\w.-])numbering\s*:\s*("([^"]*)"|[^,)]*)/.exec(args ?? "");
  if (!match) return undefined;
  return match[2] ?? null;
}

function styleArg(args) {
  return /(?<![\w.-])style\s*:\s*"([^"]*)"/.exec(args ?? "")?.[1] ?? null;
}

function levelArg(args) {
  const match = /(?<![\w.-])level\s*:\s*(\d+)/.exec(args ?? "");
  return match ? Math.min(6, Math.max(1, Number(match[1]))) : 1;
}

// The kind a figure takes from its first argument or its body: an image, a
// table or raw text. Null when neither shows one of these.
function autoKind(source) {
  const head = source.trimStart();
  if (/^image\s*\(/.test(head)) return "image";
  if (/^(?:table|grid)\s*\(/.test(head)) return "table";
  if (/^raw\s*\(/.test(head) || /^\[?\s*`/.test(head)) return "raw";
  return null;
}

// The kind of a figure: a `kind:` argument wins, then the shape of its content.
function figureKind(call) {
  const named = NAMED_KIND.exec(call.args ?? "");
  if (named) {
    const value = named[1] ?? named[2];
    return KIND_NAMES.get(value) ?? value;
  }
  const kind = autoKind(call.args ?? "") ?? autoKind(call.bodies[0] ?? "");
  return KIND_NAMES.get(kind) ?? "figure";
}

// The elements of one file that the numbering and the citations read, in
// document order. Labels of headings, figures and equations are bound here.
function eventsOf(text, maths) {
  const events = [];
  for (const match of text.matchAll(HEADING_LINE)) {
    events.push({
      at: match.index,
      type: "heading",
      level: Math.min(match[1].length, 6),
      numbering: undefined,
      label: LABEL_END.exec(match[2])?.[1] ?? null,
    });
  }
  for (const match of text.matchAll(HEADING_CALL)) {
    const call = parseCall(text, match.index + match[0].length);
    if (!isCall(call)) continue;
    events.push({
      at: match.index,
      type: "heading",
      level: levelArg(call.args),
      numbering: numberingArg(call.args),
      label: labelAfter(text, call.end),
    });
  }
  for (const match of text.matchAll(FIGURE_CALL)) {
    const call = parseCall(text, match.index + match[0].length);
    if (!isCall(call)) continue;
    events.push({
      at: match.index,
      type: "figure",
      kind: figureKind(call),
      numbering: numberingArg(call.args),
      label: labelAfter(text, call.end),
    });
  }
  for (const match of text.matchAll(SET_RULE)) {
    const call = parseCall(text, match.index + match[0].length - 1);
    events.push({ at: match.index, type: "set", target: match[1], args: call?.args ?? "" });
  }
  for (const match of text.matchAll(BIBLIOGRAPHY)) {
    const call = parseCall(text, match.index + match[0].length - 1);
    events.push({ at: match.index, type: "bibliography", args: call?.args ?? "" });
  }
  for (const match of text.matchAll(CITE)) events.push({ at: match.index, type: "cite", key: match[1] });
  for (const match of text.matchAll(AT_KEY)) events.push({ at: match.index, type: "ref", key: match[1] });
  for (const match of text.matchAll(INCLUDE)) events.push({ at: match.index, type: "include", path: match[1] });
  for (const math of maths) {
    if (math.display) events.push({ at: math.start, type: "equation", label: labelAfter(text, math.end) });
  }
  events.sort((a, b) => a.at - b.at);
  return events;
}

function symbolCount(pattern) {
  return [...pattern].filter((ch) => SYMBOLS.includes(ch)).length;
}

function alpha(n) {
  return n >= 1 && n <= 26 ? String.fromCharCode(64 + n) : String(n);
}

function roman(n) {
  if (n < 1) return String(n);
  const table = [
    [1000, "M"], [900, "CM"], [500, "D"], [400, "CD"], [100, "C"], [90, "XC"],
    [50, "L"], [40, "XL"], [10, "X"], [9, "IX"], [5, "V"], [4, "IV"], [1, "I"],
  ];
  let rest = n;
  let out = "";
  for (const [value, digits] of table) {
    while (rest >= value) {
      out += digits;
      rest -= value;
    }
  }
  return out;
}

function format(n, symbol) {
  switch (symbol) {
    case "a": return alpha(n).toLowerCase();
    case "A": return alpha(n);
    case "i": return roman(n).toLowerCase();
    case "I": return roman(n);
    default: return String(n);
  }
}

// Renders a numbering pattern with one value per counting symbol. The render
// stops at the first symbol without a value, and the punctuation at either end
// is dropped, so "1." gives "3" and "(1)" gives "4".
function render(pattern, values) {
  let out = "";
  let next = 0;
  for (const ch of pattern) {
    if (!SYMBOLS.includes(ch)) {
      out += ch;
      continue;
    }
    if (next >= values.length) break;
    out += format(values[next], ch);
    next += 1;
  }
  return out.replace(TRIM, "");
}

function bump(map, key) {
  map.set(key, (map.get(key) ?? 0) + 1);
}

/// Computes the facts of a Typst project, in the shape documentFacts returns.
/// `texts` maps each project path (no leading slash) to its source, `main` is
/// the main file, and `entries` the parsed bibliography entries. `open` is the
/// file shown in the editor: Typst has no environment numbers, so it is unused.
///
/// Labels map to { number, kind }, where kind is 'section', 'equation' or a
/// figure kind ('figure', 'table', 'listing', or the name of a `kind:`). Only
/// labels bound to a numbered element are listed. A citation is a cite of a
/// bibliography key, or a @key that is no label of the project, and it takes
/// its number at its first use in reading order.
export function typstFacts(texts, main, open, entries = []) {
  const masks = new Map();
  for (const path of Object.keys(texts)) masks.set(path, maskTypst(texts[path] ?? ""));
  const labelNames = new Set();
  for (const { text } of masks.values()) {
    for (const match of text.matchAll(LABEL_DEFINITION)) labelNames.add(match[1]);
  }
  const entryKeys = new Set(rankEntries(entries).map((entry) => entry.key));

  const labels = new Map();
  const order = new Map();
  const cited = (key) => {
    if (!order.has(key)) order.set(key, order.size + 1);
  };
  // Heading counters by level. The per-kind counters count the whole document
  // (globals) and the current chapter (chapters, reset by each first-level heading).
  const counters = [0, 0, 0, 0, 0, 0];
  const globals = new Map();
  const chapters = new Map();
  const countValues = (pattern, key) =>
    symbolCount(pattern) >= 2 ? [counters[0], chapters.get(key)] : [globals.get(key)];
  let headingPattern = null;
  let figurePattern = "1";
  let equationPattern = null;
  let bibliographyStyle = null;
  let setStyle = null;

  const resolve = (name, from) => {
    const target = name.trim();
    const path = normalize(target.startsWith("/") ? target : folderOf(from) + target);
    return masks.has(path) ? path : null;
  };

  const scanFile = (path, stack) => {
    const { text, maths } = masks.get(path);
    for (const event of eventsOf(text, maths)) {
      switch (event.type) {
        case "heading": {
          const pattern = event.numbering !== undefined ? event.numbering : headingPattern;
          if (pattern === null) break;
          counters[event.level - 1] += 1;
          counters.fill(0, event.level);
          if (event.level === 1) chapters.clear();
          const values = symbolCount(pattern) >= 2 ? counters.slice(0, event.level) : [counters[event.level - 1]];
          const number = render(pattern, values);
          if (event.label && number) labels.set(event.label, { number, kind: "section" });
          break;
        }
        case "figure": {
          const pattern = event.numbering !== undefined ? event.numbering : figurePattern;
          if (pattern === null) break;
          bump(globals, event.kind);
          bump(chapters, event.kind);
          const number = render(pattern, countValues(pattern, event.kind));
          if (event.label && number) labels.set(event.label, { number, kind: event.kind });
          break;
        }
        case "equation": {
          if (equationPattern === null) break;
          bump(globals, "equation");
          bump(chapters, "equation");
          const number = render(equationPattern, countValues(equationPattern, "equation"));
          if (event.label && number) labels.set(event.label, { number, kind: "equation" });
          break;
        }
        case "set": {
          if (event.target === "cite") {
            setStyle = styleArg(event.args) ?? setStyle;
            break;
          }
          const value = numberingArg(event.args);
          if (value === undefined) break;
          if (event.target === "heading") headingPattern = value;
          else if (event.target === "figure") figurePattern = value;
          else equationPattern = value;
          break;
        }
        case "bibliography":
          bibliographyStyle = styleArg(event.args) ?? bibliographyStyle;
          break;
        case "cite":
          if (entryKeys.has(event.key)) cited(event.key);
          break;
        case "ref":
          if (!labelNames.has(event.key) && entryKeys.has(event.key)) cited(event.key);
          break;
        case "include": {
          const next = resolve(event.path, path);
          if (next && !stack.includes(next)) scanFile(next, [...stack, next]);
          break;
        }
      }
    }
  };

  const root = normalize(main);
  if (masks.has(root)) scanFile(root, [root]);

  const style = bibliographyStyle ?? setStyle;
  return {
    labels,
    citations: citationFacts(entries, order),
    acronyms: new Map(),
    textMacros: new Map(),
    mathMacros: {},
    theorems: new Map(),
    environmentNumbers: new Map(),
    citeStyle: style && AUTHOR_YEAR.test(style) ? "author-year" : "numeric",
  };
}
