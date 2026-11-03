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

// Clicking a version shows it (older) against the project as it stands (newer).
await controller.view("B");
assert.equal(controller.selected, "B");
assert.equal(controller.older, "B");
assert.equal(controller.newer, CURRENT);
assert.equal(controller.problem, "");
assert.equal(controller.result.oldTree.texts["main.md"], "revision");
assert.equal(controller.result.newTree.texts["main.md"], "current");
assert.equal(controller.result.status["main.md"], "changed");
assert.equal(controller.result.diff, true);
assert.equal(controller.target, "B");

await controller.view("A");
assert.equal(controller.selected, "A");
assert.equal(controller.older, "A");
assert.equal(controller.newer, CURRENT);
assert.equal(controller.result.oldTree.texts["main.md"], "original");
assert.equal(controller.target, "A");

// The first version is compared with an empty project, so all of it is new,
// when a comparison asks for that.
await controller.compare(NOTHING, "A", "A");
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

// Selection mode. beginSelect names the endpoint to replace, and choose answers
// it: only that endpoint changes, and a choice outside the pair is put back in
// time order. Every read is recorded, so a choice that changes nothing is visible.
const reads = [];
const picker = createHistorySource({
  currentTree: () => live,
  labels: () => [{ sha: "A", seq: 1 }, { sha: "B", seq: 2 }, { sha: "C", seq: 3 }],
  read: sha => { reads.push(sha); return read(sha); },
});
stored.C = tree("C", { "main.md": "third" });

await picker.compare("A", "B", "B");
const readsBefore = reads.length;
picker.beginSelect("older");
assert.equal(picker.mode, "select-older");
assert.equal(picker.cancelled, "");
assert.deepEqual([picker.older, picker.newer, picker.selected], ["A", "B", "B"]);
assert.deepEqual([picker.result.older, picker.result.newer], ["A", "B"]);
assert.equal(picker.result.oldTree.texts["main.md"], "original");
assert.equal(reads.length, readsBefore, "beginning a selection reads nothing");
picker.beginSelect("newer");
assert.equal(picker.mode, "select-newer");
assert.deepEqual([picker.older, picker.newer], ["A", "B"]);
picker.beginSelect("newer");
assert.equal(picker.mode, "normal");
assert.equal(picker.cancelled, "newer");
assert.deepEqual([picker.older, picker.newer, picker.selected], ["A", "B", "B"]);

// Cancelling keeps the comparison, remembers the endpoint, and does nothing when nothing is picked.
picker.beginSelect("older");
picker.cancelSelect();
assert.equal(picker.mode, "normal");
assert.equal(picker.cancelled, "older");
assert.deepEqual([picker.older, picker.newer], ["A", "B"]);
picker.cancelSelect();
assert.equal(picker.cancelled, "older");
assert.equal(reads.length, readsBefore);

// Choosing for the newer endpoint replaces only that endpoint.
picker.beginSelect("newer");
assert.equal(picker.cancelled, "");
await picker.choose("C");
assert.equal(picker.mode, "normal");
assert.equal(picker.cancelled, "");
assert.deepEqual([picker.older, picker.newer, picker.selected], ["A", "C", "C"]);
assert.equal(picker.result.newTree.texts["main.md"], "third");
assert.equal(picker.result.oldTree.texts["main.md"], "original");

// Choosing for the older endpoint replaces only that endpoint.
picker.beginSelect("older");
await picker.choose("B");
assert.deepEqual([picker.older, picker.newer, picker.selected], ["B", "C", "B"]);
assert.equal(picker.result.oldTree.texts["main.md"], "revision");
assert.equal(picker.result.newTree.texts["main.md"], "third");

// The other endpoint's id, or the id being replaced, changes nothing and leaves the mode.
const readsAtChoice = reads.length;
picker.beginSelect("older");
await picker.choose("C");
assert.equal(picker.mode, "normal");
assert.deepEqual([picker.older, picker.newer, picker.selected], ["B", "C", "B"]);
picker.beginSelect("older");
await picker.choose("B");
assert.equal(picker.mode, "normal");
assert.deepEqual([picker.older, picker.newer, picker.selected], ["B", "C", "B"]);
assert.equal(reads.length, readsAtChoice, "no reload for a choice that changes nothing");

// A choice later than the newer endpoint, while selecting the older one, is normalised.
await picker.compare("A", "B", "B");
picker.beginSelect("older");
await picker.choose("C");
assert.deepEqual([picker.older, picker.newer, picker.selected], ["B", "C", "C"]);
assert.equal(picker.result.oldTree.texts["main.md"], "revision");
// And an earlier one while selecting the newer endpoint.
picker.beginSelect("newer");
await picker.choose("A");
assert.deepEqual([picker.older, picker.newer, picker.selected], ["A", "B", "A"]);
assert.equal(picker.result.oldTree.texts["main.md"], "original");
assert.equal(picker.result.newTree.texts["main.md"], "revision");

// Choosing the Current row for the newer endpoint sets it to the project as it stands.
await picker.compare("A", "B", "B");
picker.beginSelect("newer");
await picker.choose("");
assert.equal(picker.mode, "normal");
assert.deepEqual([picker.older, picker.newer, picker.selected], ["A", CURRENT, ""]);
assert.equal(picker.result.newTree.texts["main.md"], "current");

// Outside selection, choose is a plain view of the version.
await picker.choose("A");
assert.deepEqual([picker.older, picker.newer, picker.selected], ["A", CURRENT, "A"]);
picker.dispose();

// Rapid choices: a slower read for an earlier choice does not overwrite a later one.
const gated = {};
const rapid = createHistorySource({
  currentTree: () => live,
  labels: () => [{ sha: "A", seq: 1 }, { sha: "B", seq: 2 }, { sha: "C", seq: 3 }],
  read: sha => (sha === "C" ? new Promise(resolve => { gated.C = resolve; }) : read(sha)),
});
await rapid.compare("A", "B", "B");
rapid.beginSelect("newer");
const slowChoice = rapid.choose("C");
await settle();
assert.equal(typeof gated.C, "function");
rapid.beginSelect("older");
await rapid.choose("");
gated.C({ ...stored.C });
await slowChoice;
assert.deepEqual([rapid.older, rapid.newer, rapid.selected], ["C", CURRENT, ""]);
assert.equal(rapid.result.oldTree.texts["main.md"], "third");
assert.equal(rapid.result.newTree.texts["main.md"], "current");
assert.equal(rapid.problem, "");
rapid.dispose();

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

// The Current row waits for the newest version too: a failed lookup is retried by reload().
let newestLookups = 0;
const unknownNewest = createHistorySource({
  currentTree: () => live,
  labels: () => labelList,
  read,
  newest: async () => {
    newestLookups += 1;
    if (newestLookups === 1) throw new Error("lookup failed");
    return "B";
  },
});
await unknownNewest.view("");
assert.notEqual(unknownNewest.problem, "");
assert.equal(unknownNewest.older, "");
await unknownNewest.reload();
assert.equal(unknownNewest.problem, "");
assert.equal(unknownNewest.older, "B");
assert.ok(unknownNewest.result);
unknownNewest.dispose();

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
stored.T = tree("T", { "main.md": "other", "removed.md": "edited removal", "steady.md": "same" });
stored.V = tree("V", { "main.md": "original", "removed.md": "edited removal", "steady.md": "same" });
const files = createHistorySource({
  currentTree: () => live,
  labels: () => [{ sha: "P", seq: 1 }, { sha: "Q", seq: 2 }, { sha: "T", seq: 3 }, { sha: "V", seq: 4 }],
  read,
  preferredPath: () => "removed.md",
});
await files.compare("P", "Q", "Q");
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
// A changed pair keeps the file while it still changed: main.md changed from P to T.
files.beginSelect("newer");
await files.choose("T");
assert.deepEqual(files.result.changed, ["main.md", "removed.md"]);
assert.equal(files.path, "main.md");
// From P to V only removed.md changed, so the file falls back to the first changed one.
files.beginSelect("newer");
await files.choose("V");
assert.deepEqual(files.result.changed, ["removed.md"]);
assert.equal(files.path, "removed.md");
files.dispose();

// The reader's file did not change, so the comparison opens on one that did:
// an unchanged file on screen would read as "this version changed nothing".
const steady = createHistorySource({
  currentTree: () => live,
  labels: () => [{ sha: "P", seq: 1 }, { sha: "Q", seq: 2 }],
  read,
  preferredPath: () => "steady.md",
});
await steady.compare("P", "Q", "Q");
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
