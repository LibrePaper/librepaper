/// Ranges of Org source that are markup rather than prose, for the spellchecker
/// to skip. The scanner walks the text line by line, keeping the state of open
/// blocks, drawers and LaTeX environments, and matches inline constructs with
/// sticky regexes. A search for a closing delimiter that fails is remembered
/// for the rest of the line, so the cost stays linear on hostile input.

// Blocks whose bodies are code or raw text. Other blocks (QUOTE, VERSE,
// CENTER, special blocks) keep their body as prose; only their delimiters are skipped.
const SKIP_BLOCKS = new Set(["SRC", "EXAMPLE", "EXPORT", "COMMENT"]);

// Keyword values that are prose. Every other #+KEY: line is markup.
const PROSE_KEYWORDS = new Set(["TITLE", "SUBTITLE", "CAPTION", "DESCRIPTION"]);

const TODO = /(?:TODO|DONE)(?:[ \t]+|$)/y;
const PRIORITY = /\[#[A-Za-z0-9]+\][ \t]*/y;
// A tag group such as ":a:b:", checked against the last word of a headline.
const TAGS = /^:(?:[^\s:]+:)+$/;
const CHECKBOX = /\[[ Xx-]\](?=[ \t]|$)[ \t]*/y;

const MATH = /\$[^\s$](?:[^$\n]*[^\s$])?\$(?![\p{L}\p{N}])/uy;
const CODE = /~[^\s~](?:[^~\n]*[^\s~])?~(?![\p{L}\p{N}])/uy;
const VERBATIM = /=[^\s=](?:[^=\n]*[^\s=])?=(?![\p{L}\p{N}])/uy;
const LINK_HEAD = /\[\[[^\[\]\n]*\]/y;
const FOOTNOTE = /\[fn:[^\]\n]*\]/y;
const CITATION = /\[cite[^\]\n]*\]/y;
const TIMESTAMP = /[<[]\d{4}-\d{2}-\d{2}[^\]>\n]*[>\]]/y;
const TARGET = /<<[^<>\n]*>>/y;
const SRC_INLINE = /src_[\w+-]+(?:\[[^\]\n]*\])?\{/y;

/// Ranges of an Org source that are markup rather than prose, for the
/// spellchecker to skip. Half-open [from, to) offsets into `text` (UTF-16).
/// Any order; overlaps allowed. An unterminated block or drawer runs to the
/// end of the text.
/// @param {string} text
/// @returns {Array<[number, number]>}
export function orgSkips(text) {
  /** @type {Array<[number, number]>} */
  const out = [];
  if (typeof text !== "string" || !text) return out;
  const state = { block: /** @type {string | null} */ (null), drawer: false, math: /** @type {string | null} */ (null) };
  const n = text.length;
  let at = 0;
  while (at < n) {
    let end = text.indexOf("\n", at);
    if (end === -1) end = n;
    scanLine(text.slice(at, end), at, state, out);
    at = end + 1;
  }
  return out;
}

// Classifies one line, given the state carried over from the lines before it.
function scanLine(line, base, state, out) {
  if (state.block !== null) {
    mark(out, base, 0, line.length);
    const end = /^[ \t]*#\+end_(\w+)/i.exec(line);
    if (end && end[1].toUpperCase() === state.block) state.block = null;
    return;
  }
  if (state.drawer) {
    mark(out, base, 0, line.length);
    if (/^[ \t]*:end:[ \t]*$/i.test(line)) state.drawer = false;
    return;
  }
  if (state.math === null) {
    const env = /^[ \t]*\\begin\{([A-Za-z]+\*?)\}/.exec(line);
    if (env) state.math = env[1];
  }
  if (state.math !== null) {
    const from = mathEnd(line, base, state, out);
    if (from !== -1) scanInline(line, from, base, out);
    return;
  }

  const block = /^[ \t]*#\+begin_(\w+)/i.exec(line);
  if (block) {
    mark(out, base, 0, line.length);
    const name = block[1].toUpperCase();
    if (SKIP_BLOCKS.has(name)) state.block = name;
    return;
  }
  const keyword = /^([ \t]*)#\+(\w[\w-]*)(?:\[[^\]\n]*\])?:[ \t]*/.exec(line);
  if (keyword) {
    if (PROSE_KEYWORDS.has(keyword[2].toUpperCase())) {
      const from = mark(out, base, 0, keyword[0].length);
      scanInline(line, from, base, out);
    } else {
      mark(out, base, 0, line.length);
    }
    return;
  }
  // Other #+ lines (end delimiters, #+CALL and the like) and comment lines.
  if (/^[ \t]*#\+/.test(line) || /^[ \t]*#(?:\s|$)/.test(line)) {
    mark(out, base, 0, line.length);
    return;
  }
  if (/^[ \t]*:[\w-]+:[ \t]*$/.test(line)) {
    mark(out, base, 0, line.length);
    if (!/^[ \t]*:end:[ \t]*$/i.test(line)) state.drawer = true;
    return;
  }
  if (/^[ \t]*(?:SCHEDULED|DEADLINE|CLOSED|CLOCK):/.test(line)) {
    mark(out, base, 0, line.length);
    return;
  }
  if (/^\*+(?:[ \t]|$)/.test(line)) {
    headlineSkips(line, base, out);
    return;
  }
  if (/^[ \t]*-{5,}[ \t]*$/.test(line)) {
    mark(out, base, 0, line.length);
    return;
  }
  const item = /^[ \t]*(?:[-+]|\*|\d+[.)])(?=[ \t]|$)[ \t]*/.exec(line);
  if (item) {
    let from = mark(out, base, 0, item[0].length);
    CHECKBOX.lastIndex = from;
    if (CHECKBOX.exec(line)) from = mark(out, base, from, CHECKBOX.lastIndex);
    scanInline(line, from, base, out);
    return;
  }
  if (/^[ \t]*\|/.test(line)) {
    if (line.includes("-") && /^[ \t]*\|[ \t|+-]*$/.test(line)) {
      mark(out, base, 0, line.length);
      return;
    }
    for (let i = 0; i < line.length; i++) {
      if (line[i] === "|") mark(out, base, i, i + 1);
    }
  }
  scanInline(line, 0, base, out);
}

// Skips the part of a line that is inside a LaTeX environment, up to and
// including its \end. Returns the index where prose resumes, or -1 when the
// environment is still open at the end of the line.
function mathEnd(line, base, state, out) {
  const close = `\\end{${state.math}}`;
  const at = line.indexOf(close);
  if (at === -1) {
    mark(out, base, 0, line.length);
    return -1;
  }
  state.math = null;
  return mark(out, base, 0, at + close.length);
}

// A headline: the stars, a leading TODO or DONE, a priority cookie and trailing
// tags are markup. The title between them is prose.
function headlineSkips(line, base, out) {
  const stars = /^\*+[ \t]*/.exec(line);
  let from = mark(out, base, 0, stars ? stars[0].length : 0);
  TODO.lastIndex = from;
  if (TODO.exec(line)) from = mark(out, base, from, TODO.lastIndex);
  PRIORITY.lastIndex = from;
  if (PRIORITY.exec(line)) from = mark(out, base, from, PRIORITY.lastIndex);
  // The tags are the last word, if it is a tag group. Scanning back from the
  // end keeps this linear.
  let end = line.length;
  while (end > from && /[ \t\r]/.test(line[end - 1])) end--;
  let start = end;
  while (start > from && !/\s/.test(line[start - 1])) start--;
  if (start < end && TAGS.test(line.slice(start, end))) mark(out, base, start, end);
  scanInline(line, from, base, out);
}

// Skips inline markup from `from` to the end of the line.
function scanInline(line, from, base, out) {
  // Which closing delimiters are known to be missing from this line.
  const misses = new Set();
  let i = from;
  while (i < line.length) {
    const next = inlineAt(line, i, base, out, misses);
    i = next > i ? next : i + 1;
  }
}

// Handles the inline construct that may start at `i`. Returns the index to
// continue from, which is `i` itself when nothing starts there.
function inlineAt(line, i, base, out, misses) {
  switch (line[i]) {
    case "$": {
      if (line[i + 1] === "$") {
        const close = seek(line, "$$", i + 2, misses);
        return close === -1 ? i + 2 : mark(out, base, i, close + 2);
      }
      return span(MATH, line, i, base, out);
    }
    case "\\": {
      const next = line[i + 1];
      if (next !== "(" && next !== "[") return i;
      const close = seek(line, next === "(" ? "\\)" : "\\]", i + 2, misses);
      return close === -1 ? i + 2 : mark(out, base, i, close + 2);
    }
    case "~":
      return span(CODE, line, i, base, out);
    case "=":
      return span(VERBATIM, line, i, base, out);
    case "{": {
      if (!line.startsWith("{{{", i)) return i;
      const close = seek(line, "}}}", i + 3, misses);
      return close === -1 ? i + 3 : mark(out, base, i, close + 3);
    }
    case "s": {
      if (!line.startsWith("src_", i)) return i;
      SRC_INLINE.lastIndex = i;
      const m = SRC_INLINE.exec(line);
      if (!m) return i;
      const open = i + m[0].length;
      const close = seek(line, "}", open, misses);
      return close === -1 ? open : mark(out, base, i, close + 1);
    }
    case "<": {
      const target = span(TARGET, line, i, base, out);
      return target > i ? target : span(TIMESTAMP, line, i, base, out);
    }
    case "[": {
      if (line[i + 1] === "[") return link(line, i, base, out, misses);
      if (line.startsWith("[cite", i)) {
        const cite = span(CITATION, line, i, base, out);
        if (cite > i) return cite;
      }
      if (line.startsWith("[fn:", i)) return footnote(line, i, base, out);
      return span(TIMESTAMP, line, i, base, out);
    }
    default:
      return i;
  }
}

// A link. [[target]] is skipped whole. In [[target][description]] the target
// and brackets are skipped and the description is left for the caller to scan.
function link(line, i, base, out, misses) {
  LINK_HEAD.lastIndex = i;
  const m = LINK_HEAD.exec(line);
  if (!m) return i + 1;
  const end = i + m[0].length;
  if (line[end] === "]") return mark(out, base, i, end + 1);
  if (line[end] !== "[") return mark(out, base, i, end);
  const close = seek(line, "]]", end + 1, misses);
  if (close === -1) return mark(out, base, i, end + 1);
  mark(out, base, close, close + 2);
  return mark(out, base, i, end + 1);
}

// A footnote. [fn:label] is skipped whole. In [fn:label:text] and [fn::text]
// only the marker up to the text and the closing bracket are skipped.
function footnote(line, i, base, out) {
  FOOTNOTE.lastIndex = i;
  const m = FOOTNOTE.exec(line);
  if (!m) return i + 1;
  const end = i + m[0].length;
  const colon = m[0].indexOf(":", 4);
  if (colon === -1) return mark(out, base, i, end);
  const text = mark(out, base, i, i + colon + 1);
  mark(out, base, end - 1, end);
  return text;
}

// Skips the match of a sticky regex at `i`, if any. Returns the end of the
// skipped span, or `i` when nothing matched.
function span(re, line, i, base, out) {
  re.lastIndex = i;
  const m = re.exec(line);
  return m ? mark(out, base, i, i + m[0].length) : i;
}

// The index of `needle` at or after `from`, or -1. A miss is remembered for the
// line: the callers search from positions that only move forward, so a later
// search for the same needle would miss too.
function seek(line, needle, from, misses) {
  if (misses.has(needle)) return -1;
  const at = line.indexOf(needle, from);
  if (at === -1) misses.add(needle);
  return at;
}

// Records [from, to) of a line as a skip, and returns `to`.
function mark(out, base, from, to) {
  if (to > from) out.push([base + from, base + to]);
  return to;
}
