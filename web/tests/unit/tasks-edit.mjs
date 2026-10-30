import assert from "node:assert/strict";
import { readTasks } from "../../src/lib/tasks/model.js";
import { cycle } from "../../src/lib/tasks/edit.js";

// Applies change specs to a string, right to left, as CodeMirror does.
function applyChanges(source, changes) {
  return [...changes]
    .sort((a, b) => b.from - a.from)
    .reduce((text, change) => text.slice(0, change.from) + change.insert + text.slice(change.to), source);
}

function firstTask(source) {
  return readTasks(source)[0];
}

// Each box steps to the next state in the cycle, and the last wraps to the first. X counts as x.
{
  const steps = [[" ", "x"], ["x", "/"], ["X", "/"], ["/", "-"], ["-", " "]];
  for (const [box, next] of steps) {
    const source = `- [${box}] Task due:2026-11-05\n`;
    const changes = cycle(firstTask(source));
    const boxAt = source.indexOf("[") + 1;
    assert.deepEqual(changes, [{ from: boxAt, to: boxAt + 1, insert: next }], box);
    assert.equal(applyChanges(source, changes), `- [${next}] Task due:2026-11-05\n`, box);
  }
}

// A cycle changes the box character only, so the rest of the line keeps its exact text.
{
  const source = "- [/]   Working   priority:2  #writing  \n";
  assert.equal(applyChanges(source, cycle(firstTask(source))), "- [-]   Working   priority:2  #writing  \n");
}
