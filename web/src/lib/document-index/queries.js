// Queries over the project index, named after LSP's. Each takes the project
// index first, then the file path, then a UTF-16 position where needed.

import { dirname, relativePath } from "../completion/paths.js";

/** @typedef {import("./schema.js").IndexItem} IndexItem */
/** @typedef {import("./schema.js").Location} Location */
/** @typedef {import("./schema.js").Edit} Edit */
/** @typedef {import("./schema.js").Diagnostic} Diagnostic */
/** @typedef {import("./schema.js").Namespace} Namespace */
/** @typedef {import("./project.js").ProjectIndex} ProjectIndex */

const LOCATABLE = new Set(["label", "ref", "cite", "include", "asset"]);

/** @param {string} a @param {string} b @returns {number} */
const byText = (a, b) => (a < b ? -1 : a > b ? 1 : 0);

/** @param {IndexItem[]} items @returns {IndexItem[]} */
const byStart = (items) => [...items].sort((a, b) => a.range[0] - b.range[0]);

/**
 * Sort by path, then offset.
 * @template {{ path: string, from: number }} T
 * @param {T[]} list
 * @returns {T[]}
 */
const sortBySpan = (list) => list.sort((a, b) => byText(a.path, b.path) || a.from - b.from);

/**
 * The span a user navigates to or renames: the name when there is one.
 * @param {IndexItem} item
 * @returns {[number, number]}
 */
const spanOf = (item) => item.nameRange ?? item.range;

/**
 * @param {string} path
 * @param {IndexItem} item
 * @returns {Location}
 */
function toLocation(path, item) {
  const [from, to] = spanOf(item);
  return { path, from, to };
}

/** @param {string} name @returns {boolean} */
const hasExtension = (name) => /\.[^./]+$/.test(name.slice(name.lastIndexOf("/") + 1));

/**
 * Edit distance between two strings (insert, delete, substitute, cost 1 each).
 * @param {string} a
 * @param {string} b
 * @returns {number}
 */
export function levenshtein(a, b) {
  let previous = Array.from({ length: b.length + 1 }, (_, j) => j);
  for (let i = 1; i <= a.length; i++) {
    const current = [i];
    for (let j = 1; j <= b.length; j++) {
      const cost = a[i - 1] === b[j - 1] ? 0 : 1;
      current[j] = Math.min(previous[j] + 1, current[j - 1] + 1, previous[j - 1] + cost);
    }
    previous = current;
  }
  return previous[b.length];
}

/**
 * Up to three candidates within edit distance 2 of `name`, closest first.
 * @param {string} name
 * @param {string[]} candidates
 * @returns {string[]}
 */
function closest(name, candidates) {
  return [...new Set(candidates)]
    .map((candidate) => ({ candidate, distance: levenshtein(name, candidate) }))
    .filter(({ distance }) => distance > 0 && distance <= 2)
    .sort((a, b) => a.distance - b.distance || byText(a.candidate, b.candidate))
    .slice(0, 3)
    .map(({ candidate }) => candidate);
}

/**
 * The item at a position, among the locatable kinds: the one with the smallest span.
 * @param {ProjectIndex} project
 * @param {string} path
 * @param {number} pos
 * @returns {IndexItem | null}
 */
export function itemAt(project, path, pos) {
  const file = project.file(path);
  if (!file) return null;
  /** @type {IndexItem | null} */
  let best = null;
  let bestSpan = Infinity;
  for (const item of file.items) {
    if (!LOCATABLE.has(item.kind)) continue;
    const [from, to] = spanOf(item);
    if (pos < from || pos > to) continue;
    if (to - from < bestSpan) {
      best = item;
      bestSpan = to - from;
    }
  }
  return best;
}

/**
 * The namespace and name a ref, cite or label item stands for, or null.
 * @param {ProjectIndex} project
 * @param {IndexItem} item
 * @returns {{ namespace: Namespace, name: string } | null}
 */
function targetOf(project, item) {
  if (item.name === undefined) return null;
  if (item.kind === "label") return { namespace: "label", name: item.name };
  if (item.kind === "ref" || item.kind === "cite") {
    const namespace = project.namespaceOf(item);
    return namespace ? { namespace, name: item.name } : null;
  }
  return null;
}

/**
 * Where a ref or cite at pos points to (labels only), or where an include or
 * asset at pos resolves to.
 * @param {ProjectIndex} project
 * @param {string} path
 * @param {number} pos
 * @returns {Location[]}
 */
export function definition(project, path, pos) {
  const item = itemAt(project, path, pos);
  if (!item || item.name === undefined) return [];
  if (item.kind === "ref" || item.kind === "cite") {
    if (project.namespaceOf(item) !== "label") return [];
    return project.definitions("label", item.name).map(({ path: defPath, item: def }) => toLocation(defPath, def));
  }
  if (item.kind === "include" || item.kind === "asset") {
    const resolved = project.resolveFile(path, item.name);
    return resolved === null ? [] : [{ path: resolved, from: 0, to: 0 }];
  }
  return [];
}

/**
 * Every use of the name at pos, and optionally its declaration.
 * @param {ProjectIndex} project
 * @param {string} path
 * @param {number} pos
 * @param {{ includeDeclaration?: boolean }} [options]
 * @returns {Location[]}
 */
export function references(project, path, pos, { includeDeclaration = true } = {}) {
  const item = itemAt(project, path, pos);
  const target = item ? targetOf(project, item) : null;
  if (!target) return [];
  const out = project
    .references(target.namespace, target.name)
    .map(({ path: usePath, item: use }) => toLocation(usePath, use));
  if (includeDeclaration) {
    for (const { path: defPath, item: def } of project.definitions(target.namespace, target.name)) {
      out.push(toLocation(defPath, def));
    }
  }
  return sortBySpan(out);
}

/**
 * Rename a label: its definitions and every reference that resolves to it.
 * @param {ProjectIndex} project
 * @param {string} path
 * @param {number} pos
 * @param {string} newName
 * @returns {Edit[] | null} null when the position is not a label or the name is not usable
 */
export function rename(project, path, pos, newName) {
  const item = itemAt(project, path, pos);
  if (!item || item.name === undefined) return null;
  const isLabel = item.kind === "label" || (item.kind === "ref" && project.namespaceOf(item) === "label");
  if (!isLabel) return null;
  if (typeof newName !== "string" || newName === "" || /\s/.test(newName) || newName === item.name) return null;
  const sites = [
    ...project.definitions("label", item.name),
    ...project.references("label", item.name),
  ];
  /** @type {Edit[]} */
  const edits = [];
  for (const { path: sitePath, item: site } of sites) {
    if (!site.nameRange) continue;
    edits.push({ path: sitePath, from: site.nameRange[0], to: site.nameRange[1], insert: newName });
  }
  return sortBySpan(edits);
}

/**
 * The target of a ref or cite at pos, with label titles.
 * @param {ProjectIndex} project
 * @param {string} path
 * @param {number} pos
 * @returns {{ name: string, namespace: Namespace, targets: Array<Location & { title?: string }> } | null}
 */
export function hover(project, path, pos) {
  const item = itemAt(project, path, pos);
  if (!item || (item.kind !== "ref" && item.kind !== "cite")) return null;
  const target = targetOf(project, item);
  if (!target) return null;
  const targets = project.definitions(target.namespace, target.name).map(({ path: defPath, item: def }) => {
    const location = toLocation(defPath, def);
    return def.title === undefined ? location : { ...location, title: def.title };
  });
  return { name: target.name, namespace: target.namespace, targets };
}

/**
 * Headings and labels of one file, in source order.
 * @param {ProjectIndex} project
 * @param {string} path
 * @returns {{ headings: IndexItem[], labels: IndexItem[] }}
 */
export function symbols(project, path) {
  const file = project.file(path);
  if (!file) return { headings: [], labels: [] };
  return {
    headings: byStart(file.items.filter((item) => item.kind === "heading")),
    labels: byStart(file.items.filter((item) => item.kind === "label")),
  };
}

/**
 * Block ranges, and a span for each heading up to the next heading of the same or higher level.
 * @param {ProjectIndex} project
 * @param {string} path
 * @returns {Array<[number, number]>}
 */
export function foldingRanges(project, path) {
  const file = project.file(path);
  if (!file) return [];
  /** @type {Array<[number, number]>} */
  const ranges = [];
  for (const item of file.items) {
    if (item.kind === "block") ranges.push(item.range);
  }
  const headings = byStart(file.items.filter((item) => item.kind === "heading"));
  headings.forEach((heading, index) => {
    const level = heading.level ?? 1;
    const next = headings.slice(index + 1).find((other) => (other.level ?? 1) <= level);
    ranges.push([heading.range[1], next ? next.range[0] : file.length]);
  });
  return ranges
    .filter(([from, to]) => to > from)
    .sort((a, b) => a[0] - b[0] || a[1] - b[1]);
}

/**
 * Unknown labels, citations and references, and missing includes and assets.
 * Citation checks are skipped while the bibliography is unknown; include and
 * asset checks are skipped while the file list is unknown.
 * @param {ProjectIndex} project
 * @param {string} path
 * @returns {Diagnostic[]}
 */
export function diagnostics(project, path) {
  const file = project.file(path);
  if (!file) return [];
  const known = project.knownFiles();
  /** @type {Diagnostic[]} */
  const out = [];

  /**
   * @param {IndexItem} item
   * @param {Diagnostic["code"]} code
   * @param {string[]} suggestions
   */
  const report = (item, code, suggestions) => {
    const [from, to] = spanOf(item);
    out.push({ path, from, to, severity: "warning", code, name: item.name ?? "", suggestions });
  };

  for (const item of byStart(file.items)) {
    if (item.name === undefined) continue;
    const name = item.name;

    if (item.kind === "include" || item.kind === "asset") {
      if (known === null || project.resolveFile(path, name) !== null) continue;
      const candidates = known.map((knownPath) => {
        const display = relativePath(dirname(path), knownPath);
        return hasExtension(name) ? display : display.replace(/\.[^./]*$/, "");
      });
      report(item, "missing-file", closest(name, candidates));
      continue;
    }

    if (item.kind === "ref") {
      const namespace = project.namespaceOf(item);
      if (namespace === "label" && item.namespace === "label") {
        if (project.definitions("label", name).length === 0) {
          report(item, "unknown-label", closest(name, project.names("label")));
        }
      } else if (namespace === "cite" && item.namespace === "either") {
        if (project.hasCitation(name) === false) {
          report(item, "unknown-reference", closest(name, project.names("cite")));
        }
      }
      continue;
    }

    if (item.kind === "cite" && project.hasCitation(name) === false) {
      report(item, "unknown-citation", closest(name, project.names("cite")));
    }
  }
  return out;
}

/**
 * Ranges of the file that are prose: the whole file minus code, math, and
 * items marked `attrs.prose === "skip"`. Overlapping skips are merged.
 * @param {ProjectIndex} project
 * @param {string} path
 * @returns {Array<[number, number]>}
 */
export function proseRanges(project, path) {
  const file = project.file(path);
  if (!file) return [];
  const skips = file.items
    .filter((item) => item.kind === "code" || item.kind === "math" || item.attrs?.prose === "skip")
    .map((item) => /** @type {[number, number]} */ ([
      Math.max(0, item.range[0]),
      Math.min(file.length, item.range[1]),
    ]))
    .filter(([from, to]) => to > from)
    .sort((a, b) => a[0] - b[0] || a[1] - b[1]);
  /** @type {Array<[number, number]>} */
  const out = [];
  let cursor = 0;
  for (const [from, to] of skips) {
    if (from > cursor) out.push([cursor, from]);
    cursor = Math.max(cursor, to);
  }
  if (file.length > cursor) out.push([cursor, file.length]);
  return out;
}
