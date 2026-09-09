// The engine ABI runs exclusively in the renderer worker.
const loads = {};

export function load(url) {
  if (!url) return Promise.reject(new Error("no renderer URL"));
  if (loads[url]) return loads[url];
  loads[url] = WebAssembly.instantiateStreaming(fetch(url), {})
    .then(({ instance }) => instance.exports)
    .catch(async (error) => {
      // Some servers do not send application/wasm, which streaming requires.
      // Falling back costs a copy of the module in memory, so it is a fallback
      // rather than the path.
      const response = await fetch(url);
      if (!response.ok) throw error;
      const { instance } = await WebAssembly.instantiate(await response.arrayBuffer(), {});
      return instance.exports;
    })
    .then((wasm) => {
      // The compiler has no clock of its own, so typst's datetime.today() is
      // whatever this tab says it is.
      const now = new Date();
      if (wasm.set_today) wasm.set_today(now.getFullYear(), now.getMonth() + 1, now.getDate());
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
  const written = strings.map((value) => {
    const bytes = typeof value === "string" ? encoder.encode(value) : value;
    const pointer = wasm.alloc(bytes.length);
    new Uint8Array(wasm.memory.buffer, pointer, bytes.length).set(bytes);
    return { pointer, length: bytes.length };
  });
  let length;
  try {
    length = wasm[name](...written.flatMap(({ pointer, length }) => [pointer, length]));
  } finally {
    for (const { pointer, length } of written) wasm.dealloc(pointer, length);
  }
  // Copy the output before reading any other ABI channel. A diagnostics call
  // or a later compile may grow/replace WASM memory; retaining a view here
  // would otherwise make the PDF silently change underneath the caller.
  const out = wasm.output_ptr && length > 0
    ? new Uint8Array(wasm.memory.buffer, wasm.output_ptr(), length).slice()
    : new Uint8Array();
  const decoder = new TextDecoder();
  // The second result channel: what the compiler had to say, as JSON, beside
  // the page rather than wrapped around it. A module built before it existed
  // says nothing, which reads as an empty list.
  let diagnostics = [];
  if (wasm.diagnostics && wasm.diagnostics_ptr) {
    const size = wasm.diagnostics();
    if (size > 0) {
      const raw = new Uint8Array(wasm.memory.buffer, wasm.diagnostics_ptr(), size).slice();
      try {
        diagnostics = JSON.parse(decoder.decode(raw)) || [];
      } catch {
        diagnostics = [];
      }
    }
  }
  // Old modules put a textual failure message in the output buffer and have
  // no output_kind export. Treat their buffer as HTML for the worker's legacy
  // diagnostic fallback; current modules explicitly return kind 0 on error.
  const kind = wasm.output_kind ? wasm.output_kind() : 1;
  return {
    bytes: out,
    text: kind === 1 ? decoder.decode(out) : "",
    kind: kind === 2 ? "pdf" : kind === 1 ? "html" : null,
    ok: wasm.ok() !== 0,
    diagnostics,
  };
}

export function handOver(wasm, tree) {
  if (!wasm.add_file || !wasm.clear_files) return;
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
  if (wasm.set_asset_url) {
    for (const [path, url] of Object.entries(tree.urls || {})) {
      call(wasm, "set_asset_url", path, url);
    }
  }
  if (wasm.set_main) call(wasm, "set_main", tree.main || "");
}
