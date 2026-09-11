import assert from "node:assert/strict";
import { createResultsLoader } from "../src/lib/results-loader.js";

const jsonResponse = (body, ok = true, status = ok ? 200 : 404) => ({
  ok, status,
  json: async () => body,
});
const artifactResponse = (bytes = Uint8Array.of(1, 2, 3)) => ({
  ok: true,
  arrayBuffer: async () => bytes.slice().buffer,
});
const manifest = (context, renderId = `render-${context}`) => ({
  render_id: renderId,
  context: { id: context },
  artifact: { entrypoint: "index.html", sha256: "a", size: 3, kind: "html" },
  assets: [],
});

// Same-context calls share one in-flight request and one prepared resource.
let selectedCalls = 0;
let applied = [];
const api = {
  selectedResults: async (context) => { selectedCalls += 1; return jsonResponse({ manifest: manifest(context), selection: { generation: 2 } }); },
  resultArtifact: async () => artifactResponse(),
  resultAsset: async () => ({ ok: true, arrayBuffer: async () => new ArrayBuffer(0) }),
};
const prepared = { html: "<p>saved</p>", dispose() {} };
const loader = createResultsLoader({ api, apply: (value) => applied.push(value), prepare: async () => prepared });
const sameA = loader.load("html");
const sameB = loader.load("html");
assert.strictEqual(sameA, sameB, "concurrent same-context loads are deduplicated");
assert.equal((await sameA).manifest.render_id, "render-html");
assert.equal(selectedCalls, 1);
assert.equal(applied.length, 1);

// A forced newer load wins and disposes a prepared result that finishes late.
let oldPreparedDone;
const oldPrepared = new Promise((resolve) => { oldPreparedDone = resolve; });
let oldPrepareStarted;
const oldStarted = new Promise((resolve) => { oldPrepareStarted = resolve; });
let forceCalls = 0;
const forceApi = {
  selectedResults: async (context) => { forceCalls += 1; return jsonResponse({ manifest: manifest("html", forceCalls === 1 ? "old" : "new"), selection: { generation: forceCalls } }); },
  resultArtifact: async () => artifactResponse(),
  resultAsset: async () => ({ ok: true, arrayBuffer: async () => new ArrayBuffer(0) }),
};
const disposed = [];
const forceLoader = createResultsLoader({
  api: forceApi,
  apply: (value) => applied.push(value),
  prepare: async (m) => { if (m.render_id === "old") { oldPrepareStarted(); await oldPrepared; } return { html: m.render_id, dispose: () => disposed.push(m.render_id) }; },
});
const old = forceLoader.load("html");
await oldStarted;
const newer = forceLoader.load("html", { force: true });
assert.equal((await newer).manifest.render_id, "new");
oldPreparedDone();
assert.equal(await old, null, "a superseded load cannot replace the newer selection");
assert.deepEqual(disposed, ["old"]);

// A failed preparation leaves the last good value available for the next
// ordinary load, without applying a partially downloaded bundle.
let prepareCalls = 0;
const stable = createResultsLoader({
  api,
  apply: (value) => applied.push(value),
  prepare: async () => {
    prepareCalls += 1;
    if (prepareCalls === 2) throw new Error("asset missing");
    return { html: "good", dispose() {} };
  },
});
const good = await stable.load("stable");
await assert.rejects(stable.load("stable", { force: true }), /asset missing/);
assert.strictEqual(await stable.load("stable"), good);

// A selection tombstone clears the selected value while retaining its
// generation so a subsequent render cannot reuse generation zero.
let tombstone;
const tombstoneLoader = createResultsLoader({
  api: { selectedResults: async () => jsonResponse({ generation: 17, selection: { generation: 17 }, message: "No saved output" }, false, 404) },
  apply: (value) => { tombstone = value; },
  prepare: async () => { throw new Error("must not prepare a tombstone"); },
});
const cleared = await tombstoneLoader.load("html");
assert.equal(cleared.manifest, null);
assert.equal(cleared.generation, 17);
assert.deepEqual(tombstone, cleared);

console.log("quarto loader: request deduplication, supersession disposal, failure retention, and tombstones passed");

// A portable cell-output bundle need not include a complete Quarto artifact.
const cellsOnly = { ...manifest("cells"), artifact:null };
let cellsPrepared = false;
const cellLoader = createResultsLoader({
  api: {
    selectedResults: async () => jsonResponse({ manifest:cellsOnly, selection:{ generation:3 } }),
    resultArtifact: async () => { throw new Error("no full artifact exists"); },
  },
  apply: () => {},
  prepare: async (bundle, bytes) => {
    assert.equal(bundle, cellsOnly);
    assert.equal(bytes, null);
    cellsPrepared = true;
    return { kind:null, assets:{}, dispose() {} };
  },
});
assert.equal((await cellLoader.load("cells")).manifest, cellsOnly);
assert.ok(cellsPrepared);
