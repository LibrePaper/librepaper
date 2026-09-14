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

for (const mode of ["current", "previous", "checkpoint"]) {
  for (const sha of ["A", "B", "A", "B"]) {
    await controller.select(sha, mode);
    assert.equal(controller.selected, sha);
    assert.equal(controller.problem, "");
    const selected = points.find(point => point.sha === sha);
    assert.equal(controller.result.oldTree.texts["main.md"], mode === "previous" ? "original" : selected.texts["main.md"]);
    assert.equal(controller.result.newTree.texts["main.md"], mode === "current" ? "current" : selected.texts["main.md"]);
    assert.equal(controller.result.diff, mode !== "checkpoint");
  }
}

// Capture current before a delayed read, and never share its mutable maps.
const pending = [];
const raced = createHistorySource({ currentTree: () => live, checkpoint: sha => new Promise((resolve, reject) => pending.push({sha, resolve, reject})) });
const first = raced.select("A");
live.texts["main.md"] = "new live edit";
pending.shift().resolve(points[0]); await first;
assert.equal(raced.result.newTree.texts["main.md"], "current");

// The same SHA can have two requests for different modes. Identity alone
// cannot arbitrate their replies: only the latest request owns the screen.
const older = raced.select("A", "current");
const newer = raced.select("A", "checkpoint");
assert.equal(raced.result, null, "no stale source while loading");
pending[1].resolve(points[0]); await newer;
pending[0].resolve(points[0]); await older;
pending.length = 0;
assert.equal(raced.mode, "checkpoint");
assert.equal(raced.result.diff, false);
const leaving = raced.select("B"); raced.close(); pending.shift().resolve(points[1]); await leaving;
assert.equal(raced.selected, ""); assert.equal(raced.result, null);

// A pruned original parent cannot be silently replaced by a retained ancestor.
points.push(tree("C", "after pruning", { parent: "A", original_parent: "missing", ancestry_gap: true }));
await controller.select("C", "previous");
assert.match(controller.problem, /previous checkpoint is unavailable/i);
assert.equal(controller.result, null);
await controller.select("C", "checkpoint"); assert.equal(controller.problem, "");

// File addition/deletion and empty text must remain distinguishable.
points[1].texts = { "new.md": "", "main.md": "revision" };
points[0].texts["removed.md"] = "removed text";
await controller.select("B", "previous", "removed.md");
assert.equal(controller.path, "removed.md");
assert.equal(controller.result.oldTree.texts[controller.path], "removed text");
assert.equal(controller.result.newTree.texts[controller.path], undefined);
controller.selectFile("new.md");
assert.equal(controller.result.oldTree.texts[controller.path], undefined);
assert.equal(controller.result.newTree.texts[controller.path], "");
assert.equal(controller.mode, "previous");
controller.dispose(); raced.dispose();
console.log("history source: endpoints, mode/selection races, snapshots, pruning and file presence passed");
