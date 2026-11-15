// Figures a Quarto or Typst document references but only the author's Run
// directory holds. They travel with a run's results: the browser names the
// ones it needs before a run, and keeps the ones the run returned.
//
// Nothing here is user-visible text. Everything returns data.

import { isRunFigurePath } from "./calepin-results.js";

/** The referenced figure paths that are not project files, each once, in order
 * of first reference. `references` is `referencedFigures(tree)`.
 * @param {Array<{ path?: string }>} references
 * @param {{ texts?: object, assets?: object, digests?: object } | null | undefined} tree
 * @returns {string[]} */
export function wantedPaths(references, tree) {
  const known = (map, path) => Boolean(map) && Object.hasOwn(map, path);
  const seen = new Set();
  for (const reference of references ?? []) {
    const path = reference?.path;
    if (typeof path !== "string" || path === "" || seen.has(path)) continue;
    if (known(tree?.texts, path) || known(tree?.assets, path) || known(tree?.digests, path)) continue;
    seen.add(path);
  }
  return [...seen];
}

/** The run figures in a Quarto run's `results` job file (`{ version: 1, files }`),
 * as `[{ path, base64 }]`. Anything that is not a safe base64 figure is dropped.
 * @param {Uint8Array | string | null | undefined} bytes */
export function runFiguresOf(bytes) {
  let source;
  if (typeof bytes === "string") source = bytes;
  else if (bytes instanceof Uint8Array) source = new TextDecoder().decode(bytes);
  else return [];
  let parsed;
  try {
    parsed = JSON.parse(source);
  } catch {
    return [];
  }
  if (!parsed || typeof parsed !== "object" || parsed.version !== 1 || !Array.isArray(parsed.files)) return [];
  const seen = new Set();
  const kept = [];
  for (const file of parsed.files) {
    if (!file || typeof file.base64 !== "string" || typeof file.text === "string") continue;
    if (!isRunFigurePath(file.path) || seen.has(file.path)) continue;
    seen.add(file.path);
    kept.push({ path: file.path, base64: file.base64 });
  }
  return kept;
}

/** How many entries of a record's files are Run-directory figures
 * (outside `.calepin/`). @param {Array<{ path?: string }> | null | undefined} files */
export function runFigureCount(files) {
  return (Array.isArray(files) ? files : []).filter((file) => isRunFigurePath(file?.path)).length;
}

/** The Run-directory figures among `files`. @param {Array<{ path?: string }> | null | undefined} files */
export function runFigureFiles(files) {
  return (Array.isArray(files) ? files : []).filter((file) => isRunFigurePath(file?.path));
}

/** A Quarto record with this run's figures on it; unchanged when there are none.
 * @template {object} T @param {T} record @param {Array<object>} files
 * @returns {T & { files?: Array<object> }} */
export function withRunFigures(record, files) {
  return files.length ? { ...record, files } : record;
}

/** The record a Quarto PDF run publishes: this run's figures, and the cell
 * outputs of the last HTML run of the same file (this tab's, else the shared
 * one), since a PDF run produces no splittable cells. `own` and `shared` are
 * Quarto result records (or null); a record for another path or a Calepin
 * record is ignored.
 * @param {{ path: string, own?: any, shared?: any, files?: Array<object> }} input */
export function pdfRunRecord({ path, own = null, shared = null, files = [] }) {
  const usable = (record) => Boolean(record) && record.path === path && record.engine !== "calepin" && Array.isArray(record.cells);
  const base = usable(own) ? own : usable(shared) ? shared : null;
  const record = { version: 1, path, cells: base ? base.cells : [], inlines: base?.inlines ?? [] };
  return withRunFigures(record, files);
}
