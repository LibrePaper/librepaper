// Which parts of a source the browser's spellchecker should skip. The browser
// does the checking; each format's scanner marks the markup it should not see
// (commands, math, citation keys, code), and the editor sets spellcheck="false"
// on those spans. The scanners read the text rather than the editor's syntax
// tree because the LaTeX and Typst grammars here are line-based highlighters
// that do not know which arguments hold names.
import { latexSkips } from "./latex.js";
import { typstSkips } from "./typst.js";
import { markdownSkips } from "./markdown.js";
import { htmlSkips } from "./html.js";

const SCANNERS = new Map([
  ["latex", latexSkips],
  ["typst", typstSkips],
  ["markdown", markdownSkips],
  ["quarto", markdownSkips],
  ["html", htmlSkips],
]);

/**
 * The parts of a source the spellchecker should skip: sorted, merged,
 * non-empty [from, to) ranges.
 * @param {string} text
 * @param {string} format  "latex" | "typst" | "markdown" | "quarto" | "html" | anything else
 * @returns {Array<[number, number]>}
 */
export function skipRanges(text, format) {
  const scan = SCANNERS.get(format);
  if (!scan) return [];
  const end = text.length;
  const clamped = scan(text)
    .map(([from, to]) => [Math.max(0, Math.min(from, end)), Math.max(0, Math.min(to, end))])
    .filter(([from, to]) => to > from)
    .sort((a, b) => a[0] - b[0]);
  /** @type {Array<[number, number]>} */
  const merged = [];
  for (const [from, to] of clamped) {
    const last = merged[merged.length - 1];
    if (last && from <= last[1]) last[1] = Math.max(last[1], to);
    else merged.push([from, to]);
  }
  return merged;
}
