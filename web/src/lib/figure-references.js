// The figures a document refers to, and the project path each one names.
//
// The path rule is how the real tool resolves a name, so the preview, the
// editor, visual mode and paper mode all look in the same place:
//   Markdown, Quarto, Org, HTML   relative to the main file's directory (an
//                                 include is textual, so the page being built
//                                 is the main file's page); a leading "/" is
//                                 the project root.
//   Typst                         relative to the file that writes the image;
//                                 a leading "/" is the project root.
//   LaTeX                         from the project root, then beside the file,
//                                 with graphicx's extension guessing (the
//                                 document index's own resolver).

import { dirname, normalizePath, resolvePath } from "./completion/paths.js";
import { indexFile } from "./document-index/adapters/index.js";
import { createProjectIndex } from "./document-index/project.js";
import { sourceFormatOf } from "./formats.js";
import { loadOrgSyntax, orgSyntax } from "./org-language/module.js";
import { loadTypstSyntax, typstSyntax } from "./typst-language/module.js";

/** @typedef {{ path: string, name: string, file: string, line: number }} FigureReference */

const SCHEME = /^(?:[a-z][a-z0-9+.-]*:|\/\/)/i;

/**
 * The project path an image named `name`, written in `file`, refers to. Null
 * for a name that is a URL. The path may be absent from the project; LaTeX
 * returns the root-relative name when no file matches.
 * @param {{ format: string, file: string, main: string, name: string, resolveFile?: (from: string, name: string, kind?: string) => string | null }} options
 * @returns {string | null}
 */
export function figurePath({ format, file, main, name, resolveFile }) {
  if (!name || SCHEME.test(name)) return null;
  if (format === "latex") return resolveFile?.(file, name, "asset") ?? normalizePath(name);
  if (name.startsWith("/")) return normalizePath(name);
  return resolvePath(dirname(format === "typst" ? file : main), name) || null;
}

/**
 * Loads the parsers the Typst and Org adapters read figures with, when the
 * project has such files. Until they load those adapters list nothing, and a
 * preview or a run can come before the editor has opened a file of that kind.
 * A parser that cannot load leaves the references empty, as before.
 * Null when the project has no such file, so a caller need not wait at all.
 * @param {{ texts?: Record<string, string> }} tree
 * @returns {Promise<unknown> | null}
 */
export function figureSyntaxReady(tree) {
  const names = Object.keys(tree?.texts ?? {});
  const typst = names.some((name) => /\.typ$/i.test(name));
  const org = names.some((name) => /\.org$/i.test(name));
  if (!typst && !org) return null;
  return Promise.all([typst ? loadTypstSyntax() : null, org ? loadOrgSyntax() : null]);
}

/** @param {string} text @returns {number[]} offsets at which each line starts */
function lineStarts(text) {
  const starts = [0];
  for (let i = text.indexOf("\n"); i !== -1; i = text.indexOf("\n", i + 1)) starts.push(i + 1);
  return starts;
}

// referencedFigures runs on every keystroke, but most keystrokes leave the
// included files alone, so each file's index and line starts are kept keyed by
// path and exact text. Only a file whose text changed is parsed again. The
// cached index and starts are never mutated (the loop sorts a copy of
// `items`), so sharing them between calls is safe. Bounded to the 64 most
// recently used paths.
const INDEX_CACHE_LIMIT = 64;
/** @type {Map<string, { text: string, index: import("./document-index/schema.js").FileIndex | null, starts: number[] }>} */
const indexCache = new Map();

/**
 * The index and line starts of one file, from the cache when its text is
 * unchanged. The Typst and Org adapters index nothing until their parser has
 * loaded, so a file of those formats is only cached once its parser is ready;
 * otherwise a render that came first would keep an empty index.
 * @param {string} file
 * @param {string} text
 */
function indexed(file, text) {
  const hit = indexCache.get(file);
  if (hit && hit.text === text) {
    indexCache.delete(file);
    indexCache.set(file, hit);
    return hit;
  }
  const entry = { text, index: indexFile(file, text), starts: lineStarts(text) };
  const format = sourceFormatOf(file);
  const ready = (format !== "typst" || typstSyntax() !== null) && (format !== "org" || orgSyntax() !== null);
  if (ready) {
    indexCache.delete(file);
    indexCache.set(file, entry);
    if (indexCache.size > INDEX_CACHE_LIMIT) indexCache.delete(indexCache.keys().next().value);
  }
  return entry;
}

/** @param {number[]} starts @param {number} offset @returns {number} 1-based line */
function lineOf(starts, offset) {
  let low = 0;
  let high = starts.length - 1;
  while (low < high) {
    const mid = (low + high + 1) >> 1;
    if (starts[mid] <= offset) low = mid;
    else high = mid - 1;
  }
  return low + 1;
}

/**
 * Local module paths a Typst file imports (`#import "mod.typ": x`). Package
 * imports (`@preview/...`) are skipped.
 * @param {string} text
 * @returns {string[]}
 */
function typstImports(text) {
  const out = [];
  for (const match of text.matchAll(/#import\s+"([^"\n]+)"/g)) {
    if (!match[1].startsWith("@")) out.push(match[1]);
  }
  return out;
}

/**
 * Every image reference in the main document and in the files it includes, in
 * document order, each resolved by the path rule above. `tree` is the
 * `{ main, texts, assets?, digests? }` the renderers take.
 * @param {{ main?: string, texts?: Record<string, string>, assets?: Record<string, unknown>, digests?: Record<string, string> }} tree
 * @returns {FigureReference[]}
 */
export function referencedFigures(tree) {
  const main = tree?.main ?? "";
  const texts = tree?.texts ?? {};
  if (!main || typeof texts[main] !== "string") return [];
  const known = new Set([...Object.keys(texts), ...Object.keys(tree.assets ?? {}), ...Object.keys(tree.digests ?? {})].map(normalizePath));
  const project = createProjectIndex();
  project.setFiles([...known]);
  /** @type {FigureReference[]} */
  const out = [];
  const seen = new Set();
  const queue = [main];
  while (queue.length) {
    const file = /** @type {string} */ (queue.shift());
    if (seen.has(file)) continue;
    seen.add(file);
    const text = texts[file];
    if (typeof text !== "string") continue;
    const { index, starts } = indexed(file, text);
    if (!index) continue;
    const format = sourceFormatOf(main) || index.format;
    if (index.format === "typst") {
      for (const name of typstImports(text)) {
        const target = project.resolveFile(file, name, "include");
        if (target && !seen.has(target)) queue.push(target);
      }
    }
    for (const item of [...index.items].sort((a, b) => a.range[0] - b.range[0])) {
      if (item.name === undefined) continue;
      if (item.kind === "asset") {
        const path = figurePath({ format, file, main, name: item.name, resolveFile: project.resolveFile });
        if (path) out.push({ path, name: item.name, file, line: lineOf(starts, item.range[0]) });
      } else if (item.kind === "include" && index.format !== "html") {
        const target = project.resolveFile(file, item.name, "include");
        if (target && !seen.has(target)) queue.push(target);
      }
    }
  }
  return out;
}
