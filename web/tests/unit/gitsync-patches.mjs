// Character patches on UTF-8 byte offsets.

import assert from "node:assert/strict";

import { textPatches } from "../../src/lib/gitsync/patches.js";

const enc = new TextEncoder();
const dec = new TextDecoder();

// The text after the patches, applied at their UTF-8 offsets, checking each
// patch's `exact` on the way.
function applied(text, patches) {
  let bytes = enc.encode(text);
  for (const patch of [...patches].sort((a, b) => b.start - a.start)) {
    assert.equal(dec.decode(bytes.slice(patch.start, patch.end)), patch.exact);
    bytes = new Uint8Array([
      ...bytes.slice(0, patch.start),
      ...enc.encode(patch.replacement),
      ...bytes.slice(patch.end),
    ]);
  }
  return dec.decode(bytes);
}

{
  const base = "# Title\n\nSome text.\n";
  const next = "# Title\n\nSome longer text.\nMore.\n";
  const patches = textPatches("a.md", base, next);
  assert.ok(patches.length >= 1);
  assert.ok(patches.every((patch) => patch.path === "a.md"));
  assert.equal(applied(base, patches), next);
}

// Offsets count bytes, past two-byte and four-byte characters.
{
  const base = "café 😀 end\n";
  const next = "café 😀 fin\n";
  const patches = textPatches("a.md", base, next);
  assert.equal(applied(base, patches), next);
  assert.ok(patches.every((patch) => patch.start >= 10));
}

// A boundary inside a surrogate pair moves outward.
assert.equal(applied("x😀y", textPatches("a.md", "x😀y", "x😁y")), "x😁y");

// No difference, no patch.
assert.deepEqual(textPatches("a.md", "same\n", "same\n"), []);

// Texts with several astral characters round-trip.
for (const [from, to] of [["a😀b😀c", "a😁b😁c"], ["😀😀😀", "😁😁😁"], ["abc", ""], ["", "abc"]]) {
  assert.equal(applied(from, textPatches("a.md", from, to)), to);
}

console.log("gitsync patches ok");
