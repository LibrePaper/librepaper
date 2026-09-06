// Exercise the binary renderer ABI without downloading a compiler. In
// particular, this catches accidentally decoding PDF bytes as UTF-8 and
// retaining a view into memory that a later ABI call can replace.
import assert from "node:assert/strict";
import { call } from "../src/lib/renderer-wasm.js";

const memory = new WebAssembly.Memory({ initial: 1 });
let next = 16;
const output = Uint8Array.of(0x25, 0x50, 0x44, 0x46, 0x00, 0xff);
const diagnostics = new TextEncoder().encode("[]");
new Uint8Array(memory.buffer, 2048, output.length).set(output);
new Uint8Array(memory.buffer, 3072, diagnostics.length).set(diagnostics);
const wasm = {
  memory,
  alloc(length) { const pointer = next; next += length; return pointer; },
  dealloc() {},
  compile() { return output.length; },
  output_ptr() { return 2048; },
  output_kind() { return 2; },
  ok() { return 1; },
  diagnostics() { return diagnostics.length; },
  diagnostics_ptr() { return 3072; },
};

const result = call(wasm, "compile", "source", "title");
assert.deepEqual([...result.bytes], [...output]);
assert.equal(result.text, "");
assert.equal(result.kind, "pdf");
assert.deepEqual(result.diagnostics, []);
// A later operation can overwrite the old output region and grow linear
// memory. The caller still owns the copy it received from `call`.
new Uint8Array(memory.buffer, 2048, output.length).fill(0x41);
memory.grow(1);
assert.deepEqual([...result.bytes], [...output]);
new Uint8Array(memory.buffer, 2048, output.length).set(output);

const oldModule = { ...wasm, output_kind: undefined, ok() { return 1; } };
const old = call(oldModule, "compile", "html", "title");
assert.equal(old.kind, "html");
assert.equal(old.text, "%PDF\0�");
console.log("renderer-wasm: binary copy, typed output kind and diagnostics passed");
