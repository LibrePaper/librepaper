// Aligns a block's source text with the text its rendering shows, so that a
// range of the source (a comment, a tracked change, a lint) can be found in
// the rendered DOM. Pure strings and numbers: the DOM walk lives in overlays.js.
//
// The alignment is a character diff (Myers' O(ND) algorithm). Characters the
// diff keeps are the ones both texts share; markup (`*`, `#`, link targets)
// is source-only and stays unmatched, and rendered-only text (list numbers)
// is skipped.

/** The longest pair of texts, in characters together, that is aligned. */
export const MAX_LENGTH = 20000;
/** The most edits (insertions plus deletions) the diff searches for. */
export const MAX_EDITS = 1500;

/** Every whitespace character becomes a plain space, one for one, so offsets keep. */
export function normalizeSpace(text) {
  return text.replace(/\s/g, " ");
}

/**
 * For each character of `a`, the index of the character of `b` it is aligned
 * with, or -1. Null when the texts are too long or too different to align.
 * @param {string} a the source
 * @param {string} b the rendered text
 * @param {{ maxLength?: number, maxEdits?: number }} [limits]
 * @returns {Int32Array | null}
 */
export function alignText(a, b, { maxLength = MAX_LENGTH, maxEdits = MAX_EDITS } = {}) {
  if (a.length + b.length > maxLength) return null;
  const map = new Int32Array(a.length).fill(-1);
  let start = 0;
  while (start < a.length && start < b.length && a[start] === b[start]) {
    map[start] = start;
    start++;
  }
  let endA = a.length;
  let endB = b.length;
  while (endA > start && endB > start && a[endA - 1] === b[endB - 1]) {
    endA--;
    endB--;
    map[endA] = endB;
  }
  const n = endA - start;
  const m = endB - start;
  if (n === 0 || m === 0) return map;

  const limit = Math.min(n + m, maxEdits);
  const offset = limit + 1;
  const v = new Int32Array(2 * limit + 3);
  /** @type {Int32Array[]} trace[d] holds v[-d..d] after round d */
  const trace = [];
  let found = -1;
  for (let d = 0; d <= limit && found < 0; d++) {
    for (let k = -d; k <= d; k += 2) {
      let x;
      if (k === -d || (k !== d && v[offset + k - 1] < v[offset + k + 1])) x = v[offset + k + 1];
      else x = v[offset + k - 1] + 1;
      let y = x - k;
      while (x < n && y < m && a.charCodeAt(start + x) === b.charCodeAt(start + y)) {
        x++;
        y++;
      }
      v[offset + k] = x;
      if (x >= n && y >= m) found = d;
    }
    trace.push(v.slice(offset - d, offset + d + 1));
  }
  if (found < 0) return null;

  let x = n;
  let y = m;
  for (let d = found; d > 0; d--) {
    const previous = trace[d - 1];
    const at = (k) => previous[k + d - 1];
    const k = x - y;
    const down = k === -d || (k !== d && at(k - 1) < at(k + 1));
    const previousK = down ? k + 1 : k - 1;
    const previousX = at(previousK);
    const previousY = previousX - previousK;
    const midX = down ? previousX : previousX + 1;
    const midY = down ? previousY + 1 : previousY;
    for (let i = 0; i < x - midX; i++) map[start + midX + i] = start + midY + i;
    x = previousX;
    y = previousY;
  }
  for (let i = 0; i < x; i++) map[start + i] = start + i;
  return map;
}

/**
 * The rendered range a source range lands on: from the first non-blank
 * aligned character to just after the last. Null when no character of the
 * range is aligned, as for a mark over pure markup.
 * @param {Int32Array} map from alignText
 * @param {string} source the text the map was made from
 * @param {number} from
 * @param {number} to
 * @returns {{ from: number, to: number } | null}
 */
export function mapRange(map, source, from, to) {
  let first = -1;
  let last = -1;
  const end = Math.min(to, map.length);
  for (let i = Math.max(from, 0); i < end; i++) {
    if (map[i] < 0 || /\s/.test(source[i])) continue;
    if (first < 0) first = map[i];
    last = map[i];
  }
  return first < 0 ? null : { from: first, to: last + 1 };
}

/**
 * The rendered offset a source position lands at: before the next aligned
 * character, else after the previous one. Null when nothing is aligned.
 * @param {Int32Array} map
 * @param {number} position
 * @returns {number | null}
 */
export function mapPoint(map, position) {
  for (let i = Math.max(position, 0); i < map.length; i++) if (map[i] >= 0) return map[i];
  for (let i = Math.min(position, map.length) - 1; i >= 0; i--) if (map[i] >= 0) return map[i] + 1;
  return null;
}
