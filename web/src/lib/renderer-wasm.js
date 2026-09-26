// The engine ABI runs exclusively in the renderer worker.
const loads = {};
const REQUIRED_EXPORTS = [
  "memory", "alloc", "dealloc", "compile", "output_ptr", "ok", "output_kind",
  "diagnostics", "diagnostics_ptr", "failure_page", "title_of", "add_file",
  "clear_files", "set_main", "set_asset_url", "set_today", "word_diff",
];

// Keep the ABI check at the module boundary. This makes a stale or wrong
// artifact fail while loading, with the complete list of what it lacks,
// instead of failing later in a worker operation with an opaque TypeError.
export function validateExports(wasm, url = "renderer") {
  const missing = REQUIRED_EXPORTS.filter((name) => {
    if (name === "memory") return !(wasm[name] && wasm[name].buffer instanceof ArrayBuffer);
    return typeof wasm[name] !== "function";
  });
  if (missing.length) {
    throw new Error(`incompatible renderer module ${url}: missing or invalid exports: ${missing.join(", ")}`);
  }
  return wasm;
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
      validateExports(wasm, url);
      // The compiler has no clock of its own, so typst's datetime.today() is
      // whatever this tab says it is.
      const now = new Date();
      wasm.set_today(now.getFullYear(), now.getMonth() + 1, now.getDate());
      return wasm;
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
    length = wasm[name](...written.flatMap(({ pointer, length }) => [pointer, length]));
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
