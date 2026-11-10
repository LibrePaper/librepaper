// Source-mode keys for pipe tables in Org and Markdown. Tab and Shift-Tab move between cells and
// realign the table; Enter moves down the same column, adding a row at the end. Each key returns
// false outside a table, so the other bindings run there. In Org, Tab on a `|-` line expands it
// into a full rule.
import { syntaxTree } from "@codemirror/language";
import { cellAt, cellList, cellOffset, formatPipeTable, parsePipeTable, tableLines } from "./pipe-tables.js";

const TBLFM = /^[ \t]*#\+TBLFM:/im;

/**
 * The Table node under a position (Org and Markdown both name it so), widened to whole lines.
 * Null inside a quote, where the lines carry a `>` that the cell reader would take for text.
 * @param {import("@codemirror/state").EditorState} state
 * @param {number} pos
 * @returns {{ from: number, to: number } | null}
 */
export function pipeTableAt(state, pos) {
  let node = syntaxTree(state).resolve(pos, 0);
  while (node && node.name !== "Table") node = node.parent;
  if (!node) return null;
  const first = state.doc.lineAt(node.from);
  if (/^[ \t]*>/.test(first.text)) return null;
  const last = state.doc.lineAt(Math.max(node.from, node.to - 1));
  return { from: first.from, to: last.to };
}

// The table at pos, as its text. #+TBLFM lines after the rows are cut off: they stay where they are.
function tableAround(state, findTable, pos) {
  const range = findTable(state, pos);
  if (!range) return null;
  const text = state.sliceDoc(range.from, range.to);
  const cut = text.search(TBLFM);
  const body = cut < 0 ? text : text.slice(0, cut).replace(/\n$/, "");
  const to = range.from + body.length;
  if (pos < range.from || pos > to) return null;
  return { from: range.from, to, text: body };
}

// Replaces the table with text and puts the cursor at cursor (an offset into text).
function commit(view, table, text, cursor) {
  view.dispatch({
    changes: text === table.text ? [] : { from: table.from, to: table.to, insert: text },
    selection: { anchor: table.from + cursor },
    scrollIntoView: true
  });
  return true;
}

// Tab and Shift-Tab: realign, then move to the next or previous cell. Past the last cell, Tab
// adds a row.
function moveCell(view, flavor, findTable, step) {
  const { state } = view;
  const pos = state.selection.main.head;
  const table = tableAround(state, findTable, pos);
  if (!table) return false;
  const cells = cellList(table.text);
  if (!cells.length) return false;

  // The last cell that starts at or before the cursor is the current one.
  const rel = pos - table.from;
  let current = -1;
  cells.forEach((cell, index) => {
    if (cell.from <= rel) current = index;
  });
  const target = step > 0 ? current + 1 : Math.max(current - 1, 0);

  const { rows, indent } = parsePipeTable(table.text);
  let goal;
  if (target < cells.length) {
    goal = { row: cells[target].row, col: cells[target].col };
  } else {
    rows.push({ kind: "cells", cells: [] });
    goal = { row: rows.length - 1, col: 0 };
  }
  const text = formatPipeTable(rows, flavor, indent);
  return commit(view, table, text, cellOffset(text, goal.row, goal.col));
}

// Enter: realign, then move to the same column of the next cells row, adding a row at the end.
function newlineInTable(view, flavor, findTable) {
  const { state } = view;
  const pos = state.selection.main.head;
  const table = tableAround(state, findTable, pos);
  if (!table) return false;
  const here = cellAt(table.text, pos - table.from);
  if (!here) return false;

  const { rows, indent } = parsePipeTable(table.text);
  let next = here.row + 1;
  while (next < rows.length && rows[next].kind !== "cells") next++;
  if (next >= rows.length) {
    rows.push({ kind: "cells", cells: [] });
    next = rows.length - 1;
  }
  const text = formatPipeTable(rows, flavor, indent);
  return commit(view, table, text, cellOffset(text, next, here.col, here.offset));
}

// Org: a `|-` line at the end of the cursor, below a cells row, becomes a full rule.
function expandRule(view, findTable) {
  const { state } = view;
  const pos = state.selection.main.head;
  const line = state.doc.lineAt(pos);
  if (line.text.trim() !== "|-" || pos !== line.to) return false;
  const table = tableAround(state, findTable, pos);
  if (!table) return false;

  const rel = pos - table.from;
  const lines = tableLines(table.text);
  const index = lines.findIndex(item => item.start <= rel && rel <= item.end);
  if (index < 1 || lines[index - 1].rule) return false;

  const { rows, indent } = parsePipeTable(table.text);
  const text = formatPipeTable(rows, "org", indent);
  return commit(view, table, text, tableLines(text)[index].end);
}

/**
 * The key bindings for pipe tables of one flavor. findTable(state, pos) returns the {from, to} of
 * the table at pos, or null. Pass them to keymap.of.
 * @param {"org" | "markdown"} flavor
 * @param {(state: import("@codemirror/state").EditorState, pos: number) => { from: number, to: number } | null} findTable
 * @returns {import("@codemirror/view").KeyBinding[]}
 */
export function pipeTableKeymap(flavor, findTable) {
  return [
    {
      key: "Tab",
      run: view => (flavor === "org" && expandRule(view, findTable)) || moveCell(view, flavor, findTable, 1)
    },
    { key: "Shift-Tab", run: view => moveCell(view, flavor, findTable, -1) },
    { key: "Enter", run: view => newlineInTable(view, flavor, findTable) }
  ];
}
