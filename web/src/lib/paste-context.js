import { inLatexMath } from "./emphasis.js";

/// Whether a paste at `at` in `text` should insert plain text rather than
/// markup converted from clipboard HTML. Inside math or code, converted markup
/// would mangle the source, so the paste keeps the clipboard text as it is.
/** @param {string} text the whole document @param {number} at offset @param {string} format "latex"|"typst"|"markdown"|"quarto"|"org"|"html"|"" */
export function pastesPlainAt(text, at, format) {
  if (format === "latex") return latexPlain(text, at);
  if (format === "markdown" || format === "quarto") return markdownPlain(text, at);
  if (format === "typst") return typstPlain(text, at);
  if (format === "org") return orgPlain(text, at);
  // HTML files and files with no known format always take plain text.
  return true;
}

/* ---------------------------------------------------------------- helpers */

// Offset of the first character on the line that holds `at`.
const lineStart = (text, at) => text.lastIndexOf("\n", at - 1) + 1;

// Index of the first unescaped `%` in `s`, or -1. A backslash escapes the next
// character, so `\%` does not start a comment.
function firstUnescapedPercent(s) {
  for (let i = 0; i < s.length; i++) {
    if (s[i] === "\\") i += 1;
    else if (s[i] === "%") return i;
  }
  return -1;
}

// Whether `s` has an odd number of single `$` signs, once escapes and `$$` are
// removed. An odd count means a `$...$` span is open at the end of `s`.
function oddDollars(s) {
  const dollars = s.replace(/\\./g, "").replace(/\$\$/g, "").match(/\$/g);
  return dollars !== null && dollars.length % 2 === 1;
}

/* ----------------------------------------------------------------- LaTeX */

// Environments whose body is literal text, so a paste there must stay plain.
const VERBATIM = new Set(["verbatim", "verbatim*", "lstlisting", "minted", "Verbatim", "comment"]);

// Math (reused from emphasis.js), a verbatim-like environment, or a comment
// on the current line.
function latexPlain(text, at) {
  if (inLatexMath(text, at)) return true;
  if (firstUnescapedPercent(text.slice(lineStart(text, at), at)) !== -1) return true;
  return inVerbatim(text, at);
}

// Whether `at` is inside a verbatim-like environment. The scan skips `%`
// comments, since a `\begin{verbatim}` in a comment opens nothing, and inside
// a verbatim body `%` is just a character.
function inVerbatim(text, at) {
  const head = text.slice(0, at);
  const token = /\\(begin|end)\{([^}]*)\}|\\.|%/gs;
  let open = null;
  let match;
  while ((match = token.exec(head)) !== null) {
    if (match[0] === "%") {
      if (open === null) {
        // A comment hides the rest of its line.
        const eol = head.indexOf("\n", token.lastIndex);
        if (eol === -1) break;
        token.lastIndex = eol;
      }
    } else if (match[1] === "begin" && open === null && VERBATIM.has(match[2])) {
      open = match[2];
    } else if (match[1] === "end" && match[2] === open) {
      open = null;
    }
  }
  return open !== null;
}

/* -------------------------------------------------------------- Markdown */

// A fence opens with three or more backticks or tildes, indented up to three
// spaces. It closes with at least as many of the same character.
const FENCE_OPEN = /^ {0,3}(`{3,}|~{3,})/;
const FENCE_CLOSE = /^ {0,3}(`{3,}|~{3,})[ \t\r]*$/;

// Whether a `$$` count in `s` leaves display math open or closed.
function toggleDisplay(open, s) {
  const count = (s.match(/\$\$/g) || []).length;
  return count % 2 === 1 ? !open : open;
}

// Fenced code, YAML front matter, `$$` display math, and on the current line
// an inline code span or `$...$`. Lines are read from the start, so a fence or
// display block opened far above still counts.
function markdownPlain(text, at) {
  if (inFrontMatter(text, at)) return true;
  let fence = null; // the open fence, as its character and length
  let display = false; // inside $$...$$
  let pos = 0;
  for (;;) {
    let eol = text.indexOf("\n", pos);
    if (eol === -1) eol = text.length;
    if (eol >= at) break; // `at` is on this line
    const line = text.slice(pos, eol);
    if (fence !== null) {
      const close = line.match(FENCE_CLOSE);
      if (close && close[1][0] === fence.char && close[1].length >= fence.length) fence = null;
    } else {
      const open = line.match(FENCE_OPEN);
      if (open) fence = { char: open[1][0], length: open[1].length };
      else display = toggleDisplay(display, line);
    }
    pos = eol + 1;
  }
  if (fence !== null) return true;
  const { open, prose } = scanInline(text.slice(pos, at));
  if (open) return true; // the caret is inside a code span
  if (toggleDisplay(display, prose)) return true;
  return oddDollars(prose);
}

// Scans the part of a line before the caret. A backtick run opens a code span
// that closes at the next run of the same length; a backslash escapes the next
// character outside code spans. Returns whether a code span is still open at
// the end, and the prose left once escapes and closed code spans are removed.
function scanInline(s) {
  let prose = "";
  let i = 0;
  while (i < s.length) {
    if (s[i] === "\\") {
      i += 2;
    } else if (s[i] === "`") {
      let end = i;
      while (s[end] === "`") end += 1;
      const length = end - i;
      const close = nextBacktickRun(s, end, length);
      if (close === -1) return { open: true, prose };
      i = close + length;
    } else {
      prose += s[i];
      i += 1;
    }
  }
  return { open: false, prose };
}

// Index of the next backtick run of exactly `length` at or after `from`, or -1.
// Runs of another length are part of the span and do not close it.
function nextBacktickRun(s, from, length) {
  const runs = /`+/g;
  runs.lastIndex = from;
  for (let match = runs.exec(s); match !== null; match = runs.exec(s)) {
    if (match[0].length === length) return match.index;
  }
  return -1;
}

// Whether `at` is inside YAML front matter: the document opens with `---` and
// `at` comes before the closing `---` line.
function inFrontMatter(text, at) {
  if (!/^---[ \t\r]*\n/.test(text)) return false;
  const close = /^---[ \t\r]*$/gm;
  close.lastIndex = text.indexOf("\n") + 1;
  const match = close.exec(text);
  return match !== null && at < match.index;
}

/* ----------------------------------------------------------------- Typst */

// Math is `$...$`. Raw blocks, raw spans and comments hide their dollar signs,
// and `\` escapes the next character. The scan starts at the document start.
function typstPlain(text, at) {
  let math = false;
  let i = 0;
  while (i < at) {
    if (text.startsWith("```", i)) {
      const end = text.indexOf("```", i + 3);
      if (end === -1 || at < end + 3) return true;
      i = end + 3;
    } else if (text[i] === "`") {
      const end = text.indexOf("`", i + 1);
      if (end === -1) {
        i += 1;
      } else {
        if (at <= end) return true;
        i = end + 1;
      }
    } else if (text.startsWith("//", i)) {
      const eol = text.indexOf("\n", i);
      if (eol === -1 || at <= eol) return true;
      i = eol;
    } else if (text.startsWith("/*", i)) {
      const end = text.indexOf("*/", i + 2);
      if (end === -1 || at < end + 2) return true;
      i = end + 2;
    } else if (text[i] === "\\") {
      i += 2;
    } else {
      if (text[i] === "$") math = !math;
      i += 1;
    }
  }
  return math;
}

/* ------------------------------------------------------------------- Org */

const ORG_BLOCK_BEGIN = /^\s*#\+begin_(src|example|export)\b/i;
const ORG_BLOCK_END = /^\s*#\+end_(src|example|export)\b/i;
// Display delimiters: `\[` and `\]`, or a LaTeX math environment.
const ORG_DISPLAY = /\\([[\]])|\\(begin|end)\{(?:equation|align|gather|multline|displaymath)\*?\}/g;

// Whether a display delimiter in `s` leaves display math open or closed.
function orgDisplay(open, s) {
  for (const [, bracket, env] of s.matchAll(ORG_DISPLAY)) {
    if (bracket === "[" || env === "begin") open = true;
    else if (bracket === "]" || env === "end") open = false;
  }
  return open;
}

// Source, example and export blocks, LaTeX display math from earlier lines,
// and on the current line `\(...\)`, `\[...\]` or `$...$`.
function orgPlain(text, at) {
  let block = null; // the open block's name, lowercased
  let display = false;
  let pos = 0;
  for (;;) {
    let eol = text.indexOf("\n", pos);
    if (eol === -1) eol = text.length;
    if (eol >= at) break; // `at` is on this line
    const line = text.slice(pos, eol);
    const begin = line.match(ORG_BLOCK_BEGIN);
    const end = line.match(ORG_BLOCK_END);
    if (block === null && begin) block = begin[1].toLowerCase();
    else if (block !== null && end && end[1].toLowerCase() === block) block = null;
    else if (block === null) display = orgDisplay(display, line);
    pos = eol + 1;
  }
  if (block !== null || display) return true;
  const prefix = text.slice(pos, at);
  if (prefix.lastIndexOf("\\(") > prefix.lastIndexOf("\\)")) return true;
  if (prefix.lastIndexOf("\\[") > prefix.lastIndexOf("\\]")) return true;
  return oddDollars(prefix);
}
