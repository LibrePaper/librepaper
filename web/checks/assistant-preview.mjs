import assert from "node:assert/strict";
import { previewCandidate } from "../src/lib/assistant-preview.js";
import { snapshotDigest } from "../src/lib/tree-digest.js";

const tree = { main: "main.md", texts: { "main.md": "Before", "other.md": "Other" },
  files: { "main.md": { kind: "text", id: "main" }, "other.md": { kind: "text", id: "other" } },
  assets: {}, digests: {} };
const base_revision = await snapshotDigest(tree);
const revision = await snapshotDigest({ ...tree, texts: { ...tree.texts, "main.md": "After" } });
const request = { files: { "main.md": "After" }, revision, base_revision };
let compiled = 0;
const result = await previewCandidate({ tree, request, render: async (candidate) => {
  compiled++;
  assert.equal(candidate.texts["main.md"], "After");
  assert.equal(candidate.texts["other.md"], "Other");
  candidate.files["main.md"].id = "mutated by renderer";
  return { html: "<p>After</p>", diagnostics: [] };
} });
assert.equal(result.ok, true);
assert.equal(result.revision, revision);
assert.equal(tree.texts["main.md"], "Before");
assert.equal(tree.files["main.md"].id, "main");
for (const invalid of [
  { ...request, base_revision: "stale" },
  { ...request, revision: "wrong candidate" },
  { ...request, files: { "missing.md": "New" } },
  { ...request, files: { "../main.md": "Outside" } },
]) {
  await assert.rejects(previewCandidate({ tree, request: invalid, render: () => { compiled++; } }));
}
assert.equal(compiled, 1, "invalid candidates never reach the compiler");
const failed = await previewCandidate({ tree, request, render: async () => ({ diagnostics: [{ severity: "error", file: "main.md", line: 1, message: "Bad syntax" }] }) });
assert.equal(failed.ok, false);
assert.equal(failed.diagnostics[0].revision, revision);
assert.equal(failed.diagnostics[0].source, "After");

// A source edit during async hashing must not change what gets compiled.
let resume;
const wait = new Promise((resolve) => { resume = resolve; });
const running = previewCandidate({ tree, request, digest: async (captured) => { await wait; return snapshotDigest(captured); }, render: async (candidate) => {
  assert.equal(candidate.texts["other.md"], "Other");
  return { html: "", diagnostics: [] };
} });
tree.texts["other.md"] = "Concurrent edit";
resume();
assert.equal((await running).ok, true);

// A candidate reference keeps the relay small even when one source file is
// larger than its 16 KiB control-frame budget. The browser renders the
// immutable manifest and source fetched by the agent client, independently of
// whatever the live editor contains now.
const longSource = `${"large source ".repeat(2000)}\nAfter`;
const sourceHash = [...new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(longSource)))]
  .map((byte) => byte.toString(16).padStart(2, "0")).join("");
const candidateManifest = {
  "main.md": { kind: "text", id: "main", sha: sourceHash, size: new TextEncoder().encode(longSource).byteLength },
};
const referenced = { candidate_id: "candidate-large", base_revision, revision: "", main: "main.md", files: candidateManifest, texts: { "main.md": longSource } };
referenced.revision = await snapshotDigest({ main: referenced.main, texts: referenced.texts, files: referenced.files, digests: {} });
const referencedResult = await previewCandidate({
  request: { candidate_id: referenced.candidate_id, base_revision, revision: referenced.revision, candidate: referenced },
  tree,
  render: async (candidate) => { assert.equal(candidate.texts["main.md"], longSource); return { html: "<p>large</p>", diagnostics: [] }; },
});
assert.equal(referencedResult.ok, true);
assert.equal(referencedResult.revision, referenced.revision);
assert.ok(longSource.length > 16 * 1024);

// Asset bytes gathered after the metadata request remain part of the same
// immutable candidate tree. A candidate render must not rebuild the tree and
// silently lose those bytes.
const assetBytes = new Uint8Array([137, 80, 78, 71, 0, 1, 2, 3]);
const assetSha = [...new Uint8Array(await crypto.subtle.digest("SHA-256", assetBytes))]
  .map((byte) => byte.toString(16).padStart(2, "0")).join("");
const assetCandidate = {
  candidate_id: "candidate-assets", base_revision, main: "main.md",
  files: {
    "main.md": { kind: "text", id: "main", sha: sourceHash, size: new TextEncoder().encode(longSource).byteLength },
    "figure.png": { kind: "asset", id: "figure", sha: assetSha, size: assetBytes.byteLength },
  },
  texts: { "main.md": longSource },
  revision: await snapshotDigest({ main: "main.md", texts: { "main.md": longSource }, files: {
    "main.md": { kind: "text", id: "main", sha: sourceHash, size: new TextEncoder().encode(longSource).byteLength },
    "figure.png": { kind: "asset", id: "figure", sha: assetSha, size: assetBytes.byteLength },
  }, digests: { "figure.png": assetSha }, assets: { "figure.png": assetBytes } }),
};
const assetResult = await previewCandidate({
  request: { candidate: assetCandidate, base_revision, revision: assetCandidate.revision },
  tree: { ...tree, assets: { "figure.png": assetBytes }, digests: { "figure.png": assetSha },
    urls: { "figure.png": "blob:figure" } },
  render: async (candidate) => {
    assert.deepEqual([...candidate.assets["figure.png"]], [...assetBytes]);
    assert.equal(candidate.urls["figure.png"], "blob:figure");
    return { html: "<p>asset</p>", diagnostics: [] };
  },
});
assert.equal(assetResult.ok, true);
console.log("assistant candidate preview checks passed");
