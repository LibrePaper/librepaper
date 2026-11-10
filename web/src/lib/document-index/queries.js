// Queries over the project index, named after LSP's. Each takes the project
// index first, then the file path, then a UTF-16 position where needed.

import { dirname, relativePath } from "../completion/paths.js";
import { labelNameAllowed } from "./label-names.js";

/** @typedef {import("./schema.js").IndexItem} IndexItem */
/** @typedef {import("./schema.js").Location} Location */
/** @typedef {import("./schema.js").Edit} Edit */
/** @typedef {import("./schema.js").Diagnostic} Diagnostic */
/** @typedef {import("./schema.js").Namespace} Namespace */
/** @typedef {import("./project.js").ProjectIndex} ProjectIndex */
/** @typedef {{ namespace: Namespace, name: string, path?: string }} Target `path` is the file the symbol was named in, which scopes its labels */

const LOCATABLE = new Set(["label", "ref", "cite", "include", "asset", "link"]);

/** Target kinds whose labels are numbered and listed, so an unreferenced one is worth a note. */
const NUMBERED_TARGETS = new Set(["figure", "table", "equation", "listing"]);

/** Target kinds that get a display number in `numbering`. Each kind counts on its own, except theorems, which share one sequence. */
const COUNTED_TARGETS = new Set(["figure", "table", "equation", "listing", "theorem"]);

/** Diagnostic codes at severity "info"; every other code is a "warning". */
const INFO_CODES = new Set(["unused-footnote", "unused-target", "unused-bibliography"]);

/**
 * The kind a label names, from its `attrs.target`, or null when it has none.
 * @param {IndexItem} item
 * @returns {string | null}
 */
export function targetKind(item) {
  return item.attrs?.target ?? null;
}

/**
 * A link to a project file, as opposed to a URL.
 * @param {IndexItem} item
 * @returns {boolean}
 */
const isFileLink = (item) => item.kind === "link" && item.attrs?.file === "true";

/**
 * The file's metadata items grouped by name ("title", "author", ...), in source order.
 * @param {ProjectIndex} project
 * @param {string} path
 * @returns {Record<string, IndexItem[]>}
 */
export function metadata(project, path) {
  const file = project.file(path);
  /** @type {Record<string, IndexItem[]>} */
  const out = {};
  if (!file) return out;
  for (const item of byStart(file.items)) {
    if (item.kind !== "metadata" || item.name === undefined) continue;
    (out[item.name] ??= []).push(item);
  }
  return out;
}

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
 * @param {string} path the file holding the item
 * @returns {Target | null}
 */
function targetOf(project, item, path) {
  if (item.name === undefined) return null;
  if (item.kind === "label" || item.kind === "ref" || item.kind === "cite") {
    const namespace = project.namespaceOf(item, path);
    return namespace ? { namespace, name: item.name, path } : null;
  }
  return null;
}

/**
 * Where a ref or cite at pos points to (labels and footnotes only), or where an
 * include, asset or file link at pos resolves to.
 * @param {ProjectIndex} project
 * @param {string} path
 * @param {number} pos
 * @returns {Location[]}
 */
export function definition(project, path, pos) {
  const item = itemAt(project, path, pos);
  if (!item || item.name === undefined) return [];
  if (item.kind === "ref" || item.kind === "cite") {
    const namespace = project.namespaceOf(item, path);
    if (namespace !== "label" && namespace !== "footnote") return [];
    return project.definitions(namespace, item.name, path).map(({ path: defPath, item: def }) => toLocation(defPath, def));
  }
  if (item.kind === "include" || item.kind === "asset" || isFileLink(item)) {
    const resolved = project.resolveFile(path, item.name, item.kind);
    return resolved === null ? [] : [{ path: resolved, from: 0, to: 0 }];
  }
  return [];
}

/**
 * The symbol a ref, cite or label at pos stands for, or null.
 * @param {ProjectIndex} project
 * @param {string} path
 * @param {number} pos
 * @returns {Target | null}
 */
export function targetAt(project, path, pos) {
  const item = itemAt(project, path, pos);
  return item ? targetOf(project, item, path) : null;
}

/**
 * Every use of a symbol, and optionally its declaration, sorted by path then offset.
 * @param {ProjectIndex} project
 * @param {Target} target
 * @param {{ includeDeclaration?: boolean }} [options]
 * @returns {Location[]}
 */
export function referencesTo(project, target, { includeDeclaration = true } = {}) {
  const out = project
    .references(target.namespace, target.name, target.path)
    .map(({ path: usePath, item: use }) => toLocation(usePath, use));
  if (includeDeclaration) {
    for (const { path: defPath, item: def } of project.definitions(target.namespace, target.name, target.path)) {
      out.push(toLocation(defPath, def));
    }
  }
  return sortBySpan(out);
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
  const target = targetAt(project, path, pos);
  return target ? referencesTo(project, target, { includeDeclaration }) : [];
}

/**
 * Why a rename cannot apply: "invalid" for a position that is not a label or a
 * name that is empty, spaced or unchanged, "chars" for a name the file's format
 * cannot hold, "exists" when another label already has the name. Null when it can.
 * @param {ProjectIndex} project
 * @param {string} path
 * @param {number} pos
 * @param {string} newName
 * @returns {"invalid" | "chars" | "exists" | null}
 */
export function renameProblem(project, path, pos, newName) {
  const item = itemAt(project, path, pos);
  if (!item || item.name === undefined) return "invalid";
  const namespace = item.kind === "label" || item.kind === "ref" ? project.namespaceOf(item, path) : null;
  if (namespace !== "label" && namespace !== "footnote") return "invalid";
  if (typeof newName !== "string" || newName === "" || /\s/.test(newName) || newName === item.name) return "invalid";
  // An Org headline is named by its title, which rename does not rewrite, so
  // a headline's label cannot be renamed without breaking its links.
  const fixed = (form) => form === "heading";
  const unrenameable =
    fixed(item.attrs?.form) ||
    project.definitions(namespace, item.name, path).some(({ item: def }) => fixed(def.attrs?.form));
  if (unrenameable) return "invalid";
  const format = project.file(path)?.format ?? "";
  if (!labelNameAllowed(format, item.name, newName)) return "chars";
  if (project.definitions(namespace, newName, path).length > 0) return "exists";
  return null;
}

/**
 * Rename a label or footnote: its definitions and every reference that
 * resolves to it. Only the bare name is rewritten, so a footnote's `[^` and
 * `]` stay where they are.
 * @param {ProjectIndex} project
 * @param {string} path
 * @param {number} pos
 * @param {string} newName
 * @returns {Edit[] | null} null when `renameProblem` finds one
 */
export function rename(project, path, pos, newName) {
  if (renameProblem(project, path, pos, newName) !== null) return null;
  const item = /** @type {IndexItem} */ (itemAt(project, path, pos));
  const name = /** @type {string} */ (item.name);
  const namespace = /** @type {Namespace} */ (project.namespaceOf(item, path));
  const sites = [
    ...project.definitions(namespace, name, path),
    ...project.references(namespace, name, path),
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
 * The target of a ref or cite at pos, with label titles and target kinds
 * ("figure", "footnote", ...) where the definition has them.
 * @param {ProjectIndex} project
 * @param {string} path
 * @param {number} pos
 * @returns {{ name: string, namespace: Namespace, targets: Array<Location & { title?: string, kind?: string }> } | null}
 */
export function hover(project, path, pos) {
  const item = itemAt(project, path, pos);
  if (!item || (item.kind !== "ref" && item.kind !== "cite")) return null;
  const target = targetOf(project, item, path);
  if (!target) return null;
  const targets = project.definitions(target.namespace, target.name, target.path).map(({ path: defPath, item: def }) => {
    /** @type {Location & { title?: string, kind?: string }} */
    const entry = toLocation(defPath, def);
    if (def.title !== undefined) entry.title = def.title;
    const kind = targetKind(def);
    if (kind !== null) entry.kind = kind;
    return entry;
  });
  return { name: target.name, namespace: target.namespace, targets };
}

/**
 * Display numbers for the labels of figures, tables, equations, listings and
 * theorems, project-wide. The map is keyed `label:<name>` and holds the number
 * as an integer; hover formats it for the interface language.
 *
 * Reading order: each root file (an indexed file that no other indexed file
 * includes) is walked in source order, and at each `include` the included file
 * is walked at that point, so numbers continue through it and resume after it.
 * Each file is walked once per root, so a cycle ends and a file included twice
 * is counted at its first inclusion. Counters start at zero for each root. When
 * no root exists (files that only include each other), the unreached files are
 * walked as roots in path order. A label in a file shared by two roots keeps
 * the number from the first root, and a name defined twice takes the first
 * definition in that order. Implicit labels and labels of other targets are
 * skipped. Labels that a root's walk does not reach are not numbered.
 *
 * Sections are not numbered here. Their numbers depend on the format (LaTeX's
 * chapter and section depth, Typst's heading levels, the HTML outline), so
 * each format's numbering belongs with its adapter.
 *
 * Not cached: the project index has no revision counter, and a walk is cheap
 * next to a hover.
 * @param {ProjectIndex} project
 * @returns {Map<string, number>}
 */
export function numbering(project) {
  const paths = project.paths();
  const indexed = new Set(paths);
  /** @type {Map<IndexItem, string>} include items that resolve to an indexed file */
  const resolvedOf = new Map();
  const included = new Set();
  for (const path of paths) {
    for (const item of project.file(path)?.items ?? []) {
      if (item.kind !== "include" || item.name === undefined) continue;
      const resolved = project.resolveFile(path, item.name, "include");
      if (resolved === null || resolved === path || !indexed.has(resolved)) continue;
      resolvedOf.set(item, resolved);
      included.add(resolved);
    }
  }

  /** @type {Map<string, number>} */
  const numbers = new Map();
  /** @type {Set<string>} files walked from any root */
  const reached = new Set();

  /**
   * @param {string} path
   * @param {{ visited: Set<string>, counters: Record<string, number> }} walk
   */
  const visit = (path, walk) => {
    if (walk.visited.has(path)) return;
    walk.visited.add(path);
    reached.add(path);
    const file = project.file(path);
    if (!file) return;
    for (const item of byStart(file.items)) {
      const target = resolvedOf.get(item);
      if (target !== undefined) {
        visit(target, walk);
        continue;
      }
      if (item.kind !== "label" || item.name === undefined || item.attrs?.implicit === "true") continue;
      const kind = targetKind(item);
      if (kind === null || !COUNTED_TARGETS.has(kind)) continue;
      const count = (walk.counters[kind] ?? 0) + 1;
      walk.counters[kind] = count;
      const key = `label:${item.name}`;
      if (!numbers.has(key)) numbers.set(key, count);
    }
  };

  const walkRoot = (root) => visit(root, { visited: new Set(), counters: {} });
  for (const path of paths) {
    if (!included.has(path)) walkRoot(path);
  }
  for (const path of paths) {
    if (!reached.has(path)) walkRoot(path);
  }
  return numbers;
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
    labels: byStart(file.items.filter((item) => item.kind === "label" && project.namespaceOf(item, path) === "label")),
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
 * Unknown labels, citations, references and footnotes, duplicate labels,
 * missing files (includes, assets and file links), and notes nothing uses: a
 * footnote with no reference, a figure, table, equation or listing nothing
 * refers to, and a bibliography in a project that cites nothing. Citation
 * checks are skipped while the bibliography is unknown; file checks while the
 * file list is unknown. The unused notes are "info"; the rest are "warning".
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
    const severity = INFO_CODES.has(code) ? "info" : "warning";
    out.push({ path, from, to, severity, code, name: item.name ?? "", suggestions });
  };

  /** @returns {boolean} whether any file of the project cites a key */
  const citedAnywhere = () =>
    project.paths().some((other) =>
      (project.file(other)?.items ?? []).some(
        (item) => item.kind === "cite" || (item.kind === "ref" && project.namespaceOf(item, other) === "cite"),
      ),
    );

  for (const item of byStart(file.items)) {
    if (item.kind === "bibliography") {
      if (!citedAnywhere()) report(item, "unused-bibliography", []);
      continue;
    }
    if (item.name === undefined) continue;
    const name = item.name;

    if (item.kind === "label") {
      if (item.attrs?.implicit === "true") continue;
      const namespace = /** @type {Namespace} */ (project.namespaceOf(item, path));
      const duplicated = project
        .definitions(namespace, name, path)
        .some(({ item: def }) => def !== item && def.attrs?.implicit !== "true");
      if (duplicated) report(item, "duplicate-label", []);
      if (namespace === "footnote") {
        // An inline definition ([fn:x:text]) is used where it stands.
        if (item.attrs?.inline !== "true" && project.references("footnote", name, path).length === 0) report(item, "unused-footnote", []);
      } else {
        const kind = targetKind(item);
        if (kind !== null && NUMBERED_TARGETS.has(kind) && project.references("label", name, path).length === 0) {
          report(item, "unused-target", []);
        }
      }
      continue;
    }

    if (item.kind === "include" || item.kind === "asset" || isFileLink(item)) {
      if (known === null || project.resolveFile(path, name, item.kind) !== null) continue;
      // LaTeX names read from the project root, so suggest them from there.
      const fromRoot = file.format === "latex";
      const candidates = known.map((knownPath) => {
        const display = fromRoot ? knownPath : relativePath(dirname(path), knownPath);
        return hasExtension(name) ? display : display.replace(/\.[^./]*$/, "");
      });
      report(item, "missing-file", closest(name, candidates));
      continue;
    }

    if (item.kind === "ref") {
      const namespace = project.namespaceOf(item, path);
      if (namespace === "label" && item.namespace === "label") {
        if (project.definitions("label", name, path).length === 0) {
          report(item, "unknown-label", closest(name, project.names("label", path)));
        }
      } else if (namespace === "footnote") {
        if (project.definitions("footnote", name, path).length === 0) {
          report(item, "unknown-footnote", closest(name, project.names("footnote", path)));
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
