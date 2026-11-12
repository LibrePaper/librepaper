// The Markdown renderer for visual mode: the same markdown.wasm the preview
// uses, so a block drawn in the editor reads the way it reads in the preview.
// The module runs in the page, not a worker, because the editor asks for one
// block at a time and waits for each answer.

import { verifiedModuleBytes } from "../renderer-wasm.js";
import { createTypstPaperRenderer } from "./typst-renderer.js";

// Enough for a screenful of blocks and their recent edits, without holding a
// whole long document's worth of HTML.
const CACHE_LIMIT = 2000;

const encoder = new TextEncoder();
const decoder = new TextDecoder();

/**
 * A top-level block of a whole document, as the module renders it: its
 * 1-based inclusive lines and its HTML.
 * @typedef {{ fromLine: number, toLine: number, html: string }} RenderedBlock
 * @typedef {{
 *   render: (markdown: string) => string,
 *   renderBlocks?: (source: string, main: string) => RenderedBlock[] | null,
 *   mathCss?: string,
 *   setProject?: (project: any) => void,
 *   prepare?: (project: any, preamble: string, onUpdate: () => void) => Promise<boolean>,
 *   pageCss: string,
 * }} PaperRenderer
 */

/** @type {Map<string, PaperRenderer>} the loaded renderer of each module key */
const current = new Map();
/** @type {Map<string, Promise<PaperRenderer>>} the load in flight, or done, of each module key */
const loading = new Map();

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
 * The top-level blocks the module renders for a whole document, or null when
 * the module has no `compile_blocks` export or the compile fails.
 * @param {WebAssembly.Exports & Record<string, any>} wasm
 * @param {string} source
 * @param {string} main the main file's name; a `.qmd` name selects Quarto
 * @returns {RenderedBlock[] | null}
 */
function compileBlocks(wasm, source, main) {
  if (typeof wasm.compile_blocks !== "function") return null;
  const mainBytes = encoder.encode(main);
  const mainPointer = wasm.alloc(mainBytes.length);
  new Uint8Array(wasm.memory.buffer, mainPointer, mainBytes.length).set(mainBytes);
  wasm.set_main(mainPointer, mainBytes.length);
  if (typeof wasm.dealloc === "function") wasm.dealloc(mainPointer, mainBytes.length);

  const bytes = encoder.encode(source);
  const sourcePointer = wasm.alloc(bytes.length);
  new Uint8Array(wasm.memory.buffer, sourcePointer, bytes.length).set(bytes);
  const length = wasm.compile_blocks(sourcePointer, bytes.length);
  if (typeof wasm.dealloc === "function") wasm.dealloc(sourcePointer, bytes.length);
  if (wasm.ok() === 0 || length === 0) return null;
  // Read the buffer only now: compile_blocks may have grown memory.
  const json = decoder.decode(new Uint8Array(wasm.memory.buffer, wasm.output_ptr(), length));
  try {
    const parsed = JSON.parse(json);
    return parsed.blocks.map((/** @type {any} */ block) => ({
      fromLine: block.from_line,
      toLine: block.to_line,
      html: block.html,
    }));
  } catch {
    return null;
  }
}

/**
 * The inner HTML of the page's body, with the footnotes section taken out:
 * the editor shows a block alone, and the footnotes come from its definitions
 * in the document, not from a list at the end of this one block. Org wraps
 * its content in sections, so the empty ones a block leaves behind go too.
 * @param {string} page
 * @returns {string}
 */
function bodyOf(page) {
  const match = /<body[^>]*>([\s\S]*)<\/body>/.exec(page);
  if (!match) return "";
  let body = match[1].replace(/<section class="footnotes"[^>]*>[\s\S]*?<\/section>/g, "");
  // Nested empty sections empty their parents, so the pass repeats until none is left.
  let next = body.replace(/<section[^>]*>\s*<\/section>/g, "");
  while (next !== body) {
    body = next;
    next = body.replace(/<section[^>]*>\s*<\/section>/g, "");
  }
  return body.trim();
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

  // The editor asks for a document's blocks once per change, so one entry is enough.
  /** @type {{ key: string, blocks: RenderedBlock[] | null } | null} */
  let lastBlocks = null;

  /**
   * The document's top-level blocks as the module renders them whole, or null
   * when the module cannot (no `compile_blocks` export). `main` is the file's
   * name; a `.qmd` name selects Quarto.
   * @param {string} source
   * @param {string} main
   * @returns {RenderedBlock[] | null}
   */
  function renderBlocks(source, main) {
    const key = main + "\0" + source;
    if (lastBlocks && lastBlocks.key === key) return lastBlocks.blocks;
    const blocks = compileBlocks(wasm, source, main);
    lastBlocks = { key, blocks };
    return blocks;
  }

  // The stylesheet is the page's own, from an empty document. A `<style>`
  // element carries no document content, so compiling nothing is enough.
  const style = /<style[^>]*>([\s\S]*?)<\/style>/.exec(compilePage(wasm, ""));
  const pageCss = style ? style[1].trim() : "";

  return { render, renderBlocks, pageCss };
}

/**
 * The renderer of one module, loaded once per key. A failed load is forgotten
 * for that key, so the next call tries again. Rejects when no module URL is
 * configured for the key.
 * @param {string} [key] the module's name in LIBREPAPER_MODULES: "markdown", "org" or "typst"
 * @returns {Promise<PaperRenderer>}
 */
export function loadPaperRenderer(key = "markdown") {
  const pending = loading.get(key);
  if (pending) return pending;
  const url = globalThis.LIBREPAPER_MODULES?.[key];
  if (!url) return Promise.reject(new Error(`no ${key} renderer URL`));
  const attempt = verifiedModuleBytes(url)
    .then((bytes) => /** @type {Promise<PaperRenderer>} */ (key === "typst" ? createTypstPaperRenderer(bytes) : createPaperRenderer(bytes)))
    .then((renderer) => {
      current.set(key, renderer);
      return renderer;
    }, (error) => {
      loading.delete(key);
      throw error;
    });
  loading.set(key, attempt);
  return attempt;
}

/**
 * The renderer of one module if it has loaded, else null. Synchronous, so a
 * view can draw at once when the module is already in place.
 * @param {string} [key] "markdown" or "org"
 * @returns {PaperRenderer | null}
 */
export function currentPaperRenderer(key = "markdown") {
  return current.get(key) ?? null;
}
