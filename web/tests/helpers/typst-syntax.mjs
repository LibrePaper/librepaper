// Loads typst-syntax.wasm for unit tests of the Typst consumers, which read its tree.
import { existsSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { loadTypstSyntax } from "../../src/lib/typst-language/module.js";

const wasm = process.env.LIBREPAPER_TYPST_SYNTAX_WASM || fileURLToPath(new URL("../../wasm/typst-syntax.wasm", import.meta.url));

/** Loads the module; throws when it is missing, so Typst tests never pass by skipping. */
export async function loadTypstSyntaxForTests() {
  if (!existsSync(wasm)) throw new Error(`typst-syntax.wasm missing at ${wasm}: run node tools/assets/pins.mjs fetch`);
  const api = await loadTypstSyntax(readFileSync(wasm));
  if (!api) throw new Error("typst-syntax.wasm did not load");
  return api;
}
