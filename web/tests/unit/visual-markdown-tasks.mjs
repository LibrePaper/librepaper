import assert from "node:assert/strict";
import { Tokenizer } from "../../src/lib/visual-markup/core/tokenizer.ts";
import { walk } from "../../src/lib/visual-markup/core/tokens.ts";
import { markdown } from "../../src/lib/visual-markup/languages/markdown/index.ts";

// Every token of a source, nested children included, in tokenizer order.
function tokensOf(source) {
  const found = [];
  walk(new Tokenizer(markdown).tokenize(source), token => found.push(token));
  return found;
}

function named(source, name) {
  return tokensOf(source).filter(token => token.name === name);
}

function items(source) {
  return tokensOf(source).filter(token => token.kind === "item");
}

// Every box character maps to its state, and the item token ends just after the box.
{
  const states = { " ": "todo", x: "done", X: "done", "/": "doing", "?": "blocked", "-": "cancelled" };
  for (const [box, state] of Object.entries(states)) {
    const source = `- [${box}] Task`;
    const [item] = items(source);
    assert.equal(item.name, "bullet", source);
    assert.equal(item.meta.task, state, source);
    assert.equal(item.meta.box, box, source);
    assert.equal(source.slice(item.to - 3, item.to), `[${box}]`, source);
    assert.equal(source[item.to - 2], box, source);
  }

  const [ordered] = items("1. [/] Step");
  assert.equal(ordered.name, "number");
  assert.equal(ordered.meta.task, "doing");

  assert.deepEqual(items("- [ ] Task")[0].meta, { marker: "-", task: "todo", box: " " });
  assert.equal(items("- plain bullet")[0].meta.task, undefined);
  assert.equal(items("- plain bullet")[0].meta.box, undefined);
}

// The task item styles: a checkbox widget and a line class named for the state.
{
  const [item] = items("- [/] Working");
  assert.deepEqual(markdown.style(item), { widget: "taskbox", lineClass: "cm-lp-task cm-lp-task-doing" });
  const [plain] = items("- plain bullet");
  assert.equal(markdown.style(plain).widget, undefined);
}

// Fields and tags on a task line, with exact ranges and meta.
{
  const source = "- [ ] Write #paper/intro priority:1 due:2026-11-05 assignee:alice,bob estimate:2h";
  const fields = named(source, "taskfield");
  assert.deepEqual(fields.map(token => source.slice(token.from, token.to)), [
    "priority:1", "due:2026-11-05", "assignee:alice,bob", "estimate:2h",
  ]);
  assert.deepEqual(fields.map(token => token.meta), [
    { key: "priority", value: "1", state: "todo" },
    { key: "due", value: "2026-11-05", state: "todo" },
    { key: "assignee", value: "alice,bob", state: "todo" },
    { key: "estimate", value: "2h", state: "todo" },
  ]);

  const tags = named(source, "tasktag");
  assert.deepEqual(tags.map(token => source.slice(token.from, token.to)), ["#paper/intro"]);
  assert.deepEqual(tags.map(token => token.meta), [{ tag: "paper/intro" }]);
  assert.equal(tags[0].from, source.indexOf("#paper"));
}

// The field's state follows the task's box.
{
  const [field] = named("- [x] Done thing due:2026-11-05", "taskfield");
  assert.equal(field.meta.state, "done");
}

// A gap after the colon is still a field: the token runs from the key to the end of the value.
{
  const source = "- [ ] Write due: 2026-11-05 priority:  2";
  const fields = named(source, "taskfield");
  assert.deepEqual(fields.map(token => source.slice(token.from, token.to)), ["due: 2026-11-05", "priority:  2"]);
  assert.deepEqual(fields.map(token => token.meta.value), ["2026-11-05", "2"]);
  assert.deepEqual(fields.map(token => token.meta.warning ?? null), [null, null]);
}

// A gap makes text a field only when the value is valid, so prose stays text.
{
  assert.deepEqual(named("- [ ] what is done: the tables", "taskfield"), []);
  assert.deepEqual(named("- [ ] Ship priority: 9", "taskfield"), []);
  // The same field without the gap is a field with a warning.
  const [field] = named("- [ ] Ship priority:9", "taskfield");
  assert.equal(field.meta.warning, "Priority must be 1 to 5.");
}

// A tag ends at punctuation, so the sentence's full stop is not part of it.
{
  const tags = named("- [ ] Ask about #paper.", "tasktag");
  assert.deepEqual(tags.map(token => token.meta.tag), ["paper"]);
}

// Nothing inside code, links, URLs, escapes, plain lines, paragraphs or a task's second line.
{
  const sources = [
    "- [ ] Read `due:2026-01-01 #code` and \\priority:1 with [site](https://x.org/#y) https://x.org/#z",
    "- plain bullet priority:1 #tag",
    "Paragraph with priority:1 and #tag.",
    "- [ ] Task\n  due:2026-01-01 #tag",
  ];
  for (const source of sources) {
    const found = tokensOf(source).filter(token => token.name === "taskfield" || token.name === "tasktag");
    assert.deepEqual(found, [], source);
  }
}

// A key written after an earlier field with a gap is a duplicate of it.
{
  const source = "- [ ] Task due: 2026-11-05 due:2026-11-06";
  const fields = named(source, "taskfield");
  assert.deepEqual(fields.map(token => token.meta.warning ?? null), [null, "Duplicate due."]);
}

// Warnings: an invalid value keeps its text and warns, and a repeated key warns after the first.
{
  const source = "- [ ] Task priority:1 priority:2 due:2026-02-30 start:2026-02-30 priority:9 unknown:1";
  const fields = named(source, "taskfield");
  assert.deepEqual(fields.map(token => token.meta.warning ?? null), [
    null,
    "Duplicate priority.",
    "due must be a real date as YYYY-MM-DD.",
    "start must be a real date as YYYY-MM-DD.",
    "Priority must be 1 to 5.",
  ]);
  assert.equal(fields[2].meta.value, "2026-02-30");
  assert.equal(fields.some(token => token.meta.key === "unknown"), false);
}

// Nested progress: done over the non-cancelled task descendants, and none without children.
{
  const source = "- [ ] Parent\n  - [x] one\n  - [ ] two\n  - [/] three\n  - [-] four\n- [X] Sibling\n";
  const [parent, one, two, three, four, sibling] = items(source);
  assert.equal(parent.meta.progress, "1/3");
  assert.equal(one.meta.progress, undefined);
  assert.equal(two.meta.progress, undefined);
  assert.equal(three.meta.progress, undefined);
  assert.equal(four.meta.progress, undefined);
  assert.equal(sibling.meta.progress, undefined);
}

// A parent whose only children are cancelled has no progress to show.
{
  const [parent] = items("- [ ] Parent\n  - [-] gone\n");
  assert.equal(parent.meta.progress, undefined);
}
