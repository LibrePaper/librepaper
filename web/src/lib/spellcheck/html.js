/// Ranges of HTML source that are markup rather than prose, for the
/// spellchecker to skip. The scanner walks the string once. Each construct
/// is consumed as it is found, and a search for a closing delimiter that
/// fails is never repeated, so the cost stays linear on hostile input.

const RAW_ELEMENTS = new Set(["script", "style", "pre", "code", "math", "svg", "kbd", "samp"]);

// Attributes whose quoted values are words a reader sees, so they stay prose.
const PROSE_ATTRIBUTES = new Set(["alt", "title"]);

const ENTITY = /&(?:[A-Za-z][A-Za-z0-9]*|#[0-9]+|#[xX][0-9A-Fa-f]+);/y;
const URL = /https?:\/\/[^\s<]*/iy;

/// Ranges of an HTML source that are markup rather than prose, for the
/// spellchecker to skip. Half-open [from, to) offsets into `text` (UTF-16, as
/// JavaScript strings index). Any order; overlaps allowed.
/// @param {string} text
/// @returns {Array<[number, number]>}
export function htmlSkips(text) {
  if (typeof text !== "string") return [];
  const n = text.length;
  const out = [];
  // Once a closing delimiter is known to be missing, no later opener can
  // find one either, so the search is not repeated.
  const noClose = new Set();
  // The ASCII-lowercased text, built on first use. Only ASCII letters change
  // case, so its indices match the original text.
  let lower = null;
  let i = 0;
  while (i < n) {
    const c = text[i];
    const next = text[i + 1];
    if (c === "<" && next === "!") {
      let end;
      if (text.startsWith("<!--", i)) end = after(text.indexOf("-->", i + 4), 3, n);
      else if (text.startsWith("<![CDATA[", i)) end = after(text.indexOf("]]>", i + 9), 3, n);
      else end = after(text.indexOf(">", i + 2), 1, n);
      push(out, i, end);
      i = end;
    } else if (c === "<" && next === "?") {
      const end = after(text.indexOf("?>", i + 2), 2, n);
      push(out, i, end);
      i = end;
    } else if (c === "<" && (next === "/" || isAsciiLetter(next))) {
      const end = skipTag(text, i, out);
      // A start tag that is not self-closing opens a raw element, whose body
      // is markup or code and so is skipped whole.
      const selfClosing = text[end - 1] === ">" && text[end - 2] === "/";
      const name = next !== "/" && !selfClosing ? tagNameAt(text, i + 1) : "";
      if (RAW_ELEMENTS.has(name)) {
        if (lower === null) lower = asciiLower(text);
        const stop = rawEnd(text, lower, name, end);
        push(out, end, stop);
        i = stop;
      } else {
        i = end;
      }
    } else if (c === "&") {
      ENTITY.lastIndex = i;
      const match = ENTITY.exec(text);
      if (match) {
        push(out, i, i + match[0].length);
        i += match[0].length;
      } else {
        i++;
      }
    } else if (c === "\\" && (next === "(" || next === "[")) {
      // TeX math in HTML pages: \(...\) and \[...\]. A lone opener is prose.
      const closer = next === "(" ? "\\)" : "\\]";
      const at = noClose.has(next) ? -1 : text.indexOf(closer, i + 2);
      if (at === -1) {
        noClose.add(next);
        i++;
      } else {
        push(out, i, at + 2);
        i = at + 2;
      }
    } else if (c === "h" || c === "H") {
      URL.lastIndex = i;
      const match = URL.exec(text);
      if (match) {
        push(out, i, i + match[0].length);
        i += match[0].length;
      } else {
        i++;
      }
    } else {
      i++;
    }
  }
  return out;
}

function push(out, from, to) {
  if (from < to) out.push([from, to]);
}

// The index just past a delimiter found at `at`, or the end of the text when
// the delimiter is missing: an unterminated construct runs to the end.
function after(at, length, n) {
  return at === -1 ? n : at + length;
}

// The text with only its ASCII letters lowercased. Other characters, and the
// length of the string, are unchanged.
function asciiLower(text) {
  return text.replace(/[A-Z]+/g, (m) => m.toLowerCase());
}

// The index just past the end tag of the raw element `name` that is searched
// from `from` on, or the end of the text when there is none. The search runs
// forward only, and a missing `>` ends it, since no later candidate can have
// one either. `lower` is asciiLower(text).
function rawEnd(text, lower, name, from) {
  const n = text.length;
  const needle = `</${name}`;
  let at = lower.indexOf(needle, from);
  while (at !== -1) {
    const after = at + needle.length;
    // The name must end here, so `</scripts` is not an end tag.
    if (after >= n || !/[A-Za-z0-9_-]/.test(text[after])) {
      const gt = text.indexOf(">", after);
      return gt === -1 ? n : gt + 1;
    }
    at = lower.indexOf(needle, at + 1);
  }
  return n;
}

function isAsciiLetter(ch) {
  return ch !== undefined && /^[A-Za-z]$/.test(ch);
}

// The lowercased element name that starts at `from`.
function tagNameAt(text, from) {
  let to = from;
  while (to < text.length && /[A-Za-z0-9-]/.test(text[to])) to++;
  return text.slice(from, to).toLowerCase();
}

// Skips the tag that starts at `start` (a `<`) and returns the index just
// past it. A quoted `>` does not end the tag. The quoted value of an alt or
// title attribute is prose, so only the parts around it are skipped.
function skipTag(text, start, out) {
  const n = text.length;
  let segStart = start;
  let i = start + 1;
  let attr = "";
  let afterEquals = false;
  while (i < n) {
    const c = text[i];
    if (c === ">") {
      i++;
      push(out, segStart, i);
      return i;
    }
    if (c === '"' || c === "'") {
      const close = text.indexOf(c, i + 1);
      if (afterEquals && PROSE_ATTRIBUTES.has(attr)) {
        push(out, segStart, i + 1);
        segStart = close === -1 ? n : close;
      }
      i = close === -1 ? n : close + 1;
      attr = "";
      afterEquals = false;
    } else if (c === "=") {
      afterEquals = true;
      i++;
    } else if (/\s/.test(c)) {
      i++;
    } else {
      // A bare word is an attribute name, or an unquoted value after `=`.
      let j = i;
      while (j < n && !/[\s"'=>]/.test(text[j])) j++;
      attr = afterEquals ? "" : text.slice(i, j).toLowerCase();
      afterEquals = false;
      i = j;
    }
  }
  // An unterminated tag runs to the end of the text.
  push(out, segStart, n);
  return n;
}
