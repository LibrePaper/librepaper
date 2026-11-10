// Decides, for each edit, whether the preview on screen may be patched at
// once, and sends the patch to the frame. The real compile replaces the
// patched preview a moment later.
//
// The rendered page's `data-sourcepos` lines describe the text of the last
// compile. A patch addresses a line, so it is only sent while the edited
// file still has the line count that compile saw.

import { livePatch } from "./patch.js";

const isTex = (path) => /\.tex$/i.test(path);
const lineCount = (text) => String(text).split("\n").length;

/**
 * @param {{ send: (message: { type: "preview-patch", patch: object }) => void }} options
 */
export function createLivePreview({ send }) {
  /** @type {Map<string, string>} */
  let texts = new Map();
  /** @type {Map<string, number>} line counts as of the last compile */
  let compiledLines = new Map();
  /** @type {Set<string>} paths whose line count has left the compiled one */
  let shifted = new Set();

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
      for (const path of changed) {
        if (!isTex(path) || !compiledLines.has(path)) continue;
        if (lineCount(texts.get(path) ?? "") !== compiledLines.get(path)) shifted.add(path);
      }
      if (changed.length !== 1) return;
      const [path] = changed;
      if (!isTex(path) || !compiledLines.has(path) || shifted.has(path)) return;
      if (!previous.has(path) || !texts.has(path)) return;
      const patch = livePatch({ path, oldText: previous.get(path), newText: texts.get(path), isMain: path === main });
      if (patch) send({ type: "preview-patch", patch });
    },

    /** @param {Record<string, string>} next the texts that were compiled */
    compiled(next) {
      texts = remember(next);
      compiledLines = new Map([...texts].map(([path, text]) => [path, lineCount(text)]));
      shifted = new Set();
    },

    reset() {
      texts = new Map();
      compiledLines = new Map();
      shifted = new Set();
    },
  };
}
