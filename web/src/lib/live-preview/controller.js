// Decides, for each edit, whether the preview on screen may be patched at
// once, and sends the patch to the frame. The real compile replaces the
// patched preview a moment later.
//
// The rendered page's `data-sourcepos` lines describe the text of the last
// compile. A patch addresses a line of the text before the edit, so its line
// is mapped to the compiled text first. A line in text the compile has not
// seen gets no patch.

import { livePatch } from "./patch.js";

const isTex = (path) => /\.tex$/i.test(path);

/**
 * The line of `compiled` that line `line` of `current` corresponds to, or null
 * when it lies in text the compile did not have. Uses the common run of lines
 * at the start and at the end of the two texts. When both have the same number
 * of lines the changed lines are edits in place and keep their number.
 * @param {string} compiled
 * @param {string} current
 * @param {number} line 1-based
 * @returns {number | null}
 */
function mapLine(compiled, current, line) {
  const a = compiled.split("\n");
  const b = current.split("\n");
  const max = Math.min(a.length, b.length);
  let head = 0;
  while (head < max && a[head] === b[head]) head++;
  let tail = 0;
  while (tail < max - head && a[a.length - 1 - tail] === b[b.length - 1 - tail]) tail++;
  if (line <= head) return line;
  if (line > b.length - tail) return line - (b.length - a.length);
  return a.length === b.length ? line : null;
}

/**
 * @param {{ send: (message: { type: "preview-patch", patch: object }) => void }} options
 */
export function createLivePreview({ send }) {
  /** @type {Map<string, string>} */
  let texts = new Map();
  /** @type {Map<string, string>} the texts as of the last compile */
  let compiledTexts = new Map();

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
      if (changed.length !== 1) return;
      const [path] = changed;
      if (!isTex(path) || !compiledTexts.has(path)) return;
      if (!previous.has(path) || !texts.has(path)) return;
      const patch = livePatch({ path, oldText: previous.get(path), newText: texts.get(path), isMain: path === main });
      if (!patch) return;
      const line = mapLine(compiledTexts.get(path), previous.get(path), patch.line);
      if (line !== null) send({ type: "preview-patch", patch: { ...patch, line } });
    },

    /** @param {Record<string, string>} next the texts that were compiled */
    compiled(next) {
      texts = remember(next);
      compiledTexts = new Map(texts);
    },

    reset() {
      texts = new Map();
      compiledTexts = new Map();
    },
  };
}
