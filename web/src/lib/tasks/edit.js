// Edits to a task's source, as CodeMirror change specs. Each edit writes the
// smallest change it can, so the rest of the line keeps its exact text.

import { addRepeat } from "./syntax.js";
import { fieldOf } from "./model.js";

// Dates that move forward by one repeat interval when a task recurs.
const ADVANCED = ["due", "scheduled", "start"];
// Fields the completed copy drops.
const DROPPED = ["done", "cancelled"];

/** @typedef {{ from: number, to: number, insert: string }} Change */

/** Replaces the box character only. */
export function setState(task, char) {
  return [{ from: task.boxAt, to: task.boxAt + 1, insert: char }];
}

/**
 * Completes a todo, or reopens a done task. A todo with a valid repeat rule
 * also writes the next copy. Other states are left alone.
 * @returns {Change[]}
 */
export function toggle(source, task, today) {
  if (task.state === "todo") {
    const repeat = fieldOf(task, "repeat");
    if (repeat && repeat.warning === null) return completeRecurring(source, task, today);
    return setState(task, "x");
  }
  if (task.state === "done") return setState(task, " ");
  return [];
}

/** Replaces the value of the first field with this key, or adds the field at the end of the first line. */
export function setField(task, key, value) {
  const field = fieldOf(task, key);
  if (field) return [{ from: field.valueFrom, to: field.to, insert: value }];
  return [{ from: task.insertAt, to: task.insertAt, insert: ` ${key}:${value}` }];
}

/** Removes the first field with this key and the spaces or tabs before it. */
export function removeField(task, key) {
  const field = fieldOf(task, key);
  return field ? [cutWithSpace(task.source, task, field)] : [];
}

/** Adds a tag at the end of the first line. */
export function addTag(task, name) {
  return [{ from: task.insertAt, to: task.insertAt, insert: ` #${name}` }];
}

/** Removes the first tag with this name and the spaces or tabs before it. */
export function removeTag(task, name) {
  const tag = task.tags.find(candidate => candidate.name === name);
  return tag ? [cutWithSpace(task.source, task, tag)] : [];
}

/**
 * Marks the task done and inserts its next copy after the task's block. The
 * copy is the first line with its box cleared, its dates advanced by the
 * repeat rule, done and cancelled removed, and a fresh id.
 * The copy advances from its own dates, so `today` only keeps the signature
 * shared with toggle.
 * @returns {Change[]}
 */
export function completeRecurring(source, task, today) {
  const repeat = fieldOf(task, "repeat");
  const rule = repeat && repeat.warning === null ? repeat.value : null;

  /** @type {Change[]} */
  const edits = [];
  for (const field of task.fields) {
    if (rule && ADVANCED.includes(field.key)) {
      // An invalid date is left as written, so addRepeat returning null keeps it.
      const next = addRepeat(field.value, rule);
      if (next) edits.push({ from: field.valueFrom, to: field.to, insert: next });
    } else if (DROPPED.includes(field.key)) {
      edits.push(cutWithSpace(source, task, field));
    }
  }
  const id = fieldOf(task, "id");
  if (id && id.warning === null) {
    edits.push({ from: id.valueFrom, to: id.to, insert: freshId(source, id.value) });
  }

  const copy = applyEdits(source.slice(task.from, task.lineEnd), task.from, [
    { from: task.boxAt, to: task.boxAt + 1, insert: " " },
    ...edits,
  ]);
  return [
    { from: task.boxAt, to: task.boxAt + 1, insert: "x" },
    { from: task.blockEnd, to: task.blockEnd, insert: `\n${copy}` },
  ];
}

// The token and the run of spaces or tabs written before it. The run cannot
// reach the box, because the closing bracket stops it.
function cutWithSpace(source, task, token) {
  let from = token.from;
  while (from > task.from && (source[from - 1] === " " || source[from - 1] === "\t")) from--;
  return { from, to: token.to, insert: "" };
}

// Applies edits given as offsets in the source to the text that starts at
// `base`. Edits go right to left, so earlier offsets stay valid.
function applyEdits(text, base, edits) {
  return [...edits]
    .sort((a, b) => b.from - a.from)
    .reduce((out, edit) => out.slice(0, edit.from - base) + edit.insert + out.slice(edit.to - base), text);
}

// The id with any trailing -N removed, then the first -2, -3, ... that no id
// in the source already uses at a whitespace boundary.
function freshId(source, value) {
  const base = value.replace(/-\d+$/, "");
  for (let n = 2; ; n++) {
    const candidate = `${base}-${n}`;
    if (!idInSource(source, candidate)) return candidate;
  }
}

function idInSource(source, id) {
  const escaped = id.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  return new RegExp(`(?:^|\\s)id:${escaped}(?=\\s|$)`).test(source);
}
