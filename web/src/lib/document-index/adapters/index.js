// The adapter for each source format, chosen by file path. Files of any other
// type have no adapter and are not indexed.

import { sourceFormatOf } from "../../formats.js";
import { htmlAdapter } from "./html.js";
import { latexAdapter } from "./latex.js";
import { markdownAdapter } from "./markdown.js";
import { orgAdapter } from "./org.js";
import { typstAdapter } from "./typst.js";

/** @typedef {import("../schema.js").IndexItem} IndexItem */
/** @typedef {import("../schema.js").FileIndex} FileIndex */
/** @typedef {(text: string, path: string) => IndexItem[]} Adapter */

/** @type {Record<string, Adapter>} */
const ADAPTERS = {
  latex: latexAdapter,
  typst: typstAdapter,
  markdown: markdownAdapter,
  quarto: markdownAdapter,
  html: htmlAdapter,
  org: orgAdapter,
};

/**
 * The adapter for a path, or null when its format is not indexed.
 * @param {string} path
 * @returns {Adapter | null}
 */
export function adapterFor(path) {
  return ADAPTERS[sourceFormatOf(path)] ?? null;
}

/**
 * Index one file's text, or null when its format is not indexed.
 * @param {string} path
 * @param {string} text
 * @returns {FileIndex | null}
 */
export function indexFile(path, text) {
  const format = sourceFormatOf(path);
  const adapter = ADAPTERS[format];
  if (!adapter) return null;
  return { path, format, items: adapter(text, path), length: text.length };
}
