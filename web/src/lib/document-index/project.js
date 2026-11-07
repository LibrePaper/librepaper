// The project index: the latest FileIndex per path, the project's file list and
// bibliography keys, and the lookups that cross files.

import { dirname, normalizePath, resolvePath } from "../completion/paths.js";

/** @typedef {import("./schema.js").FileIndex} FileIndex */
/** @typedef {import("./schema.js").IndexItem} IndexItem */
/** @typedef {import("./schema.js").Namespace} Namespace */
/** @typedef {(fromPath: string, name: string, kind?: string) => string | null} ResolveFile */

// Extensions LaTeX tries for an image named without one.
const ASSET_EXTENSIONS = ["pdf", "png", "jpg", "jpeg", "eps", "svg"];

/** @param {string} a @param {string} b @returns {number} */
const byText = (a, b) => (a < b ? -1 : a > b ? 1 : 0);

/** @param {IndexItem[]} items @returns {IndexItem[]} */
const byStart = (items) => [...items].sort((a, b) => a.range[0] - b.range[0]);

/** @param {string} path @returns {string} */
function extensionOf(path) {
  const match = /\.([^./]+)$/.exec(path.slice(path.lastIndexOf("/") + 1));
  return match ? match[1] : "";
}

/**
 * @param {{ resolveFile?: ResolveFile }} [options] `resolveFile` replaces the
 *   default include and asset resolution.
 * @returns {{
 *   update: (fileIndex: FileIndex) => void,
 *   remove: (path: string) => void,
 *   file: (path: string) => FileIndex | undefined,
 *   paths: () => string[],
 *   setFiles: (paths: string[] | null) => void,
 *   knownFiles: () => string[] | null,
 *   setBibliography: (keys: string[] | null) => void,
 *   namespaceOf: (item: IndexItem) => Namespace | null,
 *   definitions: (namespace: Namespace, name: string) => Array<{ path: string, item: IndexItem }>,
 *   hasCitation: (key: string) => boolean | null,
 *   references: (namespace: Namespace, name: string) => Array<{ path: string, item: IndexItem }>,
 *   names: (namespace: Namespace) => string[],
 *   resolveFile: ResolveFile,
 * }}
 */
export function createProjectIndex(options = {}) {
  /** @type {Map<string, FileIndex>} */
  const files = new Map();
  /** @type {Set<string> | null} */
  let fileSet = null;
  /** @type {Set<string> | null} */
  let bibliography = null;

  /** @returns {Array<[string, FileIndex]>} */
  const sortedEntries = () => [...files.entries()].sort(([a], [b]) => byText(a, b));

  /** @param {string} name @returns {boolean} */
  function labelExists(name) {
    for (const file of files.values()) {
      if (file.items.some((item) => item.kind === "label" && item.name === name)) return true;
    }
    return false;
  }

  /**
   * @param {IndexItem} item
   * @returns {Namespace | null}
   */
  function namespaceOf(item) {
    if (item.kind === "ref") {
      if (item.namespace === "either") {
        return item.name !== undefined && labelExists(item.name) ? "label" : "cite";
      }
      return item.namespace ?? "label";
    }
    if (item.kind === "cite") return "cite";
    if (item.kind === "label") return "label";
    return null;
  }

  /**
   * Resolve a name written relative to the directory of `fromPath`. Tries the
   * name as written, then, when the name has no extension, the image
   * extensions for an asset or the extension of `fromPath` for anything else.
   * @type {ResolveFile}
   */
  function resolveFileDefault(fromPath, name, kind) {
    const known = fileSet;
    if (!known) return null;
    const base = resolvePath(dirname(fromPath), name);
    const candidates = [base];
    const hasExtension = /\.[^./]+$/.test(name.slice(name.lastIndexOf("/") + 1));
    const extension = extensionOf(fromPath);
    if (!hasExtension && kind === "asset") candidates.push(...ASSET_EXTENSIONS.map((ext) => `${base}.${ext}`));
    else if (!hasExtension && extension) candidates.push(`${base}.${extension}`);
    return candidates.find((candidate) => known.has(candidate)) ?? null;
  }

  const resolveFile = options.resolveFile ?? resolveFileDefault;

  return {
    /**
     * Store or replace the index of one file.
     * @param {FileIndex} fileIndex
     */
    update(fileIndex) {
      files.set(fileIndex.path, fileIndex);
    },

    /**
     * Forget one file.
     * @param {string} path
     */
    remove(path) {
      files.delete(path);
    },

    /**
     * @param {string} path
     * @returns {FileIndex | undefined}
     */
    file(path) {
      return files.get(path);
    },

    /** @returns {string[]} indexed paths, sorted */
    paths() {
      return sortedEntries().map(([path]) => path);
    },

    /**
     * The project's file list, or null when unknown.
     * @param {string[] | null} paths
     */
    setFiles(paths) {
      fileSet = paths === null ? null : new Set(paths.map(normalizePath));
    },

    /** @returns {string[] | null} the project's file list, sorted, or null */
    knownFiles() {
      return fileSet === null ? null : [...fileSet].sort(byText);
    },

    /**
     * Known bibliography keys, or null when not parsed yet.
     * @param {string[] | null} keys
     */
    setBibliography(keys) {
      bibliography = keys === null ? null : new Set(keys);
    },

    namespaceOf,

    /**
     * Label definitions across files. Citations are defined outside the index,
     * so the cite namespace has none here.
     * @param {Namespace} namespace
     * @param {string} name
     * @returns {Array<{ path: string, item: IndexItem }>}
     */
    definitions(namespace, name) {
      if (namespace !== "label") return [];
      /** @type {Array<{ path: string, item: IndexItem }>} */
      const out = [];
      for (const [path, file] of sortedEntries()) {
        for (const item of file.items) {
          if (item.kind === "label" && item.name === name) out.push({ path, item });
        }
      }
      return out;
    },

    /**
     * @param {string} key
     * @returns {boolean | null} null when the bibliography is unknown
     */
    hasCitation(key) {
      return bibliography === null ? null : bibliography.has(key);
    },

    /**
     * Every ref or cite that resolves to `namespace` with this name, in path
     * then offset order.
     * @param {Namespace} namespace
     * @param {string} name
     * @returns {Array<{ path: string, item: IndexItem }>}
     */
    references(namespace, name) {
      /** @type {Array<{ path: string, item: IndexItem }>} */
      const out = [];
      for (const [path, file] of sortedEntries()) {
        for (const item of byStart(file.items)) {
          if (
            (item.kind === "ref" || item.kind === "cite") &&
            item.name === name &&
            namespaceOf(item) === namespace
          ) {
            out.push({ path, item });
          }
        }
      }
      return out;
    },

    /**
     * Sorted unique names: label names, or bibliography keys (empty if unknown).
     * @param {Namespace} namespace
     * @returns {string[]}
     */
    names(namespace) {
      if (namespace === "cite") return bibliography === null ? [] : [...bibliography].sort(byText);
      if (namespace === "label") {
        const set = new Set();
        for (const file of files.values()) {
          for (const item of file.items) {
            if (item.kind === "label" && item.name !== undefined) set.add(item.name);
          }
        }
        return [...set].sort(byText);
      }
      return [];
    },

    resolveFile,
  };
}

/** @typedef {ReturnType<typeof createProjectIndex>} ProjectIndex */
