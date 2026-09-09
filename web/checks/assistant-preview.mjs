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
console.log("assistant candidate preview checks passed");
