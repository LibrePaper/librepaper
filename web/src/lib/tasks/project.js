// Tasks across a project: cross-file checks, filters, sorts and the vocabulary
// the editor suggests from.

import { addRepeat, isDate, isOverdue } from "./syntax.js";
import { fieldOf, readTasks } from "./model.js";

const MARKDOWN = /\.(?:md|markdown|qmd)$/i;
const DEFAULT_PRIORITY = 3;
const CLOSED = new Set(["done", "cancelled"]);

/**
 * @typedef {import("./model.js").Task & {
 *   path: string,
 *   priority: number,
 *   due: string|null,
 *   warnings: string[],
 *   blockedBy: string[],
 * }} ProjectTask
 */

/** Tasks of every Markdown file (.md, .markdown, .qmd) in `texts` ({ path: text }), files in path order. */
export function projectTasks(texts) {
  const paths = Object.keys(texts).filter(path => MARKDOWN.test(path)).sort();
  /** @type {ProjectTask[]} */
  const tasks = [];
  for (const path of paths) {
    for (const task of readTasks(texts[path])) {
      tasks.push({
        ...task,
        path,
        priority: priorityOf(task),
        due: validDate(task, "due"),
        warnings: task.fields.flatMap(field => (field.warning === null ? [] : [field.warning])),
        blockedBy: [],
      });
    }
  }
  linkDependencies(tasks);
  return tasks;
}

/**
 * Keeps the tasks that meet every criterion given. An empty or missing
 * `states` keeps every state. A tag matches its descendants too.
 * @param {{ path?: string, states?: string[], priority?: number, assignee?: string, tag?: string, due?: "overdue"|"today"|"week"|"none"|"", blocked?: boolean, query?: string }} filter
 */
export function filterTasks(tasks, filter, today) {
  return tasks.filter(task => matches(task, filter, today));
}

/** by: "document" | "priority" | "due" | "created". Stable; ties keep document order; missing dates last. */
export function sortTasks(tasks, by) {
  const order = Object.hasOwn(ORDERS, by) ? ORDERS[by] : ORDERS.document;
  return [...tasks].sort((a, b) => order(a, b) || documentOrder(a, b));
}

/** Every assignee and every tag used, sorted and unique. */
export function taskVocabulary(tasks) {
  const assignees = new Set();
  const tags = new Set();
  for (const task of tasks) {
    for (const name of assigneesOf(task)) assignees.add(name);
    for (const tag of task.tags) tags.add(tag.name);
  }
  return { assignees: [...assignees].sort(), tags: [...tags].sort() };
}

// Adds the cross-file warnings and blockedBy, in place.
function linkDependencies(tasks) {
  /** @type {Map<string, ProjectTask[]>} */
  const byId = new Map();
  for (const task of tasks) {
    const id = idOf(task);
    if (id === null) continue;
    if (!byId.has(id)) byId.set(id, []);
    byId.get(id).push(task);
  }
  for (const [id, group] of byId) {
    if (group.length > 1) {
      for (const task of group) task.warnings.push(`Duplicate id ${id}.`);
    }
  }

  // The dependency graph has one node per id, so a cycle is found once per id.
  /** @type {Map<string, Set<string>>} */
  const edges = new Map([...byId.keys()].map(id => [id, new Set()]));
  for (const task of tasks) {
    const own = idOf(task);
    for (const dep of afterOf(task)) {
      const group = byId.get(dep);
      if (!group) {
        task.warnings.push(`No task has id ${dep}.`);
        continue;
      }
      if (group.some(other => !CLOSED.has(other.state))) task.blockedBy.push(dep);
      if (own !== null) edges.get(own).add(dep);
    }
  }

  const cyclic = idsOnCycles(edges);
  for (const task of tasks) {
    const own = idOf(task);
    if (own !== null && cyclic.has(own)) task.warnings.push(`Dependency cycle through ${own}.`);
  }
}

// Every id that lies on a cycle, found by depth-first search with a path stack.
function idsOnCycles(edges) {
  /** @type {Map<string, 1|2>} */
  const state = new Map();
  const path = [];
  const onCycle = new Set();
  const visit = id => {
    state.set(id, 1);
    path.push(id);
    for (const next of edges.get(id)) {
      const seen = state.get(next);
      if (seen === 1) {
        for (let i = path.indexOf(next); i < path.length; i++) onCycle.add(path[i]);
      } else if (seen === undefined) {
        visit(next);
      }
    }
    path.pop();
    state.set(id, 2);
  };
  for (const id of edges.keys()) {
    if (state.get(id) === undefined) visit(id);
  }
  return onCycle;
}

function matches(task, filter, today) {
  if (filter.path && task.path !== filter.path) return false;
  if (filter.states?.length && !filter.states.includes(task.state)) return false;
  if (typeof filter.priority === "number" && task.priority !== filter.priority) return false;
  if (filter.assignee && !assigneesOf(task).includes(filter.assignee)) return false;
  if (filter.tag && !hasTag(task, filter.tag)) return false;
  if (filter.due && !dueMatches(task, filter.due, today)) return false;
  if (filter.blocked === true && task.blockedBy.length === 0 && task.state !== "blocked") return false;
  const query = filter.query?.trim().toLowerCase();
  if (query && !task.text.toLowerCase().includes(query) && !task.path.toLowerCase().includes(query)) return false;
  return true;
}

function hasTag(task, tag) {
  const wanted = tag.replace(/^#/, "");
  return task.tags.some(entry => entry.name === wanted || entry.name.startsWith(`${wanted}/`));
}

function dueMatches(task, due, today) {
  switch (due) {
    case "overdue":
      return isOverdue(task.state, task.due, today);
    case "today":
      return task.due === today;
    case "week": {
      const end = addRepeat(today, "6d");
      return end !== null && task.due !== null && task.due >= today && task.due <= end;
    }
    case "none":
      return task.due === null;
    default:
      return true;
  }
}

const ORDERS = {
  priority: (a, b) => a.priority - b.priority,
  due: (a, b) => compareLast(a.due, b.due),
  created: (a, b) => compareLast(validDate(a, "created"), validDate(b, "created")),
  document: () => 0,
};

function documentOrder(a, b) {
  if (a.path !== b.path) return a.path < b.path ? -1 : 1;
  return a.index - b.index;
}

// Ascending, with a missing value after every present one.
function compareLast(x, y) {
  if (x === y) return 0;
  if (x === null) return 1;
  if (y === null) return -1;
  return x < y ? -1 : 1;
}

function priorityOf(task) {
  const field = fieldOf(task, "priority");
  return field && field.warning === null ? Number(field.value) : DEFAULT_PRIORITY;
}

function validDate(task, key) {
  const field = fieldOf(task, key);
  return field && field.warning === null && isDate(field.value) ? field.value : null;
}

// An id counts only when its field is valid, so a bad id is never a target.
function idOf(task) {
  const field = fieldOf(task, "id");
  return field && field.warning === null ? field.value : null;
}

function afterOf(task) {
  const field = fieldOf(task, "after");
  return field ? field.value.split(",").filter(Boolean) : [];
}

function assigneesOf(task) {
  const field = fieldOf(task, "assignee");
  return field && field.warning === null ? field.value.split(",") : [];
}
