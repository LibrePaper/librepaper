// The engine ABI runs exclusively in the renderer worker.
const loads = {};
const REQUIRED_EXPORTS = [
  "memory", "alloc", "dealloc", "compile", "output_ptr", "ok", "output_kind",
  "diagnostics", "diagnostics_ptr", "failure_page", "title_of", "add_file",
  "clear_files", "set_main", "set_asset_url", "set_today", "word_diff",
];

/** @typedef {(...args: number[]) => number} RendererFunction */
/** @typedef {WebAssembly.Exports & {memory: WebAssembly.Memory, alloc: RendererFunction, dealloc: RendererFunction, compile: RendererFunction, compile_html?: RendererFunction, output_ptr: RendererFunction, ok: RendererFunction, output_kind: RendererFunction, diagnostics: RendererFunction, diagnostics_ptr: RendererFunction, failure_page: RendererFunction, title_of: RendererFunction, add_file: RendererFunction, clear_files: RendererFunction, set_main: RendererFunction, set_asset_url: RendererFunction, set_today: RendererFunction, word_diff: RendererFunction, bibliography?: RendererFunction, needs?: RendererFunction, needs_ptr?: RendererFunction}} RendererWasm */

/** @param {RendererWasm} wasm @param {string} name @returns {RendererFunction} */
function functionExport(wasm, name) {
  const value = wasm[name];
  if (typeof value !== "function") throw new Error(`renderer export ${name} is unavailable`);
  return value;
}

// Keep the ABI check at the module boundary. This makes a stale or wrong
// artifact fail while loading, with the complete list of what it lacks,
// instead of failing later in a worker operation with an opaque TypeError.
/** @param {WebAssembly.Exports} wasm @param {string} [url] @returns {RendererWasm} */
export function validateExports(wasm, url = "renderer") {
  const missing = REQUIRED_EXPORTS.filter((name) => {
    if (name === "memory") return !(wasm[name] && wasm[name].buffer instanceof ArrayBuffer);
    return typeof wasm[name] !== "function";
  });
  if (missing.length) {
    throw new Error(`incompatible renderer module ${url}: missing or invalid exports: ${missing.join(", ")}`);
  }
  if (typeof wasm.default_fonts_required === "function" && wasm.default_fonts_required() !== 0) {
    throw new Error(`incompatible Typst module ${url}: external fonts are unsupported; use an embedded-font build`);
  }
  return /** @type {RendererWasm} */ (wasm);
}

async function instantiate(url) {
  try {
    return (await WebAssembly.instantiateStreaming(fetch(url), {})).instance;
  } catch (streamingError) {
    // Some servers do not send application/wasm, which streaming requires.
    // Falling back costs a copy of the module in memory, so it is a fallback
    // rather than the path.
    const response = await fetch(url);
    if (!response.ok) throw streamingError;
    return (await WebAssembly.instantiate(await response.arrayBuffer(), {})).instance;
  }
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
/** @param {RendererWasm} wasm @param {string} name @param {...(string|Uint8Array)} strings */
export function call(wasm, name, ...strings) {
  const encoder = new TextEncoder();
  const written = [];
  try {
    for (const value of strings) {
      const bytes = typeof value === "string" ? encoder.encode(value) : value;
      const pointer = wasm.alloc(bytes.length);
      written.push({ pointer, length: bytes.length });
      new Uint8Array(wasm.memory.buffer, pointer, bytes.length).set(bytes);
    }
  } catch (error) {
    for (const { pointer, length } of written) wasm.dealloc(pointer, length);
    throw error;
  }
  let length;
  try {
    length = functionExport(wasm, name)(...written.flatMap(({ pointer, length }) => [pointer, length]));
  } finally {
    for (const { pointer, length } of written) wasm.dealloc(pointer, length);
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
