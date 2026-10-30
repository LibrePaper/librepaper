// The task controls of the Markdown visual layer. A task's box, its fields and its
// tags are drawn over the source, and each control writes the smallest change it
// can. Every action reads the task from the source again when it runs: the widget's
// position moves as the text around it changes, and a popup can stay open across edits.

import { Facet } from "@codemirror/state";
import { WidgetType } from "@codemirror/view";
import "./tasks.css";
import { registerWidget } from "./markup.ts";
import { fieldOf, taskAt } from "../tasks/model.js";
import { addTag, completeRecurring, removeField, setField, setState, toggle } from "../tasks/edit.js";
import { isDate, isOverdue, PRIORITIES, shortDate, STATES, todayISO, validateField } from "../tasks/syntax.js";

// The hooks the editor provides. `vocabulary()` returns { assignees, tags } for
// suggestions, `openTask(id)` opens the task with that id, and `searchTag(tag)`
// shows the tasks with that tag. A missing hook makes its control do nothing.
export const taskOptions = Facet.define({ combine: (values) => values[0] ?? null });

const STATE_LABEL = Object.fromEntries(STATES.map((item) => [item.state, item.label]));
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

// The fields a task menu offers when the task lacks them, in menu order.
const ADD_FIELDS = [
  { key: "priority", label: "Add priority" },
  { key: "due", label: "Add due date" },
  { key: "scheduled", label: "Add scheduled date" },
  { key: "start", label: "Add start date" },
  { key: "assignee", label: "Add assignee" },
  { key: "estimate", label: "Add estimate" },
  { key: "repeat", label: "Add repeat" },
];

// The fields that take typed text, with the prompt's label and an example of the form.
const TEXT_FIELDS = {
  assignee: { label: "Assignee", placeholder: "alice,bob" },
  estimate: { label: "Estimate", placeholder: "2h" },
  repeat: { label: "Repeat", placeholder: "weekly" },
  id: { label: "Identifier", placeholder: "task-123" },
};

// Fields that are useful to read but should not compete with the task's text.
const QUIET_FIELDS = new Set(["id", "created", "repeat", "estimate"]);

// A tag name without its `#`. Tags match the grammar the source reads.
const TAG_NAME = /^[A-Za-z][A-Za-z0-9_\/-]*$/;

// The one popup open at a time, or null. Its anchor is the control that opened it.
let popup = null;
let suggestionCount = 0;

// A DOM element with a class and text, in one call.
function el(tag, className = "", text = "") {
  const element = document.createElement(tag);
  if (className) element.className = className;
  if (text) element.textContent = text;
  return element;
}

// Keeps presses inside a control. The editor's keymap and caret must not act on them.
function wireControl(element) {
  element.addEventListener("mousedown", (event) => event.stopPropagation());
  element.addEventListener("keydown", (event) => event.stopPropagation());
}

function dispatch(view, changes) {
  if (changes.length) view.dispatch({ changes });
}

// The task under a control, read from the source as it is now, or null.
function currentTask(view, anchor) {
  const widget = anchor.closest(".cm-lv-widget");
  const pos = widget ? view.posAtDOM(widget) : -1;
  if (pos < 0) return null;
  const source = view.state.doc.toString();
  const task = taskAt(source, pos);
  return task ? { source, task } : null;
}

// Applies the changes that `change(source, task)` makes to the task under `anchor`.
function edit(view, anchor, change) {
  const found = currentTask(view, anchor);
  if (found) dispatch(view, change(found.source, found.task));
}

function vocabularyOf(view) {
  return view.state.facet(taskOptions)?.vocabulary?.() ?? { assignees: [], tags: [] };
}

// ---- Popups ----------------------------------------------------------------

function closePopup(restore) {
  if (!popup) return;
  const open = popup;
  popup = null;
  open.teardown();
  open.element.remove();
  if (restore && open.anchor.isConnected) open.anchor.focus();
}

// Ends an action: the popup closes and focus goes back to its control, or to the editor when the control was redrawn.
function finish(view, anchor) {
  closePopup(false);
  if (anchor.isConnected) anchor.focus();
  else view.focus();
}

// Puts the popup under its control, pulled back in when it would run past the editor's right edge.
function place(view, element, anchor) {
  const frame = view.dom.getBoundingClientRect();
  const box = anchor.getBoundingClientRect();
  const left = box.left - frame.left;
  element.style.top = `${box.bottom - frame.top + 4}px`;
  element.style.left = `${left}px`;
  const over = element.getBoundingClientRect().right - frame.right;
  if (over > 0) element.style.left = `${Math.max(0, left - over)}px`;
}

// Opens a popup under `anchor`, inside the editor so that it scrolls with the text.
// `build` fills it. It closes on Escape, on a press outside it, and on scroll.
// Arrow keys move between the menu items, and the first control takes focus.
function openPopup(view, anchor, label, build) {
  closePopup(false);
  const element = el("div", "cm-lp-task-popup");
  element.setAttribute("aria-label", label);
  build(element);
  view.dom.append(element);
  place(view, element, anchor);

  // A press on the anchor is left to the anchor, which opens or closes its own popup.
  const onPress = (event) => {
    if (!element.contains(event.target) && !anchor.contains(event.target)) closePopup(false);
  };
  const onScroll = () => closePopup(false);
  const onKey = (event) => {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      closePopup(true);
      return;
    }
    // Arrows only move between menu items; in a text field they move the caret.
    const items = [...element.querySelectorAll(".cm-lp-task-item")];
    const index = items.indexOf(document.activeElement);
    if (index < 0) return;
    const next = { ArrowDown: index + 1, ArrowUp: index - 1, Home: 0, End: items.length - 1 }[event.key];
    if (next === undefined) return;
    event.preventDefault();
    items[(next + items.length) % items.length].focus();
  };

  element.addEventListener("keydown", onKey);
  document.addEventListener("mousedown", onPress, true);
  view.scrollDOM.addEventListener("scroll", onScroll);
  popup = {
    element,
    anchor,
    teardown: () => {
      document.removeEventListener("mousedown", onPress, true);
      view.scrollDOM.removeEventListener("scroll", onScroll);
    },
  };

  const target = element.querySelector("[aria-checked='true']") ?? element.querySelector("input") ?? element.querySelector(".cm-lp-task-item");
  target?.focus();
}

// A menu entry. A checked entry is a radio item, so a screen reader announces the selection.
function menuItem(menu, text, { checked = null, run }) {
  const item = el("button", "cm-lp-task-item", text);
  item.type = "button";
  if (checked === null) {
    item.setAttribute("role", "menuitem");
  } else {
    item.setAttribute("role", "menuitemradio");
    item.setAttribute("aria-checked", String(checked));
  }
  item.addEventListener("click", run);
  menu.append(item);
}

function separator() {
  const line = el("div", "cm-lp-task-sep");
  line.setAttribute("role", "separator");
  return line;
}

// A popup with one text field. Enter submits, and an empty value changes nothing.
// `submit(text)` returns an error to show, or "" once the change is made.
function openPrompt(view, anchor, { label, value = "", placeholder = "", suggestions = [], submit }) {
  openPopup(view, anchor, label, (element) => {
    element.setAttribute("role", "group");
    const form = el("form", "cm-lp-task-form");
    const input = el("input", "cm-lp-task-input");
    input.type = "text";
    input.value = value;
    input.placeholder = placeholder;
    input.autocomplete = "off";
    input.setAttribute("aria-label", label);
    const status = el("div", "cm-lp-task-status");
    status.setAttribute("role", "status");

    // The datalist offers the names already used in the project without forbidding others.
    if (suggestions.length) {
      const list = document.createElement("datalist");
      list.id = `cm-lp-task-suggestions-${++suggestionCount}`;
      for (const name of suggestions) {
        const option = document.createElement("option");
        option.value = name;
        list.append(option);
      }
      input.setAttribute("list", list.id);
      form.append(list);
    }

    form.append(input, status);
    form.addEventListener("submit", (event) => {
      event.preventDefault();
      const text = input.value.trim();
      if (text) {
        const error = submit(text);
        if (error) {
          status.textContent = error;
          return;
        }
      }
      finish(view, anchor);
    });
    element.append(form);
  });
}

// A popup with a date field and a Remove button. Choosing a date writes it at once.
function openDatePopup(view, anchor, key, value) {
  openPopup(view, anchor, DATE_LABEL[key], (element) => {
    element.setAttribute("role", "group");
    const input = el("input", "cm-lp-task-input");
    input.type = "date";
    input.value = isDate(value) ? value : "";
    input.setAttribute("aria-label", DATE_LABEL[key]);
    input.addEventListener("change", () => {
      if (!input.value) return;
      edit(view, anchor, (_, task) => setField(task, key, input.value));
      finish(view, anchor);
    });

    const remove = el("button", "cm-lp-task-item", "Remove");
    remove.type = "button";
    remove.addEventListener("click", () => {
      edit(view, anchor, (_, task) => removeField(task, key));
      finish(view, anchor);
    });
    element.append(input, remove);
  });
}

// The priorities, with the current one marked, and a way to remove the field.
function openPriorityMenu(view, anchor, task) {
  const field = fieldOf(task, "priority");
  const current = field && !validateField("priority", field.value) ? Number(field.value) : null;
  openPopup(view, anchor, "Priority", (menu) => {
    menu.setAttribute("role", "menu");
    for (const item of PRIORITIES) {
      menuItem(menu, `P${item.value}, ${item.label}`, {
        checked: current === item.value,
        run: () => {
          edit(view, anchor, (_, fresh) => setField(fresh, "priority", String(item.value)));
          finish(view, anchor);
        },
      });
    }
    menu.append(separator());
    menuItem(menu, "Remove priority", {
      run: () => {
        edit(view, anchor, (_, current) => removeField(current, "priority"));
        finish(view, anchor);
      },
    });
  });
}

function openTextPrompt(view, anchor, key, value) {
  const { label, placeholder } = TEXT_FIELDS[key];
  const suggestions = key === "assignee" ? vocabularyOf(view).assignees : [];
  openPrompt(view, anchor, {
    label,
    value,
    placeholder,
    suggestions,
    submit: (text) => {
      edit(view, anchor, (_, task) => setField(task, key, text));
      return "";
    },
  });
}

function openTagPrompt(view, anchor) {
  openPrompt(view, anchor, {
    label: "Tag",
    placeholder: "project/notes",
    suggestions: vocabularyOf(view).tags,
    submit: (text) => {
      const name = text.replace(/^#/, "");
      if (!TAG_NAME.test(name)) return "A tag starts with a letter, then letters, digits, _, - or /.";
      edit(view, anchor, (_, task) => addTag(task, name));
      return "";
    },
  });
}

// ---- Task box --------------------------------------------------------------

// The change a menu choice makes. Completing a repeating task copies it, as a click does.
function stateChanges(source, task, item) {
  if (task.state === item.state) return [];
  const repeat = fieldOf(task, "repeat");
  if (item.state === "done" && repeat && !validateField("repeat", repeat.value)) {
    return completeRecurring(source, task, todayISO());
  }
  return setState(task, item.char);
}

// The state menu, then the fields the task lacks, then a tag. Each choice reads the task again.
function openStateMenu(view, button) {
  const found = currentTask(view, button);
  if (!found) return;
  const { task } = found;
  openPopup(view, button, "Task menu", (menu) => {
    menu.setAttribute("role", "menu");
    for (const item of STATES) {
      menuItem(menu, item.label, {
        checked: task.state === item.state,
        run: () => {
          edit(view, button, (source, current) => stateChanges(source, current, item));
          finish(view, button);
        },
      });
    }
    menu.append(separator());
    for (const { key, label } of ADD_FIELDS) {
      if (fieldOf(task, key)) continue;
      menuItem(menu, label, { run: () => addField(view, button, key) });
    }
    menuItem(menu, "Add tag", { run: () => openTagPrompt(view, button) });
  });
}

// Adds a field the task lacks. Priority and dates take a default at once; the rest ask for their text.
function addField(view, button, key) {
  if (key === "priority" || key in DATE_LABEL) {
    edit(view, button, (_, task) => setField(task, key, key === "priority" ? "3" : todayISO()));
    finish(view, button);
    return;
  }
  openTextPrompt(view, button, key, "");
}

// A click toggles a todo or done box. Other states open the menu, so nothing is completed by accident.
function toggleTask(view, button) {
  const found = currentTask(view, button);
  if (!found) return;
  if (found.task.state === "todo" || found.task.state === "done") {
    dispatch(view, toggle(found.source, found.task, todayISO()));
  } else {
    openStateMenu(view, button);
  }
}

// The checkbox and its progress. The glyph is drawn by CSS, except the question mark,
// which is text so that a blocked task reads as blocked without the stylesheet.
function buildTaskBox(view, { task, progress }) {
  const root = el("span", "cm-lv-widget cm-lp-taskbox");
  const button = el("button", `cm-lp-taskbox-button cm-lp-taskbox-${task}`, task === "blocked" ? "?" : "");
  button.type = "button";
  button.setAttribute("role", "checkbox");
  button.setAttribute("aria-checked", CHECKED[task] ?? "false");
  button.setAttribute("aria-label", `Task, ${STATE_LABEL[task].toLowerCase()}`);
  wireControl(button);

  button.addEventListener("click", () => toggleTask(view, button));
  button.addEventListener("contextmenu", (event) => {
    event.preventDefault();
    openStateMenu(view, button);
  });
  button.addEventListener("keydown", (event) => {
    if (event.key === " " || event.key === "Enter") {
      event.preventDefault();
      toggleTask(view, button);
    } else if ((event.key === "F10" && event.shiftKey) || event.key === "ContextMenu") {
      event.preventDefault();
      openStateMenu(view, button);
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
  if (key === "id") return { text: "id", name: `Identifier ${value}`, title: value };
  if (key === "after") return { text: `after ${value}`, name: `Depends on ${value}`, title: value };
  const names = { assignee: "Assignee", repeat: "Repeats", estimate: "Estimate" };
  return { text: value, name: `${names[key]} ${value}` };
}

// Opens the control for a field. Priority opens its menu, dates a date picker, the
// dependency chip opens its prerequisite, and the rest take typed text.
function activateField(view, button, key) {
  const found = currentTask(view, button);
  const field = found && fieldOf(found.task, key);
  if (!field) return;
  if (key === "priority") openPriorityMenu(view, button, found.task);
  else if (key in DATE_LABEL) openDatePopup(view, button, key, field.value);
  else if (key === "after") {
    const id = field.value.split(",")[0];
    if (id) view.state.facet(taskOptions)?.openTask?.(id);
  } else openTextPrompt(view, button, key, field.value);
}

// A field chip. The chip shows the short form and its name says the whole field.
function buildField(view, { key, value, state, warning }) {
  const described = describeField(key, value, state, warning);
  const rank = key === "priority" && !warning ? PRIORITIES.find((item) => String(item.value) === value) : undefined;
  const classes = ["cm-lv-widget", "cm-lp-field", `cm-lp-field-${key}`];
  if (QUIET_FIELDS.has(key)) classes.push("cm-lp-task-quiet");
  if (rank) classes.push(`cm-lp-field-p${rank.value}`);
  if (described.overdue) classes.push("cm-lp-task-overdue");
  if (warning) classes.push("cm-lp-task-invalid");

  const button = el("button", classes.join(" "), described.text);
  button.type = "button";
  button.setAttribute("aria-label", described.name);
  if (described.title) button.title = described.title;
  wireControl(button);
  button.addEventListener("click", () => activateField(view, button, key));
  return button;
}

// ---- Tags ------------------------------------------------------------------

function buildTag(view, { tag }) {
  const button = el("button", "cm-lv-widget cm-lp-task-tag", `#${tag}`);
  button.type = "button";
  button.setAttribute("aria-label", `Tag ${tag}, show tasks with this tag`);
  wireControl(button);
  button.addEventListener("click", () => view.state.facet(taskOptions)?.searchTag?.(tag));
  return button;
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

  ignoreEvent() {
    return true;
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
