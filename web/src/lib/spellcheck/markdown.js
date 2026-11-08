// Markup ranges for the spellchecker. The browser spellchecks whatever the
// source editor shows, so markup it cannot read (fences, math, link targets,
// attributes) is reported as ranges to leave alone. The ranges come from the
// parse tree of the Pandoc Markdown parser, so they agree with highlighting and
// the document index. A few things the grammar does not model (shortcodes,
// cell option lines, bare URLs, e-mail addresses, footnote markers) are found
// by scanning the text that is still prose.
//
// Nothing here backtracks. The regexes are flat, and each search for a closing
// delimiter is remembered, so a line full of unmatched openers costs one scan
// rather than one scan per opener.

import { frontMatterRange, markdownTree } from "../source-trees.js";

// Nodes that are markup from end to end. Link text, headings and the rest stay prose.
const WHOLE = new Set([
  "FencedCode", "CodeBlock", "InlineCode",
  "InlineMath", "DisplayMath",
  "Attributes", "FencedDivMark",
  "CitationKey", "CrossRef",
  "Autolink", "URL", "HTMLTag", "Comment", "CommentBlock",
  "ProcessingInstruction", "ProcessingInstructionBlock",
  "Escape", "Entity",
  "EmphasisMark", "StrikethroughMark", "SubscriptMark", "SuperscriptMark", "TaskMarker",
]);

// Skipped ranges are blanked with this before the prose scan. It is white
// space, so no pattern runs across it, and it is not a line break.
const MASK = " ";

// Quarto shortcodes, which end with their line when unterminated, and cell
// option lines. Cell options sit inside fences, so a hit there repeats a skip.
const LINE_MARKUP = /\{\{<(?:[^\n>]|>(?!\}\}))*(?:>\}\}|(?=\n|$))|^[ \t]*#\|[^\n]*/gm;

// Markup in prose, one group per kind: bare URL, e-mail address, footnote
// marker, the target after a closing bracket, HTML comment, HTML tag, a tag
// opened but not closed, and a display math opener at the start of a line.
// Inline HTML is a node already; these catch raw HTML blocks and broken markup.
const PROSE = new RegExp(
  [
    String.raw`(https?:\/\/[^\s)>]*)`,
    String.raw`((?<![\w.+-])[\w.+-]+@[\w-]+(?:\.[\w-]+)+)`,
    String.raw`(\[\^)`,
    String.raw`(\]\()`,
    String.raw`(<!--)`,
    String.raw`(<(?:\/[A-Za-z]|![A-Za-z]|[A-Za-z])[^<>\n]*>)`,
    String.raw`(<[A-Za-z\/!][^\s<>]*)`,
    String.raw`(^ {0,3}\$\$)`,
  ].join("|"),
  "gm",
);
// Finds the next `]` or white space. Used with lastIndex set.
const STOP = /[\]\s]/g;

/// Ranges of a Markdown or Quarto source that are markup rather than prose,
/// for the spellchecker to skip. Half-open [from, to) offsets into `text`.
/// Any order; overlaps allowed.
/// @param {string} text
/// @returns {Array<[number, number]>}
export function markdownSkips(text) {
  /** @type {Array<[number, number]>} */
  const skips = [];
  if (typeof text !== "string" || !text) return skips;
  // Front matter still being typed has no closing line yet. The index reads
  // the text as Markdown then, but its YAML keys are not prose to check.
  if (/^---[ \t]*\r?(?:\n|$)/.test(text) && !frontMatterRange(text)) return [[0, text.length]];
  const { tree, body, frontMatter } = markdownTree(text);
  const n = body.length;
  const skip = (from, to) => {
    const end = Math.min(to, n);
    if (end > from) skips.push([from, end]);
  };

  // Front matter is skipped whole, keys and values alike.
  if (frontMatter) skip(frontMatter[0], frontMatter[1]);

  tree.iterate({
    enter(node) {
      if (WHOLE.has(node.name)) {
        skip(node.from, node.to);
        return false;
      }
      if (node.name === "LinkReference") {
        // A footnote definition's label is markup and the note is prose; the
        // label is found with the other footnote markers below.
        if (body.charCodeAt(node.from + 1) !== 94) skip(node.from, node.to);
        return false;
      }
      if (node.name === "Link" || node.name === "Image") {
        // The label stays prose. Everything after its closing bracket (target,
        // title, reference) is markup. The second mark is that bracket.
        let marks = 0;
        for (let child = node.node.firstChild; child; child = child.nextSibling) {
          if (child.name === "LinkMark" && ++marks === 2) {
            skip(child.to, node.to);
            break;
          }
        }
      }
      return true;
    },
  });

  for (const m of body.matchAll(LINE_MARKUP)) skip(m.index, m.index + m[0].length);

  scanProse(blank(body, skips), skip);
  return skips;
}

// `text` with every range replaced by MASK, so what remains is prose.
function blank(text, ranges) {
  const sorted = ranges.slice().sort((a, b) => a[0] - b[0]);
  let out = "";
  let at = 0;
  for (const [from, to] of sorted) {
    if (to <= at) continue;
    const start = Math.max(from, at);
    out += text.slice(at, start) + MASK.repeat(to - start);
    at = to;
  }
  return out + text.slice(at);
}

// Finds markup in the prose that remains, one linear pass. The finders
// remember their answer: positions only move forward, so a run of openers that
// look for the same closer scans the text once.
function scanProse(prose, skip) {
  const n = prose.length;
  const newline = finder(prose, "\n");
  const paren = finder(prose, ")");
  const comment = finder(prose, "-->");
  const dollars = finder(prose, "$$");
  const eolOf = (i) => {
    const at = newline(i);
    return at < 0 ? n : at;
  };
  // The first `]` or white space at or after `from`, or the text length.
  let stopFrom = -1;
  let stopAt = -1;
  const stopOf = (from) => {
    if (stopFrom >= 0 && from >= stopFrom && from <= stopAt) return stopAt;
    STOP.lastIndex = from;
    const m = STOP.exec(prose);
    stopFrom = from;
    stopAt = m ? m.index : n;
    return stopAt;
  };

  PROSE.lastIndex = 0;
  for (let m; (m = PROSE.exec(prose)); ) {
    const i = m.index;
    if (m[3]) {
      // A footnote marker: a run of non-space, non-`]` characters ending in `]`.
      const stop = stopOf(i + 2);
      if (stop > i + 2 && prose[stop] === "]") {
        skip(i, stop + 1);
        PROSE.lastIndex = stop + 1;
      } else PROSE.lastIndex = i + 1;
    } else if (m[4]) {
      // A target the parser did not accept still ends at its `)`, or at the
      // end of the line when unterminated.
      const close = paren(i + 2);
      const eol = eolOf(i);
      const end = close >= 0 && close < eol ? close + 1 : eol;
      skip(i + 1, end);
      PROSE.lastIndex = end;
    } else if (m[5]) {
      // A comment that is never closed ends with its line.
      const close = comment(i + 4);
      const end = close >= 0 ? close + 3 : eolOf(i);
      skip(i, end);
      PROSE.lastIndex = end;
    } else if (m[8]) {
      // A display math opener the parser left as text runs to the next `$$`,
      // or to the end of the text.
      const close = dollars(i + m[0].length);
      const end = close >= 0 ? close + 2 : n;
      skip(i, end);
      PROSE.lastIndex = end;
    } else skip(i, i + m[0].length);
  }
}

// A function giving the first index of `needle` at or after its argument, or -1.
function finder(text, needle) {
  let from = -1;
  let at = -1;
  return (start) => {
    if (from >= 0 && start >= from && (at < 0 || start <= at)) return at;
    from = start;
    at = text.indexOf(needle, start);
    return at;
  };
}
