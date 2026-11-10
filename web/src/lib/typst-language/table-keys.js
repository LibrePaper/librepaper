// Keyboard moves between the cells of a Typst `table(...)` or `grid(...)` call.
// Tab goes to the next cell and, from the last cell, appends a row; Shift-Tab
// goes to the previous cell. Outside such a call the commands return false, so
// the editor's own Tab (indent) runs. Without the wasm tree there are no call
// nodes at all, so they return false then too.
import { EditorSelection } from "@codemirror/state";
import { syntaxTree } from "@codemirror/language";

const TABLE_CALL = /^(?:table|grid)$/;
const CELL_CALL = /^(?:(?:table|grid)\.)?cell$/;
const HEADER_CALL = /^(?:(?:table|grid)\.)?header$/;
const TRIVIA = new Set(["Space", "LineComment", "BlockComment"]);
const NOT_VALUE = new Set([...TRIVIA, "LeftParen", "RightParen", "Comma"]);

/** A command target: the state and the dispatch of a view, or a stand-in for one. */
/** @typedef {{ state: import("@codemirror/state").EditorState, dispatch: (tr: import("@codemirror/state").Transaction) => void }} CommandTarget */

/** A cell: the offset of its `[`, its inner text range, and the offset just after its `]`. */
/** @typedef {{ open: number, inner: { from: number, to: number }, close: number }} Cell */

function textOf(state, node) {
  return state.sliceDoc(node.from, node.to);
}

/** The name of the callee of a call, as written. */
function calleeText(state, call) {
  const callee = call.firstChild;
  return callee ? textOf(state, callee) : "";
}

/**
 * The Args node of the table or grid call whose argument list holds pos, or null.
 * @param {import("@codemirror/state").EditorState} state
 * @param {number} pos
 * @returns {import("@lezer/common").SyntaxNode | null}
 */
function tableArgsAt(state, pos) {
  for (let node = syntaxTree(state).resolveInner(pos, 1); node; node = node.parent) {
    if (node.name !== "Args" || pos <= node.from || pos >= node.to) continue;
    const call = node.parent;
    if (call?.name === "FuncCall" && TABLE_CALL.test(calleeText(state, call))) return node;
  }
  return null;
}

/** The cell of a ContentBlock node, or null when its brackets are not both there. */
function cellOf(block) {
  const open = block.getChild("LeftBracket");
  const close = block.getChild("RightBracket");
  if (!open || !close) return null;
  return { open: open.from, inner: { from: open.to, to: close.from }, close: close.to };
}

/** The ContentBlock children of an argument list, in order. */
function blocksOf(args) {
  const blocks = [];
  for (let child = args.firstChild; child; child = child.nextSibling) {
    if (child.name === "ContentBlock") blocks.push(child);
  }
  return blocks;
}

/**
 * The cells of a table's argument list, in order. A content block is a cell;
 * so is the content of a `cell(...)` call, and every content block of a
 * `header(...)` call counts as a cell in turn.
 * @param {import("@codemirror/state").EditorState} state
 * @param {import("@lezer/common").SyntaxNode} args
 * @returns {Cell[]}
 */
function cellsOf(state, args) {
  const cells = [];
  for (let child = args.firstChild; child; child = child.nextSibling) {
    if (child.name === "ContentBlock") {
      const cell = cellOf(child);
      if (cell) cells.push(cell);
    } else if (child.name === "FuncCall") {
      const inner = child.getChild("Args");
      if (!inner) continue;
      const name = calleeText(state, child);
      if (CELL_CALL.test(name)) {
        const blocks = blocksOf(inner);
        const cell = blocks.length ? cellOf(blocks[blocks.length - 1]) : null;
        if (cell) cells.push(cell);
      } else if (HEADER_CALL.test(name)) {
        for (const block of blocksOf(inner)) {
          const cell = cellOf(block);
          if (cell) cells.push(cell);
        }
      }
    }
  }
  return cells;
}

/** The last node of a Named argument that is not whitespace or a comment. */
function valueOf(named) {
  let node = named.lastChild;
  while (node && TRIVIA.has(node.name)) node = node.prevSibling;
  return node;
}

/** The number of columns a table's `columns:` argument gives: an Int, or the length of an Array. Defaults to 1. */
function columnCount(state, args) {
  for (let child = args.firstChild; child; child = child.nextSibling) {
    if (child.name !== "Named") continue;
    const key = child.firstChild;
    if (!key || key.name !== "Ident" || textOf(state, key) !== "columns") continue;
    const value = valueOf(child);
    if (value?.name === "Int") return Math.max(1, Number(textOf(state, value)));
    if (value?.name === "Array") {
      let count = 0;
      for (let item = value.firstChild; item; item = item.nextSibling) {
        if (!NOT_VALUE.has(item.name)) count++;
      }
      return Math.max(1, count);
    }
    return 1;
  }
  return 1;
}

/** The index of the first cell that does not end before pos, or the cell count when none does. */
function indexAfter(cells, pos) {
  const index = cells.findIndex((cell) => cell.close > pos);
  return index < 0 ? cells.length : index;
}

/** Selects the inner text of a cell; an empty cell just gets the cursor. */
function selectCell(target, cell) {
  target.dispatch(target.state.update({
    selection: EditorSelection.single(cell.inner.from, cell.inner.to),
    scrollIntoView: true,
    userEvent: "select",
  }));
  return true;
}

/**
 * Adds a row after the last cell: as many empty cells as there are columns,
 * on a new line indented like the last cell's line. The cursor goes into the
 * first new cell.
 */
function appendRow(target, args, last) {
  const { state } = target;
  const row = Array.from({ length: columnCount(state, args) }, () => "[]").join(", ");
  const indent = /^[ \t]*/.exec(state.doc.lineAt(last.open).text)[0];
  let at = last.close;
  while (/[ \t]/.test(state.sliceDoc(at, at + 1))) at++;
  const comma = state.sliceDoc(at, at + 1) === ",";
  const from = comma ? at + 1 : last.close;
  const lead = comma ? "\n" : ",\n";
  const insert = `${lead}${indent}${row}${comma ? "," : ""}`;
  target.dispatch(state.update({
    changes: { from, insert },
    selection: { anchor: from + lead.length + indent.length + 1 },
    scrollIntoView: true,
    userEvent: "input",
  }));
  return true;
}

/**
 * Tab: the next cell of the table or grid around the cursor, or a new row
 * after the last cell.
 * @param {CommandTarget} target
 * @returns {boolean}
 */
export function nextTableCell(target) {
  const { state } = target;
  const pos = state.selection.main.head;
  const args = tableArgsAt(state, pos);
  if (!args) return false;
  const cells = cellsOf(state, args);
  if (!cells.length) return false;
  const index = indexAfter(cells, pos);
  const inside = index < cells.length && cells[index].open < pos;
  const next = inside ? index + 1 : index;
  if (next < cells.length) return selectCell(target, cells[next]);
  return appendRow(target, args, cells[cells.length - 1]);
}

/**
 * Shift-Tab: the previous cell of the table or grid around the cursor. At the
 * first cell the cursor stays where it is.
 * @param {CommandTarget} target
 * @returns {boolean}
 */
export function previousTableCell(target) {
  const { state } = target;
  const pos = state.selection.main.head;
  const args = tableArgsAt(state, pos);
  if (!args) return false;
  const cells = cellsOf(state, args);
  if (!cells.length) return false;
  const previous = indexAfter(cells, pos) - 1;
  if (previous < 0) return true;
  return selectCell(target, cells[previous]);
}

/** Tab and Shift-Tab between the cells of Typst table and grid calls. */
export const typstTableKeymap = [
  { key: "Tab", run: nextTableCell },
  { key: "Shift-Tab", run: previousTableCell },
];
