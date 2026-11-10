// Decides, for each edit, whether the preview on screen may be patched at
// once, and sends the patch to the frame. The real compile replaces the
// patched preview a moment later.
//
// The rendered page's `data-sourcepos` lines describe the text of the last
// compile. A patch addresses a line of the text before the edit, so its line
// is mapped back to the compiled text first. Each edit that adds or removes a
// newline is recorded as a line operation since the last compile, and the
// mapping walks those operations backwards. A line in text the compile has
// not seen gets no patch.

import { livePatch } from "./patch.js";

const isTex = (path) => /\.tex$/i.test(path);

/**
 * The single changed span between two texts: common prefix and common suffix,
 * by character. Null when the texts are equal.
 * @param {string} a
 * @param {string} b
 * @returns {{ from: number, oldTo: number, newTo: number } | null}
 */
function changedSpan(a, b) {
  const max = Math.min(a.length, b.length);
  let from = 0;
  while (from < max && a[from] === b[from]) from++;
  let tail = 0;
  while (tail < max - from && a[a.length - 1 - tail] === b[b.length - 1 - tail]) tail++;
  if (from === a.length - tail && from === b.length - tail) return null;
  return { from, oldTo: a.length - tail, newTo: b.length - tail };
}

/**
 * The line operation an edit of `previous` into `next` performed, or null when
 * it changed no newline.
 * @param {string} previous
 * @param {string} next
 * @returns {{ at: number, removed: number, added: number, lineStart: boolean } | null}
 */
function lineOperation(previous, next) {
  const span = changedSpan(previous, next);
  if (!span) return null;
  const { from, oldTo, newTo } = span;
  const removed = previous.slice(from, oldTo).split("\n").length - 1;
  const added = next.slice(from, newTo).split("\n").length - 1;
  if (removed === 0 && added === 0) return null;
  const at = previous.slice(0, from).split("\n").length;
  // An edit that begins at the start of a line pushes that whole line down;
  // one that begins inside it leaves the line's start where it was.
  const lineStart = from === 0 || previous[from - 1] === "\n";
  return { at, removed, added, lineStart };
}

/**
 * Maps a line of the text after the last operation back to the compiled text.
 * Operations are applied from last to first. Null when the line is one the
 * operations added, since the compile never had it.
 * @param {Array<{ at: number, removed: number, added: number, lineStart: boolean }>} ops
 * @param {number} line 1-based
 * @returns {number | null}
 */
function mapThrough(ops, line) {
  for (let i = ops.length - 1; i >= 0; i--) {
    const { at, removed, added, lineStart } = ops[i];
    if (line < at) continue;
    if (lineStart) {
      // Lines at .. at + added - 1 are new; the old line `at` is now at + added.
      if (line < at + added) return null;
    } else {
      // Line `at` keeps its start; the lines after it up to at + added are new.
      if (line === at) continue;
      if (line <= at + added) return null;
    }
    line = line - added + removed;
  }
  return line;
}

/**
 * @param {{ send: (message: { type: "preview-patch", patch: object }) => void }} options
 */
export function createLivePreview({ send }) {
  /** @type {Map<string, string>} */
  let texts = new Map();
  /** @type {Map<string, string>} the texts as of the last compile */
  let compiledTexts = new Map();
  /**
   * Per path, the line operations since the last compile. A null entry marks a
   * path that was added or removed, whose lines can no longer be mapped.
   * @type {Map<string, Array<{ at: number, removed: number, added: number, lineStart: boolean }> | null>}
   */
  let ops = new Map();

  const remember = (next) => new Map(Object.entries(next));

  return {
    /**
     * @param {Record<string, string>} next every text, by path
     * @param {string} main the entry file
     */
    edited(next, main) {
      const previous = texts;
      texts = remember(next);
      const changed = [];
      for (const [path, text] of texts) if (previous.get(path) !== text) changed.push(path);
      for (const path of previous.keys()) if (!texts.has(path)) changed.push(path);

      // The patch is mapped through the operations recorded before this edit.
      let patch = null;
      let line = null;
      if (changed.length === 1) {
        const [path] = changed;
        if (isTex(path) && compiledTexts.has(path) && previous.has(path) && texts.has(path)) {
          patch = livePatch({ path, oldText: previous.get(path), newText: texts.get(path), isMain: path === main });
          const list = ops.get(path);
          if (patch && list !== null) line = mapThrough(list ?? [], patch.line);
        }
      }

      for (const path of changed) {
        if (!isTex(path) || !compiledTexts.has(path)) continue;
        if (!previous.has(path) || !texts.has(path)) {
          ops.set(path, null);
          continue;
        }
        const operation = lineOperation(previous.get(path), texts.get(path));
        if (!operation) continue;
        const list = ops.get(path);
        if (list === null) continue;
        ops.set(path, [...(list ?? []), operation]);
      }

      if (patch && line !== null) send({ type: "preview-patch", patch: { ...patch, line } });
    },

    /** @param {Record<string, string>} next the texts that were compiled */
    compiled(next) {
      texts = remember(next);
      compiledTexts = new Map(texts);
      ops = new Map();
    },

    reset() {
      texts = new Map();
      compiledTexts = new Map();
      ops = new Map();
    },
  };
}
