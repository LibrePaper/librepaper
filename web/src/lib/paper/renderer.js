// The Markdown renderer for visual mode: the same markdown.wasm the preview
// uses, so a block drawn in the editor reads the way it reads in the preview.
// The module runs in the page, not a worker, because the editor asks for one
// block at a time and waits for each answer.

import { verifiedModuleBytes } from "../renderer-wasm.js";

// Enough for a screenful of blocks and their recent edits, without holding a
// whole long document's worth of HTML.
const CACHE_LIMIT = 2000;

const encoder = new TextEncoder();
const decoder = new TextDecoder();

/** @typedef {{ render: (markdown: string) => string, pageCss: string }} PaperRenderer */

/** @type {PaperRenderer | null} */
let current = null;
/** @type {Promise<PaperRenderer> | null} */
let loading = null;

/**
 * The whole page the module compiles for `markdown`, or "" when it fails.
 * @param {WebAssembly.Exports & Record<string, any>} wasm
 * @param {string} markdown
 * @returns {string}
 */
function compilePage(wasm, markdown) {
  const bytes = encoder.encode(markdown);
  const sourcePointer = wasm.alloc(bytes.length);
  // Written after alloc, because alloc can grow memory and replace the buffer.
  new Uint8Array(wasm.memory.buffer, sourcePointer, bytes.length).set(bytes);
  const titlePointer = wasm.alloc(0);
  const length = wasm.compile(sourcePointer, bytes.length, titlePointer, 0);
  // Inputs are released the way the renderer worker releases them, so a long
  // session does not keep every block it has ever drawn in module memory.
  if (typeof wasm.dealloc === "function") {
    wasm.dealloc(sourcePointer, bytes.length);
    wasm.dealloc(titlePointer, 0);
  }
  if (wasm.ok() === 0 || length === 0) return "";
  // Read the buffer only now: compile may have grown memory.
  return decoder.decode(new Uint8Array(wasm.memory.buffer, wasm.output_ptr(), length));
}

/**
 * The inner HTML of the page's body, with the footnotes section taken out:
 * the editor shows a block alone, and the footnotes come from its definitions
 * in the document, not from a list at the end of this one block.
 * @param {string} page
 * @returns {string}
 */
function bodyOf(page) {
  const match = /<body[^>]*>([\s\S]*)<\/body>/.exec(page);
  if (!match) return "";
  return match[1].replace(/<section class="footnotes"[^>]*>[\s\S]*?<\/section>/g, "").trim();
}

/**
 * Instantiates the Markdown module and returns its renderer.
 * @param {BufferSource} bytes the verified module bytes
 * @returns {Promise<PaperRenderer>}
 */
export async function createPaperRenderer(bytes) {
  const { instance } = await WebAssembly.instantiate(bytes, {});
  const wasm = /** @type {WebAssembly.Exports & Record<string, any>} */ (instance.exports);
  /** @type {Map<string, string>} insertion order is age, so the first key is the oldest */
  const cache = new Map();

  /** @param {string} markdown @returns {string} */
  function render(markdown) {
    const hit = cache.get(markdown);
    if (hit !== undefined) return hit;
    const html = bodyOf(compilePage(wasm, markdown));
    cache.set(markdown, html);
    if (cache.size > CACHE_LIMIT) cache.delete(cache.keys().next().value);
    return html;
  }

  // The stylesheet is the page's own, from an empty document. A `<style>`
  // element carries no document content, so compiling nothing is enough.
  const style = /<style[^>]*>([\s\S]*?)<\/style>/.exec(compilePage(wasm, ""));
  const pageCss = style ? style[1].trim() : "";

  return { render, pageCss };
}

/**
 * The Markdown renderer, loaded once. A failed load is forgotten, so the next
 * call tries again. Rejects when no module URL is configured.
 * @returns {Promise<PaperRenderer>}
 */
export function loadPaperRenderer() {
  if (loading) return loading;
  const url = globalThis.LIBREPAPER_MODULES?.markdown;
  if (!url) return Promise.reject(new Error("no Markdown renderer URL"));
  loading = verifiedModuleBytes(url)
    .then((bytes) => createPaperRenderer(bytes))
    .then((renderer) => {
      current = renderer;
      return renderer;
    }, (error) => {
      loading = null;
      throw error;
    });
  return loading;
}

/**
 * The renderer if it has loaded, else null. Synchronous, so a view can draw
 * at once when the module is already in place.
 * @returns {PaperRenderer | null}
 */
export function currentPaperRenderer() {
  return current;
}
