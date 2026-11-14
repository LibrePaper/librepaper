// The Typst renderer for visual mode: the same typst.wasm the preview uses,
// asked for HTML (compile_html) rather than a PDF. The module runs in the
// page, not a worker, because the editor asks for one block at a time and
// waits for each answer. The compiler keeps its caches between calls, so a
// block costs a millisecond or so once the first compile has warmed it.
//
// Known gaps of Typst's HTML export: the number of a display equation is still
// dropped, so a block shows none. `align` and `place` content would be dropped
// too, so two rules in front of each source keep it (see KEEP_RULES). Text size
// and weight are not exported.

import { call, needsOf } from "../renderer-wasm.js";
import { fetchNeeds as fetchPackages, ROUNDS } from "../needs.js";

// Enough for a screenful of blocks and their recent edits, without holding a
// whole long document's worth of HTML.
const CACHE_LIMIT = 2000;
// An asset larger than this is not fetched for the paper view, nor are more
// than TOTAL_LIMIT bytes of them together.
const ASSET_LIMIT = 8 * 1024 * 1024;
const TOTAL_LIMIT = 64 * 1024 * 1024;

// Typst's HTML export drops the content of `align(...)` and `place(...)`. The
// first rule keeps aligned content in a div with a `text-align` style, using
// the alignment's horizontal side (`start` when it has none). The second keeps
// placed content in the flow, where it reads as the rest of the block does.
const KEEP_RULES =
  '#show align: it => { let a = repr(it.alignment); let side = if a.contains("center") { "center" } else if a.contains("right") { "right" } else if a.contains("end") { "end" } else if a.contains("left") { "left" } else { "start" }; html.elem("div", attrs: (style: "text-align: " + side), it.body) }\n' +
  "#show place: it => it.body\n";

// A body that is one empty marked element, and the HTML it compiles to. A
// prefix's frame is what it writes around this mark.
const FRAME_MARK_SOURCE = '\n\n#html.elem("div", attrs: (id: "lp-frame-mark"))\n';
const FRAME_MARK_HTML = '<div id="lp-frame-mark"></div>';

/**
 * What the paper view knows of the project.
 * `texts` maps project paths to text; `assets` maps asset paths to a version
 * (their digest) that changes when the bytes do; `assetUrl(path)` resolves an
 * asset to a URL the page can fetch, or null.
 * @typedef {{ main: string, texts: Record<string, string>, assets?: Record<string, string>, assetUrl?: (path: string) => Promise<string | null> | string | null }} PaperProject
 * @typedef {{
 *   render: (source: string) => string,
 *   frame: (prefix: string) => { before: string, after: string } | null,
 *   unframe: (html: string, prefix: string) => string,
 *   mathCss: string,
 *   setProject: (project: PaperProject) => void,
 *   prepare: (project: PaperProject, preamble: string, onUpdate?: () => void) => Promise<boolean>,
 * }} TypstPaperRenderer
 */

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
 * Instantiates the Typst module and returns its paper renderer. `mathCss` is the
 * page's own stylesheet (the MathML rules): the house stylesheet does not
 * cover Typst's MathML.
 * @param {BufferSource} bytes the verified module bytes
 * @returns {Promise<TypstPaperRenderer>}
 */
export async function createTypstPaperRenderer(bytes, { fetchNeeds = fetchPackages, fetchBytes = defaultFetchBytes } = {}) {
  const { instance } = await WebAssembly.instantiate(bytes, {});
  const wasm = /** @type {any} */ (instance.exports);
  // The compiler has no clock of its own, so typst's datetime.today() is whatever this tab says it is.
  const now = new Date();
  wasm.set_today(now.getFullYear(), now.getMonth() + 1, now.getDate());
  /** @type {Map<string, string>} insertion order is age, so the first key is the oldest */
  const cache = new Map();

  // The project the blocks compile in. `revision` changes whenever a file the
  // compiler can see does, and the render cache is dropped with it.
  let main = "main.typ";
  /** @type {Record<string, string>} the project's texts but the main file's */
  let texts = {};
  /** @type {Record<string, string>} */
  let assetVersions = {};
  /** @type {Map<string, { version: string, bytes: Uint8Array }>} */
  const assetBytes = new Map();
  /** @type {Map<string, Uint8Array>} packages, by path, for the life of the renderer */
  const fetched = new Map();
  let revision = 0;
  let loaded = -1;
  let textsRev = 0;

  function bump() {
    revision++;
    cache.clear();
  }

  /** @param {PaperProject} project */
  function setProject(project) {
    const nextMain = project.main || "main.typ";
    /** @type {Record<string, string>} */
    const nextTexts = {};
    for (const [path, text] of Object.entries(project.texts || {})) if (path !== nextMain) nextTexts[path] = text;
    let changed = nextMain !== main;
    const previous = Object.keys(texts);
    const names = Object.keys(nextTexts);
    if (!changed && previous.length !== names.length) changed = true;
    if (!changed) for (const path of names) if (texts[path] !== nextTexts[path]) { changed = true; break; }
    const textsChanged = changed;
    main = nextMain;
    texts = nextTexts;
    assetVersions = { ...(project.assets || {}) };
    for (const [path, held] of assetBytes) {
      if (assetVersions[path] !== held.version) {
        assetBytes.delete(path);
        changed = true;
      }
    }
    if (changed) bump();
    if (textsChanged) textsRev++;
  }

  // Puts the project into the module when it has changed since the last compile.
  function loadFiles() {
    if (loaded === revision) return;
    wasm.clear_files();
    for (const [path, text] of Object.entries(texts)) call(wasm, "add_file", path, text);
    for (const [path, held] of assetBytes) call(wasm, "add_file", path, held.bytes);
    for (const [path, body] of fetched) {
      if (path in texts || assetBytes.has(path)) continue;
      call(wasm, "add_file", path, body);
    }
    loaded = revision;
  }

  /** The whole page the module compiles for `source`, or "" when it fails. @param {string} source */
  function compilePage(source) {
    loadFiles();
    call(wasm, "add_file", main, source);
    call(wasm, "set_main", main);
    const result = call(wasm, "compile_html", source, "");
    return result.ok ? result.text : "";
  }

  /** @param {string} source */
  function render(source) {
    const hit = cache.get(source);
    if (hit !== undefined) return hit;
    let html = bodyOf(compilePage(KEEP_RULES + source));
    // The rules can make a source fail that compiles without them. A source
    // with no output gives "" either way, so compiling it alone costs little.
    if (html === "") html = bodyOf(compilePage(source));
    cache.set(source, html);
    if (cache.size > CACHE_LIMIT) cache.delete(cache.keys().next().value);
    return html;
  }

  /**
   * What `prefix` writes around a document's body: the HTML before it and the
   * HTML after it. A template (`#show: article.with(...)`) writes a title
   * before the body and may write more after it. Null when the prefix does
   * not compile, or the body does not come out.
   * @param {string} prefix the Typst in front of the body
   * @returns {{ before: string, after: string } | null}
   */
  function frame(prefix) {
    const html = render(prefix + FRAME_MARK_SOURCE);
    const at = html.indexOf(FRAME_MARK_HTML);
    if (at === -1) return null;
    return { before: html.slice(0, at), after: html.slice(at + FRAME_MARK_HTML.length) };
  }

  /**
   * `html`, the HTML of a block compiled after `prefix`, without the frame
   * the prefix writes around every body. A block the frame does not fit is
   * returned as it is.
   * @param {string} html
   * @param {string} prefix
   * @returns {string}
   */
  function unframe(html, prefix) {
    const shape = frame(prefix);
    if (!shape || (shape.before === "" && shape.after === "")) return html;
    const { before, after } = shape;
    if (html.length >= before.length + after.length && html.startsWith(before) && html.endsWith(after)) {
      return html.slice(before.length, html.length - after.length);
    }
    return html;
  }

  // What the last prepare was for, so a keystroke that changes no preamble and
  // no file asks for nothing again.
  let preparedKey = "";
  /** @type {Promise<boolean> | null} */
  let preparing = null;
  let again = false;

  /** The assets the project's texts mention that are not held yet. @param {string} preamble */
  function wantedAssets(preamble) {
    const haystack = preamble + "\n" + Object.values(texts).join("\n");
    let total = 0;
    for (const held of assetBytes.values()) total += held.bytes.length;
    return Object.keys(assetVersions).filter((path) => {
      if (assetBytes.has(path)) return false;
      const base = path.slice(path.lastIndexOf("/") + 1);
      return haystack.includes(base);
    });
  }

  async function run(/** @type {PaperProject} */ project, /** @type {string} */ preamble, /** @type {() => void} */ onUpdate) {
    let changed = false;
    // Assets: fetched once per path and version, as bytes.
    let total = 0;
    for (const held of assetBytes.values()) total += held.bytes.length;
    const mainText = project.texts?.[project.main] ?? "";
    for (const path of wantedAssets(preamble + "\n" + mainText)) {
      const version = assetVersions[path];
      try {
        const url = await project.assetUrl?.(path);
        if (!url) continue;
        const body = await fetchBytes(url);
        if (!body || body.length > ASSET_LIMIT || total + body.length > TOTAL_LIMIT) continue;
        if (assetVersions[path] !== version) continue;
        total += body.length;
        assetBytes.set(path, { version, bytes: body });
        changed = true;
      } catch {
        // An asset that cannot be fetched leaves its block showing source.
      }
    }
    if (changed) bump();
    // Packages: the preamble alone, compiled until nothing more is wanted.
    // The packages a document names are in its preamble, so this runs once
    // per preamble, not once per block.
    for (let round = 0; round < ROUNDS && preamble.trim(); round++) {
      compilePage(preamble);
      const needs = needsOf(wasm);
      if (!needs || !needs.packages.length) break;
      const before = fetched.size;
      let arrived = false;
      try {
        arrived = await fetchNeeds(needs, fetched);
      } catch {
        arrived = false;
      }
      if (!arrived && fetched.size === before) break;
      changed = true;
      bump();
    }
    if (changed) onUpdate();
    return changed;
  }

  /**
   * Fetches the project's images and the packages its preamble imports, then
   * calls `onUpdate` so the view can draw again. Calls that arrive while one
   * is running are folded into one more run after it.
   * @param {PaperProject} project
   * @param {string} preamble the document's preamble rules
   * @param {() => void} [onUpdate]
   */
  function prepare(project, preamble, onUpdate = () => {}) {
    setProject(project);
    const key = [main, preamble, JSON.stringify(assetVersions), String(textsRev)].join("\0");
    if (key === preparedKey) return Promise.resolve(false);
    if (preparing) {
      again = true;
      return preparing;
    }
    preparedKey = key;
    preparing = run(project, preamble, onUpdate)
      .catch(() => false)
      .finally(() => {
        preparing = null;
        if (again) {
          again = false;
          preparedKey = "";
          prepare(project, preamble, onUpdate);
        }
      });
    return preparing;
  }

  const mathCss = styleOf(compilePage("$x$"));
  return { render, frame, unframe, mathCss, setProject, prepare };
}

/** The bytes at a URL, or null. @param {string} url */
async function defaultFetchBytes(url) {
  const response = await fetch(url);
  return response.ok ? new Uint8Array(await response.arrayBuffer()) : null;
}
