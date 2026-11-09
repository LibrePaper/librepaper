// The project index: the latest FileIndex per path, the project's file list and
// bibliography keys, and the lookups that cross files.

import { dirname, normalizePath, resolvePath } from "../completion/paths.js";
import { sourceFormatOf } from "../formats.js";
import { labelScope } from "./label-names.js";

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
 *   namespaceOf: (item: IndexItem, fromPath?: string) => Namespace | null,
 *   definitions: (namespace: Namespace, name: string, fromPath?: string) => Array<{ path: string, item: IndexItem }>,
 *   hasCitation: (key: string) => boolean | null,
 *   references: (namespace: Namespace, name: string, fromPath?: string) => Array<{ path: string, item: IndexItem }>,
 *   names: (namespace: Namespace, fromPath?: string) => string[],
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

  /**
   * Label definitions by name, in path then item order. Built on first use and
   * dropped whenever a file changes.
   * @type {Map<string, Array<{ path: string, item: IndexItem }>> | null}
   */
  let labelIndex = null;

  /** @returns {Map<string, Array<{ path: string, item: IndexItem }>>} */
  function labelDefinitions() {
    if (labelIndex) return labelIndex;
    labelIndex = new Map();
    for (const [path, file] of sortedEntries()) {
      for (const item of file.items) {
        if (item.kind !== "label" || item.name === undefined) continue;
        const list = labelIndex.get(item.name);
        if (list) list.push({ path, item });
        else labelIndex.set(item.name, [{ path, item }]);
      }
    }
    return labelIndex;
  }

  /**
   * Whether a label at `defPath` can be named from `fromPath`: the same format
   * family, and for HTML the same page. Without `fromPath`, any label counts.
   * @param {string} defPath
   * @param {string | undefined} fromPath
   * @returns {boolean}
   */
  const sameScope = (defPath, fromPath) =>
    fromPath === undefined || labelScope(files.get(defPath)?.format ?? "", defPath) === labelScope(files.get(fromPath)?.format ?? "", fromPath);

  /**
   * @param {string} name
   * @param {string} [fromPath]
   * @returns {boolean}
   */
  function labelExists(name, fromPath) {
    return (labelDefinitions().get(name) ?? []).some(({ path }) => sameScope(path, fromPath));
  }

  /**
   * @param {IndexItem} item
   * @param {string} [fromPath] the file holding `item`; scopes an either-ref
   * @returns {Namespace | null}
   */
  function namespaceOf(item, fromPath) {
    if (item.kind === "ref") {
      if (item.namespace === "either") {
        return item.name !== undefined && labelExists(item.name, fromPath) ? "label" : "cite";
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
   * LaTeX reads \input and \includegraphics names from the project root (the
   * folder the main file compiles in), so there the root-relative path is tried
   * first and the path beside `fromPath` second, as opening.js does.
   * @type {ResolveFile}
   */
  function resolveFileDefault(fromPath, name, kind) {
    const known = fileSet;
    if (!known) return null;
    // A name with a leading slash is relative to the project root, as in HTML.
    const bases = name.startsWith("/") ? [normalizePath(name)] : [resolvePath(dirname(fromPath), name)];
    if (!name.startsWith("/") && sourceFormatOf(fromPath) === "latex") bases.unshift(normalizePath(name));
    const hasExtension = /\.[^./]+$/.test(name.slice(name.lastIndexOf("/") + 1));
    const extension = extensionOf(fromPath);
    const candidates = [...new Set(bases)].flatMap((base) => {
      if (hasExtension) return [base];
      if (kind === "asset") return [base, ...ASSET_EXTENSIONS.map((ext) => `${base}.${ext}`)];
      return extension ? [base, `${base}.${extension}`] : [base];
    });
    const found = candidates.find((candidate) => known.has(candidate));
    if (found) return found;
    // Images are also looked up in folders the format configures, such as
    // LaTeX's \graphicspath, which the index does not read. Accept the one
    // project file, in any folder, whose path ends with the name.
    if (kind !== "asset") return null;
    const tails = candidates.map((candidate) => `/${candidate.slice(candidate.lastIndexOf("/") + 1)}`);
    const matches = [...known].filter((path) => tails.some((tail) => `/${path}`.endsWith(tail)));
    return matches.length === 1 ? matches[0] : null;
  }

  const resolveFile = options.resolveFile ?? resolveFileDefault;

  return {
    /**
     * Store or replace the index of one file.
     * @param {FileIndex} fileIndex
     */
    update(fileIndex) {
      files.set(fileIndex.path, fileIndex);
      labelIndex = null;
    },

    /**
     * Forget one file.
     * @param {string} path
     */
    remove(path) {
      files.delete(path);
      labelIndex = null;
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
     * so the cite namespace has none here. With `fromPath`, only labels that
     * file can name: the same format family, and for HTML the same page.
     * @param {Namespace} namespace
     * @param {string} name
     * @param {string} [fromPath]
     * @returns {Array<{ path: string, item: IndexItem }>}
     */
    definitions(namespace, name, fromPath) {
      if (namespace !== "label") return [];
      return (labelDefinitions().get(name) ?? []).filter(({ path }) => sameScope(path, fromPath));
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
     * @param {string} [fromPath] keeps only the refs that scope to this file's labels
     * @returns {Array<{ path: string, item: IndexItem }>}
     */
    references(namespace, name, fromPath) {
      /** @type {Array<{ path: string, item: IndexItem }>} */
      const out = [];
      for (const [path, file] of sortedEntries()) {
        for (const item of byStart(file.items)) {
          if (
            (item.kind === "ref" || item.kind === "cite") &&
            item.name === name &&
            namespaceOf(item, path) === namespace &&
            (namespace !== "label" || sameScope(path, fromPath))
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
     * @param {string} [fromPath]
     * @returns {string[]}
     */
    names(namespace, fromPath) {
      if (namespace === "cite") return bibliography === null ? [] : [...bibliography].sort(byText);
      if (namespace === "label") {
        const set = [...labelDefinitions()]
          .filter(([, defs]) => defs.some(({ path }) => sameScope(path, fromPath)))
          .map(([label]) => label);
        return set.sort(byText);
      }
      return [];
    },

    resolveFile,
  };
}

/** @typedef {ReturnType<typeof createProjectIndex>} ProjectIndex */
