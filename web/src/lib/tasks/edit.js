// Edits to a task's source, as CodeMirror change specs. The edit replaces the
// box character only, so the rest of the line keeps its exact text.

import { CYCLE } from "./syntax.js";

/** @typedef {{ from: number, to: number, insert: string }} Change */

/**
 * Moves the box to the next state in CYCLE, wrapping after the last. "X" counts as "x".
 * @returns {Change[]}
 */
export function cycle(task) {
  const current = task.box === "X" ? "x" : task.box;
  const next = CYCLE[(CYCLE.indexOf(current) + 1) % CYCLE.length];
  return [{ from: task.boxAt, to: task.boxAt + 1, insert: next }];
}
