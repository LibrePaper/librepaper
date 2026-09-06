import { load, call, handOver } from "./renderer-wasm.js";

function render(wasm, tree, title) {
  handOver(wasm, tree);
  const source = tree.texts?.[tree.main] ?? "";
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

// Keep ABI operations ordered even while the module is downloading.
let queue = Promise.resolve();
self.onmessage = ({ data: { id, url, operation, args } }) => {
  queue = queue.then(async () => {
    try {
      const wasm = await load(url);
      let result;
      if (operation === "render") result = render(wasm, args.tree, args.title);
      else if (operation === "title") result = call(wasm, "title_of", args.source).text;
      else if (operation === "failure") result = wasm.failure_page ? call(wasm, "failure_page", args.title).text : null;
      else if (operation !== "warm") throw new Error(`Unknown renderer operation: ${operation}`);
      self.postMessage({ id, result });
    } catch (error) {
      self.postMessage({ id, error: error.message || String(error) });
    }
  });
};
