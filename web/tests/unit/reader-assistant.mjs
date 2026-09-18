import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";

// Exercise Reader's actual selection capture.
//
// What is worth pinning down here is what a selection does *not* carry. It
// used to carry a source path and a checkpoint digest, worked out in this
// browser while the person was still deciding whether to write anything --
// which is how a quotation could end up paired with a file the reader had
// since navigated away from. A selection is now what the page showed and
// nothing else; which passage of which file that is gets decided by the
// server, from the checkpoint it holds, at the moment the comment is made.
const reader = readFileSync(new URL("../../src/components/Reader.svelte", import.meta.url), "utf8");
const start = reader.indexOf("  function showSelection(");
const end = reader.indexOf("  function placeBar(", start);
assert.ok(start > 0 && end > start);
const ctx = vm.createContext({
  mayEdit: true, publishedMode: false, publishedBundle: null,
  pending: null, docText: "same phrase",
  bar: { shown: true },
  placeBar: () => {},
});
vm.runInContext(reader.slice(start, end), ctx);

vm.runInContext(
  "showSelection({exact:'same phrase',prefix:'before ',suffix:' after',position:7}, {})",
  ctx,
);
const captured = ctx.pending;
assert.equal(captured.exact, "same phrase");
assert.equal(captured.prefix, "before ");
assert.equal(captured.suffix, " after");
assert.equal(captured.position, 7);
assert.equal(captured.bundle_id, "");
for (const field of ["source", "path", "revision", "file_id", "start", "end"]) {
  assert.ok(!(field in captured), `a selection must not carry ${field}`);
}

// A selection with no words is not a passage, and so is not something to
// comment on. There is nothing else an annotation can be about.
vm.runInContext("showSelection({exact:'',prefix:'',suffix:'',position:3}, {})", ctx);
assert.equal(ctx.pending, null);
vm.runInContext("showSelection(null, {})", ctx);
assert.equal(ctx.pending, null);

// A reader annotating a published rendering says which one they were reading.
ctx.publishedMode = true;
ctx.publishedBundle = { id: "bundle-1" };
vm.runInContext("showSelection({exact:'same phrase',prefix:'',suffix:'',position:0}, {})", ctx);
assert.equal(ctx.pending.bundle_id, "bundle-1");

console.log("reader-assistant: a selection is what the page showed, and carries no source identity");
