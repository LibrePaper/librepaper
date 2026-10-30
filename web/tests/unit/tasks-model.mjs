import assert from "node:assert/strict";
import { fieldOf, readTasks, taskAt } from "../../src/lib/tasks/model.js";

// Nesting, states, fields, tags and the first line's text come from one source.
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
  assert.equal(intro.parent, null);
  assert.deepEqual(intro.children, [1, 2]);
  assert.equal(intro.insertAt, intro.lineEnd);
  assert.equal(intro.blockEnd, source.indexOf("  - [X] Old note") + "  - [X] Old note".length);

  assert.equal(outline.parent, 0);
  assert.equal(outline.state, "doing");
  assert.equal(outline.indent, 2);
  assert.equal(outline.text, "Outline");
  assert.equal(fieldOf(outline, "assignee").value, "alice,bob");

  assert.equal(old.state, "done");
  assert.equal(old.box, "X");
  assert.equal(old.parent, 0);

  assert.equal(dropped.state, "cancelled");
  assert.equal(dropped.parent, null);
  assert.equal(dropped.blockEnd, dropped.lineEnd);
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

// Fields are read on the first line only, and unknown or unescaped tokens are text.
{
  const [plain] = readTasks("- [ ] Title\n  due:2026-11-05\n");
  assert.deepEqual(plain.fields, []);
  assert.equal(plain.blockEnd, "- [ ] Title\n  due:2026-11-05".length);

  const [noise] = readTasks("- [ ] Item #123 unknown:thing\n");
  assert.deepEqual(noise.fields, []);
  assert.deepEqual(noise.tags, []);
  assert.equal(noise.text, "Item #123 unknown:thing");
}

// A repeated field keeps its source and carries a warning.
{
  const source = "- [ ] Bad priority:9 id:a id:b\n";
  const [task] = readTasks(source);
  assert.notEqual(task.fields[0].warning, null);
  assert.equal(task.fields[1].warning, null);
  assert.equal(task.fields[2].warning, "Duplicate id.");
  assert.equal(source.slice(task.fields[2].from, task.fields[2].to), "id:b");
}

// A task with no continuation ends on its first line, and trailing blank lines are left out.
{
  const source = "- [ ] One\n\n\nTwo\n";
  const [task] = readTasks(source);
  assert.equal(task.blockEnd, task.lineEnd);
  assert.equal(task.lineEnd, "- [ ] One".length);
}

// taskAt finds the task on the line the position falls on.
{
  const source = "- [ ] First\n- [ ] Second due:2026-11-05\n\nText\n";
  assert.equal(taskAt(source, source.indexOf("Second")).index, 1);
  assert.equal(taskAt(source, source.indexOf("Text")), null);
}

// Trailing spaces stay after the insertion point.
{
  const source = "- [ ] Write intro   \n";
  const [task] = readTasks(source);
  assert.equal(task.insertAt, "- [ ] Write intro".length);
}
