import assert from "node:assert/strict";
import test from "node:test";
import { TASK_GROUPS, filterTaskGroups } from "../../src/lib/agent-tasks.js";

// Small fixtures, so the expectations do not move when the copy does.
const groups = [
  {
    id: "writing",
    label: "Writing",
    tasks: [
      { id: "proofread", label: "Proofread", text: "Check the spelling and grammar." },
      { id: "tighten", label: "Tighten", text: "Shorten a passage without losing meaning." },
    ],
  },
  {
    id: "cv",
    label: "Résumé",
    tasks: [
      { id: "cover", label: "Cover letter", text: "Draft a letter for the position." },
      { id: "polish", label: "Polish", text: "Improve the wording." },
    ],
  },
  {
    id: "review",
    label: "Review",
    tasks: [
      { id: "citations", label: "Check citations", text: "Each citation should resolve in the bibliography." },
    ],
  },
];

const ids = (shown) => shown.map((group) => [group.id, group.tasks.map((task) => task.id)]);

test("an empty or whitespace query returns the same groups", () => {
  assert.equal(filterTaskGroups(groups, ""), groups);
  assert.equal(filterTaskGroups(groups, "   \t "), groups);
});

test("every word must match, in any order", () => {
  assert.deepEqual(ids(filterTaskGroups(groups, "grammar spelling")), [["writing", ["proofread"]]]);
  assert.deepEqual(ids(filterTaskGroups(groups, "spelling grammar")), [["writing", ["proofread"]]]);
  assert.deepEqual(ids(filterTaskGroups(groups, "spelling bibliography")), []);
});

test("a task matches on its instruction text and on its group label", () => {
  assert.deepEqual(ids(filterTaskGroups(groups, "bibliography")), [["review", ["citations"]]]);
  assert.deepEqual(ids(filterTaskGroups(groups, "writing")), [["writing", ["proofread", "tighten"]]]);
});

test("matching ignores case and accents", () => {
  assert.deepEqual(ids(filterTaskGroups(groups, "RESUME")), [["cv", ["cover", "polish"]]]);
  assert.deepEqual(ids(filterTaskGroups(groups, "résumé")), [["cv", ["cover", "polish"]]]);
  assert.deepEqual(ids(filterTaskGroups(groups, "LETTER")), [["cv", ["cover"]]]);
});

test("groups without a matching task are dropped", () => {
  assert.deepEqual(ids(filterTaskGroups(groups, "letter")), [["cv", ["cover"]]]);
  assert.deepEqual(ids(filterTaskGroups(groups, "zzz")), []);
});

test("the input is not mutated", () => {
  const before = JSON.stringify(groups);
  const shown = filterTaskGroups(groups, "letter");
  assert.notEqual(shown, groups);
  assert.equal(JSON.stringify(groups), before);
  assert.equal(groups[1].tasks.length, 2);
});

test("TASK_GROUPS has unique group ids and unique task ids", () => {
  const groupIds = TASK_GROUPS.map((group) => group.id);
  assert.equal(new Set(groupIds).size, groupIds.length);
  const taskIds = TASK_GROUPS.flatMap((group) => group.tasks.map((task) => task.id));
  assert.equal(new Set(taskIds).size, taskIds.length);
});
