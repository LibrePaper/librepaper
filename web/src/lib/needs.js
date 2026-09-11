// What a typst compile could not find, fetched: packages from the registry,
// fonts from this deployment's library.
//
// The compiler never fetches. It answers what it went looking for and did
// not find -- the `needs` export, beside the diagnostics -- and this fetches
// that, adds it to the file map, and compiles again, a few rounds at most: a
// package's own dependencies surface on the round after it arrives. What was
// fetched is kept for the life of the worker, because `clear_files` empties
// the map before every compile and a keystroke is a compile.
//
// A package version never changes once published, and a font file is
// addressed by its digest, so both are cached forever by URL (see
// `cache.js`); the second document that imports cetz costs nothing.

import { cached } from "./cache.js";
import { call, handOver, needsOf } from "./renderer-wasm.js";

/// How many fetch-and-compile rounds one render may take. Dependencies nest a
/// few levels deep at most; a document that still needs something after this
/// is shown what it still needs.
export const ROUNDS = 8;

/// The regular files in a tar, by name: enough of the format for what the
/// registry produces. Directories, links and anything exotic are skipped.
export function untar(bytes) {
  const decoder = new TextDecoder();
  const field = (from, to) => {
    const slice = bytes.subarray(from, to);
    const end = slice.indexOf(0);
    return decoder.decode(end < 0 ? slice : slice.subarray(0, end));
  };
  const files = [];
  for (let at = 0; at + 512 <= bytes.length; ) {
    const header = bytes.subarray(at, at + 512);
    if (header.every((b) => b === 0)) break;
    const size = parseInt(field(at + 124, at + 136).trim(), 8) || 0;
    const start = at + 512;
    const end = start + size;
    if (end > bytes.length) break;
    const kind = header[156];
    if (kind === 48 || kind === 0) {
      const prefix = field(at + 345, at + 500);
      const name = (prefix ? `${prefix}/` : "") + field(at, at + 100);
      const full = name.replace(/^\.\//, "");
      if (full && !full.endsWith("/")) files.push([full, bytes.slice(start, end)]);
    }
    at = end + ((512 - (size % 512)) % 512);
  }
  return files;
}

async function gunzip(response) {
  const stream = response.body.pipeThrough(new DecompressionStream("gzip"));
  return new Uint8Array(await new Response(stream).arrayBuffer());
}

/// The families this deployment has been asked for and does not serve, so
/// the index is not read again for them on every keystroke.
const lacking = new Set();
let index = null;

function fontIndex(url) {
  if (!url) return Promise.resolve(null);
  if (!index) {
    index = fetch(url)
      .then((response) => (response.ok ? response.json() : null))
      .catch(() => null);
  }
  return index;
}

/// Fetches what `needs` lists and the map does not have, into `fetched`.
/// Resolves to whether anything arrived: a compile with nothing new to read
/// would only say the same thing again.
export async function fetchNeeds(needs, fetched, { fontsIndex } = {}) {
  let arrived = false;
  for (const pkg of needs.packages || []) {
    if (!pkg.url || fetched.has(`${pkg.dir}/typst.toml`)) continue;
    const response = await cached(pkg.url);
    if (!response.ok) continue;
    for (const [name, body] of untar(await gunzip(response))) {
      fetched.set(`${pkg.dir}/${name}`, body);
    }
    arrived = fetched.has(`${pkg.dir}/typst.toml`);
  }
  const families = (needs.fonts || []).filter((family) => !lacking.has(family));
  if (families.length) {
    const known = await fontIndex(fontsIndex);
    for (const family of families) {
      const files = known?.families?.[family];
      if (!Array.isArray(files) || !files.length) {
        lacking.add(family);
        continue;
      }
      for (const file of files) {
        const path = `@fonts/${family}/${file.split("/").pop()}`;
        if (fetched.has(path)) continue;
        const response = await cached(new URL(file, fontsIndex).href);
        if (!response.ok) continue;
        fetched.set(path, new Uint8Array(await response.arrayBuffer()));
        arrived = true;
      }
    }
  }
  return arrived;
}

/// Compiles a document, fetching what each compile could not find, until it
/// has everything or nothing more can be found. The result is the last
/// compile's, whichever way it went: a document that still needs a package
/// the registry lacks is shown that error, as it would be by the binary.
export async function renderResolving(wasm, tree, title, fetched, options = {}) {
  const output = options.format;
  const compileExport = output === "html" ? "compile_html" : "compile";
  if (output !== undefined && output !== "pdf" && output !== "html") {
    throw new Error(`unsupported Typst output format: ${output}`);
  }
  if (compileExport === "compile_html" && typeof wasm.compile_html !== "function") {
    throw new Error("Typst HTML rendering is unavailable: this renderer module does not export compile_html");
  }
  for (let round = 0; ; round++) {
    handOver(wasm, tree);
    for (const [path, bytes] of fetched) {
      if (path in (tree.texts || {}) || path in (tree.assets || {})) continue;
      call(wasm, "add_file", path, bytes);
    }
    const source = tree.texts?.[tree.main] ?? "";
    const result = call(wasm, compileExport, source, title);
    const needs = needsOf(wasm);
    const wanting = needs && (needs.packages.length || needs.fonts.length);
    if (!wanting || round >= ROUNDS) return result;
    if (!(await fetchNeeds(needs, fetched, options))) return result;
  }
}
