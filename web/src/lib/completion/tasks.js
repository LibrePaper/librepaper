// Completion for task items in Markdown and Quarto sources: field names, field
// values, and tags. Only the first line of a task item is completed, and never
// inside inline code, since a tag or field there is not read as metadata.
import { readTasks } from "../tasks/model.js";
import { DATE_FIELDS, FIELDS, PRIORITIES, TASK_LINE, todayISO } from "../tasks/syntax.js";

const MARKDOWN = /\.(?:md|markdown|qmd)$/i;
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
  estimate: "2h, 30m, 1d",
};
// The word before the caret: a space or tab, then the text up to the caret.
const WORD = /[ \t]([^ \t]*)$/;
// A field key before the caret, then any spaces or tabs, then the value typed so far.
const FIELD_VALUE = /[ \t]([a-z]+):([ \t]*)([^ \t]*)$/;
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

// The ISO date `days` after `iso`, counted in whole calendar days.
function addDays(iso, days) {
  const [year, month, day] = iso.split("-").map(Number);
  return new Date(Date.UTC(year, month - 1, day + days)).toISOString().slice(0, 10);
}

// Today, tomorrow and the next Monday, as ISO dates. A Monday today gives the
// Monday a week on, so the suggestion is always later than today.
function dateOptions(today) {
  const [year, month, day] = today.split("-").map(Number);
  const weekday = new Date(Date.UTC(year, month - 1, day)).getUTCDay();
  const toMonday = ((1 - weekday + 7) % 7) || 7;
  return [
    { label: today, type: "constant", detail: "today" },
    { label: addDays(today, 1), type: "constant", detail: "tomorrow" },
    { label: addDays(today, toMonday), type: "constant", detail: "next Monday" },
  ];
}

// The editor's text with the caret's line left out.
function otherLines(context) {
  const line = context.state.doc.lineAt(context.pos);
  const text = context.state.doc.toString();
  return text.slice(0, line.from) + text.slice(line.to);
}

// Every assignee and tag in the project's Markdown texts and in the open document, sorted and unique.
// The open document is read from the editor without the line being typed, so a half-typed name
// is not offered back; its copy in the project texts is skipped for the same reason.
function vocabulary(current, doc) {
  const texts = [doc, ...Object.entries(current.texts)
    .filter(([path]) => MARKDOWN.test(path) && path !== current.path)
    .map(([, text]) => text)];
  const assignees = new Set();
  const tags = new Set();
  for (const text of texts) {
    for (const task of readTasks(text)) {
      for (const field of task.fields) {
        if (field.key !== "assignee") continue;
        for (const name of field.value.split(",")) {
          if (name) assignees.add(name);
        }
      }
      for (const tag of task.tags) tags.add(tag.name);
    }
  }
  return { assignees: [...assignees].sort(), tags: [...tags].sort() };
}

function fieldNameOptions(pos, partial) {
  const wanted = partial.toLowerCase();
  const names = FIELDS.filter((name) => name.startsWith(wanted));
  // The name is applied with its space, so the value can be typed straight after it.
  const options = names.map((name) => ({
    label: `${name}:`,
    apply: `${name}: `,
    type: "property",
    detail: FIELD_DETAIL[name],
  }));
  return result(pos - partial.length, /^[A-Za-z]*$/, options);
}

// The value starts after the colon and any gap, so the completion starts at the value, whatever the gap.
function fieldValueOptions(pos, current, doc, key, value) {
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
  if (key === "assignee") {
    const partial = lastItem(value);
    const { assignees } = vocabulary(current, doc);
    const options = assignees
      .filter((assignee) => assignee.startsWith(partial))
      .map((assignee) => ({ label: assignee, type: "variable", detail: "" }));
    return result(pos - partial.length, /^[A-Za-z0-9_.-]*$/, options);
  }
  return null;
}

// A tag typed after "#". The "#" is already in the document, so the completion
// starts after it and applies the bare tag name.
function tagOptions(pos, current, doc, partial) {
  if (!TAG_PARTIAL.test(partial)) return null;
  const { tags } = vocabulary(current, doc);
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

    const rest = before.slice(box);
    const field = rest.match(FIELD_VALUE);
    if (field) return fieldValueOptions(context.pos, current, otherLines(context), field[1], field[3]);

    const word = rest.match(WORD);
    if (!word) return null;
    const text = word[1];
    if (text.startsWith("#")) return tagOptions(context.pos, current, otherLines(context), text.slice(1));
    if (FIELD_NAME.test(text)) return fieldNameOptions(context.pos, text);
    return null;
  };
}
