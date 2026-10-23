// The supported browser Typst module embeds its default fonts and needs only
// its WASM request; loading must not require a mirror manifest or font files.
import assert from "node:assert/strict";
import { load } from "../../src/lib/renderer-wasm.js";
import { sha256Hex } from "../../src/lib/digest.js";

const fetched = [];
// WebAssembly.instantiate is faked below, so any fixed bytes will do. Their
// digest goes in the URL, as the mirror would put it.
const body = Uint8Array.of(0, 97, 115, 109, 1, 0, 0, 0);
const sha = await sha256Hex(body);
const originalInstantiate = WebAssembly.instantiate;
const originalFetch = globalThis.fetch;
const memory = new WebAssembly.Memory({ initial: 1 });
let date;
const wasm = {
  memory,
  alloc() { return 0; }, dealloc() {}, compile() { return 0; }, output_ptr() { return 0; }, ok() { return 1; },
  output_kind() { return 0; }, diagnostics() { return 0; }, diagnostics_ptr() { return 0; },
  failure_page() { return 0; }, title_of() { return 0; }, add_file() { return 0; },
  clear_files() {}, set_main() {}, set_asset_url() {}, set_today(...parts) { date = parts; }, word_diff() { return 0; },
  default_fonts_required() { return 0; },
};

WebAssembly.instantiate = async () => ({ instance: { exports: wasm } });
globalThis.fetch = async (url) => {
  fetched.push(String(url));
  return new Response(body);
};

try {
  const url = `https://app.example/wasm/${sha}/typst.embedded.wasm`;
  assert.equal(await load(url), wasm);
  assert.deepEqual(fetched, [url]);
  assert.equal(date.length, 3);

  wasm.default_fonts_required = () => 1;
  const oldExternalModule = `https://app.example/wasm/${sha}/typst.old-external-font-build.wasm`;
  await assert.rejects(load(oldExternalModule), /external fonts are unsupported/);
  assert.deepEqual(fetched, [url, oldExternalModule]);
} finally {
  WebAssembly.instantiate = originalInstantiate;
  globalThis.fetch = originalFetch;
}

console.log("typst-embedded-fonts: loaded the local module without sidecar assets");
