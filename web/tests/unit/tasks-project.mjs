import assert from "node:assert/strict";
import { filterTasks, projectTasks, sortTasks, taskVocabulary } from "../../src/lib/tasks/project.js";

const TODAY = "2026-10-30";
const texts = tasks => tasks.map(task => task.text);

// Only Markdown and Quarto files contribute tasks, in path order.
{
  const tasks = projectTasks({
    "notes.typ": "- [ ] Not a task\n",
    "b.qmd": "- [ ] Second\n",
    "a.md": "- [ ] First\n",
    "c.MARKDOWN": "- [ ] Third\n",
  });
  assert.deepEqual(tasks.map(task => task.path), ["a.md", "b.qmd", "c.MARKDOWN"]);
}

// An after id with no task is a warning on the dependent task.
{
  const tasks = projectTasks({ "a.md": "- [ ] One id:a after:nope\n" });
  assert.deepEqual(tasks[0].warnings, ["No task has id nope."]);
  assert.deepEqual(tasks[0].blockedBy, []);
}

// An id used in two files warns on both tasks.
{
  const tasks = projectTasks({
    "b.md": "- [ ] Two id:x\n",
    "a.md": "- [ ] One id:x\n",
  });
  assert.deepEqual(tasks[0].warnings, ["Duplicate id x."]);
  assert.deepEqual(tasks[1].warnings, ["Duplicate id x."]);
}

// A two-task cycle warns on each task once, and a task outside the cycle does not.
{
  const tasks = projectTasks({
    "plan.md": "- [ ] A id:a after:b\n- [ ] B id:b after:a\n- [ ] C id:c after:a\n",
  });
  assert.deepEqual(tasks[0].warnings, ["Dependency cycle through a."]);
  assert.deepEqual(tasks[1].warnings, ["Dependency cycle through b."]);
  assert.deepEqual(tasks[2].warnings, []);
}

// A dependency is blocking while its task is open, and not once it is done or cancelled.
{
  const tasks = projectTasks({
    "plan.md": "- [ ] A id:a\n- [ ] B after:a\n- [x] C id:c\n- [ ] D after:c,a\n- [-] E id:e\n- [ ] F after:e\n",
  });
  assert.deepEqual(tasks[1].blockedBy, ["a"]);
  assert.deepEqual(tasks[3].blockedBy, ["a"]);
  assert.deepEqual(tasks[5].blockedBy, []);
}

// A field warning carries over to the project task.
{
  const tasks = projectTasks({ "plan.md": "- [ ] Bad priority:9\n" });
  assert.equal(tasks[0].warnings.length, 1);
  assert.equal(tasks[0].priority, 3);
}

// A tag filter matches the tag and its descendants, not a longer name.
{
  const tasks = projectTasks({
    "plan.md": "- [ ] One #paper/intro\n- [ ] Two #paper\n- [ ] Three #papers\n- [ ] Four #other\n",
  });
  assert.deepEqual(texts(filterTasks(tasks, { tag: "paper" }, TODAY)), ["One", "Two"]);
}

// Overdue keeps only open tasks past their due date, and criteria combine.
{
  const tasks = projectTasks({
    "plan.md": "- [ ] Late due:2026-10-01\n- [x] Finished due:2026-10-01\n- [ ] Today due:2026-10-30\n- [ ] Future due:2026-12-01\n- [ ] Undated\n",
  });
  assert.deepEqual(texts(filterTasks(tasks, { due: "overdue" }, TODAY)), ["Late"]);
  assert.deepEqual(texts(filterTasks(tasks, { due: "overdue", states: ["done"] }, TODAY)), []);
  assert.deepEqual(texts(filterTasks(tasks, { due: "today" }, TODAY)), ["Today"]);
  assert.deepEqual(texts(filterTasks(tasks, { due: "week" }, TODAY)), ["Today"]);
  assert.deepEqual(texts(filterTasks(tasks, { due: "none" }, TODAY)), ["Undated"]);
  assert.deepEqual(texts(filterTasks(tasks, {}, TODAY)).length, 5);
}

// The blocked filter keeps blocked tasks and tasks with open dependencies.
{
  const tasks = projectTasks({ "plan.md": "- [ ] A id:a\n- [ ] B after:a\n- [?] Stuck\n" });
  assert.deepEqual(texts(filterTasks(tasks, { blocked: true }, TODAY)), ["B", "Stuck"]);
}

// A query matches text or path, ignoring case.
{
  const tasks = projectTasks({ "plan.md": "- [ ] Late report\n- [ ] Other\n" });
  assert.deepEqual(texts(filterTasks(tasks, { query: "LATE" }, TODAY)), ["Late report"]);
  assert.deepEqual(texts(filterTasks(tasks, { query: "plan" }, TODAY)), ["Late report", "Other"]);
}

// Priority sorts ascending with equal priorities in document order, and the default is 3.
{
  const tasks = projectTasks({
    "plan.md": "- [ ] A priority:2\n- [ ] B priority:2\n- [ ] C priority:1\n- [ ] D\n",
  });
  assert.deepEqual(texts(sortTasks(tasks, "priority")), ["C", "A", "B", "D"]);
}

// Due dates sort ascending, ties keep document order, and missing dates go last.
{
  const tasks = projectTasks({
    "plan.md": "- [ ] Late due:2026-10-01\n- [ ] Nodate\n- [ ] Early due:2026-10-02\n- [ ] Also due:2026-10-01\n",
  });
  assert.deepEqual(texts(sortTasks(tasks, "due")), ["Late", "Also", "Early", "Nodate"]);
}

// Created dates sort ascending with missing dates last.
{
  const tasks = projectTasks({
    "plan.md": "- [ ] New created:2026-10-30\n- [ ] Nothing\n- [ ] Old created:2026-10-01\n",
  });
  assert.deepEqual(texts(sortTasks(tasks, "created")), ["Old", "New", "Nothing"]);
}

// Document order is path then position, whatever order the tasks arrive in.
{
  const tasks = projectTasks({
    "b.md": "- [ ] B2\n",
    "a.md": "- [ ] A1\n- [ ] A2\n",
  });
  assert.deepEqual(texts(sortTasks([...tasks].reverse(), "document")), ["A1", "A2", "B2"]);
}

// The vocabulary lists every assignee and tag once, sorted.
{
  const tasks = projectTasks({
    "plan.md": "- [ ] A assignee:alice,bob #paper/intro\n- [ ] B assignee:bob #paper\n",
  });
  assert.deepEqual(taskVocabulary(tasks), { assignees: ["alice", "bob"], tags: ["paper", "paper/intro"] });
  assert.deepEqual(texts(filterTasks(tasks, { assignee: "bob" }, TODAY)), ["A", "B"]);
}
