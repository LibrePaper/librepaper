// The examples, rendered the way the deployment renders them, so the check
// beside this one measures against the document a reader actually sees rather
// than against an approximation of it.
//
// The renderers are the engine's, compiled to WebAssembly -- the same modules
// the editor loads -- so this needs no toolchain of its own beyond what the
// build already produces.
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { tmpdir } from "node:os";

function load(name) {
  const path = new URL(`../dist/wasm/${name}.wasm`, import.meta.url);
  try {
    const module = new WebAssembly.Module(readFileSync(path));
    return new WebAssembly.Instance(module, {}).exports;
  } catch {
    return null; // not built; the caller says so rather than failing
  }
}

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
  const out = new Uint8Array(wasm.memory.buffer, wasm.output_ptr(), length).slice();
  const kind = wasm.output_kind ? wasm.output_kind() : 1;
  return { bytes: out, text: new TextDecoder().decode(out), kind, ok: wasm.ok() !== 0 };
}

// Typst's browser output is a PDF, so the sync check uses the same extracted
// text a PDF reader anchors against. Poppler is already a required dependency
// of `check:typst-pdf`; keeping this independent parser here avoids decoding a
// binary PDF as HTML and lets this check remain a Node-only source navigation
// check.
function pdfText(bytes, label) {
  const directory = mkdtempSync(join(tmpdir(), "librepaper-sync-pdf-"));
  const file = join(directory, "render.pdf");
  try {
    writeFileSync(file, bytes);
    return execFileSync("pdftotext", ["-layout", file, "-"], {
      encoding: "utf8",
      env: { ...process.env, LC_ALL: "C" },
    }).replace(/\s+/g, " ").trim();
  } catch (error) {
    throw new Error(`${label}: PDF text extraction needs pdftotext (${error.message})`);
  } finally {
    rmSync(directory, { recursive: true, force: true });
  }
}

/// What the agent publishes from a rendered document: its visible text, with
/// whitespace collapsed the way a browser collapses it.
export function visibleText(html) {
  const body = (html.match(/<body[^>]*>([\s\S]*)<\/body>/) || [null, html])[1];
  return body
    .replace(/<(script|style)\b[^>]*>[\s\S]*?<\/\1>/gi, " ")
    .replace(/<[^>]*>/g, "")
    .replace(/&amp;/g, "&")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&quot;/g, '"')
    .replace(/&#39;/g, "'")
    .replace(/&nbsp;/g, " ")
    .replace(/\s+/g, " ")
    .trim();
}

// A document is a directory, so what is rendered is a tree: `{ main, texts,
// assets }`, the same object `renderers.js` takes in the browser. The files go
// into the module before the compile and the main file is named, which is what
// lets an example import a chapter or cite a .bib here exactly as it would
// there.
//
// A bare string is still accepted and read as a document of one file, because
// most examples are one file and writing `{ texts: { "x.typ": source } }` at
// every call site would say nothing the string does not.
function treeOf(source, file) {
  if (typeof source === "string") return { main: file, texts: { [file]: source }, assets: {} };
  return { main: source.main || file, texts: source.texts || {}, assets: source.assets || {} };
}

const render = (name) => (source, file) => {
  const wasm = load(name);
  if (!wasm) return null;
  const tree = treeOf(source, file);
  if (wasm.clear_files) {
    wasm.clear_files();
    for (const [path, body] of Object.entries(tree.texts)) call(wasm, "add_file", path, body);
    for (const [path, bytes] of Object.entries(tree.assets)) call(wasm, "add_file", path, bytes);
    if (wasm.set_main) call(wasm, "set_main", tree.main || "");
  }
  const result = call(wasm, "compile", tree.texts[tree.main] ?? "", tree.main);
  if (!result.ok) throw new Error(`${tree.main}: ${result.text}`);
  return result.kind === 2 ? pdfText(result.bytes, tree.main) : visibleText(result.text);
};

export const renderMarkdown = render("markdown");
export const renderTypst = render("typst");

/// HTML's renderer is the identity, so what a reader sees is the source's own
/// visible text and no module is needed to work it out.
export const renderHtml = (source) => visibleText(source);

/// What the compiler said about a tree, rather than what it produced. The
/// second result channel, read the way `renderers.js` reads it, so a check can
/// assert which file an error is in.
export function diagnose(name, source, file) {
  const wasm = load(name);
  if (!wasm) return null;
  const tree = treeOf(source, file);
  if (wasm.clear_files) {
    wasm.clear_files();
    for (const [path, body] of Object.entries(tree.texts)) call(wasm, "add_file", path, body);
    for (const [path, bytes] of Object.entries(tree.assets)) call(wasm, "add_file", path, bytes);
    if (wasm.set_main) call(wasm, "set_main", tree.main || "");
  }
  const { ok } = call(wasm, "compile", tree.texts[tree.main] ?? "", tree.main);
  let said = [];
  if (wasm.diagnostics && wasm.diagnostics_ptr) {
    const size = wasm.diagnostics();
    if (size > 0) {
      const raw = new Uint8Array(wasm.memory.buffer, wasm.diagnostics_ptr(), size);
      try {
        said = JSON.parse(new TextDecoder().decode(raw)) || [];
      } catch {
        said = [];
      }
    }
  }
  return { ok, diagnostics: said };
}
