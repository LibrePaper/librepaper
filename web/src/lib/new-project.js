// The New project page's logic, with no DOM and no Svelte so node can test
// it: which templates the picker offers and in what order, what a new project
// is called and made in, and how files staged for upload are checked, grouped
// and named. The page itself is NewProject.svelte.
import { FORMATS, formatNamed, matchTemplates } from "./starter.js";
import { checkDirectoryPath, checkPath, collisionKey, normalisePath } from "./paths.js";
import { basename, copyPath, inside, parentPath } from "./file-manager.js";

const MAIN_ORDER = ["tex", "typ", "qmd", "md"];

/// Built-in templates and the caller's own, as one list. Blank has its own tab,
/// so it is left out here.
export function catalog(builtIn, customRows = []) {
  const shipped = builtIn
    .filter((template) => template.id !== "blank")
    .map((template) => ({
      id: template.id,
      name: template.name,
      description: template.description ?? "",
      keywords: template.keywords ?? [],
      formats: template.formats,
      custom: false,
      url: "",
    }));
  const own = customRows.map((row) => ({
    id: row.slug,
    name: row.title,
    description: "",
    keywords: [],
    formats: [row.source_format],
    custom: true,
    url: row.url,
  }));
  return [...shipped, ...own];
}

/// Filter and sort the templates. Every filter is optional and they combine.
export function browse(templates, { query = "", format = "", sort = "name-asc" } = {}) {
  const direction = sort === "name-desc" ? -1 : 1;
  return matchTemplates(templates, query, format)
    .sort((a, b) => direction * a.name.localeCompare(b.name, undefined, { sensitivity: "base" }) || a.id.localeCompare(b.id));
}

/// "Markdown, Quarto, LaTeX", or the first `max` of them and " +N" for the rest.
export function formatSummary(formats, max = 3) {
  const ordered = FORMATS.map((format) => format.id).filter((id) => formats.includes(id));
  const names = ordered.slice(0, max).map((id) => formatNamed(id).name);
  const rest = ordered.length - max;
  return rest > 0 ? `${names.join(", ")} +${rest}` : names.join(", ");
}

/// The first of the chosen, filtered and preferred formats that the template
/// offers, else the template's first format. "" when there is no template.
export function pickFormat(template, { chosen = "", filter = "", preferred = "" } = {}) {
  const offered = template?.formats ?? [];
  for (const id of [chosen, filter, preferred]) {
    if (id && offered.includes(id)) return id;
  }
  return offered[0] ?? "";
}

/// The first of "Untitled project", "Untitled project 2", 3, ... that no taken
/// title already is, ignoring case and surrounding spaces.
export function uniqueName(taken, base = "Untitled project") {
  const used = new Set([...taken].map((title) => title.trim().toLowerCase()));
  if (!used.has(base.toLowerCase())) return base;
  for (let n = 2; ; n++) {
    const candidate = `${base} ${n}`;
    if (!used.has(candidate.toLowerCase())) return candidate;
  }
}

// Hidden files and the metadata Finder leaves in a ZIP are not part of a
// project, so they are dropped without being mentioned. A ".." segment is not
// hidden but invalid, and checkPath says so.
const hidden = (path) => path.split("/").some((part) => (part.startsWith(".") && part !== "." && part !== "..") || part === "__MACOSX");

/// Adds incoming entries to a staged list under the server's rules, without
/// changing the list it is given. A path already staged, or given twice in
/// `incoming`, is a conflict for the person to settle, and is not added yet.
export function stage(staged, incoming, rules) {
  const result = [...staged];
  const conflicts = [];
  const skipped = [];
  const errors = [];
  for (const entry of incoming) {
    const path = normalisePath(entry.path);
    if (hidden(path)) continue;
    const checked = checkPath(rules, path);
    if (checked.error) {
      // A name that is a valid folder but not a supported file is skipped; a
      // name that is wrong in shape is reported.
      if (checkDirectoryPath(rules, path)) errors.push(checked.error);
      else skipped.push(path);
      continue;
    }
    const item = { path, file: entry.file, source: entry.source ?? "" };
    const key = collisionKey(path);
    if (result.some((other) => collisionKey(other.path) === key)) conflicts.push(item);
    else result.push(item);
  }
  return { staged: result, conflicts, skipped, errors };
}

/// Settles conflicts from `stage`: "replace" puts the incoming file in the
/// staged one's place, "keep-both" adds it under a new name, "skip" drops it.
export function settle(staged, conflicts, choice) {
  const result = [...staged];
  for (const conflict of conflicts) {
    if (choice === "replace") {
      const key = collisionKey(conflict.path);
      const index = result.findIndex((entry) => collisionKey(entry.path) === key);
      if (index === -1) result.push({ ...conflict });
      else result[index] = { ...conflict };
    } else if (choice === "keep-both") {
      result.push({ ...conflict, path: copyPath(conflict.path, result, []) });
    }
  }
  return result;
}

/// The staged list without a file at `path`, or without everything under it
/// when `path` is a folder.
export function unstage(staged, path) {
  const key = collisionKey(path);
  return staged.filter((entry) => {
    const own = collisionKey(entry.path);
    return own !== key && !inside(own, key);
  });
}

/// The one top-level folder every staged path sits in, or "" when there is none.
export function wrapperOf(staged) {
  if (!staged.length || staged.some((entry) => !entry.path.includes("/"))) return "";
  const tops = new Set(staged.map((entry) => entry.path.split("/")[0]));
  return tops.size === 1 ? [...tops][0] : "";
}

/// The staged paths as they will be sent, with the wrapper folder removed when
/// `strip` is true.
export function finalPaths(staged, strip) {
  const wrapper = strip ? wrapperOf(staged) : "";
  return staged.map((entry) => (wrapper ? entry.path.slice(wrapper.length + 1) : entry.path));
}

/// The paths that could be the main document: those ending in one of the
/// rules' renderable extensions, A to Z.
export function mainCandidates(paths, rules) {
  const extensions = (rules?.extensions || []).map((ext) => ext.toLowerCase());
  return paths
    .filter((path) => extensions.some((ext) => path.toLowerCase().endsWith(ext)))
    .sort((a, b) => a.localeCompare(b));
}

const rankOf = (path) => {
  const index = MAIN_ORDER.indexOf(path.slice(path.lastIndexOf(".") + 1).toLowerCase());
  return index === -1 ? MAIN_ORDER.length : index;
};

/// A sensible first document, or "" when it is ambiguous.
export function guessMain(candidates) {
  const root = candidates.filter((path) => !path.includes("/"));
  const mains = root
    .filter((path) => /^main\./i.test(path))
    .sort((a, b) => rankOf(a) - rankOf(b) || a.localeCompare(b));
  if (mains.length) return mains[0];
  const index = root.find((path) => /^index\.qmd$/i.test(path)) ?? root.find((path) => /^index\.md$/i.test(path));
  if (index) return index;
  if (root.length === 1) return root[0];
  if (candidates.length === 1) return candidates[0];
  return "";
}

/// A nested tree for display: folders first, then files, each A to Z. Folders
/// carry their children and the number of files below them.
export function tree(entries) {
  const top = { children: [] };
  const folders = new Map([["", top]]);
  const folderAt = (path) => {
    if (folders.has(path)) return folders.get(path);
    const node = { name: basename(path), path, children: [], count: 0 };
    folders.set(path, node);
    folderAt(parentPath(path)).children.push(node);
    return node;
  };
  for (const entry of entries) {
    folderAt(parentPath(entry.path)).children.push({ name: basename(entry.path), path: entry.path, size: entry.size });
  }
  const settleNode = (node) => {
    node.children.sort((a, b) => Number(!a.children) - Number(!b.children) || a.name.localeCompare(b.name));
    for (const child of node.children) if (child.children) settleNode(child);
    node.count = node.children.reduce((sum, child) => sum + (child.children ? child.count : 1), 0);
  };
  settleNode(top);
  return top.children;
}

/// A suggested project name: the ZIP's name, else the wrapper folder, else the
/// main document's stem.
export function suggestName(staged, { archives = [] } = {}) {
  for (const archive of archives) {
    const name = archive.replace(/\.zip$/i, "").trim();
    if (name) return name;
  }
  const wrapper = wrapperOf(staged);
  if (wrapper) return wrapper;
  const main = guessMain(staged.map((entry) => entry.path));
  return main ? basename(main).replace(/\.[^.]*$/, "") : "";
}

/// A size for a person to read: "512 B", "1.2 KB", "3.4 MB", "1.1 GB".
export function bytes(size) {
  if (size < 1024) return `${size} B`;
  const units = ["KB", "MB", "GB"];
  let value = size / 1024;
  let unit = 0;
  while (unit < units.length - 1 && value >= 1023.5) {
    value /= 1024;
    unit += 1;
  }
  const text = value < 10 ? value.toFixed(1) : String(Math.round(value));
  return `${text} ${units[unit]}`;
}
