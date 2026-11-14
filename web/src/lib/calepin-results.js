// Calepin's stored results, as the browser draft of a Calepin document reads them.
//
// The desktop app runs a Typst document's code chunks with `calepin compile`
// and keeps the `.calepin/` folder it writes beside the document. This module
// turns that folder into a record the page can keep and publish, lays the
// files over the project so plain Typst can show the stored results, and
// finds the chunks whose code changed since the last run. It never runs code.
//
// Nothing here is user-visible text. Everything returns data.

/** What Calepin's runtime says when a chunk has no stored result. */
export const MISSING_LABEL = /calepin results do not contain label/;

/** The `active.typ` that turns stored results off. */
export const DISABLED_ACTIVE = "#let config = (enabled: false)\n";

const ACTIVE = ".calepin/active.typ";
const IMPORT = /#import\s+"\/?\.calepin\//;
const DEFAULT_LANGS = ["python", "r", "mermaid", "dot", "tikz", "d2"];

// The `.calepin/<stem>/...` folder of a document is named after the path
// without its last extension, so `chapters/intro.typ` uses `chapters/intro`.
function stemOf(path) {
  return String(path ?? "").replace(/\.[^./]*$/, "");
}

function entriesOf(files) {
  return Array.isArray(files) ? files : [];
}

// Finds the file a document's Calepin folder keeps under `name`. The folder
// named after the document wins; otherwise the first match anywhere.
function findFile(files, path, name) {
  const stem = stemOf(path);
  const exact = stem ? `.calepin/${stem}/${name}` : null;
  const readable = entriesOf(files).filter((file) => typeof file?.text === "string");
  return (
    (exact && readable.find((file) => file.path === exact)) ||
    readable.find((file) => file.path.endsWith(`/${name}`)) ||
    null
  );
}

// True when any `.typ` text imports from `/.calepin/`. `texts` is { path: string }.
export function usesCalepin(texts) {
  if (!texts || typeof texts !== "object") return false;
  return Object.keys(texts).some(
    (path) => path.endsWith(".typ") && typeof texts[path] === "string" && IMPORT.test(texts[path]),
  );
}

// True for a record object with engine === "calepin" and an array of files.
export function isCalepinRecord(record) {
  return Boolean(record) && typeof record === "object" && record.engine === "calepin" && Array.isArray(record.files);
}

// A file entry is safe to keep when it sits under `.calepin/` with plain
// segments and carries its content as exactly one of text or base64.
function wellFormed(entry) {
  if (!entry || typeof entry !== "object") return false;
  const { path, text, base64 } = entry;
  if (typeof path !== "string" || !path.startsWith(".calepin/")) return false;
  if (path.includes("\\") || path.includes("\0")) return false;
  if (path.split("/").some((segment) => segment === "" || segment === "." || segment === "..")) return false;
  return (typeof text === "string") !== (typeof base64 === "string");
}

// The record for a run, from the JSON bytes the desktop app returned.
// `bytes` is a Uint8Array, a string, or null/undefined. Returns
// { version: 1, path, engine: "calepin", files } or null.
export function recordOf(bytes, path) {
  let source;
  if (typeof bytes === "string") source = bytes;
  else if (bytes instanceof Uint8Array) source = new TextDecoder().decode(bytes);
  else return null;

  let parsed;
  try {
    parsed = JSON.parse(source);
  } catch {
    return null;
  }
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return null;
  if (parsed.version !== 1) return null;
  const { files } = parsed;
  if (!Array.isArray(files) || files.length === 0) return null;
  if (!files.every(wellFormed)) return null;
  if (!files.some((file) => file.path.endsWith("/results.json"))) return null;

  // Keep only the known fields, so a stray property in the bytes never
  // reaches the page's record.
  const kept = files.map((file) =>
    typeof file.text === "string" ? { path: file.path, text: file.text } : { path: file.path, base64: file.base64 },
  );
  return { version: 1, path, engine: "calepin", files: kept };
}

// The bundled runtime with results switched off, as file entries. Async: the runtime is loaded on demand.
export async function runtimeFiles() {
  const { FILES } = await import("./calepin-runtime.js");
  return [
    ...FILES.filter((file) => file.path !== ACTIVE).map((file) => ({ ...file })),
    { path: ACTIVE, text: DISABLED_ACTIVE },
  ];
}

// Decodes base64 to bytes, or null when it does not decode.
function decodeBase64(base64) {
  if (typeof atob !== "function") return null;
  try {
    return Uint8Array.from(atob(base64), (char) => char.charCodeAt(0));
  } catch {
    return null;
  }
}

// A copy of `tree` whose texts and assets also hold `files`, at the project
// root (the entry's own path, e.g. ".calepin/calepin.typ"). With `disabled`,
// `.calepin/active.typ` is replaced by DISABLED_ACTIVE. The project's own
// files are never overwritten, and `tree` itself is never mutated.
export function overlay(tree, files, { disabled = false } = {}) {
  const texts = { ...tree.texts };
  const assets = { ...tree.assets };
  for (const file of entriesOf(files)) {
    const { path } = file;
    if (disabled && path === ACTIVE) continue;
    if (Object.hasOwn(texts, path) || Object.hasOwn(assets, path)) continue;
    if (typeof file.text === "string") {
      texts[path] = file.text;
    } else if (typeof file.base64 === "string") {
      const bytes = decodeBase64(file.base64);
      if (bytes) assets[path] = bytes;
    }
  }
  if (disabled) texts[ACTIVE] = DISABLED_ACTIVE;
  return { ...tree, texts, assets };
}

// The fenced code blocks Calepin would run, in order: [{ lang, source }].
// Blocks in other languages are skipped whole, so a four-backtick example
// that shows a python fence does not count as a chunk.
export function chunksOf(source, langs) {
  const wanted = new Set([...(langs ?? [])].map((lang) => String(lang).toLowerCase()));
  const lines = String(source ?? "").split(/\r?\n/);
  const blocks = [];
  let i = 0;
  while (i < lines.length) {
    const open = /^(`{3,})([A-Za-z0-9_+-]*)$/.exec(lines[i].trim());
    if (!open) {
      i += 1;
      continue;
    }
    const [, ticks, tag] = open;
    let close = -1;
    for (let j = i + 1; j < lines.length; j += 1) {
      const text = lines[j].trim();
      if (/^`+$/.test(text) && text.length >= ticks.length) {
        close = j;
        break;
      }
    }
    // An unclosed fence ends the scan: nothing after it is returned.
    if (close < 0) break;
    const lang = tag.toLowerCase();
    if (wanted.has(lang)) {
      const body = lines.slice(i + 1, close);
      // Strip the indentation the block shares, so a chunk nested in a list
      // compares equal to the same code at the top level.
      const indents = body.filter((line) => line.trim() !== "").map((line) => /^[ \t]*/.exec(line)[0].length);
      const indent = indents.length ? Math.min(...indents) : 0;
      const code = body.map((line) => line.slice(indent));
      // The `#|` option lines at the top of a chunk are not stored with its
      // code, so they are dropped here; a later `#|` line is real code and stays.
      let first = 0;
      while (first < code.length && code[first].trim().startsWith("#|")) first += 1;
      blocks.push({ lang, source: code.slice(first).join("\n") });
    }
    i = close + 1;
  }
  return blocks;
}

// The fence languages a run executed, read from its runtime-config.typ.
export function rawLangs(files, { path = "" } = {}) {
  const config = findFile(files, path, "runtime-config.typ");
  const match = config && /"raw-langs"\s*:\s*\(([^)]*)\)/.exec(config.text);
  if (!match) return [...DEFAULT_LANGS];
  return [...match[1].matchAll(/"([^"]*)"/g)].map((quoted) => quoted[1]);
}

// How many of the document's fenced chunks have no stored result with the same code.
// Never throws: an unreadable results.json means every current chunk is stale.
export function staleChunks(files, source, { path = "" } = {}) {
  const normalise = (text) => String(text ?? "").replace(/\s+$/, "");
  const stored = findFile(files, path, "results.json");
  let chunks = {};
  if (stored) {
    try {
      const data = JSON.parse(stored.text);
      if (data && typeof data.chunks === "object" && data.chunks !== null) chunks = data.chunks;
    } catch {
      chunks = {};
    }
  }

  // One count per stored copy of each source, so duplicated code needs as
  // many stored results as it has occurrences in the document.
  const copies = new Map();
  for (const chunk of Object.values(chunks)) {
    if (typeof chunk?.source !== "string") continue;
    const key = normalise(chunk.source);
    copies.set(key, (copies.get(key) ?? 0) + 1);
  }

  let stale = 0;
  for (const block of chunksOf(source, rawLangs(files, { path }))) {
    const key = normalise(block.source);
    const count = copies.get(key) ?? 0;
    if (count > 0) copies.set(key, count - 1);
    else stale += 1;
  }
  return stale;
}
