import assert from "node:assert/strict";
import { fieldOf, readTasks, taskAt } from "../../src/lib/tasks/model.js";

// States, fields, tags and the first line's text come from one source.
{
  const source = [
    "# Plan",
    "",
    "- [ ] Write intro priority:2 due:2026-11-05 #writing/intro",
    "  - [/] Outline  assignee:alice,bob",
    "  - [X] Old note",
    "- [-] Dropped idea",
    "",
    "Later text",
    "",
  ].join("\n");
  const tasks = readTasks(source);
  assert.equal(tasks.length, 4);
  const [intro, outline, old, dropped] = tasks;

  assert.equal(intro.line, 3);
  assert.equal(intro.state, "todo");
  assert.equal(intro.box, " ");
  assert.equal(source[intro.boxAt], " ");
  assert.equal(intro.text, "Write intro");
  assert.deepEqual(intro.fields.map(field => [field.key, field.value]), [
    ["priority", "2"],
    ["due", "2026-11-05"],
  ]);
  assert.deepEqual(intro.tags.map(tag => tag.name), ["writing/intro"]);

  assert.equal(outline.state, "doing");
  assert.equal(outline.indent, 2);
  assert.equal(outline.text, "Outline");
  assert.equal(fieldOf(outline, "assignee").value, "alice,bob");

  assert.equal(old.state, "done");
  assert.equal(old.box, "X");

  assert.equal(dropped.state, "cancelled");
}

// Each field's range is its exact token, so the source can be cut at it.
{
  const source = "- [ ] Draft priority:2 due:2026-11-05\n";
  const [task] = readTasks(source);
  for (const field of task.fields) {
    assert.equal(source.slice(field.from, field.to), `${field.key}:${field.value}`);
    assert.equal(source.slice(field.valueFrom, field.to), field.value);
  }
}

// The value starts after the gap, whatever its width, and the token runs to the end of the value.
{
  const source = "- [ ] Draft due:   2026-11-05 priority:\t2\n";
  const [task] = readTasks(source);
  const [due, priority] = task.fields;
  assert.equal(due.value, "2026-11-05");
  assert.equal(source.slice(due.valueFrom, due.to), "2026-11-05");
  assert.equal(due.valueFrom, source.indexOf("2026-11-05"));
  assert.equal(due.to, source.indexOf("2026-11-05") + "2026-11-05".length);
  assert.equal(priority.value, "2");
  assert.equal(source.slice(priority.valueFrom, priority.to), "2");
}

// Fields are read on the first line only, and unknown or unescaped tokens are text.
{
  const [plain] = readTasks("- [ ] Title\n  due:2026-11-05\n");
  assert.deepEqual(plain.fields, []);

  const [noise] = readTasks("- [ ] Item #123 unknown:thing\n");
  assert.deepEqual(noise.fields, []);
  assert.deepEqual(noise.tags, []);
  assert.equal(noise.text, "Item #123 unknown:thing");
}

// A repeated field keeps its source and carries a warning.
{
  const source = "- [ ] Bad priority:9 due:2026-11-05 due:2026-11-06\n";
  const [task] = readTasks(source);
  assert.notEqual(task.fields[0].warning, null);
  assert.equal(task.fields[1].warning, null);
  assert.equal(task.fields[2].warning, "Duplicate due.");
  assert.equal(source.slice(task.fields[2].from, task.fields[2].to), "due:2026-11-06");
}

// taskAt finds the task on the line the position falls on.
{
  const source = "- [ ] First\n- [ ] Second due:2026-11-05\n\nText\n";
  assert.equal(taskAt(source, source.indexOf("Second")).index, 1);
  assert.equal(taskAt(source, source.indexOf("Text")), null);
}
