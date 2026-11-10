// Pipe tables as text, for Org and Markdown. Every function is a pure function of strings: no
// CodeMirror and no DOM. The source-mode keys and the visual table adapters both read and write
// tables through this module.
//
// A table is a block of lines. Each line is a cells row (`| a | b |`) or a rule row (`|---+---|`
// in Org, `|:---|---:|` in Markdown). A row may lack its leading or trailing pipe. Blank lines are
// not part of a table: callers pass the table's lines only.

/** @typedef {"left" | "center" | "right" | null} Alignment */
/** @typedef {{ kind: "cells", cells: string[] } | { kind: "rule", align?: Alignment[] }} PipeRow */
/** @typedef {"org" | "markdown"} PipeFlavor */

// East Asian Wide and Fullwidth characters, and emoji, take two columns.
const WIDE = [
  [0x1100, 0x115f], [0x2e80, 0x303e], [0x3041, 0x33ff], [0x3400, 0x4dbf],
  [0x4e00, 0x9fff], [0xa000, 0xa4cf], [0xac00, 0xd7a3], [0xf900, 0xfaff],
  [0xfe30, 0xfe4f], [0xff00, 0xff60], [0xffe0, 0xffe6], [0x1f300, 0x1f64f],
  [0x1f900, 0x1f9ff], [0x20000, 0x3fffd]
];
const EMOJI = /\p{Emoji_Presentation}/u;
// Combining marks, the zero-width joiner and the variation selector take no column.
const ZERO_WIDTH = /\p{M}|‍/u;

/**
 * How many terminal columns a string takes: wide characters count two, combining marks none.
 * @param {string} text
 * @returns {number}
 */
export function displayWidth(text) {
  let width = 0;
  for (const char of text) {
    if (ZERO_WIDTH.test(char)) continue;
    const code = char.codePointAt(0);
    const wide = EMOJI.test(char) || WIDE.some(([from, to]) => code >= from && code <= to);
    width += wide ? 2 : 1;
  }
  return width;
}

function isBlank(char) {
  return char === " " || char === "\t";
}

// The alignment a Markdown separator cell gives its column, from its colons.
function alignOf(text) {
  const left = text.startsWith(":");
  const right = text.length > 1 && text.endsWith(":");
  if (left && right) return "center";
  if (left) return "left";
  if (right) return "right";
  return null;
}

// Splits one line into its cells. Offsets are relative to the line. A cell's region runs from the
// pipe before it to the pipe after it; its from and to are the text inside, trimmed.
function scanRow(line) {
  const indent = /^[ \t]*/.exec(line)[0];
  const start = indent.length;
  const end = Math.max(start, line.trimEnd().length);

  const pipes = [];
  let escaped = false;
  for (let i = start; i < end; i++) {
    if (line[i] === "|" && !escaped) pipes.push(i);
    escaped = !escaped && line[i] === "\\";
  }

  const regions = [];
  let from = start;
  for (const pipe of pipes) {
    regions.push({ from, to: pipe });
    from = pipe + 1;
  }
  regions.push({ from, to: end });
  // A leading pipe opens the first region and a trailing one closes the last; neither holds a cell.
  if (pipes[0] === start) regions.shift();
  if (pipes.length && pipes[pipes.length - 1] === end - 1) regions.pop();

  const cells = regions.map(region => {
    let from = region.from;
    let to = region.to;
    while (from < to && isBlank(line[from])) from++;
    while (to > from && isBlank(line[to - 1])) to--;
    return { from, to, region, value: line.slice(from, to) };
  });

  const body = line.slice(start, end);
  const rule = pipes.length > 0 && /^[|+:\- \t]*-[|+:\- \t]*$/.test(body);
  const align = cells.some(cell => cell.value.includes(":")) ? cells.map(cell => alignOf(cell.value)) : undefined;
  return { indent, rule, align, cells };
}

/**
 * The lines of a table text with their offsets, all absolute in the text. Each line gives its
 * start and end (the end is before the newline), its text, whether it is a rule, its separator
 * alignments (when it has colons), its indentation, and its cells: from and to are the cell's
 * text, region the span between its pipes.
 * @param {string} text
 */
export function tableLines(text) {
  const lines = [];
  let start = 0;
  for (const line of text.split("\n")) {
    const scan = scanRow(line);
    lines.push({
      text: line,
      start,
      end: start + line.length,
      rule: scan.rule,
      align: scan.align,
      indent: scan.indent,
      cells: scan.cells.map(cell => ({
        from: start + cell.from,
        to: start + cell.to,
        value: cell.value,
        region: { from: start + cell.region.from, to: start + cell.region.to }
      }))
    });
    start += line.length + 1;
  }
  return lines;
}

/**
 * Reads a table text into rows. Cells are trimmed. Indent is the leading white space of the first
 * line; rows keep their own cell counts, which may differ (ragged rows).
 * @param {string} text
 * @returns {{ rows: PipeRow[], indent: string }}
 */
export function parsePipeTable(text) {
  const lines = tableLines(text);
  const rows = lines.map(line => {
    if (line.rule) return line.align ? { kind: "rule", align: line.align } : { kind: "rule" };
    return { kind: "cells", cells: line.cells.map(cell => cell.value) };
  });
  return { rows, indent: lines[0]?.indent ?? "" };
}

function ruleCell(width, alignment) {
  if (alignment === "left") return `:${"-".repeat(width - 1)}`;
  if (alignment === "right") return `${"-".repeat(width - 1)}:`;
  if (alignment === "center") return `:${"-".repeat(width - 2)}:`;
  return "-".repeat(width);
}

/**
 * Writes rows as table lines. Every column is as wide as its widest cell, in display width. Org
 * rules are dashes joined by `+`; Markdown separators keep their colons, have at least three
 * dashes, and set the alignment of right and centre cells. Rows with fewer cells are padded.
 * @param {PipeRow[]} rows
 * @param {PipeFlavor} flavor
 * @param {string} [indent] The indentation of every line.
 * @returns {string}
 */
export function formatPipeTable(rows, flavor, indent = "") {
  const markdown = flavor === "markdown";
  const cellRows = rows.filter(row => row.kind === "cells");
  const columns = Math.max(1, ...cellRows.map(row => row.cells.length));
  const align = markdown ? (rows.find(row => row.kind === "rule" && row.align)?.align ?? []) : [];
  const minimum = markdown ? 3 : 0;
  const widths = Array.from({ length: columns }, (_, column) =>
    Math.max(minimum, ...cellRows.map(row => displayWidth(row.cells[column] ?? "")))
  );

  const pad = (text, column) => {
    const gap = widths[column] - displayWidth(text);
    if (align[column] === "right") return `${" ".repeat(gap)}${text}`;
    if (align[column] === "center") {
      const left = Math.floor(gap / 2);
      return `${" ".repeat(left)}${text}${" ".repeat(gap - left)}`;
    }
    return `${text}${" ".repeat(gap)}`;
  };

  const cellLine = cells =>
    `${indent}|${widths.map((_, column) => ` ${pad(cells[column] ?? "", column)} `).join("|")}|`;

  const ruleLine = () =>
    `${indent}|${widths
      .map((width, column) => (markdown ? ruleCell(width + 2, align[column] ?? null) : "-".repeat(width + 2)))
      .join(markdown ? "|" : "+")}|`;

  return rows.map(row => (row.kind === "cells" ? cellLine(row.cells) : ruleLine())).join("\n");
}

/**
 * Every cell of the cells rows, in document order, with its row and column (both counted from 0
 * over the lines of the text).
 * @param {string} text
 * @returns {Array<{ row: number, col: number, from: number, to: number }>}
 */
export function cellList(text) {
  const list = [];
  tableLines(text).forEach((line, row) => {
    if (line.rule) return;
    line.cells.forEach((cell, col) => list.push({ row, col, from: cell.from, to: cell.to }));
  });
  return list;
}

/**
 * The cell under an offset: its row and column, and how far the offset is into the cell's text
 * (clamped to the text). Null on a rule row or outside any cell. Offsets are relative to the text.
 * @param {string} text
 * @param {number} pos
 * @returns {{ row: number, col: number, offset: number } | null}
 */
export function cellAt(text, pos) {
  const lines = tableLines(text);
  const row = lines.findIndex(line => pos >= line.start && pos <= line.end);
  if (row < 0 || lines[row].rule) return null;
  const cells = lines[row].cells;
  if (!cells.length) return null;
  let col = 0;
  cells.forEach((cell, index) => {
    if (pos >= cell.region.from) col = index;
  });
  const cell = cells[col];
  const offset = Math.min(Math.max(pos - cell.from, 0), cell.to - cell.from);
  return { row, col, offset };
}

/**
 * The offset of a cell's text, plus an offset into that text (clamped). A row with no such cell
 * gives the end of its line.
 * @param {string} text
 * @param {number} row
 * @param {number} col
 * @param {number} [offset]
 * @returns {number}
 */
export function cellOffset(text, row, col, offset = 0) {
  const line = tableLines(text)[row];
  if (!line) return text.length;
  if (!line.cells.length) return line.end;
  const cell = line.cells[Math.min(col, line.cells.length - 1)];
  return cell.from + Math.min(Math.max(offset, 0), cell.to - cell.from);
}

/**
 * The block of table lines in a text: the first run of consecutive lines with a pipe, leaving out
 * Org #+TBLFM lines. Text around the block (a caption, #+TBLFM) is not part of it.
 * @param {string} text
 * @returns {{ from: number, to: number, text: string } | null}
 */
export function pipeBlock(text) {
  const lines = tableLines(text);
  const isTable = line => line.text.includes("|") && !/^\s*#\+tblfm:/i.test(line.text);
  const first = lines.findIndex(isTable);
  if (first < 0) return null;
  let last = first;
  while (last + 1 < lines.length && isTable(lines[last + 1])) last++;
  const from = lines[first].start;
  const to = lines[last].end;
  return { from, to, text: text.slice(from, to) };
}
