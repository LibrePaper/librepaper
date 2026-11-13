import { load, call, handOverChanged, applyTree, emptyMirror } from "./renderer-wasm.js";
import { renderResolving } from "./needs.js";

// What earlier compiles fetched -- packages -- for the life of this worker.
// See `needs.js`.
const fetched = new Map();
/** @typedef {{tree: object, title: string, format?: string, old?: string, new?: string, source?: string, path?: string, offset?: number, explicit?: boolean, forPandoc?: boolean}} RendererArgs */
/** @typedef {{id: string, url: string, operation: string, args: RendererArgs}} RendererRequest */
/** @typedef {{location: Location, onmessage: ((event: MessageEvent<RendererRequest>) => void) | null, postMessage: (message: unknown, transfer?: Transferable[]) => void}} RendererWorkerScope */
/** @type {RendererWorkerScope} */
const rendererScope = /** @type {RendererWorkerScope} */ (/** @type {unknown} */ (self));

// Editor completion asks the module what can be typed at the cursor. A module
// built before completion has no answer, which is not an error: the editor
// simply shows nothing.
/** @param {import("./renderer-wasm.js").RendererWasm} wasm @param {RendererArgs} args */
function complete(wasm, { tree, path = "", source = "", offset = 0, explicit = false }) {
  if (typeof wasm.complete !== "function") return null;
  handOverChanged(wasm, tree);
  // A fresh module has none of the packages fetched by earlier renders.
  for (const [file, bytes] of fetched) {
    if (file in (tree.texts || {}) || file in (tree.assets || {})) continue;
    call(wasm, "add_file", file, bytes);
  }
  const request = JSON.stringify({ path, source, offset, explicit, files: Object.keys(tree.files || tree.texts || {}) });
  const { bytes } = call(wasm, "complete", request);
  const text = new TextDecoder().decode(bytes);
  return text ? JSON.parse(text) : null;
}

// The Quarto draft as Markdown, from a module that has the dialect. One built
// before it has no `draft` export, which the caller treats as a failure to
// fall back from.
function draft(wasm, tree, forPandoc) {
  if (typeof wasm.draft !== "function") throw new Error("this renderer module has no Quarto support");
  handOverChanged(wasm, tree);
  const { bytes, diagnostics } = call(wasm, "draft", tree.texts?.[tree.main] ?? "", forPandoc ? 1 : 0);
  const text = new TextDecoder().decode(bytes);
  const parsed = text ? JSON.parse(text) : { markdown: "", line_map: [] };
  return { markdown: parsed.markdown ?? "", line_map: parsed.line_map ?? [], diagnostics };
}

async function render(wasm, tree, title, format) {
  const { bytes, text, kind, ok, diagnostics } = await renderResolving(wasm, tree, title, fetched, { format });
  if (ok && format === "html" && kind === "html") return { html: text, diagnostics };
  if (ok && format === "pdf" && kind === "pdf") return { pdf: bytes.buffer, diagnostics };
  // Markdown and the other generic renderers do not carry a requested Typst
  // target. Preserve their native output kind instead of treating it as PDF.
  if (ok && !format && kind === "html") return { html: text, diagnostics };
  if (ok && !format && kind === "pdf") return { pdf: bytes.buffer, diagnostics };
  if (ok) throw new Error(`Typst renderer returned ${kind || "no output"} while ${format} output was requested`);
  const said = diagnostics.length
    ? diagnostics
    : [{ severity: "error", message: text || "this document could not be compiled", hints: [], file: "", line: 0, column: 0, end_line: 0, end_column: 0 }];
  return { html: null, pdf: null, diagnostics: said };
}

// What the page last sent for this worker's tree. Deltas are applied inside the
// queue, so they land in the order the page sent them.
let mirror = emptyMirror();

// Keep ABI operations ordered even while the module is downloading.
let queue = Promise.resolve();
rendererScope.onmessage = ({ data: { id, url, operation, args: incoming } }) => {
  queue = queue.then(async () => {
    try {
      let args = incoming;
      if (incoming?.tree) {
        const applied = applyTree(mirror, incoming.tree);
        mirror = applied.mirror;
        args = { ...incoming, tree: applied.tree };
      }
      const wasm = await load(url);
      let result;
      if (operation === "render") result = await render(wasm, args.tree, args.title, args.format);
      else if (operation === "bibliography") {
        const parsed = call(wasm, "bibliography", JSON.stringify(args));
        if (!parsed.ok) throw new Error(parsed.text || "Bibliography analysis failed.");
        result = JSON.parse(parsed.text);
      }
      else if (operation === "title") result = call(wasm, "title_of", args.source).text;
      else if (operation === "diff") {
        const raw = call(wasm, "word_diff", args.old, args.new).text;
        result = JSON.parse(raw || "[]");
      }
      else if (operation === "draft") result = draft(wasm, args.tree, Boolean(args.forPandoc));
      else if (operation === "complete") result = complete(wasm, args);
      else if (operation === "failure") result = call(wasm, "failure_page", args.title).text;
      else if (operation !== "warm") throw new Error(`Unknown renderer operation: ${operation}`);
      const transfer = result?.pdf instanceof ArrayBuffer ? [result.pdf] : [];
      rendererScope.postMessage({ id, result }, transfer);
    } catch (error) {
      rendererScope.postMessage({ id, error: error.message || String(error) });
    }
  });
};
