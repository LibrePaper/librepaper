// The tasks of a Markdown source, read through the same tokenizer the editor
// draws from, so the panel and the editor agree on what a task is.

import { Tokenizer } from "../visual-markup/core/tokenizer.ts";
import { markdown } from "../visual-markup/languages/markdown/index.ts";

/**
 * @typedef {{ key: string, value: string, from: number, to: number, valueFrom: number, warning: string|null }} TaskField
 * @typedef {{ name: string, from: number, to: number }} TaskTag
 * @typedef {{
 *   index: number,
 *   line: number,
 *   from: number,
 *   lineEnd: number,
 *   blockEnd: number,
 *   boxAt: number,
 *   box: string,
 *   state: string,
 *   indent: number,
 *   text: string,
 *   fields: TaskField[],
 *   tags: TaskTag[],
 *   insertAt: number,
 *   parent: number|null,
 *   children: number[],
 *   source: string,
 * }} Task
 */

/** Every task in a Markdown source, in document order. */
export function readTasks(source) {
  const tokens = flatten(new Tokenizer(markdown).tokenize(source));
  const starts = lineStarts(source);
  /** @type {Task[]} */
  const tasks = [];
  tokens.forEach((token, at) => {
    if (token.kind === "item" && token.meta?.task) {
      tasks.push(readTask(source, starts, tokens, at, tasks.length));
    }
  });
  linkParents(tasks);
  return tasks;
}

/** The task whose first line contains `pos`, or null. */
export function taskAt(source, pos) {
  return readTasks(source).find(task => pos >= task.from && pos <= task.lineEnd) ?? null;
}

/** The first field of a task with this key, or null. */
export function fieldOf(task, key) {
  return task.fields.find(field => field.key === key) ?? null;
}

// Tokens in document order, with the children of containers after their parent.
function flatten(tokens, out = []) {
  for (const token of tokens) {
    out.push(token);
    if (token.children) flatten(token.children, out);
  }
  return out;
}

function lineStarts(source) {
  const starts = [0];
  for (let i = source.indexOf("\n"); i !== -1; i = source.indexOf("\n", i + 1)) starts.push(i + 1);
  return starts;
}

// The index of the line that holds `pos`: the last start at or before it.
function lineIndexAt(starts, pos) {
  let low = 0;
  let high = starts.length - 1;
  while (low < high) {
    const mid = (low + high + 1) >> 1;
    if (starts[mid] <= pos) low = mid;
    else high = mid - 1;
  }
  return low;
}

function endOfLine(source, from) {
  const newline = source.indexOf("\n", from);
  return newline === -1 ? source.length : newline;
}

function readTask(source, starts, tokens, at, index) {
  const item = tokens[at];
  const line = lineIndexAt(starts, item.from);
  const from = starts[line];
  const lineEnd = endOfLine(source, from);
  const boxAt = item.to - 2;
  const indent = item.from - from;

  // Fields and tags sit on the first line, so the scan stops at the first token past it.
  const fields = [];
  const tags = [];
  for (let i = at + 1; i < tokens.length && tokens[i].from <= lineEnd; i++) {
    const token = tokens[i];
    if (token.kind !== "command") continue;
    if (token.name === "taskfield") {
      const key = token.meta?.key ?? "";
      fields.push({
        key,
        value: token.meta?.value ?? "",
        from: token.from,
        to: token.to,
        valueFrom: token.from + key.length + 1,
        warning: token.meta?.warning ?? null,
      });
    } else if (token.name === "tasktag") {
      tags.push({ name: token.meta?.tag ?? "", from: token.from, to: token.to });
    }
  }

  return {
    index,
    line: line + 1,
    from,
    lineEnd,
    blockEnd: blockEndOf(source, starts, line, indent, lineEnd),
    boxAt,
    box: item.meta?.box ?? source[boxAt],
    state: item.meta.task,
    indent,
    text: plainText(source, item.to, lineEnd, [...fields, ...tags]),
    fields,
    tags,
    // Trailing spaces stay after new metadata, and a CR of a CRLF line stays last.
    insertAt: insertionPoint(source, item.to, lineEnd),
    parent: null,
    children: [],
    source,
  };
}

// The first line's text with the metadata cut out. Cut ranges are in source order.
function plainText(source, from, to, cuts) {
  let text = "";
  let cursor = from;
  for (const cut of [...cuts].sort((a, b) => a.from - b.from)) {
    text += source.slice(cursor, cut.from);
    cursor = cut.to;
  }
  text += source.slice(cursor, to);
  return text.replace(/\s+/g, " ").trim();
}

// The last line of the task's block. Blank lines count only when a deeper
// line follows them, so trailing blank lines are left out.
function blockEndOf(source, starts, line, indent, lineEnd) {
  let end = lineEnd;
  for (let k = line + 1; k < starts.length; k++) {
    const stop = k + 1 < starts.length ? starts[k + 1] - 1 : source.length;
    const text = source.slice(starts[k], stop);
    if (text.trim() === "") continue;
    if (leadingWidth(text) <= indent) break;
    end = stop;
  }
  return end;
}

function leadingWidth(text) {
  return text.match(/^[ \t]*/)[0].length;
}

function insertionPoint(source, floor, lineEnd) {
  let at = lineEnd;
  while (at > floor && /[ \t\r]/.test(source[at - 1])) at--;
  return at;
}

// A task is a child of the nearest earlier task that is less indented and
// whose block still covers this one.
function linkParents(tasks) {
  /** @type {Task[]} */
  const open = [];
  for (const task of tasks) {
    while (open.length > 0) {
      const top = open[open.length - 1];
      if (top.blockEnd >= task.from && top.indent < task.indent) break;
      open.pop();
    }
    const parent = open[open.length - 1];
    if (parent) {
      task.parent = parent.index;
      parent.children.push(task.index);
    }
    open.push(task);
  }
}
