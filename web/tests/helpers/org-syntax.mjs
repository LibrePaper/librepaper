// Loads org.wasm for unit tests of the Org consumers, which read its tree.
import { existsSync, readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { loadOrgSyntax } from "../../src/lib/org-language/module.js";

const wasm = process.env.LIBREPAPER_ORG_WASM || fileURLToPath(new URL("../../wasm/org.wasm", import.meta.url));

/** Loads the module; throws when it is missing, so Org tests never pass by skipping. */
export async function loadOrgSyntaxForTests() {
  if (!existsSync(wasm)) throw new Error(`org.wasm missing at ${wasm}: run node tools/assets/pins.mjs fetch`);
  const api = await loadOrgSyntax(readFileSync(wasm));
  if (!api) throw new Error("org.wasm did not load");
  return api;
}
