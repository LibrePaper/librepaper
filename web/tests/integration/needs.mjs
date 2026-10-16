// The package round trip without a compiler: the tar reader, what `needs`
// says, the fetch into the map, and the compile-fetch-compile loop against a
// mocked module and a mocked network.
import assert from "node:assert/strict";
import { gzipSync } from "node:zlib";
import { untar, fetchNeeds, renderResolving, ROUNDS } from "../../src/lib/needs.js";
import { needsOf } from "../../src/lib/renderer-wasm.js";

const encoder = new TextEncoder();
const response = (bytes, ok = true) => ({
  ok,
  status: ok ? 200 : 404,
  body: new Blob([bytes]).stream(),
  arrayBuffer: async () => bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength),
  json: async () => JSON.parse(new TextDecoder().decode(bytes)),
});

function tarOf(files) {
  const chunks = [];
  for (const [name, body] of files) {
    const bytes = typeof body === "string" ? encoder.encode(body) : body;
    const header = new Uint8Array(512);
    header.set(encoder.encode(name), 0);
    header.set(encoder.encode("0000644"), 100);
    header.set(encoder.encode(bytes.length.toString(8).padStart(11, "0")), 124);
    header[156] = 48;
    header.set(encoder.encode("ustar"), 257);
    chunks.push(header, bytes, new Uint8Array((512 - (bytes.length % 512)) % 512));
  }
  chunks.push(new Uint8Array(1024));
  const out = new Uint8Array(chunks.reduce((n, c) => n + c.length, 0));
  let at = 0;
  for (const c of chunks) { out.set(c, at); at += c.length; }
  return out;
}

const manifest = '[package]\nname = "mini"\nversion = "0.1.0"\nentrypoint = "lib.typ"\n';
const tar = tarOf([["typst.toml", manifest], ["src/lib.typ", "#let hello = 1\n"]]);
const files = untar(tar);
assert.deepEqual(files.map(([name]) => name), ["typst.toml", "src/lib.typ"]);
assert.equal(new TextDecoder().decode(files[0][1]), manifest);
assert.equal(untar(tar.subarray(0, 600)).length, 1, "a cut archive yields the files it holds whole");
assert.equal(untar(new Uint8Array()).length, 0);
console.log("needs: tar reader passed");

// A mocked network: the package registry.
const fetches = [];
globalThis.fetch = async (url) => {
  url = String(url);
  fetches.push(url);
  if (url === "https://packages.typst.org/preview/mini-0.1.0.tar.gz") return response(new Uint8Array(gzipSync(tar)));
  return response(new Uint8Array(), false);
};

const needs = {
  packages: [{ namespace: "preview", name: "mini", version: "0.1.0", dir: "@preview/mini/0.1.0", url: "https://packages.typst.org/preview/mini-0.1.0.tar.gz" }],
};
const fetched = new Map();
assert.equal(await fetchNeeds(needs, fetched), true);
assert.equal(new TextDecoder().decode(fetched.get("@preview/mini/0.1.0/typst.toml")), manifest);
assert.ok(fetched.has("@preview/mini/0.1.0/src/lib.typ"));
assert.equal(fetched.size, 2);
const before = fetches.length;
assert.equal(await fetchNeeds(needs, fetched), false, "nothing new arrives the second time");
assert.equal(fetches.length, before, "and nothing is fetched again");
console.log("needs: fetching packages into the map passed");

// A mocked module: `needs` says the package is missing until the map holds
// it, and the loop compiles, fetches, and compiles again.
function fakeModule(script) {
  const memory = new WebAssembly.Memory({ initial: 1 });
  const added = [];
  let compiles = 0;
  let next = 16;
  let needsJson = "";
  const write = (at, bytes) => new Uint8Array(memory.buffer, at, bytes.length).set(bytes);
  const output = encoder.encode("%PDF-fake");
  write(4096, output);
  write(6144, encoder.encode("[]"));
  return {
    added,
    get compiles() { return compiles; },
    memory,
    alloc(length) { const p = next; next += length; return p; },
    dealloc() {},
    clear_files() { added.length = 0; },
    add_file(p, n) { added.push(new TextDecoder().decode(new Uint8Array(memory.buffer, p, n))); },
    set_asset_url() {},
    set_main() {},
    compile() {
      compiles++;
      needsJson = JSON.stringify(script(added, compiles));
      write(8192, encoder.encode(needsJson));
      return output.length;
    },
    output_ptr() { return 4096; },
    output_kind() { return 2; },
    ok() { return 1; },
    diagnostics() { return 2; },
    diagnostics_ptr() { return 6144; },
    needs() { return encoder.encode(needsJson).length; },
    needs_ptr() { return 8192; },
  };
}

// The engine ABI still reports fonts. A font the document names is not
// fetched and does not cost a compile round: only packages are wanted.
const wasm = fakeModule((added) => ({
  packages: added.includes("@preview/mini/0.1.0/typst.toml") ? [] : needs.packages,
  fonts: ["stand in"],
}));
const map = new Map();
const tree = { main: "main.typ", texts: { "main.typ": "#import \"@preview/mini:0.1.0\": hello" }, assets: {} };
const result = await renderResolving(wasm, tree, "T", map);
assert.equal(result.kind, "pdf");
assert.equal(wasm.compiles, 2, "one round of fetching, then the compile that had everything");
assert.ok(wasm.added.includes("main.typ"));
assert.ok(wasm.added.includes("@preview/mini/0.1.0/src/lib.typ"));
assert.equal(needsOf(wasm).packages.length, 0);
// The next keystroke costs one compile: the map already has everything.
await renderResolving(wasm, tree, "T", map);
assert.equal(wasm.compiles, 3);

// A need that names only fonts triggers no fetch and no extra compile round.
{
  const fontsOnly = fakeModule(() => ({ packages: [], fonts: ["stand in"] }));
  const requestsBefore = fetches.length;
  assert.equal(await fetchNeeds({ packages: [], fonts: ["stand in"] }, new Map()), false);
  assert.equal(fetches.length, requestsBefore, "no request is made for a font");
  await renderResolving(fontsOnly, tree, "T", new Map());
  assert.equal(fontsOnly.compiles, 1, "a font the document names does not cost another compile");
  assert.equal(fetches.length, requestsBefore);
}

// A need nothing can satisfy stops the loop after one fetch, not ROUNDS.
const stubborn = fakeModule(() => ({ packages: [{ namespace: "preview", name: "gone", version: "9.9.9", dir: "@preview/gone/9.9.9", url: "https://packages.typst.org/preview/gone-9.9.9.tar.gz" }], fonts: [] }));
await renderResolving(stubborn, tree, "T", new Map());
assert.equal(stubborn.compiles, 1);
assert.ok(ROUNDS >= 2);

// A module without the export -- markdown -- is a module with no needs.
assert.equal(needsOf({ memory: wasm.memory }), null);
console.log("needs: compile-fetch-compile loop, memoised map, and unsatisfiable needs passed");
