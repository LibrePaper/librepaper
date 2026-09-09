import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import vm from "node:vm";

// Exercise Reader's actual selection capture and handoff, including the async
// digest boundary. This catches pairing a retained quotation with a new file.
const reader = readFileSync(new URL("../src/components/Reader.svelte", import.meta.url), "utf8");
const start = reader.indexOf("  function showSelection(");
const end = reader.indexOf("  function placeBar(", start);
assert.ok(start > 0 && end > start);
let completeDigest;
let tree = { main: "a.md", texts: { "a.md": "same phrase" } };
const capturedTrees = [];
const ctx = vm.createContext({
  pending: null, docText: "same phrase", viewing: null, selectionRevision: null,
  bar: { shown: true }, width: 600, assistantRequest: null,
  session: { paths: new Map([["a", "a.md"]]) }, openFile: "a",
  treeNow: () => tree,
  sync: { sourceSelectorFor: (_text, selection, current) => ({ ...selection, path: current.main }) },
  renderers: { formatOf: () => "markdown" },
  snapshotDigest: (current) => { capturedTrees.push(current); return new Promise((resolve) => { completeDigest = resolve; }); },
  placeBar: () => {}, showPanel: () => {}, showMobileView: () => {},
  crypto: { randomUUID: () => "request" },
});
vm.runInContext(reader.slice(start, end), ctx);
vm.runInContext("showSelection({exact:'same phrase',prefix:'',suffix:'',position:0}, {})", ctx);
tree = { main: "b.md", texts: { "b.md": "different" } };
ctx.openFile = "b";
const captured = ctx.pending;
completeDigest("revision-a");
await ctx.selectionRevision;
assert.equal(capturedTrees[0].main, "a.md");
assert.equal(captured.source.path, "a.md");
assert.equal(captured.revision, "revision-a");
assert.equal(ctx.pending.revision, "revision-a");
vm.runInContext("showSelection(null, {})", ctx);
assert.equal(ctx.pending, null);
assert.equal(captured.source.path, "a.md");
console.log("reader-assistant: selection attachment keeps its captured source and revision across navigation");
