// Project-relative path helpers for completion. Paths use "/" and never start
// with a slash. Pure functions with no imports, so tests and check scripts can
// load them directly.

/** @typedef {{ path: string, format: "latex"|"typst"|"markdown"|"quarto"|"html"|"", texts: Record<string, string>, files: string[] }} CompletionProject */
/** @typedef {() => CompletionProject} ProjectAccessor */

/** Split a path into segments, dropping "" and "." and collapsing "a/..". */
function segments(path) {
  const out = [];
  for (const part of String(path ?? "").split("/")) {
    if (!part || part === ".") continue;
    if (part === "..") {
      if (out.length && out.at(-1) !== "..") out.pop();
      else out.push("..");
      continue;
    }
    out.push(part);
  }
  return out;
}

/** Normalise a project path: "./a//b/../c.tex" -> "a/c.tex". */
export function normalizePath(path) {
  return segments(path).join("/");
}

/** Directory part of a project path: "a/b/c.tex" -> "a/b", "c.tex" -> "". */
export function dirname(path) {
  return segments(path).slice(0, -1).join("/");
}

/** Resolve a reference written relative to directory `base`: ("a/b", "../c.tex") -> "a/c.tex". */
export function resolvePath(base, reference) {
  return segments(`${base ?? ""}/${reference ?? ""}`).join("/");
}

// Path of `target` relative to directory `base` (both project-relative).
export function relativePath(base, target) {
  const from = segments(base), to = segments(target);
  // The last segment of `target` is a file name, so it is never shared.
  let shared = 0;
  while (shared < from.length && shared < to.length - 1 && from[shared] === to[shared]) shared++;
  const ups = from.length - shared;
  return [...Array(ups).fill(".."), ...to.slice(shared)].join("/");
}

function extensionOf(path) {
  const name = path.slice(path.lastIndexOf("/") + 1);
  const dot = name.lastIndexOf(".");
  return dot > 0 ? name.slice(dot + 1).toLowerCase() : "";
}

// Completion options for project files, written relative to directory `base`.
export function pathOptions(files, base, { extensions = [], dropExtension = false, exclude = "" } = {}) {
  const wanted = new Set(extensions.map((extension) => extension.replace(/^\./, "").toLowerCase()));
  const skip = normalizePath(exclude);
  const options = [];
  for (const file of files || []) {
    const path = normalizePath(file);
    if (!path || path === skip) continue;
    if (wanted.size && !wanted.has(extensionOf(path))) continue;
    let label = relativePath(base, path);
    if (dropExtension) label = label.replace(/\.[^./]*$/, "");
    options.push({ label, type: "file", detail: path });
  }
  return options.sort((a, b) => (a.label < b.label ? -1 : a.label > b.label ? 1 : 0));
}
