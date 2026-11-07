// Typedefs for the editor document index. Format adapters emit lists of
// IndexItem; the project index and the queries read them. No runtime code.
// Positions are UTF-16 offsets. Ranges are [from, to) pairs.

/**
 * One construct in a file.
 * @typedef {{
 *   kind: "heading" | "label" | "ref" | "cite" | "include" | "asset"
 *       | "block" | "math" | "code" | "definition" | "bibliography",
 *   range: [number, number],
 *   name?: string,
 *   nameRange?: [number, number],
 *   level?: number,
 *   title?: string,
 *   namespace?: "label" | "cite" | "either",
 *   attrs?: Record<string, string>,
 * }} IndexItem
 */

/**
 * The index of one file. `length` is the file's text length.
 * @typedef {{ path: string, format: string, items: IndexItem[], length: number }} FileIndex
 */

/** @typedef {"label" | "cite"} Namespace */

/** @typedef {{ path: string, from: number, to: number }} Location */

/** @typedef {{ path: string, from: number, to: number, insert: string }} Edit */

/**
 * @typedef {{
 *   path: string,
 *   from: number,
 *   to: number,
 *   severity: "warning",
 *   code: "unknown-label" | "unknown-citation" | "unknown-reference" | "missing-file",
 *   name: string,
 *   suggestions: string[],
 * }} Diagnostic
 */

export {};
