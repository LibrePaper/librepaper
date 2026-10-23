// A renderer module is checked against the digest in its mirror URL before it
// is compiled. The bytes here are arbitrary: only the hash matters.
import assert from "node:assert/strict";
import { sha256Hex } from "../../src/lib/digest.js";
import { digestInUrl, verifiedModuleBytes } from "../../src/lib/renderer-wasm.js";

const bytes = new Uint8Array([0, 97, 115, 109, 1, 0, 0, 0]);
const digest = await sha256Hex(bytes);
const url = `https://mirror.example/wasm/${digest}/typst.wasm`;

assert.equal(digestInUrl(url), digest);
assert.equal(digestInUrl("https://mirror.example/wasm/typst.wasm"), null);
assert.equal(digestInUrl(`https://mirror.example/wasm/${digest}`), null);
assert.equal(digestInUrl("https://mirror.example/wasm/short/typst.wasm"), null);

const answer = (body, ok = true) => async () => ({
  ok,
  status: ok ? 200 : 404,
  arrayBuffer: async () => body.slice().buffer,
});

const got = await verifiedModuleBytes(url, answer(bytes));
assert.deepEqual(new Uint8Array(got), bytes);

await assert.rejects(
  verifiedModuleBytes(url, answer(new Uint8Array([1, 2, 3]))),
  /failed digest verification/,
);
await assert.rejects(verifiedModuleBytes(url, answer(bytes, false)), /could not be fetched/);
await assert.rejects(
  verifiedModuleBytes("https://mirror.example/wasm/typst.wasm", answer(bytes)),
  /failed digest verification/,
);
