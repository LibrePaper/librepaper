// The part of the task grammar that needs no messages: the Markdown parser reads
// it in workers too. syntax.js re-exports everything here and adds the labels.

// The box character to its state. "X" is done and keeps its case. Any other character is not a task box.
export const STATE_OF = { " ": "todo", x: "done", X: "done", "/": "doing", "-": "cancelled" };
export const DATE_FIELDS = ["due", "start", "scheduled", "created", "done", "cancelled"];
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

// Year, month and day of a date string, or null when it is not a real calendar day.
export function dateParts(iso) {
  const match = DATE.exec(typeof iso === "string" ? iso : "");
  if (!match) return null;
  const year = Number(match[1]);
  const month = Number(match[2]);
  const day = Number(match[3]);
  const date = new Date(Date.UTC(year, month - 1, day));
  if (date.getUTCFullYear() !== year || date.getUTCMonth() !== month - 1 || date.getUTCDate() !== day) return null;
  return { year, month, day };
}

/** True when `iso` is a valid calendar date string. */
export function isDate(iso) {
  return dateParts(iso) !== null;
}

/**
 * What is wrong with a field's value: "empty", "priority", "date", "assignee" or
 * "estimate", or null when it is valid.
 * @param {string} key
 * @param {string} value
 * @returns {"empty" | "priority" | "date" | "assignee" | "estimate" | null}
 */
export function fieldProblem(key, value) {
  if (value === "") return "empty";
  if (key === "priority") return /^[1-5]$/.test(value) ? null : "priority";
  if (DATE_FIELDS.includes(key)) return isDate(value) ? null : "date";
  if (key === "assignee") return ASSIGNEES.test(value) ? null : "assignee";
  if (key === "estimate") return ESTIMATE.test(value) ? null : "estimate";
  return null;
}
