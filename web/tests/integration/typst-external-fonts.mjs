// The external Typst build must have every verified default font registered
// before load() resolves. A failed partial attempt must be discarded so the
// next call can fetch and register the complete set again.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { load } from "../../src/lib/renderer-wasm.js";

const sha = (bytes) => createHash("sha256").update(bytes).digest("hex");
const fontBytes = [Buffer.from("font-one"), Buffer.from("font-two")];
const fonts = fontBytes.map((bytes, index) => ({
  url: `fonts/${index}.otf`, sha256: sha(bytes), size: bytes.length,
}));
const manifest = Buffer.from(JSON.stringify({ fonts }));
const manifestSha = sha(manifest);
const calls = [];
let failOnce = true;
let corruptFont = false;

function makeExports(external = true) {
  const memory = new WebAssembly.Memory({ initial: 1 });
  let next = 16;
  return {
    memory,
    alloc(length) { const pointer = next; next += length; return pointer; },
    dealloc() {}, compile() { return 0; }, output_ptr() { return 0; }, ok() { return 1; },
    output_kind() { return 0; }, diagnostics() { return 0; }, diagnostics_ptr() { return 0; },
    failure_page() { return 0; }, title_of() { return 0; }, add_file() { return 0; },
    clear_files() {}, set_main() {}, set_asset_url() {}, set_today() {}, word_diff() { return 0; },
    ...(external ? {
      default_fonts_required() { return 1; },
      add_default_font(pointer, length) {
        const value = new TextDecoder().decode(new Uint8Array(memory.buffer, pointer, length));
        calls.push(value);
        return value === "font-one" && instance.rejectFont ? 1 : 0;
      },
    } : {}),
  };
}

const originalStreaming = WebAssembly.instantiateStreaming;
const originalFetch = globalThis.fetch;
const originalLocation = globalThis.location;
let instance = makeExports();
WebAssembly.instantiateStreaming = async () => ({ instance: { exports: instance } });
globalThis.location = { href: "https://app.example/" };
globalThis.fetch = async (url) => {
  const href = String(url);
  if (href.endsWith("typst.wasm")) return new Response(new Uint8Array());
  if (href.endsWith("fonts.json")) return new Response(manifest);
  const index = Number(href.match(/fonts\/(\d+)\.otf$/)?.[1]);
  if (index === 1 && failOnce) {
    failOnce = false;
    return new Response("temporarily unavailable", { status: 503 });
  }
  return new Response(corruptFont && index === 0 ? Buffer.from("corrupt") : fontBytes[index]);
};

try {
  const url = `https://typst.librepaper.workers.dev/${"a".repeat(64)}/typst.wasm#fontsSha256=${manifestSha}`;
  await assert.rejects(load(url), /503/);
  assert.deepEqual(calls, [], "fonts are registered only after every fetch succeeds");
  const loaded = await load(url);
  assert.equal(loaded, instance);
  assert.deepEqual(calls, ["font-one", "font-two"], "fonts register in manifest order");

  await assert.rejects(load(`https://typst.librepaper.workers.dev/${"b".repeat(64)}/typst.wasm#fontsSha256=${"0".repeat(64)}`), /manifest checksum mismatch/);
  assert.deepEqual(calls, ["font-one", "font-two"], "a changed manifest is rejected before registration");

  corruptFont = true;
  await assert.rejects(load(`https://typst.librepaper.workers.dev/${"c".repeat(64)}/typst.wasm#fontsSha256=${manifestSha}`), /checksum or size mismatch/);
  corruptFont = false;
  assert.deepEqual(calls, ["font-one", "font-two"], "a changed font is rejected before registration");

  instance = makeExports();
  instance.rejectFont = true;
  const addFailureUrl = `https://typst.librepaper.workers.dev/${"d".repeat(64)}/typst.wasm#fontsSha256=${manifestSha}`;
  await assert.rejects(load(addFailureUrl), /rejected default font/);
  assert.deepEqual(calls.slice(2), ["font-one"], "an invalid font leaves the failed instance isolated");
  instance = makeExports();
  await load(addFailureUrl);
  assert.deepEqual(calls.slice(3), ["font-one", "font-two"], "retry instantiates a fresh module and loads all fonts");

  const registrationsBeforeEmbeddedModule = [...calls];
  instance = makeExports(false);
  await load("https://app.example/old-typst.wasm");
  assert.deepEqual(calls, registrationsBeforeEmbeddedModule, "embedded-font modules need no sidecar assets");
} finally {
  WebAssembly.instantiateStreaming = originalStreaming;
  globalThis.fetch = originalFetch;
  if (originalLocation === undefined) delete globalThis.location;
  else globalThis.location = originalLocation;
}

console.log("typst-external-fonts: verified fonts load before resolve and retry cleanly");
