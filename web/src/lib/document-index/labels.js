// Label lists read from the document index. Completion and the citation
// checker both read labels here, so a label means the same thing in each.

import { createProjectIndex } from "./project.js";
import { indexFile } from "./adapters/index.js";
import { formatOf } from "../formats.js";

/** @typedef {import("./project.js").ProjectIndex} ProjectIndex */

/**
 * One label definition. `line` is 1-based. `title` is the heading or caption
 * the label belongs to, when the adapter found one.
 * @typedef {{ id: string, path: string, line: number, title?: string }} LabelEntry
 */

/**
 * A project index built from source texts, one adapter per file. A file whose
 * format has no adapter is skipped, as is a null text.
 * @param {Record<string, string | null | undefined>} texts path to source
 * @returns {ProjectIndex}
 */
export function indexTexts(texts) {
  const project = createProjectIndex();
  for (const [path, text] of Object.entries(texts)) {
    if (text == null) continue;
    const fileIndex = indexFile(path, String(text));
    if (fileIndex) project.update(fileIndex);
  }
  return project;
}

/**
 * Offsets at which each line of a text starts.
 * @param {string} text
 * @returns {number[]}
 */
function lineStarts(text) {
  const starts = [0];
  for (let i = text.indexOf("\n"); i !== -1; i = text.indexOf("\n", i + 1)) starts.push(i + 1);
  return starts;
}

/**
 * The 1-based line of an offset, by binary search over the line starts. An
 * offset past the end is on the last line.
 * @param {number[]} starts
 * @param {number} offset UTF-16 offset
 * @returns {number}
 */
function lineIn(starts, offset) {
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
 * Whether a file is a document: one that renders on its own, such as a .tex,
 * .typ, .qmd, .md or .html file. A style or class file (.sty, .cls) is source
 * for a document, not a document, so its labels are not offered for completion.
 * Reuses formats.js, which already says which extensions are documents.
 * @param {string} path
 * @returns {boolean}
 */
export function isDocumentSource(path) {
  return formatOf(path) !== "";
}

/**
 * The labels of a project, one entry per name. The first definition wins: the
 * file `first` comes before the others, the rest follow in path order, and each
 * file's labels follow in offset order. Only labels in document files are
 * listed; labels defined in style or class files still resolve in the index.
 * @param {ProjectIndex} project
 * @param {Record<string, string | null | undefined>} texts source per path, for line numbers
 * @param {{ first?: string, formats?: Set<string> | null }} [options]
 *   `formats`, when given, keeps only the files whose format is in it.
 * @returns {LabelEntry[]}
 */
export function labelList(project, texts, { first = "", formats = null } = {}) {
  const paths = project.paths();
  const ordered = first && paths.includes(first) ? [first, ...paths.filter((path) => path !== first)] : paths;
  /** @type {Set<string>} */
  const seen = new Set();
  /** @type {LabelEntry[]} */
  const out = [];
  /** @type {Map<string, number[]>} */
  const startsOf = new Map();
  /** @param {string} path @returns {number[] | null} */
  const startsFor = (path) => {
    const text = texts[path];
    if (text == null) return null;
    let starts = startsOf.get(path);
    if (!starts) startsOf.set(path, (starts = lineStarts(String(text))));
    return starts;
  };
  for (const path of ordered) {
    const file = project.file(path);
    if (!file || !isDocumentSource(path) || (formats && !formats.has(file.format))) continue;
    const labels = file.items
      .filter((item) => item.kind === "label" && item.name !== undefined && project.namespaceOf(item, path) === "label")
      .sort((a, b) => a.range[0] - b.range[0]);
    for (const item of labels) {
      const id = /** @type {string} */ (item.name);
      if (seen.has(id)) continue;
      seen.add(id);
      /** @type {LabelEntry} */
      const entry = { id, path, line: lineIn(startsFor(path) ?? [0], item.range[0]) };
      if (item.title !== undefined) entry.title = item.title;
      out.push(entry);
    }
  }
  return out;
}
