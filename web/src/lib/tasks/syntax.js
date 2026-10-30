// The task grammar shared by the visual layer and the edits: states, field names,
// the tag and field grammars, validators and date arithmetic. Pure, no DOM.

// The box character to its state. "X" is done and keeps its case. Any other character is not a task box.
export const STATE_OF = { " ": "todo", x: "done", X: "done", "/": "doing", "-": "cancelled" };
// Menu order, with the character each state writes.
export const STATES = [
  { state: "todo", char: " ", label: "To do" },
  { state: "doing", char: "/", label: "In progress" },
  { state: "done", char: "x", label: "Done" },
  { state: "cancelled", char: "-", label: "Cancelled" },
];
// The order a click cycles the box through. Each state is followed by the next one, and the last wraps to the first.
export const CYCLE = [" ", "x", "/", "-"];
export const FIELDS = ["priority", "due", "start", "scheduled", "created", "done", "cancelled", "assignee", "estimate"];
export const DATE_FIELDS = ["due", "start", "scheduled", "created", "done", "cancelled"];
export const PRIORITIES = [
  { value: 1, label: "Critical" }, { value: 2, label: "High" }, { value: 3, label: "Normal" },
  { value: 4, label: "Low" }, { value: 5, label: "Minimal" },
];
// A field token at a position: `key:value`, with optional spaces or tabs after the colon. Sticky.
// Group 1 is the key, group 2 the gap after the colon (may be empty), group 3 the value (may be empty).
export const FIELD = /(priority|due|start|scheduled|created|done|cancelled|assignee|estimate):([ \t]*)(\S*)/y;
// A tag at a position, `#` included. Sticky.
export const TAG = /#[A-Za-z][A-Za-z0-9_\/-]*/y;
// The head of a task item's first line: indent, optional quote markers, list marker, box.
// Group 1 is the indent and markers before the list marker, group 2 the box character.
export const TASK_LINE = /^((?:[ \t]*>)*[ \t]*)(?:[-*+]|\d{1,9}[.)])[ \t]+\[([ xX\/-])\](?=[ \t]|$)/;

const ASSIGNEES = /^[A-Za-z0-9_.-]+(?:,[A-Za-z0-9_.-]+)*$/;
const ESTIMATE = /^\d+(?:\.\d+)?[mhd]$/;
const DATE = /^(\d{4})-(\d{2})-(\d{2})$/;

/**
 * A warning for a field's value, or null when it is valid.
 * @param {string} key
 * @param {string} value
 * @returns {string | null}
 */
export function validateField(key, value) {
  if (value === "") return `${key} needs a value.`;
  if (key === "priority") return /^[1-5]$/.test(value) ? null : "Priority must be 1 to 5.";
  if (DATE_FIELDS.includes(key)) return isDate(value) ? null : `${key} must be a real date as YYYY-MM-DD.`;
  if (key === "assignee") return ASSIGNEES.test(value) ? null : "Assignee must be one or more comma-separated handles.";
  if (key === "estimate") return ESTIMATE.test(value) ? null : "Estimate must be a number with m, h or d, such as 2h.";
  return null;
}

// Year, month and day of a date string, or null when it is not a real calendar day.
function parts(iso) {
  const match = DATE.exec(typeof iso === "string" ? iso : "");
  if (!match) return null;
  const year = Number(match[1]);
  const month = Number(match[2]);
  const day = Number(match[3]);
  const date = new Date(Date.UTC(year, month - 1, day));
  if (date.getUTCFullYear() !== year || date.getUTCMonth() !== month - 1 || date.getUTCDate() !== day) return null;
  return { year, month, day };
}

function pad(number, width) {
  return String(number).padStart(width, "0");
}

function formatISO(year, month, day) {
  return `${pad(year, 4)}-${pad(month, 2)}-${pad(day, 2)}`;
}

/** "YYYY-MM-DD" for the local calendar day of `date` (default now). */
export function todayISO(date = new Date()) {
  return formatISO(date.getFullYear(), date.getMonth() + 1, date.getDate());
}

/** True when `iso` is a valid calendar date string. */
export function isDate(iso) {
  return parts(iso) !== null;
}

/** A short human label for a date, e.g. "Nov 5", with the year added when it is not `today`'s year. */
export function shortDate(iso, today = todayISO(), locale = undefined) {
  const date = parts(iso);
  if (!date) return iso;
  const todayYear = parts(today)?.year;
  /** @type {Intl.DateTimeFormatOptions} */
  const options = { month: "short", day: "numeric", timeZone: "UTC" };
  if (date.year !== todayYear) options.year = "numeric";
  return new Intl.DateTimeFormat(locale, options).format(new Date(Date.UTC(date.year, date.month - 1, date.day)));
}

/** True when a task in `state` with due date `due` is overdue on `today`. Done and cancelled never are. */
export function isOverdue(state, due, today = todayISO()) {
  if (state === "done" || state === "cancelled") return false;
  if (!isDate(due) || !isDate(today)) return false;
  return due < today;
}
