import { m } from "../paraglide/messages.js";
import { locale as interfaceLocale } from "../locale.js";
import { DATE_FIELDS, FIELD, TAG, TASK_LINE, STATE_OF, dateParts, fieldProblem, isDate } from "./grammar.js";

// The task grammar shared by the visual layer and the edits: states, field names,
// the tag and field grammars, validators and date arithmetic. Pure, no DOM. The
// message-free part lives in grammar.js, which the Markdown parser also reads.
export { DATE_FIELDS, FIELD, TAG, TASK_LINE, STATE_OF, isDate };

// Menu order, with the character each state writes.
export const STATES = [
  { state: "todo", char: " ", label: m.syntax_state_todo() },
  { state: "doing", char: "/", label: m.syntax_state_doing() },
  { state: "done", char: "x", label: m.syntax_state_done() },
  { state: "cancelled", char: "-", label: m.syntax_state_cancelled() },
];
// The order a click cycles the box through. Each state is followed by the next one, and the last wraps to the first.
export const CYCLE = [" ", "x", "/", "-"];
export const FIELDS = ["priority", "due", "start", "scheduled", "created", "done", "cancelled", "assignee", "estimate"];
export const PRIORITIES = [
  { value: 1, label: m.syntax_priority_critical() }, { value: 2, label: m.syntax_priority_high() }, { value: 3, label: m.syntax_priority_normal() },
  { value: 4, label: m.syntax_priority_low() }, { value: 5, label: m.syntax_priority_minimal() },
];
/**
 * A warning for a field's value, or null when it is valid.
 * @param {string} key
 * @param {string} value
 * @returns {string | null}
 */
export function validateField(key, value) {
  switch (fieldProblem(key, value)) {
    case "empty": return m.syntax_field_needs_value({ key });
    case "priority": return m.syntax_priority_range();
    case "date": return m.syntax_date_invalid({ key });
    case "assignee": return m.syntax_assignee_invalid();
    case "estimate": return m.syntax_estimate_invalid();
    default: return null;
  }
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

/** A short human label for a date, e.g. "Nov 5", with the year added when it is not `today`'s year. */
export function shortDate(iso, today = todayISO(), locale = interfaceLocale()) {
  const date = dateParts(iso);
  if (!date) return iso;
  const todayYear = dateParts(today)?.year;
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
