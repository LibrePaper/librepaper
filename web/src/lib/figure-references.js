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
import { loadOrgSyntax } from "./org-language/module.js";
import { loadTypstSyntax } from "./typst-language/module.js";

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
    const index = indexFile(file, text);
    if (!index) continue;
    const format = sourceFormatOf(main) || index.format;
    const starts = lineStarts(text);
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
