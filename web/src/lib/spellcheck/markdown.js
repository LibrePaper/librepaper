// Markup ranges for the spellchecker. The browser spellchecks whatever the
// source editor shows, so markup it cannot read (fences, math, link targets,
// attributes) is reported as ranges to leave alone. A line pass handles block
// structure, then each prose line is walked for inline constructs.
//
// Nothing here backtracks. The regexes are sticky with flat classes, and each
// search for a closing delimiter is remembered, so a line full of unmatched
// openers costs one scan rather than one scan per opener.

const FENCE_OPEN = / {0,3}(`{3,}|~{3,})/y;
const CELL_OPTION = /[ \t]*#\|/y;
const DISPLAY_OPEN = / {0,3}\$\$/y;
const REFERENCE_DEF = / {0,3}\[(?!\^)[^\]\n]*\]:/y;
const NOTE_DEF = / {0,3}\[\^[^\]\s]+\]:/y;
const DIV_FENCE = / {0,3}:{3,}/y;
const DIV_NAME = /[ \t]*(?:\{[^{}\n]*\}|[\w-]+)/y;
const HEADING = / {0,3}#{1,6}(?=[ \t\r\n]|$)/y;
const ATTRIBUTES = /\{[^{}\n]*\}/y;
// Heading attributes end the line, so only a brace run to the end counts.
const HEADING_ATTRIBUTES = /\{[^{}\n]*\}[ \t\r]*(?=\n|$)/y;
const AUTOLINK = /<[A-Za-z][A-Za-z0-9+.-]*:[^\s<>]*>/y;
const TAG = /<(?:\/[A-Za-z]|![A-Za-z]|[A-Za-z])[^<>\n]*>/y;
const TAG_RUN = /[^\s<>]*/y;
const BARE_URL = /https?:\/\/[^\s)>]*/y;
const EMAIL = /[\w.+-]+@[\w-]+(?:\.[\w-]+)+/y;
const CITATION = /@[\w:.#$%&\-+?<>~\/]+/y;
const LOCAL = /[\w.+-]/;
const WORD = /[\p{L}\p{N}_]/u;
// Finds the next `]` or whitespace. Used through stopAt, with lastIndex set.
const STOP = /[\]\s]/g;

/// Ranges of a Markdown or Quarto source that are markup rather than prose,
/// for the spellchecker to skip. Half-open [from, to) offsets into `text`.
/// Any order; overlaps allowed.
/// @param {string} text
/// @returns {Array<[number, number]>}
export function markdownSkips(text) {
  const skips = [];
  if (typeof text !== "string") return skips;
  // `memo` remembers closing-delimiter searches; `stop` remembers the last
  // footnote stop search; `tails` maps a closing bracket to where the walk
  // resumes after the markup that follows it; `tailMemo` remembers every
  // tail attempt, failures included, so each closing bracket is matched once.
  const ctx = { text, skips, memo: new Map(), stop: null, tails: new Map(), tailMemo: new Map() };
  const n = text.length;
  let pos = 0;
  while (pos < n) {
    const lineStart = pos === 0 || text[pos - 1] === "\n";
    if (lineStart) {
      const resume = blockAt(ctx, pos);
      if (resume >= 0) {
        pos = resume;
        continue;
      }
    }
    const eol = lineEndOf(text, pos);
    const heading = lineStart && matchAt(HEADING, text, pos) !== null;
    const reached = scanInline(ctx, pos, eol, heading);
    // A construct that ran past the line end resumes where it stopped.
    pos = reached > eol ? reached : eol + 1;
  }
  return skips;
}

// A block construct at the start of the line at `pos`. Returns where scanning
// resumes, or -1 when the line is prose. Every construct moves past `pos`.
function blockAt(ctx, pos) {
  const { text } = ctx;
  const n = text.length;
  const eol = lineEndOf(text, pos);

  // Front matter only counts at the very start of the text.
  if (pos === 0 && text.slice(0, eol).trimEnd() === "---") {
    let end = n;
    for (let p = eol + 1; p < n; ) {
      const e = lineEndOf(text, p);
      const t = text.slice(p, e).trimEnd();
      if (t === "---" || t === "...") {
        end = e;
        break;
      }
      p = e + 1;
    }
    skip(ctx, pos, end);
    return Math.min(end + 1, n);
  }

  // A backtick in the info string makes the line a code span, not a fence.
  const fence = matchAt(FENCE_OPEN, text, pos);
  if (fence && (fence[1][0] !== "`" || !text.slice(pos + fence[0].length, eol).includes("`"))) {
    const end = fenceEnd(text, eol + 1, fence[1][0], fence[1].length);
    skip(ctx, pos, end);
    return Math.min(end + 1, n);
  }

  if (matchAt(CELL_OPTION, text, pos)) {
    skip(ctx, pos, eol);
    return Math.min(eol + 1, n);
  }

  // An unterminated display block runs to the end of the text, as a fence does.
  const display = matchAt(DISPLAY_OPEN, text, pos);
  if (display) {
    const close = find(ctx, "$$", pos + display[0].length);
    const end = close >= 0 ? close + 2 : n;
    skip(ctx, pos, end);
    return end;
  }

  if (matchAt(REFERENCE_DEF, text, pos)) {
    skip(ctx, pos, eol);
    return Math.min(eol + 1, n);
  }

  // A footnote definition's label is markup; the note text after it is prose.
  const note = matchAt(NOTE_DEF, text, pos);
  if (note) {
    const end = pos + note[0].length;
    skip(ctx, pos, end);
    return end;
  }

  // A div marker, then its class name. The rest of the line is prose.
  const div = matchAt(DIV_FENCE, text, pos);
  if (div) {
    const colons = pos + div[0].length;
    skip(ctx, pos, colons);
    const name = matchAt(DIV_NAME, text, colons);
    if (!name) return colons;
    const end = colons + name[0].length;
    skip(ctx, colons, end);
    return end;
  }

  return -1;
}

// The end of the line that closes a fence opened with `len` copies of `ch`,
// searching from `start`. Runs to the end of the text when none closes.
function fenceEnd(text, start, ch, len) {
  const n = text.length;
  for (let p = start; p < n; ) {
    const e = lineEndOf(text, p);
    if (closesFence(text, p, e, ch, len)) return e;
    p = e + 1;
  }
  return n;
}

// Whether the line from `s` to `e` closes a fence: up to three spaces, at
// least `len` copies of `ch`, then nothing but blanks.
function closesFence(text, s, e, ch, len) {
  let i = s;
  let spaces = 0;
  while (i < e && spaces < 3 && text[i] === " ") {
    i += 1;
    spaces += 1;
  }
  let run = 0;
  while (i < e && text[i] === ch) {
    i += 1;
    run += 1;
  }
  if (run < len) return false;
  while (i < e) {
    if (!isBlank(text[i])) return false;
    i += 1;
  }
  return true;
}

// Walks one line from `start` to `eol` and records its inline markup. Returns
// where the walk stopped: `eol`, or later when a construct such as display
// math or an HTML comment ran past the line.
function scanInline(ctx, start, eol, heading) {
  const line = { dollarOpen: true, deadRuns: new Set() };
  let i = start;
  while (i < eol) {
    const ch = ctx.text[i];
    if (ch === "\\") i = escapeAt(ctx, i, eol);
    else if (ch === "`") i = codeAt(ctx, i, eol, line);
    else if (ch === "$") i = mathAt(ctx, i, eol, line);
    else if (ch === "[") i = bracketAt(ctx, i, eol);
    else if (ch === "]") i = closeAt(ctx, i);
    else if (ch === "{") i = braceAt(ctx, i, eol, heading);
    else if (ch === "<") i = angleAt(ctx, i, eol);
    else if (ch === "@") i = citationAt(ctx, i);
    else i = wordAt(ctx, i);
  }
  return i;
}

// A backslash and the character it escapes. An escaped closing bracket still
// ends a link's label, so the walk jumps past that link's markup.
function escapeAt(ctx, i, eol) {
  const end = Math.min(i + 2, eol);
  skip(ctx, i, end);
  return ctx.tails.get(i + 1) ?? end;
}

// A code span. A run with no closing run of the same length on the line is
// literal backticks, and later runs of that length on the line are too.
function codeAt(ctx, i, eol, line) {
  const { text } = ctx;
  let run = i;
  while (run < eol && text[run] === "`") run += 1;
  const width = run - i;
  if (line.deadRuns.has(width)) return run;
  const close = backtickClose(text, run, eol, width);
  if (close < 0) {
    line.deadRuns.add(width);
    return run;
  }
  skip(ctx, i, close);
  return close;
}

// The end of the first run of exactly `width` backticks at or after `j`, or
// -1 when the line has none. Longer and shorter runs do not close.
function backtickClose(text, j, eol, width) {
  while (j < eol) {
    if (text[j] !== "`") {
      j += 1;
      continue;
    }
    let k = j;
    while (k < eol && text[k] === "`") k += 1;
    if (k - j === width) return k;
    j = k;
  }
  return -1;
}

// A dollar sign: display math when doubled, inline math otherwise. Display
// math may run past the line. An unmatched dollar sign is just text.
function mathAt(ctx, i, eol, line) {
  const { text } = ctx;
  if (text[i + 1] === "$") {
    const close = find(ctx, "$$", i + 2);
    if (close < 0) return i + 2;
    skip(ctx, i, close + 2);
    return close + 2;
  }
  // Pandoc: the opener needs a non-space character after it.
  if (!line.dollarOpen || i + 1 >= eol || isSpace(text[i + 1])) return i + 1;
  const close = inlineMathClose(text, i + 2, eol);
  if (close < 0) {
    // Later dollar signs on this line would search the same text and fail too.
    line.dollarOpen = false;
    return i + 1;
  }
  skip(ctx, i, close + 1);
  return close + 1;
}

// The closing dollar sign of inline math, searched from `j`: one that follows
// a non-space character and is not followed by a digit. Escapes are skipped.
function inlineMathClose(text, j, eol) {
  for (; j < eol; j += 1) {
    const c = text[j];
    if (c === "\\") {
      j += 1;
      continue;
    }
    if (c === "$" && !isSpace(text[j - 1]) && !isDigit(text[j + 1])) return j;
  }
  return -1;
}

// A bracket: a footnote marker, or the start of a link, image, reference link
// or span. The markup after the closing bracket (target, reference or
// attributes) is skipped now, and the walk jumps over it when it reaches that
// closing bracket. The label between the brackets is scanned as prose.
function bracketAt(ctx, i, eol) {
  const { text } = ctx;
  if (text[i + 1] === "^") {
    // The label is a run of non-space, non-`]` characters ending in `]`.
    const stop = stopAt(ctx, i + 2);
    if (stop <= i + 2 || text[stop] !== "]") return i + 1;
    skip(ctx, i, stop + 1);
    return stop + 1;
  }
  const close = find(ctx, "]", i + 1);
  if (close < 0 || close >= eol) return i + 1;
  for (let at = close; !ctx.tails.has(at); ) {
    const end = tailAt(ctx, at, eol);
    if (end < 0) break;
    ctx.tails.set(at, end);
    skip(ctx, at + 1, end);
    // An image inside a link leaves the link's own closing bracket after its markup.
    if (end >= eol || text[end] !== "]") break;
    at = end;
  }
  return i + 1;
}

// The result of `linkTailEnd` for the closing bracket at `close`, remembered
// whether it succeeded or failed (-1). Many openers can find the same closing
// bracket, and the match after it is the costly part, so it runs once.
function tailAt(ctx, close, eol) {
  const known = ctx.tailMemo.get(close);
  if (known !== undefined) return known;
  const end = linkTailEnd(ctx, close, eol);
  ctx.tailMemo.set(close, end);
  return end;
}

// The end of the target, reference or attributes that follow the closing
// bracket at `close`, or -1 when none follows. An unterminated target runs to
// the end of the line.
function linkTailEnd(ctx, close, eol) {
  const { text } = ctx;
  const after = close + 1;
  if (text[after] === "(") {
    const paren = find(ctx, ")", after + 1);
    if (paren < 0 || paren >= eol) return eol;
    let end = paren + 1;
    const attributes = matchAt(ATTRIBUTES, text, end);
    if (attributes) end += attributes[0].length;
    return end;
  }
  if (text[after] === "[") {
    const ref = find(ctx, "]", after + 1);
    return ref >= 0 && ref < eol ? ref + 1 : -1;
  }
  if (text[after] === "{") {
    const attributes = matchAt(ATTRIBUTES, text, after);
    return attributes ? after + attributes[0].length : -1;
  }
  return -1;
}

// A closing bracket. When a link's markup was recorded after it, the walk
// resumes after that markup.
function closeAt(ctx, i) {
  const end = ctx.tails.get(i);
  return end === undefined ? i + 1 : end;
}

// The first `]` or whitespace at or after `from`, or the text length when none
// follows. Remembered like `find`: positions only move forward, so a run of
// footnote openers that look for the same stop costs one scan in total.
function stopAt(ctx, from) {
  const memo = ctx.stop;
  if (memo && from >= memo.from && from <= memo.at) return memo.at;
  STOP.lastIndex = from;
  const m = STOP.exec(ctx.text);
  const at = m ? m.index : ctx.text.length;
  ctx.stop = { from, at };
  return at;
}

// A brace: a Quarto shortcode, or the attributes at the end of a heading.
function braceAt(ctx, i, eol, heading) {
  const { text } = ctx;
  if (text[i + 1] === "{" && text[i + 2] === "<") {
    // An unterminated shortcode ends with its line.
    const close = find(ctx, ">}}", i + 3);
    const end = close >= 0 && close + 3 <= eol ? close + 3 : eol;
    skip(ctx, i, end);
    return end;
  }
  if (heading) {
    const attributes = matchAt(HEADING_ATTRIBUTES, text, i);
    if (attributes) {
      skip(ctx, i, i + attributes[0].length);
      return i + attributes[0].length;
    }
  }
  return i + 1;
}

// A tag, an HTML comment, or an autolink. Comments may run past the line.
function angleAt(ctx, i, eol) {
  const { text } = ctx;
  if (text.startsWith("<!--", i)) {
    const close = find(ctx, "-->", i + 4);
    const end = close >= 0 ? close + 3 : eol;
    skip(ctx, i, end);
    return end;
  }
  const len = lengthAt(AUTOLINK, text, i) || lengthAt(TAG, text, i);
  if (len > 0) {
    skip(ctx, i, i + len);
    return i + len;
  }
  // A tag opened here but not closed on the line is markup up to the next
  // space or angle bracket, so the rest of the line stays prose.
  if (/[A-Za-z\/!]/.test(text[i + 1] ?? "")) {
    const end = i + 1 + lengthAt(TAG_RUN, text, i + 1);
    skip(ctx, i, end);
    return end;
  }
  return i + 1;
}

// A citation key. The @ must not follow a word character, so an e-mail
// address is not mistaken for one. Trailing punctuation is prose.
function citationAt(ctx, i) {
  const { text } = ctx;
  const prev = i > 0 ? text[i - 1] : "";
  if (WORD.test(prev)) return i + 1;
  let end = i + lengthAt(CITATION, text, i);
  while (end > i + 1 && ".,;:".includes(text[end - 1])) end -= 1;
  if (end <= i + 1) return i + 1;
  skip(ctx, i, end);
  return end;
}

// A word start, where a bare URL or an e-mail address may begin. Checking only
// word starts keeps the e-mail pattern from retrying inside a word.
function wordAt(ctx, i) {
  const { text } = ctx;
  const prev = i > 0 ? text[i - 1] : "";
  if (text[i] === "h") {
    const len = lengthAt(BARE_URL, text, i);
    if (len > 0) {
      skip(ctx, i, i + len);
      return i + len;
    }
  }
  if (LOCAL.test(text[i]) && !LOCAL.test(prev)) {
    const len = lengthAt(EMAIL, text, i);
    if (len > 0) {
      skip(ctx, i, i + len);
      return i + len;
    }
  }
  return i + 1;
}

// Records a skipped range. Empty ranges are dropped and ends are clamped.
function skip(ctx, from, to) {
  const end = Math.min(to, ctx.text.length);
  if (end > from) ctx.skips.push([from, end]);
}

// The first index of `needle` at or after `from` in the whole text, or -1.
// Callers compare the index with their own line end. The answer is remembered,
// so a run of openers that all look for the same closer scans the text once.
function find(ctx, needle, from) {
  const memo = ctx.memo.get(needle);
  if (memo && from >= memo.from && (memo.at < 0 || from <= memo.at)) return memo.at;
  const at = ctx.text.indexOf(needle, from);
  ctx.memo.set(needle, { from, at });
  return at;
}

// Runs a sticky pattern at `at`, or returns null.
function matchAt(re, text, at) {
  re.lastIndex = at;
  return re.exec(text);
}

// The length of a sticky pattern's match at `at`, or 0.
function lengthAt(re, text, at) {
  const m = matchAt(re, text, at);
  return m ? m[0].length : 0;
}

// The index of the newline that ends the line containing `from`, or the text
// length when that line is the last one.
function lineEndOf(text, from) {
  const at = text.indexOf("\n", from);
  return at < 0 ? text.length : at;
}

function isBlank(c) {
  return c === " " || c === "\t" || c === "\r";
}

// Pandoc's rule counts the text edge as a space.
function isSpace(c) {
  return c === undefined || c === "" || /\s/.test(c);
}

function isDigit(c) {
  return c !== undefined && c >= "0" && c <= "9";
}
