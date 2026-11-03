import assert from "node:assert/strict";
import { loadRunes } from "../helpers/runes.mjs";
import { CURRENT, NOTHING } from "../../src/lib/history-endpoints.js";

const { createHistorySource } = await loadRunes(
  new URL("../../src/lib/reader/history-source.svelte.js", import.meta.url),
);

const tree = (sha, texts, files = {}) => ({ sha, main: "main.md", texts, files });
// Saved versions in time order, and what each one stored. A moment is a
// position in the editing history, stored but never listed as a version.
const stored = {
  A: tree("A", { "main.md": "original" }),
  B: tree("B", { "main.md": "revision" }),
  "frontier:x": tree("frontier:x", { "main.md": "draft" }),
};
const labelList = [{ sha: "A", seq: 1 }, { sha: "B", seq: 2 }];
const read = async sha => {
  const found = stored[sha];
  if (!found) throw new Error("missing");
  return { ...found }; // The real endpoint omits the list's parent and seq.
};
// Lets queued promises run before a test looks at what they started.
const settle = () => new Promise(resolve => setTimeout(resolve, 0));

let live = tree("live", { "main.md": "current" });
const controller = createHistorySource({ currentTree: () => live, labels: () => labelList, read });

// Clicking a version shows what it changed: the previous version against it.
await controller.view("B");
assert.equal(controller.selected, "B");
assert.equal(controller.older, "A");
assert.equal(controller.newer, "B");
assert.equal(controller.problem, "");
assert.equal(controller.result.oldTree.texts["main.md"], "original");
assert.equal(controller.result.newTree.texts["main.md"], "revision");
assert.equal(controller.result.status["main.md"], "changed");
assert.equal(controller.result.diff, true);
assert.equal(controller.target, "B");

// The first version is compared with an empty project, so all of it is new.
await controller.view("A");
assert.equal(controller.older, NOTHING);
assert.equal(controller.newer, "A");
assert.deepEqual(controller.result.oldTree.texts, {});
assert.equal(controller.result.status["main.md"], "added");
assert.equal(controller.target, "A");

// The Current row: the newest version against the project as it stands.
await controller.view("");
assert.equal(controller.selected, "");
assert.equal(controller.older, "B");
assert.equal(controller.newer, CURRENT);
assert.equal(controller.result.oldTree.texts["main.md"], "revision");
assert.equal(controller.result.newTree.texts["main.md"], "current");
assert.equal(controller.result.diff, true);
assert.equal(controller.target, "B");

// A moment is compared with the live project, and it is not a version to restore.
await controller.view("frontier:x");
assert.equal(controller.selected, "frontier:x");
assert.equal(controller.older, "frontier:x");
assert.equal(controller.newer, CURRENT);
assert.equal(controller.result.oldTree.texts["main.md"], "draft");
assert.equal(controller.target, "");

// Nothing saved and nothing edited: the project alone is not a comparison.
const empty = createHistorySource({ currentTree: () => live, labels: () => [], read });
await empty.view("");
assert.equal(empty.older, CURRENT);
assert.equal(empty.newer, CURRENT);
assert.equal(empty.result.diff, false);
assert.equal(empty.result.status["main.md"], "same");
assert.equal(empty.target, "");
empty.dispose();

// compare() puts the pair in time order whichever way it was asked. Without
// a third argument the clicked version stays as the first argument.
await controller.view("");
await controller.compare("B", "A");
assert.equal(controller.older, "A");
assert.equal(controller.newer, "B");
assert.equal(controller.selected, "B");
await controller.compare(CURRENT, "A", "A");
assert.equal(controller.older, "A");
assert.equal(controller.newer, CURRENT);
assert.equal(controller.selected, "A");
assert.equal(controller.target, "A");

// setNewer and setOlder change one side. The clicked version follows the side
// that was just chosen when it is no longer an endpoint.
await controller.compare("A", "B", "B");
await controller.setNewer(CURRENT);
assert.equal(controller.older, "A");
assert.equal(controller.newer, CURRENT);
assert.equal(controller.selected, CURRENT);
assert.equal(controller.target, "A");
await controller.setOlder("B");
assert.equal(controller.older, "B");
assert.equal(controller.newer, CURRENT);
assert.equal(controller.selected, CURRENT, "still an endpoint, so it stays");
assert.equal(controller.target, "B");
assert.equal(controller.result.oldTree.texts["main.md"], "revision");

// Target: the clicked version when it is a real endpoint, else the newer
// saved version, else the older one, else none.
await controller.compare("A", "B", "A");
assert.equal(controller.target, "A");
await controller.compare("A", "B", "B");
assert.equal(controller.target, "B");
await controller.compare("A", CURRENT, "");
assert.equal(controller.target, "A");

// reload() captures the live project again.
await controller.view("");
live = tree("live", { "main.md": "edited" });
await controller.reload();
assert.equal(controller.result.newTree.texts["main.md"], "edited");
live = tree("live", { "main.md": "current" });

// Capture the live project before a delayed read, and never share its mutable maps.
const pending = [];
const raced = createHistorySource({
  currentTree: () => live,
  labels: () => labelList,
  read: sha => new Promise(resolve => pending.push({ sha, resolve })),
});
const first = raced.view("");
await settle();
live.texts["main.md"] = "new live edit";
pending.shift().resolve({ ...stored.B }); await first;
assert.equal(raced.result.newTree.texts["main.md"], "current");
live = tree("live", { "main.md": "current" });

// Two requests for the same comparison. Identity alone cannot arbitrate their
// replies: only the latest request owns the screen.
const older = raced.view("");
const newer = raced.view("");
await settle();
assert.equal(raced.result, null, "no stale result while loading");
pending[1].resolve({ ...stored.B }); await newer;
pending[0].resolve({ ...stored.B }); await older;
pending.length = 0;
assert.equal(raced.problem, "");
assert.ok(raced.result);

// Closing while a version is still loading drops its answer.
const leaving = raced.view("A");
await settle();
raced.close();
pending.shift().resolve({ ...stored.A }); await leaving;
assert.equal(raced.selected, ""); assert.equal(raced.result, null);

// A view overtaken while its previous version is still being looked up does nothing.
let release;
const slow = createHistorySource({
  currentTree: () => live,
  labels: () => labelList,
  read,
  previous: () => new Promise(resolve => { release = resolve; }),
});
const late = slow.view("B");
await slow.view("");
release("A"); await late;
assert.equal(slow.selected, "");
assert.equal(slow.newer, CURRENT);
assert.equal(slow.result.oldTree.texts["main.md"], "revision");
slow.dispose();

// A label whose previous version failed to load is retried by reload().
let lookups = 0;
const flaky = createHistorySource({
  currentTree: () => live,
  labels: () => labelList,
  read,
  previous: async () => {
    lookups += 1;
    if (lookups === 1) throw new Error("lookup failed");
    return "A";
  },
});
await flaky.view("B");
assert.notEqual(flaky.problem, "");
assert.equal(flaky.older, "");
await flaky.reload();
assert.equal(flaky.problem, "");
assert.equal(flaky.older, "A");
assert.equal(flaky.result.oldTree.texts["main.md"], "original");
assert.equal(flaky.result.newTree.texts["main.md"], "revision");
flaky.dispose();

// A version whose source the server will not give back says so, and shows
// nothing rather than the last comparison.
stored.C = { sha: "C", main: "main.md", files: {} };
labelList.push({ sha: "C", seq: 3 });
await controller.view("C");
assert.match(controller.problem, /did not return its source files/i);
assert.equal(controller.result, null);
labelList.pop(); delete stored.C;

// A version the server cannot read is reported too.
await controller.compare("missing", "A");
assert.notEqual(controller.problem, "");
assert.equal(controller.result, null);
controller.close();

// What changed, per file: added, removed, changed and unchanged, with an empty
// file distinguishable from a missing one. The preferred file is chosen while
// the comparison has it.
stored.P = tree("P", { "main.md": "original", "removed.md": "removed text", "steady.md": "same" });
stored.Q = tree("Q", { "main.md": "revision", "new.md": "", "steady.md": "same" });
const files = createHistorySource({
  currentTree: () => live,
  labels: () => [{ sha: "P", seq: 1 }, { sha: "Q", seq: 2 }],
  read,
  preferredPath: () => "removed.md",
});
await files.view("Q");
assert.equal(files.path, "removed.md");
assert.equal(files.result.status["removed.md"], "removed");
assert.equal(files.result.status["new.md"], "added");
assert.equal(files.result.status["main.md"], "changed");
assert.equal(files.result.status["steady.md"], "same");
assert.deepEqual(files.result.changed, ["main.md", "new.md", "removed.md"]);
assert.equal(files.result.binary["new.md"], false);
files.selectFile("main.md");
assert.equal(files.path, "main.md");
files.selectFile("missing.md");
assert.equal(files.path, "main.md");
files.dispose();

// The reader's file did not change, so the comparison opens on one that did:
// an unchanged file on screen would read as "this version changed nothing".
const steady = createHistorySource({
  currentTree: () => live,
  labels: () => [{ sha: "P", seq: 1 }, { sha: "Q", seq: 2 }],
  read,
  preferredPath: () => "steady.md",
});
await steady.view("Q");
assert.equal(steady.path, "main.md");
// With nothing changed at all, the reader's file stays.
await steady.compare("Q", "Q");
assert.deepEqual(steady.result.changed, []);
steady.dispose();

// A figure is reported as changed when its digest differs, and never compared as text.
stored.E = tree("E", { "main.md": "current" }, { "fig.png": { kind: "asset", sha: "a".repeat(64) } });
live = tree("live", { "main.md": "current" }, { "fig.png": { kind: "asset", sha: "b".repeat(64) } });
const figures = createHistorySource({ currentTree: () => live, labels: () => [{ sha: "E", seq: 1 }], read });
await figures.view("");
assert.equal(figures.result.status["fig.png"], "changed");
assert.equal(figures.result.binary["fig.png"], true);
assert.equal(figures.result.binary["main.md"], false);
figures.dispose();

controller.dispose(); raced.dispose();
console.log("history source: endpoint pairs, normalisation, targets, selection races, snapshots, and per-file status passed");
