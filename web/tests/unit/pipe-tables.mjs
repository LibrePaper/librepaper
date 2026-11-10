import assert from "node:assert/strict";
import test from "node:test";
import { cellAt, cellOffset, displayWidth, formatPipeTable, parsePipeTable, pipeBlock } from "../../src/lib/pipe-tables.js";

test("parse reads trimmed cells, Org rules and Markdown alignment", () => {
  assert.deepEqual(parsePipeTable("| a | b |\n|---+---|\n| c | d |"), {
    rows: [
      { kind: "cells", cells: ["a", "b"] },
      { kind: "rule" },
      { kind: "cells", cells: ["c", "d"] }
    ],
    indent: ""
  });

  assert.deepEqual(parsePipeTable("|:---|:---:|---:|\n| x | y | z |").rows[0], {
    kind: "rule",
    align: ["left", "center", "right"]
  });
});

test("parse accepts rows without a trailing or leading pipe", () => {
  assert.deepEqual(parsePipeTable("a | b\n--|--").rows, [
    { kind: "cells", cells: ["a", "b"] },
    { kind: "rule" }
  ]);
});

test("parse keeps ragged rows as they are and the first line's indentation", () => {
  const { rows, indent } = parsePipeTable("  | a | b | c |\n  | d |");
  assert.equal(indent, "  ");
  assert.deepEqual(rows, [
    { kind: "cells", cells: ["a", "b", "c"] },
    { kind: "cells", cells: ["d"] }
  ]);
});

test("displayWidth counts wide characters two and combining marks none", () => {
  assert.equal(displayWidth("abc"), 3);
  assert.equal(displayWidth("中文"), 4);
  assert.equal(displayWidth("😀"), 2);
  assert.equal(displayWidth("é"), 1);
});

test("Org format pads to the widest cell and writes + rules", () => {
  assert.equal(
    formatPipeTable([
      { kind: "cells", cells: ["Name", "Qty"] },
      { kind: "rule" },
      { kind: "cells", cells: ["apple", "3"] }
    ], "org"),
    "| Name  | Qty |\n|-------+-----|\n| apple | 3   |"
  );
});

test("Org format measures CJK text in display columns", () => {
  assert.equal(
    formatPipeTable([
      { kind: "cells", cells: ["中文", "x"] },
      { kind: "cells", cells: ["ab", "y"] }
    ], "org"),
    "| 中文 | x |\n| ab   | y |"
  );
});

test("Org format pads ragged rows with empty cells", () => {
  assert.equal(
    formatPipeTable([
      { kind: "cells", cells: ["a", "b", "c"] },
      { kind: "cells", cells: ["d"] }
    ], "org"),
    "| a | b | c |\n| d |   |   |"
  );
});

test("Org format keeps the indentation on every line", () => {
  assert.equal(formatPipeTable([{ kind: "cells", cells: ["a"] }], "org", "  "), "  | a |");
});

test("Markdown format keeps colons, has at least three dashes and aligns cells", () => {
  assert.equal(
    formatPipeTable([
      { kind: "cells", cells: ["Name", "Qty"] },
      { kind: "rule", align: [null, "right"] },
      { kind: "cells", cells: ["apple", "3"] }
    ], "markdown"),
    "| Name  | Qty |\n|-------|----:|\n| apple |   3 |"
  );

  assert.equal(
    formatPipeTable([
      { kind: "cells", cells: ["x"] },
      { kind: "rule", align: ["center"] },
      { kind: "cells", cells: ["ab"] }
    ], "markdown"),
    "|  x  |\n|:---:|\n| ab  |"
  );

  assert.equal(
    formatPipeTable([{ kind: "cells", cells: ["a"] }, { kind: "rule", align: ["left"] }], "markdown"),
    "| a   |\n|:----|"
  );
  assert.equal(
    formatPipeTable([{ kind: "cells", cells: ["a"] }, { kind: "rule" }], "markdown"),
    "| a   |\n|-----|"
  );
});

test("cellAt maps an offset to its row, column and place in the cell", () => {
  const text = "| a | bb |\n|---+---|\n| ccc | d |";
  assert.deepEqual(cellAt(text, text.indexOf("bb") + 1), { row: 0, col: 1, offset: 1 });
  assert.equal(cellAt(text, text.indexOf("---")), null);
});

test("cellOffset gives the start of a cell's text", () => {
  const text = "| a | bb |\n|---+---|\n| ccc | d |";
  assert.equal(cellOffset(text, 2, 0), text.indexOf("ccc"));
  assert.equal(cellOffset(text, 0, 1, 1), text.indexOf("bb") + 1);
});

test("pipeBlock leaves out a caption and #+TBLFM lines", () => {
  const text = "Caption\n| a | b |\n|-+-|\n#+TBLFM: $1=1\nafter";
  const block = pipeBlock(text);
  assert.equal(block.text, "| a | b |\n|-+-|");
  assert.equal(block.from, text.indexOf("| a"));
  assert.equal(pipeBlock("no table here"), null);
});
