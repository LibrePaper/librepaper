// Org-mode source highlighting as a CodeMirror stream mode. It colours a
// source by its shape (headlines, keywords, blocks, drawers, lists, tables)
// and by its inline markup. It does not parse Org, so unusual nesting may
// colour a little differently from Emacs.
import { LanguageSupport, StreamLanguage } from "@codemirror/language";

const VERBATIM = new Set(["src", "example", "export", "comment"]);
const BLOCKS = new Set([...VERBATIM, "quote", "verse", "center"]);

const BEGIN = /^\s*#\+begin_(\w+)/i;
const END = /^\s*#\+end_\w+/i;
const KEYWORD_LINE = /^\s*#\+\w+:/;
const COMMENT = /^\s*#(?: |$)/;
const DRAWER_OPEN = /^\s*:[A-Za-z_-]+:\s*$/;
const DRAWER_END = /^\s*:END:\s*$/i;
const CLOCK = /^\s*CLOCK:/;
const PLANNING = /^\s*(?:SCHEDULED|DEADLINE|CLOSED):/;
const LATEX_BEGIN = /^\s*\\begin\{([^}]*)\}/;
const TABLE = /^\s*\|/;
const RULE_ROW = /^\s*\|[-+|]*-[-+|]*\s*$/;
const LIST = /^(\s*(?:[-+]|\d+[.)])|\s+\*)(?=\s)/;
const CHECKBOX = /^(\s*)(\[[ Xx-]\])/;

// Inline patterns are sticky, so each one matches only at the stream position.
const INLINE = {
  stamp: /<\d{4}-\d{2}-\d{2}[^>\n]*>/y,
  bracketStamp: /\[\d{4}-\d{2}-\d{2}[^\]\n]*\]/y,
  link: /\[\[[^\]\n]*\](?:\[[^\]\n]*\])?\]/y,
  footnote: /\[fn:[^\]\n]*\]/y,
  cite: /\[cite(?:\/[a-z]+)?:[^\]\n]*\]/y,
  macro: /\{\{\{[^}\n]*\}\}\}/y,
  displayMath: /\$\$(?!\s).+?(?<!\s)\$\$/uy,
  inlineMath: /\$(?!\s)[^$\n]+?(?<!\s)\$(?![\p{L}\p{N}])/uy,
  parenMath: /\\\(.+?\\\)/uy,
  bracketMath: /\\\[.+?\\\]/uy,
  command: /\\[A-Za-z]+/y,
};

// Emphasis opens at the start of a line or after whitespace or an opening
// bracket or quote, and closes before whitespace, punctuation or line end.
function emphasisRule(marker, style) {
  const e = marker.replace(/[\\^$.*+?()[\]{}|/]/g, "\\$&");
  return {
    style,
    re: new RegExp(`${e}[^\\s${e}](?:[^${e}]*[^\\s${e}])?${e}(?=[\\s\\p{P}]|$)`, "uy"),
  };
}
const EMPHASIS = {
  "*": emphasisRule("*", "strong"),
  "/": emphasisRule("/", "emphasis"),
  "_": emphasisRule("_", "emphasis"),
  "+": emphasisRule("+", "strikethrough"),
  "=": emphasisRule("=", "monospace"),
  "~": emphasisRule("~", "monospace"),
};

/** Consume the sticky pattern at the stream position, if it matches there. */
function take(re, stream) {
  re.lastIndex = stream.pos;
  const match = re.exec(stream.string);
  if (!match) return false;
  stream.pos += match[0].length;
  return true;
}

function opensHere(stream) {
  return stream.pos === 0 || /[\s({'"]/.test(stream.string[stream.pos - 1]);
}

/**
 * The line-shaped parts of a line, as [from, to, style] ranges, plus the style
 * for the text between them (null when that text takes inline highlighting).
 */
function structure(line) {
  let m = /^\*+ /.exec(line);
  if (m) {
    const stars = m[0].length;
    const ranges = [];
    let pos = stars;
    const keyword = /^(TODO|DONE)(?=\s|$)/.exec(line.slice(stars));
    if (keyword) {
      ranges.push([stars, stars + keyword[0].length, "keyword"]);
      pos = stars + keyword[0].length;
    }
    const priority = /^(\s*)(\[#[A-Za-z0-9]\])/.exec(line.slice(pos));
    if (priority) {
      const from = pos + priority[1].length;
      ranges.push([from, from + priority[2].length, "atom"]);
    }
    const tags = /\s(:(?:[^\s:]+:)+)\s*$/.exec(line);
    if (tags && tags.index + 1 >= stars) ranges.push([tags.index + 1, line.length, "tag"]);
    return { ranges, fallback: "heading" };
  }
  if ((m = KEYWORD_LINE.exec(line))) {
    return { ranges: [[0, m[0].length, "meta"]], fallback: "string" };
  }
  if (COMMENT.test(line)) return { ranges: [], fallback: "comment" };
  if ((m = PLANNING.exec(line))) return { ranges: [[0, m[0].length, "keyword"]], fallback: null };
  if (TABLE.test(line)) {
    if (RULE_ROW.test(line)) return { ranges: [], fallback: "punctuation" };
    const ranges = [];
    for (let i = 0; i < line.length; i++) {
      if (line[i] === "|") ranges.push([i, i + 1, "punctuation"]);
    }
    return { ranges, fallback: null };
  }
  if ((m = LIST.exec(line))) {
    const bullet = m[0].length;
    const ranges = [[0, bullet, "punctuation"]];
    const box = CHECKBOX.exec(line.slice(bullet));
    if (box) {
      const from = bullet + box[1].length;
      ranges.push([from, from + box[2].length, "atom"]);
    }
    return { ranges, fallback: null };
  }
  return { ranges: [], fallback: null };
}

// The tokenizer asks for the structure of the same line once per token, so
// keep the last answer.
let cachedLine = null;
let cachedStructure = null;
function structureOf(line) {
  if (line !== cachedLine) {
    cachedLine = line;
    cachedStructure = structure(line);
  }
  return cachedStructure;
}

/** Highlight markup inside a line that has no line-level structure. */
function inlineToken(stream) {
  const c = stream.string[stream.pos];
  if (c === "<" && take(INLINE.stamp, stream)) return "number";
  if (c === "[") {
    if (take(INLINE.bracketStamp, stream)) return "number";
    if (take(INLINE.link, stream)) return "link";
    if (take(INLINE.footnote, stream) || take(INLINE.cite, stream)) return "labelName";
  }
  if (c === "$" && (take(INLINE.displayMath, stream) || take(INLINE.inlineMath, stream))) return "string";
  if (c === "\\") {
    if (take(INLINE.parenMath, stream) || take(INLINE.bracketMath, stream)) return "string";
    if (take(INLINE.command, stream)) return "keyword";
  }
  if (c === "{" && take(INLINE.macro, stream)) return "processingInstruction";
  const rule = EMPHASIS[c];
  if (rule && opensHere(stream) && take(rule.re, stream)) return rule.style;
  stream.next();
  return null;
}

/**
 * Handle the lines that open, close or sit inside a block, drawer or LaTeX
 * environment. Each consumes the whole line, so the state changes happen once.
 * Returns undefined when the line is none of these.
 */
function lineStart(stream, state) {
  const line = stream.string;
  if (state.block && VERBATIM.has(state.block)) {
    if (END.test(line)) {
      state.block = null;
      stream.skipToEnd();
      return "meta";
    }
    stream.skipToEnd();
    return state.block === "comment" ? "comment" : "monospace";
  }
  if (state.latexEnv !== null) {
    stream.skipToEnd();
    if (line.trimStart().startsWith(`\\end{${state.latexEnv}}`)) {
      state.latexEnv = null;
      return "keyword";
    }
    return "string";
  }
  const begin = BEGIN.exec(line);
  if (begin) {
    const kind = begin[1].toLowerCase();
    state.block = BLOCKS.has(kind) ? kind : "other";
    stream.skipToEnd();
    return "meta";
  }
  if (END.test(line)) {
    state.block = null;
    stream.skipToEnd();
    return "meta";
  }
  if (state.drawer) {
    if (DRAWER_END.test(line)) {
      state.drawer = false;
      stream.skipToEnd();
      return "meta";
    }
    // The rest of a CLOCK line is read as meta by the drawer rule in token().
    if (stream.match(CLOCK)) return "keyword";
    stream.skipToEnd();
    return "meta";
  }
  if (DRAWER_END.test(line)) {
    stream.skipToEnd();
    return "meta";
  }
  if (DRAWER_OPEN.test(line)) {
    state.drawer = true;
    stream.skipToEnd();
    return "meta";
  }
  const env = LATEX_BEGIN.exec(line);
  if (env) {
    state.latexEnv = env[1];
    stream.skipToEnd();
    return "keyword";
  }
  return undefined;
}

/** The raw stream-mode spec, exported so tests can drive the tokenizer directly. */
export const orgStreamParser = {
  name: "org",

  startState() {
    return { block: null, drawer: false, latexEnv: null };
  },

  copyState(state) {
    return { ...state };
  },

  token(stream, state) {
    if (stream.sol()) {
      const style = lineStart(stream, state);
      if (style !== undefined) return style;
    } else if (state.drawer) {
      stream.skipToEnd();
      return "meta";
    }
    const { ranges, fallback } = structureOf(stream.string);
    const hit = ranges.find(([from, to]) => stream.pos >= from && stream.pos < to);
    if (hit) {
      stream.pos = hit[1];
      return hit[2];
    }
    if (fallback) {
      const next = ranges.find(([from]) => from > stream.pos);
      stream.pos = next ? next[0] : stream.string.length;
      return fallback;
    }
    return inlineToken(stream);
  },

  languageData: {
    commentTokens: { line: "# " },
  },
};

export const orgLanguage = StreamLanguage.define(orgStreamParser);

/** The Org-mode language support. */
export function org() {
  return new LanguageSupport(orgLanguage);
}
