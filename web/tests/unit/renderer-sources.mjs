import assert from "node:assert/strict";
import { sourcesOf } from "../../src/lib/renderer-wasm.js";

// A stand-in for a module's exports: the table is JSON in its memory.
function module(json) {
  const bytes = json === null ? new Uint8Array() : new TextEncoder().encode(json);
  const memory = new WebAssembly.Memory({ initial: 1 });
  new Uint8Array(memory.buffer, 8, bytes.length).set(bytes);
  return { memory, sources: () => bytes.length, sources_ptr: () => 8 };
}

// Typst names its main file and each included one, by tag.
assert.deepEqual(sourcesOf(module('["main.typ","chapters/one.typ"]')), ["main.typ", "chapters/one.typ"]);
// An entry that is not a path is no file of the author's.
assert.deepEqual(sourcesOf(module('["main.typ",null,""]')), ["main.typ", null, null]);
// No table, an empty one, or a broken one is no table.
assert.equal(sourcesOf(module(null)), null);
assert.equal(sourcesOf(module("[]")), null);
assert.equal(sourcesOf(module("{")), null);
assert.equal(sourcesOf(module('{"a":1}')), null);
// A module without the export (markdown, an older typst) has none.
assert.equal(sourcesOf({ memory: new WebAssembly.Memory({ initial: 1 }) }), null);
