import assert from "node:assert/strict";
import { readTasks } from "../../src/lib/tasks/model.js";
import {
  addTag,
  completeRecurring,
  removeField,
  removeTag,
  setField,
  setState,
  toggle,
} from "../../src/lib/tasks/edit.js";

const TODAY = "2026-10-30";

// Applies change specs to a string, right to left, as CodeMirror does.
function applyChanges(source, changes) {
  return [...changes]
    .sort((a, b) => b.from - a.from)
    .reduce((text, change) => text.slice(0, change.from) + change.insert + text.slice(change.to), source);
}

function firstTask(source) {
  return readTasks(source)[0];
}

// Priority 2 to 1 writes one character.
{
  const source = "- [ ] Write intro priority:2 due:2026-11-05\n";
  const changes = setField(firstTask(source), "priority", "1");
  assert.equal(changes.length, 1);
  assert.equal(changes[0].to - changes[0].from, 1);
  assert.equal(applyChanges(source, changes), "- [ ] Write intro priority:1 due:2026-11-05\n");
}

// A missing due date goes at the end of the first line, before trailing spaces.
{
  const source = "- [ ] Write intro   \n";
  const changes = setField(firstTask(source), "due", "2026-11-05");
  assert.equal(applyChanges(source, changes), "- [ ] Write intro due:2026-11-05   \n");
}

// Removing the middle of three fields removes its token and the space before it.
{
  const source = "- [ ] Draft priority:2 due:2026-11-05 id:draft\n";
  assert.equal(applyChanges(source, removeField(firstTask(source), "due")), "- [ ] Draft priority:2 id:draft\n");
}

// Removing the field right after the box leaves one space after the box.
{
  const source = "- [ ] due:2026-11-05 Write\n";
  assert.equal(applyChanges(source, removeField(firstTask(source), "due")), "- [ ] Write\n");
}

// Done checks clear, and an uppercase done box clears too.
{
  const source = "- [X] Done thing\n";
  assert.equal(applyChanges(source, toggle(source, firstTask(source), TODAY)), "- [ ] Done thing\n");
}

// A doing task is not completed by a toggle.
{
  const source = "- [/] Half done\n";
  assert.deepEqual(toggle(source, firstTask(source), TODAY), []);
}

// Setting a state writes the box character only.
{
  const source = "- [ ] a\n";
  assert.equal(applyChanges(source, setState(firstTask(source), "?")), "- [?] a\n");
}

// Tags are added at the end of the first line and removed with the space before them.
{
  const source = "- [ ] Draft   \n";
  assert.equal(applyChanges(source, addTag(firstTask(source), "writing")), "- [ ] Draft #writing   \n");

  const tagged = "- [ ] Draft #writing #x\n";
  assert.equal(applyChanges(tagged, removeTag(firstTask(tagged), "writing")), "- [ ] Draft #x\n");
}

// A recurring task completes in place and a copy with the next due date follows its block.
{
  const source = [
    "- [ ] Review draft repeat:weekly due:2026-11-05 id:review-2 done:2026-11-04",
    "  - [ ] Check figures",
    "",
    "Next paragraph",
    "",
  ].join("\n");
  const expected = [
    "- [x] Review draft repeat:weekly due:2026-11-05 id:review-2 done:2026-11-04",
    "  - [ ] Check figures",
    "- [ ] Review draft repeat:weekly due:2026-11-12 id:review-3",
    "",
    "Next paragraph",
    "",
  ].join("\n");
  assert.equal(applyChanges(source, toggle(source, firstTask(source), TODAY)), expected);
}

// The fresh id skips ids already in the source.
{
  const source = [
    "- [ ] Review repeat:weekly due:2026-11-05 id:review-2 done:2026-11-04",
    "- [ ] Other id:review-3",
    "",
  ].join("\n");
  const expected = [
    "- [x] Review repeat:weekly due:2026-11-05 id:review-2 done:2026-11-04",
    "- [ ] Review repeat:weekly due:2026-11-12 id:review-4",
    "- [ ] Other id:review-3",
    "",
  ].join("\n");
  assert.equal(applyChanges(source, toggle(source, firstTask(source), TODAY)), expected);
}

// A monthly task on Jan 31 lands on Feb 28 in a non-leap year.
{
  const source = "- [ ] Rent repeat:1m due:2026-01-31\n";
  const expected = [
    "- [x] Rent repeat:1m due:2026-01-31",
    "- [ ] Rent repeat:1m due:2026-02-28",
    "",
  ].join("\n");
  assert.equal(applyChanges(source, toggle(source, firstTask(source), TODAY)), expected);
}

// A recurring task with no date is copied unchanged apart from its box and id.
{
  const source = "- [ ] Stretch repeat:daily id:stretch\n";
  const expected = [
    "- [x] Stretch repeat:daily id:stretch",
    "- [ ] Stretch repeat:daily id:stretch-2",
    "",
  ].join("\n");
  assert.equal(applyChanges(source, toggle(source, firstTask(source), TODAY)), expected);
}

// completeRecurring returns the two changes the toggle returns.
{
  const source = "- [ ] Stretch repeat:daily id:stretch\n";
  const task = firstTask(source);
  assert.deepEqual(completeRecurring(source, task, TODAY), toggle(source, task, TODAY));
}
