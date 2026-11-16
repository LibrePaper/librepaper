import { sha256Hex } from "./digest.js";

// The engine ABI runs exclusively in the renderer worker.
const loads = {};
const REQUIRED_EXPORTS = [
  "memory", "alloc", "dealloc", "compile", "output_ptr", "ok", "output_kind",
  "diagnostics", "diagnostics_ptr", "failure_page", "title_of", "add_file",
  "clear_files", "set_main", "set_asset_url", "set_today", "word_diff",
];

/** @typedef {(...args: number[]) => number} RendererFunction */
/** @typedef {WebAssembly.Exports & {memory: WebAssembly.Memory, alloc: RendererFunction, dealloc: RendererFunction, compile: RendererFunction, compile_html?: RendererFunction, output_ptr: RendererFunction, ok: RendererFunction, output_kind: RendererFunction, diagnostics: RendererFunction, diagnostics_ptr: RendererFunction, failure_page: RendererFunction, title_of: RendererFunction, add_file: RendererFunction, clear_files: RendererFunction, set_main: RendererFunction, set_asset_url: RendererFunction, set_today: RendererFunction, word_diff: RendererFunction, bibliography?: RendererFunction, complete?: RendererFunction, draft?: RendererFunction, needs?: RendererFunction, needs_ptr?: RendererFunction}} RendererWasm */

/** @param {RendererWasm} wasm @param {string} name @returns {RendererFunction} */
function functionExport(wasm, name) {
  const value = wasm[name];
  if (!isRendererFunction(value)) throw new Error(`renderer export ${name} is unavailable`);
  return value;
}

/** @param {WebAssembly.ExportValue} value @returns {value is RendererFunction} */
function isRendererFunction(value) {
  return typeof value === "function";
}

// Keep the ABI check at the module boundary. This makes a stale or wrong
// artifact fail while loading, with the complete list of what it lacks,
// instead of failing later in a worker operation with an opaque TypeError.
/** @param {WebAssembly.Exports} wasm @param {string} [url] @returns {RendererWasm} */
export function validateExports(wasm, url = "renderer") {
  const missing = REQUIRED_EXPORTS.filter((name) => {
    if (name === "memory") return !(wasm[name] instanceof WebAssembly.Memory);
    return !isRendererFunction(wasm[name]);
  });
  if (missing.length) {
    throw new Error(`incompatible renderer module ${url}: missing or invalid exports: ${missing.join(", ")}`);
  }
  if (typeof wasm.default_fonts_required === "function" && wasm.default_fonts_required() !== 0) {
    throw new Error(`incompatible Typst module ${url}: external fonts are unsupported; use an embedded-font build`);
  }
  return /** @type {RendererWasm} */ (wasm);
}

/// The 64-hex digest the mirror puts in a module URL, as in
/// `{mirror}wasm/{sha256}/{module}.wasm`: the path segment just before the
/// last one. `null` when the URL has no such segment.
export function digestInUrl(url) {
  try {
    const segments = new URL(url, globalThis.location?.href).pathname.split("/");
    const candidate = segments.at(-2);
    return candidate && /^[0-9a-f]{64}$/.test(candidate) ? candidate : null;
  } catch {
    return null;
  }
}

/// Fetches a module and returns its bytes only if they hash to the digest in
/// the URL. The mirror is a third party to the page, and the module runs with
/// the page's authority, so a swapped file must not get as far as compiling.
export async function verifiedModuleBytes(url, fetchImpl = fetch) {
  const expected = digestInUrl(url);
  if (!expected) throw new Error(`renderer module ${url} failed digest verification: the URL names no digest`);
  const response = await fetchImpl(url);
  if (!response.ok) throw new Error(`renderer module ${url} could not be fetched (${response.status})`);
  const buffer = await response.arrayBuffer();
  if (await sha256Hex(new Uint8Array(buffer)) !== expected) {
    throw new Error(`renderer module ${url} failed digest verification: the bytes do not match the digest in the URL`);
  }
  return buffer;
}

async function instantiate(url) {
  return (await WebAssembly.instantiate(await verifiedModuleBytes(url), {})).instance;
}

export function load(url) {
  if (!url) return Promise.reject(new Error("no renderer URL"));
  if (loads[url]) return loads[url];
  loads[url] = instantiate(url)
    .then(({ exports: wasm }) => {
      const renderer = validateExports(wasm, url);
      // The compiler has no clock of its own, so typst's datetime.today() is
      // whatever this tab says it is.
      const now = new Date();
      renderer.set_today(now.getFullYear(), now.getMonth() + 1, now.getDate());
      return renderer;
    }).catch((error) => {
      delete loads[url];
      throw error;
    });
  return loads[url];
}

// Each argument is written into the module's memory and passed as a (pointer,
// length) pair. A string is written as UTF-8; bytes are written as they are,
// which is how a figure reaches the compiler. The module's memory can be
// replaced when it grows, so a view of it is taken after every call that might
// have grown it, never held across one.
/** @param {RendererWasm} wasm @param {string} name @param {...(string|Uint8Array|number)} strings */
export function call(wasm, name, ...strings) {
  const encoder = new TextEncoder();
  const written = [];
  try {
    for (const value of strings) {
      // A number is a plain argument (a flag), passed as it is.
      if (typeof value === "number") { written.push({ plain: value }); continue; }
      const bytes = typeof value === "string" ? encoder.encode(value) : value;
      const pointer = wasm.alloc(bytes.length);
      written.push({ pointer, length: bytes.length });
      new Uint8Array(wasm.memory.buffer, pointer, bytes.length).set(bytes);
    }
  } catch (error) {
    for (const { pointer, length } of written) if (pointer !== undefined) wasm.dealloc(pointer, length);
    throw error;
  }
  let length;
  try {
    length = functionExport(wasm, name)(...written.flatMap((entry) => "plain" in entry ? [entry.plain] : [entry.pointer, entry.length]));
  } finally {
    for (const { pointer, length } of written) if (pointer !== undefined) wasm.dealloc(pointer, length);
  }
  // Copy the output before reading any other ABI channel. A diagnostics call
  // or a later compile may grow/replace WASM memory; retaining a view here
  // would otherwise make the PDF silently change underneath the caller.
  const out = length > 0
    ? new Uint8Array(wasm.memory.buffer, wasm.output_ptr(), length).slice()
    : new Uint8Array();
  const decoder = new TextDecoder();
  // The second result channel: what the compiler had to say, as JSON, beside
  // the page rather than wrapped around it.
  let diagnostics = [];
  const size = wasm.diagnostics();
  if (size > 0) {
    const raw = new Uint8Array(wasm.memory.buffer, wasm.diagnostics_ptr(), size).slice();
    try {
      diagnostics = JSON.parse(decoder.decode(raw)) || [];
    } catch {
      diagnostics = [];
    }
  }
  const kind = wasm.output_kind();
  return {
    bytes: out,
    text: kind === 1 ? decoder.decode(out) : "",
    kind: kind === 2 ? "pdf" : kind === 1 ? "html" : null,
    ok: wasm.ok() !== 0,
    diagnostics,
  };
}

/** @param {RendererWasm} wasm @param {{main?: string, texts?: Record<string, string>, assets?: Record<string, Uint8Array>, urls?: Record<string, string>}} tree */
export function handOver(wasm, tree) {
  wasm.clear_files();
  for (const [path, body] of Object.entries(tree.texts || {})) {
    call(wasm, "add_file", path, body);
  }
  for (const [path, bytes] of Object.entries(tree.assets || {})) {
    call(wasm, "add_file", path, bytes);
  }
  // Where each figure is, for the renderer that needs a URL rather than
  // bytes. Typst reads a figure out of the map above and writes it into the
  // page itself; markdown produces HTML a browser will fetch from, so its
  // images are pointed at a blob in this browser -- never at the route they
  // came from, which would put a credential in a rendered page.
  for (const [path, url] of Object.entries(tree.urls || {})) {
    call(wasm, "set_asset_url", path, url);
  }
  call(wasm, "set_main", tree.main || "");
}

// What each module holds from the page's tree, as of its last hand-over. A
// package fetched by `needs.js` is not recorded: it is not in the tree, and a
// full hand-over (which clears the map) drops it. See `renderResolving`.
const held = new WeakMap();

function recordOf(texts, assets, urls) {
  return {
    files: new Map([...Object.entries(texts), ...Object.entries(assets)]),
    urls: new Map(Object.entries(urls)),
  };
}

/// As `handOver`, but sends only what differs from what this module already
/// holds. Any path the module holds that the tree no longer has, or a new
/// worker, means a full hand-over. A failed hand-over leaves no record, so the
/// next one is full.
/** @param {RendererWasm} wasm @param {{main?: string, texts?: Record<string, string>, assets?: Record<string, Uint8Array>, urls?: Record<string, string>}} tree */
export function handOverChanged(wasm, tree) {
  const texts = tree.texts || {};
  const assets = tree.assets || {};
  const urls = tree.urls || {};
  const record = held.get(wasm);
  held.delete(wasm);
  const covered = record
    && [...record.files.keys()].every((path) => Object.hasOwn(texts, path) || Object.hasOwn(assets, path))
    && [...record.urls.keys()].every((path) => Object.hasOwn(urls, path));
  if (!covered) {
    handOver(wasm, tree);
    held.set(wasm, recordOf(texts, assets, urls));
    return;
  }
  for (const [path, body] of Object.entries(texts)) {
    if (record.files.get(path) !== body) call(wasm, "add_file", path, body);
  }
  for (const [path, bytes] of Object.entries(assets)) {
    if (record.files.get(path) !== bytes) call(wasm, "add_file", path, bytes);
  }
  for (const [path, url] of Object.entries(urls)) {
    if (record.urls.get(path) !== url) call(wasm, "set_asset_url", path, url);
  }
  call(wasm, "set_main", tree.main || "");
  held.set(wasm, recordOf(texts, assets, urls));
}

/// The worker's copy of the page's tree. Null-prototype maps, so a path such
/// as `__proto__` is an ordinary key.
export function emptyMirror() {
  return { texts: Object.create(null), assets: Object.create(null), urls: Object.create(null) };
}

/// Applies one delta from the page (see `treeDelta` in `renderer-client.js`)
/// to the mirror and returns the new mirror and the full tree it stands for.
/** @param {{texts: Record<string, string>, assets: Record<string, Uint8Array>, urls: Record<string, string>}} mirror @param {{full: boolean, main?: string, settings?: object, texts?: Record<string, string>, assets?: Record<string, Uint8Array>, urls?: Record<string, string>, removed?: {texts?: string[], assets?: string[], urls?: string[]}}} delta */
export function applyTree(mirror, delta) {
  let next = mirror;
  if (delta.full) {
    next = {
      texts: Object.assign(Object.create(null), delta.texts),
      assets: Object.assign(Object.create(null), delta.assets),
      urls: Object.assign(Object.create(null), delta.urls),
    };
  } else {
    Object.assign(next.texts, delta.texts || {});
    Object.assign(next.assets, delta.assets || {});
    Object.assign(next.urls, delta.urls || {});
    for (const path of delta.removed?.texts || []) delete next.texts[path];
    for (const path of delta.removed?.assets || []) delete next.assets[path];
    for (const path of delta.removed?.urls || []) delete next.urls[path];
  }
  return {
    mirror: next,
    tree: { main: delta.main, settings: delta.settings, texts: next.texts, assets: next.assets, urls: next.urls },
  };
}

/// What the last compile went looking for and did not find -- packages and
/// font families -- from a module that says so, or `null` from one that has
/// no such export (markdown) or has compiled nothing yet. See `needs.js`.
/** @param {RendererWasm} wasm @returns {{packages: {url?: string, dir: string}[], fonts: string[]} | null} */
export function needsOf(wasm) {
  if (typeof wasm.needs !== "function" || typeof wasm.needs_ptr !== "function") return null;
  const size = wasm.needs();
  if (!(size > 0)) return null;
  const raw = new Uint8Array(wasm.memory.buffer, wasm.needs_ptr(), size).slice();
  try {
    const parsed = JSON.parse(new TextDecoder().decode(raw)) || {};
    return { packages: parsed.packages || [], fonts: parsed.fonts || [] };
  } catch {
    return null;
  }
}
