// Keeps a project index in step with the editor's texts. Each refresh re-indexes
// only the files whose text changed since the previous refresh, drops files
// that vanished, and reports whether anything the index answers from changed.

import { createProjectIndex } from "./project.js";
import { indexFile } from "./adapters/index.js";

/** @typedef {import("./project.js").ProjectIndex} ProjectIndex */

/**
 * @typedef {{
 *   texts: Record<string, string>,
 *   files: string[],
 *   bibliography: string[] | null,
 * }} LiveSnapshot
 */

/**
 * Two string lists hold the same members, in any order. Both null counts as equal.
 * @param {string[] | null | undefined} a
 * @param {string[] | null | undefined} b
 * @returns {boolean}
 */
function sameList(a, b) {
  if (a == null || b == null) return a == b;
  if (a.length !== b.length) return false;
  const sortedA = [...a].sort();
  const sortedB = [...b].sort();
  return sortedA.every((value, i) => value === sortedB[i]);
}

/**
 * Items computed by something sharper than the adapter (the typst module's
 * parser) for one file's text. `source` is the exact text they were computed
 * from; they apply only while the file still has that text.
 * @typedef {{ kinds: Set<string>, items: import("./schema.js").IndexItem[], source: string }} Overlay
 */

/**
 * A project index that follows the editor's texts. The first refresh always
 * reports a change, since nothing was known before it.
 * @returns {{
 *   project: ProjectIndex,
 *   refresh: (snapshot: LiveSnapshot) => boolean,
 *   setOverlay: (path: string, overlay: Overlay) => void,
 *   clearOverlay: (path: string) => void,
 * }}
 */
export function createLiveIndex() {
  const project = createProjectIndex();
  /** @type {Map<string, string>} path to the text it was last indexed from */
  const seen = new Map();
  /** @type {Map<string, Overlay>} */
  const overlays = new Map();
  /** @type {Map<string, Overlay>} the overlay each file was last indexed with */
  const applied = new Map();
  /** @type {string[] | null | undefined} undefined until the first refresh */
  let lastFiles;
  /** @type {string[] | null | undefined} undefined until the first refresh */
  let lastBibliography;

  return {
    project,

    /**
     * Replace the items of `overlay.kinds` that the adapter finds in `path`
     * with `overlay.items`, while the file's text equals `overlay.source`. The
     * next refresh applies it; once the text changes the overlay stops
     * applying, so a late answer never shifts offsets.
     * @param {string} path
     * @param {Overlay} overlay
     */
    setOverlay(path, overlay) {
      overlays.set(path, overlay);
    },

    /** @param {string} path */
    clearOverlay(path) {
      overlays.delete(path);
    },

    /**
     * @param {LiveSnapshot} snapshot
     * @returns {boolean} true when the index or what it answers from changed
     */
    refresh({ texts, files, bibliography }) {
      let changed = false;

      for (const path of [...seen.keys()]) {
        if (Object.prototype.hasOwnProperty.call(texts, path)) continue;
        seen.delete(path);
        applied.delete(path);
        overlays.delete(path);
        if (project.file(path)) {
          project.remove(path);
          changed = true;
        }
      }

      for (const [path, text] of Object.entries(texts)) {
        const stored = overlays.get(path);
        const overlay = stored && stored.source === text ? stored : undefined;
        if (seen.get(path) === text && applied.get(path) === overlay) continue;
        seen.set(path, text);
        if (overlay) applied.set(path, overlay);
        else applied.delete(path);
        const fileIndex = indexFile(path, text);
        if (fileIndex && overlay) {
          fileIndex.items = [...fileIndex.items.filter((item) => !overlay.kinds.has(item.kind)), ...overlay.items];
        }
        if (fileIndex) {
          project.update(fileIndex);
          changed = true;
        } else if (project.file(path)) {
          project.remove(path);
          changed = true;
        }
      }

      if (!sameList(files, lastFiles)) {
        lastFiles = [...files];
        project.setFiles(files);
        changed = true;
      }

      if (!sameList(bibliography, lastBibliography)) {
        lastBibliography = bibliography === null ? null : [...bibliography];
        project.setBibliography(bibliography);
        changed = true;
      }

      return changed;
    },
  };
}
