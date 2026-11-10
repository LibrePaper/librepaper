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

import { livePatch, paragraphText } from "./patch.js";

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
 * Operations are applied from last to first. A line the operations added has no
 * compiled line: with `created: "null"` the answer is null; with
 * `created: "nearest"` it is the nearest line above that existed, which may be
 * a wrong guess that the frame's exact text match then rejects.
 * @param {Array<{ at: number, removed: number, added: number, lineStart: boolean }>} ops
 * @param {number} line 1-based
 * @param {{ created?: "null" | "nearest" }} [options]
 * @returns {number | null}
 */
function mapThrough(ops, line, { created = "null" } = {}) {
  for (let i = ops.length - 1; i >= 0; i--) {
    const { at, removed, added, lineStart } = ops[i];
    if (line < at) continue;
    if (lineStart) {
      // Lines at .. at + added - 1 are new; the old line `at` is now at + added.
      if (line < at + added) {
        if (created === "null" || at <= 1) return null;
        line = at - 1;
        continue;
      }
    } else {
      // Line `at` keeps its start; the lines after it up to at + added are new.
      if (line === at) continue;
      if (line <= at + added) {
        if (created === "null") return null;
        line = at;
        continue;
      }
    }
    line = line - added + removed;
  }
  return line;
}

/**
 * The paragraph patch for an edit that created a paragraph the last compile
 * has not seen, or null.
 * @param {string} path
 * @param {string} oldText
 * @param {string} newText
 * @param {string} main
 * @param {Array<{ at: number, removed: number, added: number, lineStart: boolean }>} list operations before this edit
 */
function newParagraph(path, oldText, newText, main, list) {
  const span = changedSpan(oldText, newText);
  if (!span) return null;
  const found = paragraphText({ path, text: newText, at: span.from, isMain: path === main });
  if (!found) return null;
  const operation = lineOperation(oldText, newText);
  const all = operation ? [...list, operation] : list;
  for (let l = found.startLine; l <= found.endLine; l++) {
    if (mapThrough(all, l) !== null) return null;
  }
  const lines = newText.split("\n");
  for (let l = found.startLine - 1; l >= 1; l--) {
    if (lines[l - 1].trim() === "") continue;
    const anchorLine = mapThrough(all, l);
    if (anchorLine !== null) return { kind: "paragraph", file: path, anchorLine, text: found.text };
  }
  return null;
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
      /** @type {object | null} */
      let paragraph = null;
      if (changed.length === 1) {
        const [path] = changed;
        if (isTex(path) && compiledTexts.has(path) && previous.has(path) && texts.has(path)) {
          const oldText = previous.get(path);
          const newText = texts.get(path);
          const list = ops.get(path);
          patch = livePatch({ path, oldText, newText, isMain: path === main });
          if (patch && list !== null) line = mapThrough(list ?? [], patch.line, { created: "nearest" });
          if (line === null && list !== null) paragraph = newParagraph(path, oldText, newText, main, list ?? []);
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
      else if (paragraph) send({ type: "preview-patch", patch: paragraph });
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
