import { Change, presentableDiff } from "@codemirror/merge";

const WORD = /[\p{L}\p{N}_]/u;

/** @param {string | undefined} ch */
function isWord(ch) {
  return ch !== undefined && ch !== "" && WORD.test(ch);
}

/**
 * Diff two strings, then widen every change outward to word boundaries so a
 * changed word is highlighted whole. Widening never crosses a newline.
 * @param {string} a
 * @param {string} b
 * @param {{ scanLimit?: number, timeout?: number }} [config]
 * @returns {Change[]}
 */
export function wordDiff(a, b, config = {}) {
  const raw = presentableDiff(a, b, { scanLimit: config.scanLimit, timeout: config.timeout });
  /** @type {{ fromA: number, toA: number, fromB: number, toB: number }[]} */
  const widened = [];
  for (let i = 0; i < raw.length; i++) {
    const c = raw[i];
    const prevEnd = i ? raw[i - 1].toA : 0;
    const nextStart = i + 1 < raw.length ? raw[i + 1].fromA : a.length;
    const gapBefore = c.fromA - prevEnd;
    const gapAfter = nextStart - c.toA;
    let left = 0;
    while (left < gapBefore
      && ((isWord(a[c.fromA - 1 - left]) && isWord(a[c.fromA - left]))
        || (isWord(b[c.fromB - 1 - left]) && isWord(b[c.fromB - left])))) left++;
    let right = 0;
    while (right < gapAfter
      && ((isWord(a[c.toA + right - 1]) && isWord(a[c.toA + right]))
        || (isWord(b[c.toB + right - 1]) && isWord(b[c.toB + right])))) right++;
    widened.push({ fromA: c.fromA - left, toA: c.toA + right, fromB: c.fromB - left, toB: c.toB + right });
  }
  /** @type {Change[]} */
  const out = [];
  /** @type {{ fromA: number, toA: number, fromB: number, toB: number } | null} */
  let cur = null;
  for (const w of widened) {
    // Merge changes that touch, overlap, or are separated by one space/tab.
    const gap = cur ? a.slice(cur.toA, w.fromA) : "";
    if (cur && (w.fromA <= cur.toA || gap === " " || gap === "\t")) {
      cur.toA = Math.max(cur.toA, w.toA);
      cur.toB = Math.max(cur.toB, w.toB);
    } else {
      if (cur) out.push(new Change(cur.fromA, cur.toA, cur.fromB, cur.toB));
      cur = { ...w };
    }
  }
  if (cur) out.push(new Change(cur.fromA, cur.toA, cur.fromB, cur.toB));
  return out;
}
