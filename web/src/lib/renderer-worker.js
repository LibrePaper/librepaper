import { load, call } from "./renderer-wasm.js";
import { renderResolving } from "./needs.js";

// What earlier compiles fetched -- packages and fonts -- for the life of
// this worker. See `needs.js`.
const fetched = new Map();
/** @typedef {{tree: object, title: string, format?: string, old?: string, new?: string, source?: string}} RendererArgs */
/** @typedef {{id: string, url: string, operation: string, args: RendererArgs}} RendererRequest */
/** @typedef {{location: Location, onmessage: ((event: MessageEvent<RendererRequest>) => void) | null, postMessage: (message: unknown, transfer?: Transferable[]) => void}} RendererWorkerScope */
/** @type {RendererWorkerScope} */
const rendererScope = /** @type {unknown} */ (self);

function fontsIndex() {
  try {
    return new URL("/api/fonts/index.json", self.location.href).href;
  } catch {
    return "";
  }
}

async function render(wasm, tree, title, format) {
  const { bytes, text, kind, ok, diagnostics } = await renderResolving(wasm, tree, title, fetched, {
    format,
    fontsIndex: fontsIndex(),
  });
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

// Keep ABI operations ordered even while the module is downloading.
let queue = Promise.resolve();
rendererScope.onmessage = ({ data: { id, url, operation, args } }) => {
  queue = queue.then(async () => {
    try {
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
      else if (operation === "failure") result = call(wasm, "failure_page", args.title).text;
      else if (operation !== "warm") throw new Error(`Unknown renderer operation: ${operation}`);
      const transfer = result?.pdf instanceof ArrayBuffer ? [result.pdf] : [];
      rendererScope.postMessage({ id, result }, transfer);
    } catch (error) {
      rendererScope.postMessage({ id, error: error.message || String(error) });
    }
  });
};
