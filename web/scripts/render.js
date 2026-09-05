// The examples, rendered the way the deployment renders them, so the check
// beside this one measures against the document a reader actually sees rather
// than against an approximation of it.
//
// The renderers are the engine's, compiled to WebAssembly -- the same modules
// the editor loads -- so this needs no toolchain of its own beyond what the
// build already produces.
import { readFileSync } from "node:fs";

function load(name) {
  const path = new URL(`../../src/shell/wasm/${name}.wasm`, import.meta.url);
  try {
    const module = new WebAssembly.Module(readFileSync(path));
    return new WebAssembly.Instance(module, {}).exports;
  } catch {
    return null; // not built; the caller says so rather than failing
  }
}

function call(wasm, name, ...strings) {
  const encoder = new TextEncoder();
  const written = strings.map((text) => {
    const bytes = encoder.encode(text);
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
  const out = new Uint8Array(wasm.memory.buffer, wasm.output_ptr(), length);
  return { text: new TextDecoder().decode(out), ok: wasm.ok() !== 0 };
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

const render = (name) => (source, file) => {
  const wasm = load(name);
  if (!wasm) return null;
  const { text, ok } = call(wasm, "compile", source, file);
  if (!ok) throw new Error(`${file}: ${text}`);
  return visibleText(text);
};

export const renderMarkdown = render("markdown");
export const renderTypst = render("typst");

/// HTML's renderer is the identity, so what a reader sees is the source's own
/// visible text and no module is needed to work it out.
export const renderHtml = (source) => visibleText(source);
