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
 * A project index that follows the editor's texts. The first refresh always
 * reports a change, since nothing was known before it.
 * @returns {{
 *   project: ProjectIndex,
 *   refresh: (snapshot: LiveSnapshot) => boolean,
 *   invalidate: (matches?: (path: string) => boolean) => void,
 * }}
 */
export function createLiveIndex() {
  const project = createProjectIndex();
  /** @type {Map<string, string>} path to the text it was last indexed from */
  const seen = new Map();
  /** @type {string[] | null | undefined} undefined until the first refresh */
  let lastFiles;
  /** @type {string[] | null | undefined} undefined until the first refresh */
  let lastBibliography;

  return {
    project,

    /**
     * Forgets what was indexed from the paths `matches` accepts (all of them
     * without it), so the next refresh indexes them again even though their
     * text is the same. For a file whose index depends on more than its text,
     * such as Typst source, which is read once typst-syntax.wasm has loaded.
     * @param {(path: string) => boolean} [matches]
     */
    invalidate(matches) {
      for (const path of [...seen.keys()]) {
        if (!matches || matches(path)) seen.delete(path);
      }
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
        if (project.file(path)) {
          project.remove(path);
          changed = true;
        }
      }

      for (const [path, text] of Object.entries(texts)) {
        if (seen.get(path) === text) continue;
        seen.set(path, text);
        const fileIndex = indexFile(path, text);
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
