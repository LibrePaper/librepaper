// Which file of a project renders on its own, which one it opens on, and
// which one the preview shows for the file open in the source.
//
// A project has no main file. These rules read the text of every file and
// nothing else. This is the twin of `librepaper-document`'s `opening.rs`, and
// both pass `web/tests/fixtures/opening.json`.

import { formatOf } from "./formats.js";
import { compareUtf8 } from "./projection-digest.js";

function baseName(path) {
  return path.slice(path.lastIndexOf("/") + 1);
}

function folderOf(path) {
  const at = path.lastIndexOf("/");
  return at < 0 ? "" : path.slice(0, at);
}

// Joins a folder and a relative name, folding `.` and `..`. A name that
// climbs out of the project root gives null.
function join(folder, name) {
  const parts = folder ? folder.split("/") : [];
  for (const part of name.split("/")) {
    if (part === "" || part === ".") continue;
    if (part === "..") {
      if (parts.length === 0) return null;
      parts.pop();
    } else {
      parts.push(part);
    }
  }
  return parts.length === 0 ? null : parts.join("/");
}

// The text with every LaTeX comment removed. A comment starts at a `%` that
// an even number of backslashes precedes.
function stripLatexComments(body) {
  return body
    .split("\n")
    .map((line) => {
      let slashes = 0;
      for (let at = 0; at < line.length; at += 1) {
        const ch = line[at];
        if (ch === "%" && slashes % 2 === 0) return line.slice(0, at);
        slashes = ch === "\\" ? slashes + 1 : 0;
      }
      return line;
    })
    .join("\n");
}

function hasDocumentClass(body) {
  let rest = stripLatexComments(body);
  const command = "\\documentclass";
  for (let at = rest.indexOf(command); at >= 0; at = rest.indexOf(command)) {
    rest = rest.slice(at + command.length);
    if (!/^[A-Za-z]/.test(rest)) return true;
  }
  return false;
}

function latexNames(text) {
  const names = [];
  for (const command of ["\\input", "\\include", "\\subfile"]) {
    let rest = text;
    for (let at = rest.indexOf(command); at >= 0; at = rest.indexOf(command)) {
      rest = rest.slice(at + command.length);
      const open = rest.replace(/^[ \t]+/, "");
      if (!open.startsWith("{")) continue;
      const close = open.indexOf("}");
      if (close < 0) continue;
      const name = open.slice(1, close).trim();
      if (name) names.push(name);
    }
  }
  return names;
}

function typstNames(body) {
  const names = [];
  for (const command of ["#include", "#import"]) {
    let rest = body;
    for (let at = rest.indexOf(command); at >= 0; at = rest.indexOf(command)) {
      rest = rest.slice(at + command.length);
      const quoted = rest.replace(/^\s+/, "");
      if (!quoted.startsWith('"')) continue;
      const close = quoted.indexOf('"', 1);
      if (close < 0) continue;
      const name = quoted.slice(1, close);
      if (name && !name.startsWith("@")) names.push(name);
    }
  }
  return names;
}

function shortcodeNames(body) {
  const names = [];
  let rest = body;
  for (let at = rest.indexOf("{{<"); at >= 0; at = rest.indexOf("{{<")) {
    rest = rest.slice(at + 3);
    const call = rest.replace(/^\s+/, "");
    if (!call.startsWith("include")) continue;
    const args = call.slice("include".length);
    if (!/^\s/.test(args)) continue;
    const token = args.split(">}}")[0].trim().split(/\s+/)[0] || "";
    const name = token.replace(/^["']+|["']+$/g, "");
    if (name) names.push(name);
  }
  return names;
}

function includedNames(format, body) {
  if (format === "latex") return latexNames(stripLatexComments(body));
  if (format === "typst") return typstNames(body);
  if (format === "quarto" || format === "markdown") return shortcodeNames(body);
  return [];
}

// The file a name inside `from` points at, when it is a text of the project.
function resolve(format, from, name, texts) {
  const folder = folderOf(from);
  const found = (candidate) => (candidate !== null && Object.hasOwn(texts, candidate) ? candidate : null);
  if (format === "latex") {
    const named = baseName(name).includes(".") ? name : `${name}.tex`;
    return found(join(folder, named)) ?? found(join("", named));
  }
  if (format === "typst" && name.startsWith("/")) return found(join("", name.slice(1)));
  return found(join(folder, name));
}

/// The files that include each file, as a `Map` from path to a sorted list.
/// `texts` is `{ [path]: body }`. Only paths present in `texts` appear, and a
/// file never includes itself.
export function includersOf(texts) {
  const found = new Map();
  for (const [path, body] of Object.entries(texts)) {
    const format = formatOf(path);
    if (!format) continue;
    for (const name of includedNames(format, body)) {
      const target = resolve(format, path, name, texts);
      if (target === null || target === path) continue;
      if (!found.has(target)) found.set(target, new Set());
      found.get(target).add(path);
    }
  }
  const out = new Map();
  for (const [target, from] of found) out.set(target, [...from].sort(compareUtf8));
  return out;
}

/// Whether `path` renders on its own: it is a document and nothing makes it a
/// part of another one. A file with no document format never does.
export function rendersAlone(path, texts, includers = includersOf(texts)) {
  const included = (includers.get(path) || []).length > 0;
  switch (formatOf(path)) {
    case "latex":
      return Object.hasOwn(texts, path) && hasDocumentClass(texts[path]);
    case "typst":
      return !included;
    case "quarto":
    case "markdown":
      return !baseName(path).startsWith("_") && !included;
    case "html":
      return true;
    default:
      return false;
  }
}

function isReadmeOrLicense(path) {
  const stem = baseName(path).split(".")[0].toUpperCase();
  return stem === "README" || stem === "LICENSE";
}

function compareCaseInsensitive(left, right) {
  return compareUtf8(left.toLowerCase(), right.toLowerCase()) || compareUtf8(left, right);
}

/// The file a project opens on, or null for a project with no text. Paths sort
/// case-insensitively, ties broken by byte order. The first match of these
/// wins: a top-level file that renders on its own, is not included and is not
/// a README or LICENSE; the same over the whole tree; any file that renders on
/// its own, top level first; any text file, top level first.
export function openingFile(texts) {
  const includers = includersOf(texts);
  const paths = Object.keys(texts).sort(compareCaseInsensitive);
  const top = paths.filter((path) => !path.includes("/"));
  const alone = (path) => rendersAlone(path, texts, includers);
  const free = (path) => alone(path) && !includers.has(path) && !isReadmeOrLicense(path);
  return (
    top.find(free) ??
    paths.find(free) ??
    top.find(alone) ??
    paths.find(alone) ??
    top[0] ??
    paths[0] ??
    null
  );
}

/// The file the preview renders for the file open in the source.
///
/// `open` is the path open in the source, `locked` the path the preview is
/// locked on (or null), and `texts` is `{ [path]: body }`. The answer is
/// `{ path, reason, includers }`, where `includers` are the files that include
/// the open file directly, sorted.
///
/// - `locked`: the locked file exists and renders on its own.
/// - `alone`: the open file renders on its own.
/// - `inside`: the open file cannot render alone, and walking up through files
///   that each have exactly one includer reaches one that does. `path` is that
///   file.
/// - `none`: no preview. `path` is null.
export function previewTarget({ open, locked, texts }) {
  const includers = includersOf(texts);
  const direct = (path) => includers.get(path) || [];
  const alone = (path) => rendersAlone(path, texts, includers);
  const listed = open ? direct(open) : [];

  if (locked && Object.hasOwn(texts, locked) && alone(locked)) {
    return { path: locked, reason: "locked", includers: listed };
  }
  if (open && Object.hasOwn(texts, open)) {
    if (alone(open)) return { path: open, reason: "alone", includers: listed };
    const seen = new Set([open]);
    let current = open;
    for (;;) {
      const above = direct(current);
      if (above.length !== 1 || seen.has(above[0])) break;
      current = above[0];
      seen.add(current);
      if (alone(current)) return { path: current, reason: "inside", includers: listed };
    }
  }
  return { path: null, reason: "none", includers: listed };
}
