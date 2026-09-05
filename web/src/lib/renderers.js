// The renderers, loaded into the editor.
//
// Both modules are the engine crate compiled to WebAssembly -- the same crate
// the command line renders with -- so the preview is the document a save would
// store, byte for byte, and an edit made here renders exactly as one made from
// the terminal. The deployment renders nothing: it stores what this browser
// produced.
//
// They are plain WebAssembly with a handful of exports rather than
// wasm-bindgen, so this is the whole of the glue: reserve memory in the
// module, write the source into it as UTF-8, call compile, read the page back
// out. The URLs carry a digest of each module's bytes and are handed to the
// page by the server, so a module cached for a year cannot outlive the loader
// that speaks to it.

const loads = {};

function urls() {
  return globalThis.KOMODOC_MODULES || {};
}

/// Whether this deployment serves a renderer for this format at all. The typst
/// module is thirty megabytes and optional, so a build may not have one.
///
/// HTML is the exception, and always true: its renderer is the identity, so it
/// is the one format every deployment can edit whatever it was built with.
export function available(format) {
  return format === "html" || Boolean(urls()[format]);
}

function load(format) {
  const url = urls()[format];
  if (!url) return Promise.reject(new Error(`no renderer for ${format}`));
  if (loads[format]) return loads[format];
  loads[format] = WebAssembly.instantiateStreaming(fetch(url), {})
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
    });
  return loads[format];
}

// Each argument is written into the module's memory and passed as a (pointer,
// length) pair. A string is written as UTF-8; bytes are written as they are,
// which is how a figure reaches the compiler. The module's memory can be
// replaced when it grows, so a view of it is taken after every call that might
// have grown it, never held across one.
function call(wasm, name, ...strings) {
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
  const decoder = new TextDecoder();
  const out = new Uint8Array(wasm.memory.buffer, wasm.output_ptr(), length);
  const text = decoder.decode(out);
  // The second result channel: what the compiler had to say, as JSON, beside
  // the page rather than wrapped around it. A module built before it existed
  // says nothing, which reads as an empty list.
  let diagnostics = [];
  if (wasm.diagnostics && wasm.diagnostics_ptr) {
    const size = wasm.diagnostics();
    if (size > 0) {
      const raw = new Uint8Array(wasm.memory.buffer, wasm.diagnostics_ptr(), size);
      try {
        diagnostics = JSON.parse(decoder.decode(raw)) || [];
      } catch {
        diagnostics = [];
      }
    }
  }
  return { text, ok: wasm.ok() !== 0, diagnostics };
}

/// What a document is written in, which follows from what its main file is
/// called. A document is a directory, so the format is a property of one file
/// in it rather than of the whole.
export function formatOf(path) {
  const lower = (path || "").toLowerCase();
  if (lower.endsWith(".typ")) return "typst";
  if (lower.endsWith(".md") || lower.endsWith(".markdown")) return "markdown";
  if (lower.endsWith(".html") || lower.endsWith(".htm")) return "html";
  return "";
}

/// Puts the document's directory where the compiler can read it, and nothing
/// else: `clear_files` first, because the files of the last document are not
/// the files of this one, and the main file's name after, because what it
/// imports resolves relative to it.
///
/// This is the interface the ABI has had since diagnostics were added. Only
/// the caller is new: the map stayed empty for as long as a document was one
/// text.
function handOver(wasm, tree) {
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

/// Renders a document into the page a save would store, and says what the
/// compiler had to say about it.
///
/// `tree` is the whole directory -- `{ main, texts: {path: string}, assets:
/// {path: Uint8Array} }` -- because a paper is a main file, its chapters, its
/// bibliography and its figures, and a compiler given only one of them
/// produces the error a reader would otherwise be shown.
///
/// A document that does not compile is an ordinary state of an editor rather
/// than an error of this loader's, so it resolves with no page and a list of
/// diagnostics; it throws only for what it threw for before, a module that
/// could not be fetched.
export async function render(tree, title) {
  const source = tree.texts?.[tree.main] ?? "";
  const format = formatOf(tree.main);
  // HTML's renderer is the identity, so there is nothing to fetch and nothing
  // that can fail: the source is the page. A directory whose main file is
  // HTML is allowed, and nothing in it is rewritten -- an HTML document is
  // self-contained, as it always was.
  if (format === "html") return { html: source, diagnostics: [] };
  const wasm = await load(format);
  handOver(wasm, tree);
  const { text, ok, diagnostics } = call(wasm, "compile", source, title);
  if (ok) return { html: text, diagnostics };
  // A module built before the second result channel says nothing about why it
  // failed, and puts its message where the page would be. Rather than show
  // nothing at all, that message becomes a diagnostic with no place in the
  // source, which is what such a module can honestly say.
  const said = diagnostics.length
    ? diagnostics
    : [{ severity: "error", message: text || "this document could not be compiled", hints: [], file: "", line: 0, column: 0, end_line: 0, end_column: 0 }];
  return { html: null, diagnostics: said };
}

/// The page to show where a document would be when there is nothing else to
/// show it: what the last compile said, dressed as a document rather than as a
/// crash. The engine builds it, so the command line and a reader can show the
/// same one. Only meaningful straight after a render that produced no page.
export async function failurePage(title, format) {
  const wasm = await load(format);
  if (!wasm.failure_page) return null; // an older module; the badge says it
  return call(wasm, "failure_page", title).text;
}

/// The document's first heading, which names a document that was never given a
/// title of its own. For HTML that is its own `<title>`, or its first `<h1>`.
///
/// The main file's heading, and only its: a chapter's first heading names the
/// chapter, and a document is titled by the file that is the document.
export async function titleOf(tree) {
  const source = tree.texts?.[tree.main] ?? "";
  const format = formatOf(tree.main);
  if (format === "html") return htmlTitleOf(source);
  if (!format) return "";
  const wasm = await load(format);
  return call(wasm, "title_of", source).text;
}

/// The identity renderer's title scan, which matches the engine's: the
/// `<title>`, or failing that the first `<h1>`, so the landing page, the
/// command line and this browser all name an uploaded file the same way.
export function htmlTitleOf(source) {
  const inside = (open, close) => {
    const lower = source.toLowerCase();
    const start = lower.indexOf(open);
    if (start < 0) return "";
    const after = source.indexOf(">", start);
    if (after < 0) return "";
    const end = lower.indexOf(close, after);
    if (end < 0) return "";
    const parsed = new DOMParser().parseFromString(source.slice(after + 1, end), "text/html");
    return (parsed.body.textContent || "").split(/\s+/).filter(Boolean).join(" ");
  };
  return inside("<title", "</title>") || inside("<h1", "</h1>");
}

/// Starts the download before anyone asks to render, so the wait for the
/// renderer overlaps with reading the document rather than following it.
export function warm(format) {
  if (format === "html") return; // the identity has nothing to fetch
  load(format).catch(() => {
    /* reported when something is actually rendered */
  });
}
