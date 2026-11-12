// The Typst renderer for visual mode: the same typst.wasm the preview uses,
// asked for HTML (compile_html) rather than a PDF. The module runs in the
// page, not a worker, because the editor asks for one block at a time and
// waits for each answer. The compiler keeps its caches between calls, so a
// block costs a millisecond or so once the first compile has warmed it.
//
// Known gaps of Typst's HTML export: the number of a display equation and
// `align` content are dropped, so a block shows neither.

import { call } from "../renderer-wasm.js";

// Enough for a screenful of blocks and their recent edits, without holding a
// whole long document's worth of HTML.
const CACHE_LIMIT = 2000;

/** @typedef {{ render: (source: string) => string, pageCss: string, mathCss: string }} TypstPaperRenderer */

/**
 * The text of the first <style> of a page, or "".
 * @param {string} page
 */
function styleOf(page) {
  const match = /<style[^>]*>([\s\S]*?)<\/style>/.exec(page);
  return match ? match[1].trim() : "";
}

/**
 * The inner HTML of the page's body, or "".
 * @param {string} page
 */
function bodyOf(page) {
  const match = /<body[^>]*>([\s\S]*)<\/body>/.exec(page);
  return match ? match[1].trim() : "";
}

/**
 * Instantiates the Typst module and returns its paper renderer. `pageCss` and
 * `mathCss` are both the page's own stylesheet (the MathML rules): the
 * Markdown page stylesheet does not apply to Typst.
 * @param {BufferSource} bytes the verified module bytes
 * @returns {Promise<TypstPaperRenderer>}
 */
export async function createTypstPaperRenderer(bytes) {
  const { instance } = await WebAssembly.instantiate(bytes, {});
  const wasm = /** @type {any} */ (instance.exports);
  // The compiler has no clock of its own, so typst's datetime.today() is whatever this tab says it is.
  const now = new Date();
  wasm.set_today(now.getFullYear(), now.getMonth() + 1, now.getDate());
  /** @type {Map<string, string>} insertion order is age, so the first key is the oldest */
  const cache = new Map();

  /** The whole page the module compiles for `source`, or "" when it fails. @param {string} source */
  function compilePage(source) {
    wasm.clear_files();
    call(wasm, "add_file", "main.typ", source);
    call(wasm, "set_main", "main.typ");
    const result = call(wasm, "compile_html", source, "");
    return result.ok ? result.text : "";
  }

  /** @param {string} source */
  function render(source) {
    const hit = cache.get(source);
    if (hit !== undefined) return hit;
    const html = bodyOf(compilePage(source));
    cache.set(source, html);
    if (cache.size > CACHE_LIMIT) cache.delete(cache.keys().next().value);
    return html;
  }

  const mathCss = styleOf(compilePage("$x$"));
  return { render, pageCss: mathCss, mathCss };
}
