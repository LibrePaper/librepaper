// What Harper is shown. It checks prose, so the markup the spellchecker already
// skips is blanked before the text goes in. Blanking keeps every offset, so a
// lint maps straight back onto the editor's document.
import { skipRanges } from "../spellcheck/index.js";

/**
 * The text with each skipped character replaced by a space (line breaks are
 * kept so paragraphs stay apart), and the skip ranges themselves.
 * @param {string} text
 * @param {string} format
 * @returns {{ text: string, skips: Array<[number, number]> }}
 */
export function proseText(text, format) {
  const skips = skipRanges(text, format);
  let out = "";
  let at = 0;
  for (const [from, to] of skips) {
    out += text.slice(at, from) + text.slice(from, to).replace(/[^\r\n]/g, " ");
    at = to;
  }
  return { text: out + text.slice(at), skips };
}

/**
 * Whether a lint is worth showing. Masked markup becomes runs of spaces that
 * Harper reports as formatting problems, and a lint that touches real markup
 * is about something the writer did not write as prose.
 * @param {{ from: number, to: number, problem?: string }} lint
 * @param {Array<[number, number]>} skips  sorted, disjoint
 */
export function keepLint(lint, skips) {
  if (typeof lint.problem === "string" && lint.problem.trim() === "") return false;
  let low = 0;
  let high = skips.length;
  // The first range that ends after the lint starts.
  while (low < high) {
    const mid = (low + high) >> 1;
    if (skips[mid][1] <= lint.from) low = mid + 1;
    else high = mid;
  }
  return !(low < skips.length && skips[low][0] < lint.to);
}

/**
 * The paragraphs that [from, to) touches, each widened to its blank-line
 * edges. Blank lines belong to no paragraph.
 * @param {string} text
 * @param {number} from
 * @param {number} to
 * @returns {Array<{ from: number, to: number }>}
 */
export function paragraphs(text, from, to) {
  /** @type {Array<{ from: number, to: number }>} */
  const found = [];
  let start = -1;
  let end = -1;
  let at = 0;
  while (at <= text.length) {
    let next = text.indexOf("\n", at);
    if (next === -1) next = text.length;
    const line = text.slice(at, next);
    if (line.trim() === "") {
      if (start !== -1) found.push({ from: start, to: end });
      start = -1;
    } else {
      if (start === -1) start = at;
      end = at + line.replace(/\s+$/, "").length;
    }
    at = next + 1;
  }
  if (start !== -1) found.push({ from: start, to: end });
  return found.filter((p) => p.from <= to && p.to >= from);
}
