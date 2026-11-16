// Typedefs for the editor document index. Format adapters emit lists of
// IndexItem; the project index and the queries read them. No runtime code.
// Positions are UTF-16 offsets. Ranges are [from, to) pairs.

/**
 * One construct in a file. `markup` is syntax that is not prose (commands,
 * delimiters, attributes); adapters mark it `attrs: { prose: "skip" }`.
 *
 * A `label` carries `attrs.target`, the kind of thing it names: "section" |
 * "figure" | "table" | "equation" | "listing" | "footnote" | "theorem" | "item"
 * | "other". A label the author did not write (an automatic heading id, an Org
 * headline's title) also has `attrs.implicit: "true"`.
 *
 * A footnote is a `label` (its definition) and a `ref` in the "footnote"
 * namespace, with its bare label as `name`.
 *
 * A `link` is a URL (`attrs.external: "true"`) or a project file by relative
 * path (`attrs.file: "true"`, with any `#fragment` in `attrs.fragment`).
 *
 * A `metadata` item is one value of a document property: `name` is "title",
 * "subtitle", "author", "date", "language" or "abstract", and `title` its plain
 * text.
 *
 * @typedef {{
 *   kind: "heading" | "label" | "ref" | "cite" | "include" | "asset"
 *       | "link" | "metadata" | "block" | "math" | "code" | "definition"
 *       | "bibliography" | "markup",
 *   range: [number, number],
 *   name?: string,
 *   nameRange?: [number, number],
 *   level?: number,
 *   title?: string,
 *   namespace?: "label" | "cite" | "either" | "footnote",
 *   attrs?: Record<string, string>, // also inline: "true" on a footnote defined where it is used
 * }} IndexItem
 */

/**
 * The index of one file. `length` is the file's text length.
 * @typedef {{ path: string, format: string, items: IndexItem[], length: number }} FileIndex
 */

/** @typedef {"label" | "cite" | "footnote"} Namespace */

/** @typedef {{ path: string, from: number, to: number }} Location */

/** @typedef {{ path: string, from: number, to: number, insert: string }} Edit */

/**
 * @typedef {{
 *   path: string,
 *   from: number,
 *   to: number,
 *   severity: "warning" | "info",
 *   code: "unknown-label" | "unknown-citation" | "unknown-reference"
 *       | "unknown-footnote" | "missing-file" | "duplicate-label"
 *       | "unused-footnote" | "unused-target" | "unused-bibliography",
 *   name: string,
 *   suggestions: string[],
 * }} Diagnostic
 */

export {};
