import assert from "node:assert/strict";
import { compileModule } from "svelte/compiler";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

const source = new URL("../../src/lib/reader/history-source.svelte.js", import.meta.url);
const dir = mkdtempSync(join(tmpdir(), "history-source-test-"));
let createHistorySource;
try {
  const compiled = compileModule(readFileSync(source, "utf8"), { filename: source.pathname, generate: "client" });
  const code = compiled.js.code.replace(/from (["'])([^"']+)\1/g, (_, quote, specifier) => `from ${JSON.stringify(import.meta.resolve(specifier))}`);
  const output = join(dir, "source.mjs"); writeFileSync(output, code);
  ({ createHistorySource } = await import(pathToFileURL(output).href));
} finally { rmSync(dir, { recursive: true, force: true }); }

const tree = (sha, text, extra = {}) => ({ sha, main: "main.md", texts: { "main.md": text }, files: {}, ...extra });
const points = [tree("A", "original"), tree("B", "revision", { parent: "A" })];
let live = tree("live", "current");
const controller = createHistorySource({
  currentTree: () => live,
  checkpoints: () => points,
  checkpoint: async sha => {
    const found = points.find(point => point.sha === sha);
    if (!found) throw new Error("missing");
    const { parent, ...endpoint } = found; // The real endpoint omits the list's parent.
    return endpoint;
  },
});

// One comparison: whichever version is selected, against the source as it
// stands. Selecting another version never carries the last one's result.
for (const sha of ["A", "B", "A", "B"]) {
  await controller.select(sha);
  assert.equal(controller.selected, sha);
  assert.equal(controller.problem, "");
  const selected = points.find(point => point.sha === sha);
  assert.equal(controller.result.oldTree.texts["main.md"], selected.texts["main.md"]);
  assert.equal(controller.result.newTree.texts["main.md"], "current");
  assert.equal(controller.result.newLabel, "Current version");
  assert.equal(controller.result.diff, true);
}

// The current source on its own: nothing selected is not a comparison.
await controller.select("");
assert.equal(controller.result.diff, false);
assert.equal(controller.result.oldTree.texts["main.md"], "current");
assert.equal(controller.result.status["main.md"], "same");

// Capture current before a delayed read, and never share its mutable maps.
const pending = [];
const raced = createHistorySource({ currentTree: () => live, checkpoint: sha => new Promise((resolve, reject) => pending.push({sha, resolve, reject})) });
const first = raced.select("A");
live.texts["main.md"] = "new live edit";
pending.shift().resolve(points[0]); await first;
assert.equal(raced.result.newTree.texts["main.md"], "current");

// Two requests for the same version. Identity alone cannot arbitrate their
// replies: only the latest request owns the screen.
const older = raced.select("A");
const newer = raced.select("A");
assert.equal(raced.result, null, "no stale source while loading");
pending[1].resolve(points[0]); await newer;
pending[0].resolve(points[0]); await older;
pending.length = 0;
assert.equal(raced.problem, "");
assert.ok(raced.result);
const leaving = raced.select("B"); raced.close(); pending.shift().resolve(points[1]); await leaving;
assert.equal(raced.selected, ""); assert.equal(raced.result, null);

// A version whose source the server will not give back says so, and shows
// nothing rather than the last comparison.
points.push({ sha: "C", main: "main.md", files: {} });
await controller.select("C");
assert.match(controller.problem, /did not return its source files/i);
assert.equal(controller.result, null);
points.pop();

// What changed, per file: added, removed, changed, and unchanged, with an
// empty file distinguishable from a missing one.
points[1].texts = { "new.md": "", "main.md": "revision" };
points[0].texts["removed.md"] = "removed text";
live = { sha: "live", main: "main.md", texts: { "main.md": "current", "steady.md": "same" }, files: {} };
points[0].texts["steady.md"] = "same";
await controller.select("A", "removed.md");
assert.equal(controller.path, "removed.md");
assert.equal(controller.result.status["removed.md"], "removed");
assert.equal(controller.result.status["main.md"], "changed");
assert.equal(controller.result.status["steady.md"], "same");
assert.deepEqual(controller.result.changed, ["main.md", "removed.md"]);
assert.equal(controller.result.oldTree.texts[controller.path], "removed text");
assert.equal(controller.result.newTree.texts[controller.path], undefined);
controller.selectFile("main.md");
assert.equal(controller.path, "main.md");

// A figure is reported as added, removed or changed, and never compared.
const withAsset = { sha: "D", main: "main.md", texts: { "main.md": "current" },
  files: { "fig.png": { kind: "asset", sha: "a".repeat(64) } } };
points.push(withAsset);
live = { sha: "live", main: "main.md", texts: { "main.md": "current" },
  files: { "fig.png": { kind: "asset", sha: "b".repeat(64) } } };
await controller.select("D");
assert.equal(controller.result.status["fig.png"], "changed");
assert.equal(controller.result.binary["fig.png"], true);
assert.equal(controller.result.binary["main.md"], false);

// The file the reader chose stays chosen while the new version still has it.
assert.equal(controller.path, "main.md");
// A comparison that does not have it lands on a file that differs instead.
controller.selectFile("fig.png");
live = { sha: "live", main: "main.md", texts: { "main.md": "current", "steady.md": "same" }, files: {} };
await controller.select("A");
assert.equal(controller.path, "main.md");

controller.dispose(); raced.dispose();
console.log("history source: one comparison against current, selection races, snapshots, and per-file status passed");
