import assert from "node:assert/strict";
import { reviewGroups, rejectPass, diagnosticContext } from "../src/lib/assistant-review.js";

const items = [
  { id: "a", motivation: "editing", pass: "one" },
  { id: "note", motivation: "commenting", pass: "one" },
  { id: "b", motivation: "editing", pass: "one", resolved: true, outcome: "accepted" },
  { id: "c", motivation: "editing", pass: "two" },
  { id: "d", motivation: "editing", pass: "one" },
];
const groups = reviewGroups(items);
assert.deepEqual(groups.map((group) => group.comments.map((item) => item.id)), [["a", "b", "d"], ["note"], ["c"]]);
assert.deepEqual(groups[0].pending.map((item) => item.id), ["a", "d"]);
const decisions = [];
const result = await rejectPass(groups[0].comments, async (item) => {
  decisions.push(item.id);
  if (item.id === "a") throw new Error("Access changed.");
});
assert.deepEqual(decisions, ["a", "d"]);
assert.deepEqual(result, [{ id: "a", rejected: false, error: "Access changed." }, { id: "d", rejected: true }]);
const tree = { main: "paper.md", texts: { "paper.md": "first\nold error\nlast" } };
const diagnostic = diagnosticContext({ severity: "error", message: "Invalid", line: 2 }, tree, "captured-sha");
tree.texts["paper.md"] = "new text";
assert.equal(diagnostic.source, "old error");
assert.equal(diagnostic.revision, "captured-sha");
assert.equal(diagnostic.file, "paper.md");
console.log("assistant-review: grouped passes, partial rejection and captured diagnostic context passed");
