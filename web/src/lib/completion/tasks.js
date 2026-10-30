// Completion for task items in Markdown and Quarto sources: field names, field
// values, and tags. Only the first line of a task item is completed, and never
// inside inline code, since a tag or field there is not read as metadata.
import { projectTasks, taskVocabulary } from "../tasks/project.js";
import { addRepeat, DATE_FIELDS, FIELDS, PRIORITIES, TASK_LINE, todayISO } from "../tasks/syntax.js";

// What each field takes, shown beside its name.
const FIELD_DETAIL = {
  priority: "1 to 5",
  due: "date",
  start: "date",
  scheduled: "date",
  created: "date",
  done: "date",
  cancelled: "date",
  assignee: "handles",
  repeat: "daily, weekly, 2w",
  id: "id",
  after: "ids",
  estimate: "2h, 30m, 1d",
};
const REPEATS = ["daily", "weekly", "monthly", "yearly"];
// The word before the caret: a space or tab, then the text up to the caret.
const WORD = /[ \t]([^ \t]*)$/;
const FIELD_WORD = /^([a-z]+):(.*)$/;
const FIELD_NAME = /^[A-Za-z]+$/;
const TAG_PARTIAL = /^(?:[A-Za-z][A-Za-z0-9_/-]*)?$/;

// The completion result, or null when no option matches.
function result(from, validFor, options) {
  return options.length ? { from, validFor, options } : null;
}

// The part of a comma list the caret is in, after the last comma.
function lastItem(value) {
  return value.slice(value.lastIndexOf(",") + 1);
}

// Today, tomorrow and the next Monday, as ISO dates. A Monday today gives the
// Monday a week on, so the suggestion is always later than today.
function dateOptions(today) {
  const [year, month, day] = today.split("-").map(Number);
  const weekday = new Date(Date.UTC(year, month - 1, day)).getUTCDay();
  const toMonday = ((1 - weekday + 7) % 7) || 7;
  return [
    { label: today, type: "constant", detail: "today" },
    { label: addRepeat(today, "1d"), type: "constant", detail: "tomorrow" },
    { label: addRepeat(today, `${toMonday}d`), type: "constant", detail: "next Monday" },
  ];
}

function fieldNameOptions(pos, partial) {
  const wanted = partial.toLowerCase();
  const names = FIELDS.filter((name) => name.startsWith(wanted));
  const options = names.map((name) => ({ label: `${name}:`, type: "property", detail: FIELD_DETAIL[name] }));
  return result(pos - partial.length, /^[A-Za-z]*$/, options);
}

function fieldValueOptions(pos, current, key, value) {
  if (!FIELDS.includes(key)) return null;
  if (key === "priority") {
    const options = PRIORITIES
      .filter((priority) => String(priority.value).startsWith(value))
      .map((priority) => ({ label: String(priority.value), type: "constant", detail: priority.label }));
    return result(pos - value.length, /^[0-9]*$/, options);
  }
  if (DATE_FIELDS.includes(key)) {
    const options = dateOptions(todayISO()).filter((option) => option.label.startsWith(value));
    return result(pos - value.length, /^[0-9-]*$/, options);
  }
  if (key === "repeat") {
    const options = REPEATS
      .filter((rule) => rule.startsWith(value))
      .map((rule) => ({ label: rule, type: "constant", detail: "" }));
    return result(pos - value.length, /^[a-z]*$/, options);
  }
  if (key === "assignee") {
    const partial = lastItem(value);
    const { assignees } = taskVocabulary(projectTasks(current.texts));
    const options = assignees
      .filter((assignee) => assignee.startsWith(partial))
      .map((assignee) => ({ label: assignee, type: "variable", detail: "" }));
    return result(pos - partial.length, /^[A-Za-z0-9_.-]*$/, options);
  }
  if (key === "after") {
    const partial = lastItem(value);
    // Several tasks may share an id, so each one is offered once.
    const ids = new Set();
    for (const task of projectTasks(current.texts)) {
      for (const field of task.fields) {
        if (field.key === "id" && field.value) ids.add(field.value);
      }
    }
    const options = [...ids]
      .filter((id) => id.startsWith(partial))
      .map((id) => ({ label: id, type: "variable", detail: "" }));
    return result(pos - partial.length, /^[A-Za-z0-9_-]*$/, options);
  }
  return null;
}

// A tag typed after "#". The "#" is already in the document, so the completion
// starts after it and applies the bare tag name.
function tagOptions(pos, current, partial) {
  if (!TAG_PARTIAL.test(partial)) return null;
  const { tags } = taskVocabulary(projectTasks(current.texts));
  const options = tags
    .filter((tag) => tag.startsWith(partial))
    .map((tag) => ({ label: tag, type: "keyword", detail: "" }));
  return result(pos - partial.length, /^[A-Za-z0-9_/-]*$/, options);
}

/** Completion source for task fields and tags. Returns null outside Markdown and Quarto task items. */
export function taskCompletion(project) {
  return (context) => {
    const current = project();
    if (current.format !== "markdown" && current.format !== "quarto") return null;
    const line = context.state.doc.lineAt(context.pos);
    const before = line.text.slice(0, context.pos - line.from);
    const head = line.text.match(TASK_LINE);
    if (!head) return null;
    // The caret must be past the box, so the box itself is never completed.
    const box = head[0].length;
    if (before.length < box) return null;
    // An odd number of backticks means the caret is inside inline code.
    if ((before.match(/`/g) || []).length % 2) return null;

    const word = before.slice(box).match(WORD);
    if (!word) return null;
    const text = word[1];
    if (text.startsWith("#")) return tagOptions(context.pos, current, text.slice(1));
    const field = text.match(FIELD_WORD);
    if (field) return fieldValueOptions(context.pos, current, field[1], field[2]);
    if (FIELD_NAME.test(text)) return fieldNameOptions(context.pos, text);
    return null;
  };
}
