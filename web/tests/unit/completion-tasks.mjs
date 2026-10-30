// Task completion: field names, field values, and tags on a task's first line,
// run against a real EditorState with a fake project accessor.
import assert from "node:assert/strict";
import { EditorState } from "@codemirror/state";
import { CompletionContext } from "@codemirror/autocomplete";
import { taskCompletion } from "../../src/lib/completion/tasks.js";
import { todayISO } from "../../src/lib/tasks/syntax.js";

// The "|" marks the cursor. Returns the source's result for that position.
function complete(doc, { path = "tasks.md", format = "markdown", texts = {} } = {}) {
  const pos = doc.indexOf("|");
  const text = doc.slice(0, pos) + doc.slice(pos + 1);
  const state = EditorState.create({ doc: text });
  const source = taskCompletion(() => ({ path, format, texts, files: Object.keys(texts) }));
  return { result: source(new CompletionContext(state, pos, false)), pos };
}
const labels = (result) => (result ? result.options.map((option) => option.label) : null);

// Dates for the date options, computed in UTC so the local offset does not matter.
const DAY = 86400000;
const today = todayISO();
const [year, month, day] = today.split("-").map(Number);
const todayUTC = Date.UTC(year, month - 1, day);
const isoOf = (time) => new Date(time).toISOString().slice(0, 10);
const weekday = new Date(todayUTC).getUTCDay();
const nextMonday = isoOf(todayUTC + (((1 - weekday + 7) % 7) || 7) * DAY);

// --- Field names -------------------------------------------------------------
// A partial name offers the fields it starts with, applied with the colon.
{
  const { result, pos } = complete("- [ ] pri|");
  assert.deepEqual(labels(result), ["priority:"]);
  assert.equal(result.from, pos - "pri".length);
  assert.ok(result.validFor.test("pri"));
  assert.equal(result.options[0].detail, "1 to 5");
  // The name is applied with its space, so the value is typed straight after it.
  assert.equal(result.options[0].apply, "priority: ");
}
// Several fields share a prefix, in field order.
assert.deepEqual(labels(complete("- [ ] d|").result), ["due:", "done:"]);
// A name is completed after an earlier field on the line.
assert.deepEqual(labels(complete("- [ ] write due:2026-01-02 pri|").result), ["priority:"]);
// Numbered items are task items too.
assert.deepEqual(labels(complete("1. [ ] pri|").result), ["priority:"]);
// A name that starts no field gives nothing.
assert.equal(complete("- [ ] zz|").result, null);
// The caret must be past the box, so the box itself is never completed.
assert.equal(complete("- [ ]|").result, null);
assert.equal(complete("- [ ] |").result, null);

// --- Priority ----------------------------------------------------------------
{
  const { result, pos } = complete("- [ ] task priority:|");
  assert.deepEqual(labels(result), ["1", "2", "3", "4", "5"]);
  assert.equal(result.options[0].detail, "Critical");
  assert.equal(result.from, pos);
}
assert.deepEqual(labels(complete("- [ ] priority:2|").result), ["2"]);

// --- Dates -------------------------------------------------------------------
{
  const { result, pos } = complete("- [ ] due:|");
  assert.deepEqual(labels(result), [today, isoOf(todayUTC + DAY), nextMonday]);
  assert.deepEqual(result.options.map((option) => option.detail), ["today", "tomorrow", "next Monday"]);
  assert.equal(result.from, pos);
}
// After a gap the value starts at the first character typed, so the date replaces only the value.
{
  const { result, pos } = complete(`- [ ] due: ${today.slice(0, 4)}|`);
  assert.ok(labels(result).includes(today));
  assert.equal(result.from, pos - 4);
}
{
  const { result, pos } = complete("- [ ] priority:  2|");
  assert.deepEqual(labels(result), ["2"]);
  assert.equal(result.from, pos - 1);
}
// A gapped key with text that is not a value gives nothing, so prose stays unchanged.
assert.equal(complete("- [ ] what is done: the|").result, null);

// Typed digits narrow the dates.
assert.deepEqual(labels(complete(`- [ ] start:${today}|`).result), [today]);
// Every date field gets the same options.
assert.deepEqual(labels(complete("- [ ] scheduled:|").result), [today, isoOf(todayUTC + DAY), nextMonday]);
// Estimates have no completions.
assert.equal(complete("- [ ] estimate:|").result, null);

// --- Assignees ---------------------------------------------------------------
const people = { "people.md": "- [ ] a assignee:alice,bob\n- [ ] b assignee:carol" };
assert.deepEqual(labels(complete("- [ ] c assignee:|", { texts: people }).result), ["alice", "bob", "carol"]);
{
  // After a comma only the last handle is matched.
  const { result, pos } = complete("- [ ] c assignee:alice,c|", { texts: people });
  assert.deepEqual(labels(result), ["carol"]);
  assert.equal(result.from, pos - 1);
}

// --- Tags --------------------------------------------------------------------
const writing = { "tasks.md": "- [ ] a #writing #paper/intro\n- [ ] b #writing" };
{
  const { result, pos } = complete("- [ ] c #|", { texts: writing });
  assert.deepEqual(labels(result), ["paper/intro", "writing"]);
  assert.equal(result.from, pos);
}
{
  // The "#" stays in the document, so the completion starts after it.
  const { result, pos } = complete("- [ ] c #pa|", { texts: writing });
  assert.deepEqual(labels(result), ["paper/intro"]);
  assert.equal(result.from, pos - 2);
}
// Tags in the open document count too, even when no other file has them.
assert.deepEqual(labels(complete("- [ ] c #|\n- [ ] d #local", { texts: writing }).result), ["local", "paper/intro", "writing"]);

// A tag must start after a space, so a "#" inside a word is not a tag.
assert.equal(complete("- [ ] c x#w|", { texts: writing }).result, null);
// Digits cannot start a tag.
assert.equal(complete("- [ ] c #1|", { texts: writing }).result, null);

// --- Nulls -------------------------------------------------------------------
// A plain bullet is not a task.
assert.equal(complete("- pri|").result, null);
// Neither is a paragraph.
assert.equal(complete("pri|").result, null);
assert.equal(complete("Some text #wr|", { texts: writing }).result, null);
// Inside inline code on the task line.
assert.equal(complete("- [ ] `code pri|").result, null);
assert.equal(complete("- [ ] `code #w|", { texts: writing }).result, null);
// After the code span closes, completion works again.
assert.deepEqual(labels(complete("- [ ] `x` #w|", { texts: writing }).result), ["writing"]);
// LaTeX sources never complete tasks.
assert.equal(complete("- [ ] pri|", { format: "latex" }).result, null);
assert.equal(complete("- [ ] c #w|", { format: "latex", texts: writing }).result, null);
// Quarto files get the same contexts.
assert.deepEqual(labels(complete("- [ ] pri|", { format: "quarto", path: "a.qmd" }).result), ["priority:"]);
