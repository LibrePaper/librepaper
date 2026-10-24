// Typst source is prose in markup and code after `#`. The spellchecker reads
// only prose, so this lists the markup ranges it must skip. The scan is one
// pass with an explicit stack of frames, so nesting costs no recursion, and
// every search ends at a delimiter, at a line end or at the end of the text.

const HASH_CALL = /#([A-Za-z_][\w-]*(?:\.[A-Za-z_][\w-]*)*)/y;
const LABEL = /<[\w:.-]+>/y;
const REFERENCE = /@[\w:.-]+/y;
const URL = /https?:\/\/[^\s)\]]*/y;
const ELSE = /[ \t]*else\b/y;
// Keywords that start a code line. The line ends at a line break, or after a
// content block that no `else` follows on the same line.
const STATEMENT_KEYWORDS = new Set([
  "let",
  "set",
  "show",
  "import",
  "include",
  "context",
  "if",
  "for",
  "while",
  "return",
  "break",
  "continue",
]);

/**
 * Ranges of a Typst source that are markup rather than prose, for the
 * spellchecker to skip. Half-open [from, to) offsets into `text` (UTF-16, as
 * JavaScript strings index). Any order; overlaps allowed.
 * @param {string} text
 * @returns {Array<[number, number]>}
 */
export function typstSkips(text) {
  const source = typeof text === "string" ? text : "";
  // A multi-line argument list is honoured when it closes. When one never
  // closes, the text is scanned again with every code group ending at its
  // line, so the prose after a stray `#f(` is still checked.
  return scan(source, false) ?? scan(source, true);
}

function scan(s, strict) {
  const n = s.length;
  const out = [];
  // Markup frames are prose. A content block is a markup frame closed by
  // "]", and its depth counts the brackets nested inside it. Code frames are
  // argument lists, code blocks and `#let`-style lines.
  const stack = [{ kind: "m", close: null, depth: 0, line: false }];
  let openLines = 0;
  // Start of the code run not yet emitted, or -1. Code is emitted in runs,
  // split only where a content block (prose) begins or ends.
  let runStart = -1;
  // Set once a search for a closing `$` reaches the end of the text. No later
  // `$` can close then, so later openers stop at their line end at once.
  let mathUnclosed = false;
  let i = 0;

  const top = () => stack[stack.length - 1];

  // The end of a sticky match at p, or -1 when there is none.
  const span = (re, p) => {
    re.lastIndex = p;
    return re.exec(s) ? re.lastIndex : -1;
  };

  // The last line-end search in this scan. No newline lies from `eolFrom` up
  // to `eolAt`, which is the newline found there, or the text end.
  let eolFrom = -1;
  let eolAt = -1;
  const eol = (p) => {
    if (p >= eolFrom && p <= eolAt) return eolAt;
    const e = s.indexOf("\n", p);
    eolFrom = p;
    eolAt = e < 0 ? n : e;
    return eolAt;
  };

  const skip = (from, to) => {
    if (to > from) out.push([from, to]);
  };

  const enterCode = (p) => {
    if (runStart < 0) runStart = p;
  };

  const leaveCode = (p) => {
    if (runStart >= 0) {
      skip(runStart, p);
      runStart = -1;
    }
  };

  const pushCode = (close, line) => {
    stack.push({ kind: "c", close, depth: 0, line });
    if (line) openLines++;
  };

  // A `[` at p opens a content block. Its bracket is code, its inside is prose.
  const openContent = (p) => {
    enterCode(p);
    leaveCode(p + 1);
    stack.push({ kind: "m", close: "]", depth: 0, line: false });
    return p + 1;
  };

  // A call or code group ends at p. A directly following argument list or
  // content block continues the call. Otherwise the code run ends here, when
  // the frame below is prose.
  const afterCall = (p) => {
    if (s[p] === "(") {
      pushCode(")", false);
      return p + 1;
    }
    if (s[p] === "[") return openContent(p);
    if (top().kind === "m") leaveCode(p);
    return p;
  };

  const closeFrame = (p) => {
    const frame = stack.pop();
    if (frame.line) openLines--;
    return afterCall(p);
  };

  // Strings end at their quote, or at the line break when they never close.
  const stringEnd = (p) => {
    let q = p + 1;
    while (q < n && s[q] !== '"' && s[q] !== "\n") {
      q += s[q] === "\\" && s[q + 1] !== "\n" ? 2 : 1;
    }
    if (q < n && s[q] === '"') q++;
    return Math.min(q, n);
  };

  // A line break inside code. p is the index of the break. Returns true when
  // frames closed.
  // In the normal pass a break ends a `#let`-style line only when that line
  // is the top frame. A group still open above it keeps the break as
  // whitespace, so a multi-line `#set text(` argument list stays code.
  // The strict pass ends every code group above the nearest markup frame.
  const breakCode = (p) => {
    if (!strict) {
      if (openLines === 0 || !top().line) return false;
      closeFrame(p);
      return true;
    }
    let base = stack.length;
    while (stack[base - 1].kind === "c") base--;
    while (stack.length > base) closeFrame(p);
    return true;
  };

  // A run of backticks of length k at or after p, before limit, or -1.
  const findRun = (p, k, limit) => {
    let q = p;
    while (q < limit) {
      if (s[q] !== "`") {
        q++;
        continue;
      }
      let r = q;
      while (r < limit && s[r] === "`") r++;
      if (r - q === k) return q;
      q = r;
    }
    return -1;
  };

  // A `$` is escaped when an odd number of backslashes precede it.
  const evenBackslashes = (q) => {
    let k = 0;
    while (q - 1 - k >= 0 && s[q - 1 - k] === "\\") k++;
    return k % 2 === 0;
  };

  // The end of the math that opens at p. Unclosed math ends at its line end.
  const mathEnd = (p) => {
    if (!mathUnclosed) {
      for (let q = p + 1; q < n; q++) {
        if (s[q] === "$" && evenBackslashes(q)) return q + 1;
      }
      mathUnclosed = true;
    }
    return eol(p);
  };

  while (i < n) {
    const frame = top();
    const c = s[i];

    if (frame.kind === "c") {
      if (c === "\n") {
        if (!breakCode(i)) i++;
      } else if (c === ";" && frame.line) {
        // A semicolon ends a statement as a line break does, and markup follows.
        i = closeFrame(i + 1);
      } else if (c === '"') {
        i = stringEnd(i);
      } else if (c === "/" && s[i + 1] === "/") {
        i = eol(i);
      } else if (c === "/" && s[i + 1] === "*") {
        const close = s.indexOf("*/", i + 2);
        i = close < 0 ? n : close + 2;
      } else if (c === "(" || c === "{") {
        pushCode(c === "(" ? ")" : "}", false);
        i++;
      } else if (c === ")" || c === "}") {
        i = frame.close === c ? closeFrame(i + 1) : i + 1;
      } else if (c === "[") {
        i = openContent(i);
      } else {
        i++;
      }
      continue;
    }

    if (c === "\\") {
      skip(i, Math.min(i + 2, n));
      i += 2;
    } else if (c === "/" && s[i + 1] === "/") {
      const end = eol(i);
      skip(i, end);
      i = end;
    } else if (c === "/" && s[i + 1] === "*") {
      const close = s.indexOf("*/", i + 2);
      const end = close < 0 ? n : close + 2;
      skip(i, end);
      i = end;
    } else if (c === "$") {
      const end = mathEnd(i);
      skip(i, end);
      i = end;
    } else if (c === "`") {
      let k = 1;
      while (s[i + k] === "`") k++;
      // A block raw runs to the end of the text, an inline one to its line end.
      // A search that finds no closing run ends at that limit, and the scan
      // moves past it, so the same stretch is never searched again.
      const limit = k >= 3 ? n : eol(i);
      const open = findRun(i + k, k, limit);
      const end = open < 0 ? limit : open + k;
      skip(i, end);
      i = end;
    } else if (c === "<" && span(LABEL, i) > 0) {
      const end = span(LABEL, i);
      skip(i, end);
      i = end;
    } else if (c === "@" && span(REFERENCE, i) > 0) {
      // A trailing period or colon is punctuation, not part of the reference.
      let end = span(REFERENCE, i);
      while (end - i > 1 && (s[end - 1] === "." || s[end - 1] === ":")) end--;
      if (end - i > 1) {
        skip(i, end);
        i = end;
      } else {
        i++;
      }
    } else if (c === "h" && span(URL, i) > 0) {
      const end = span(URL, i);
      skip(i, end);
      i = end;
    } else if (c === "#") {
      const next = s[i + 1];
      if (next === "(" || next === "{") {
        enterCode(i);
        pushCode(next === "(" ? ")" : "}", false);
        i += 2;
      } else if (next === "[") {
        enterCode(i);
        i = openContent(i + 1);
      } else if (span(HASH_CALL, i) > 0) {
        const end = span(HASH_CALL, i);
        const name = s.slice(i + 1, end).split(".")[0];
        enterCode(i);
        if (STATEMENT_KEYWORDS.has(name)) {
          pushCode(null, true);
          i = end;
        } else {
          i = afterCall(end);
        }
      } else {
        i++;
      }
    } else if (c === "[") {
      if (frame.close === "]") frame.depth++;
      i++;
    } else if (c === "]") {
      if (frame.close === "]" && frame.depth === 0) {
        enterCode(i);
        i = closeFrame(i + 1);
        // The text after a content block is markup, so a statement line ends
        // with its block. An `else` on the same line keeps the line open for
        // the `else` branch, which is itself a content block or code.
        if (top().line && span(ELSE, i) < 0) i = closeFrame(i);
      } else {
        if (frame.close === "]") frame.depth--;
        i++;
      }
    } else {
      i++;
    }
  }

  // Strict mode never fails. A pass that finds a group still open at the end
  // of the text is rejected, so the strict pass can take over.
  if (!strict && stack.some((f) => f.kind === "c" && !f.line)) return null;
  while (stack.length > 1) closeFrame(n);
  leaveCode(n);
  return out;
}
