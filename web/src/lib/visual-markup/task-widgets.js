// The task widgets of the Markdown visual layer. A task's box is drawn over the source,
// and a click on it cycles the task's state. Its fields and tags are quiet chips that
// reveal the source when pressed, so the text is typed by hand. Every action reads the
// task from the source again when it runs, because the widget's position moves as the
// text around it changes.

import { WidgetType } from "@codemirror/view";
import "./tasks.css";
import { registerWidget, revealOnMousedown } from "./markup.ts";
import { taskAt } from "../tasks/model.js";
import { cycle } from "../tasks/edit.js";
import { isOverdue, PRIORITIES, shortDate, STATES, todayISO } from "../tasks/syntax.js";

const STATE_LABEL = Object.fromEntries(STATES.map((item) => [item.state, item.label]));
// The order a click moves through. The box cycles to the next state in this list.
const CYCLE = ["todo", "done", "doing", "blocked", "cancelled"];
const NEXT_LABEL = Object.fromEntries(CYCLE.map((state, index) => [state, STATE_LABEL[CYCLE[(index + 1) % CYCLE.length]]]));
// The value aria-checked takes for each box. A partly done task is "mixed".
const CHECKED = { done: "true", doing: "mixed" };

// The date fields, with the words a chip puts before the date and the name of the date.
const DATE_LABEL = {
  due: "Due date",
  start: "Start date",
  scheduled: "Scheduled date",
  created: "Created date",
  done: "Completed date",
  cancelled: "Cancelled date",
};
const DATE_PREFIX = { due: "", start: "start", scheduled: "sched.", created: "created", done: "done", cancelled: "cancelled" };

// Fields that are useful to read but should not compete with the task's text.
const QUIET_FIELDS = new Set(["created", "estimate"]);

// A DOM element with a class and text, in one call.
function el(tag, className = "", text = "") {
  const element = document.createElement(tag);
  if (className) element.className = className;
  if (text) element.textContent = text;
  return element;
}

// The task under a widget, read from the source as it is now, or null.
function currentTask(view, element) {
  const widget = element.closest(".cm-lv-widget");
  const pos = widget ? view.posAtDOM(widget) : -1;
  if (pos < 0) return null;
  return taskAt(view.state.doc.toString(), pos);
}

// Cycles the state of the task under the box and writes the change.
function cycleTask(view, button) {
  const task = currentTask(view, button);
  if (!task) return;
  const changes = cycle(task);
  if (changes.length) view.dispatch({ changes });
}

// The checkbox and its progress. The glyph is drawn by CSS, except the question mark,
// which is text so that a blocked task reads as blocked without the stylesheet.
function buildTaskBox(view, { task, progress }) {
  const root = el("span", "cm-lv-widget cm-lp-taskbox");
  const button = el("button", `cm-lp-taskbox-button cm-lp-taskbox-${task}`, task === "blocked" ? "?" : "");
  button.type = "button";
  button.setAttribute("role", "checkbox");
  button.setAttribute("aria-checked", CHECKED[task] ?? "false");
  button.setAttribute("aria-label", `Task, ${STATE_LABEL[task].toLowerCase()}. Click for ${NEXT_LABEL[task].toLowerCase()}.`);
  // Presses stay inside the box. The editor's keymap and caret must not act on them.
  button.addEventListener("mousedown", (event) => event.stopPropagation());
  button.addEventListener("keydown", (event) => event.stopPropagation());

  button.addEventListener("click", () => cycleTask(view, button));
  button.addEventListener("keydown", (event) => {
    if (event.key === " " || event.key === "Enter") {
      event.preventDefault();
      cycleTask(view, button);
    }
  });

  root.append(button);
  if (progress) root.append(el("span", "cm-lp-task-progress", progress.replace("/", " of ")));
  return root;
}

// ---- Fields ----------------------------------------------------------------

// The chip's text, its accessible name and its title for one field. A warning
// keeps the source as written, and says so in the name and the title.
function describeField(key, value, state, warning) {
  if (warning) return { text: `${key}:${value}`, name: `${key}:${value}. ${warning}`, title: warning };
  if (key === "priority") {
    const rank = PRIORITIES.find((item) => String(item.value) === value);
    if (rank) return { text: `P${rank.value}`, name: `Priority ${rank.value}, ${rank.label.toLowerCase()}` };
    return { text: value, name: `Priority ${value}` };
  }
  if (key in DATE_LABEL) {
    const short = shortDate(value);
    const overdue = key === "due" && isOverdue(state, value);
    const prefix = DATE_PREFIX[key];
    return {
      text: prefix ? `${prefix} ${short}` : short,
      name: `${DATE_LABEL[key]} ${short}${overdue ? ", overdue" : ""}`,
      overdue,
    };
  }
  const names = { assignee: "Assignee", estimate: "Estimate" };
  return { text: value, name: `${names[key]} ${value}` };
}

// A field chip. The chip shows the short form and its name says the whole field.
// A press is not stopped, so the caret moves to the chip and its source shows for typing.
function buildField(view, { key, value, state, warning }) {
  const described = describeField(key, value, state, warning);
  const rank = key === "priority" && !warning ? PRIORITIES.find((item) => String(item.value) === value) : undefined;
  const classes = ["cm-lv-widget", "cm-lp-field", `cm-lp-field-${key}`];
  if (QUIET_FIELDS.has(key)) classes.push("cm-lp-task-quiet");
  if (rank) classes.push(`cm-lp-field-p${rank.value}`);
  if (described.overdue) classes.push("cm-lp-task-overdue");
  if (warning) classes.push("cm-lp-task-invalid");

  const chip = el("span", classes.join(" "), described.text);
  chip.setAttribute("aria-label", described.name);
  if (described.title) chip.title = described.title;
  revealOnMousedown(view, chip);
  return chip;
}

// ---- Tags ------------------------------------------------------------------

function buildTag(view, { tag }) {
  const chip = el("span", "cm-lv-widget cm-lp-task-tag", `#${tag}`);
  chip.setAttribute("aria-label", `Tag ${tag}`);
  revealOnMousedown(view, chip);
  return chip;
}

// ---- Widget ----------------------------------------------------------------

const BUILDERS = { taskbox: buildTaskBox, taskfield: buildField, tasktag: buildTag };

// One drawn task control. Its data is everything the control shows, so a widget with
// the same data is interchangeable with the one on screen and the DOM is kept. The day
// is part of the signature, so a redraw on a later day picks up a new overdue mark.
class TaskWidget extends WidgetType {
  constructor(kind, data) {
    super();
    this.kind = kind;
    this.data = data;
    this.signature = JSON.stringify([kind, data, todayISO()]);
  }

  eq(other) {
    return other instanceof TaskWidget && other.signature === this.signature;
  }

  toDOM(view) {
    return BUILDERS[this.kind](view, this.data);
  }

  // The box handles its own presses. A chip or tag lets the editor move the caret,
  // as the other read-only widgets do.
  ignoreEvent() {
    return this.kind === "taskbox";
  }
}

registerWidget("taskbox", ({ token }) => {
  const meta = token.meta ?? {};
  return new TaskWidget("taskbox", { task: meta.task ?? "todo", progress: meta.progress ?? null });
});

registerWidget("taskfield", ({ token }) => {
  const meta = token.meta ?? {};
  return new TaskWidget("taskfield", {
    key: meta.key,
    value: meta.value ?? "",
    state: meta.state ?? "todo",
    warning: meta.warning ?? null,
  });
});

registerWidget("tasktag", ({ token }) => new TaskWidget("tasktag", { tag: token.meta?.tag ?? "" }));
